#![forbid(unsafe_code)]

use std::{path::PathBuf, sync::Arc};

use cce_core::{QueryIntent, SearchRequest};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DEFAULT_LOCAL_RERANKER_MODEL,
    DEFAULT_LOCAL_RERANKER_REVISION, DataflowBackendConfig, DenseBackendConfig, EngineConfig,
    RerankerBackendConfig, ScipBackendConfig, local_embedding_preset,
};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const LATEST_PROTOCOL: &str = "2025-11-25";

#[derive(Debug, Parser)]
#[command(name = "cce-mcp", version)]
struct Arguments {
    #[arg(default_value = ".")]
    repository: PathBuf,
    #[arg(long, env = "CCE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = DenseMode::Disabled)]
    dense: DenseMode,
    #[arg(long, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long, env = "CCE_EMBEDDING_REVISION")]
    embedding_revision: Option<String>,
    #[arg(long, env = "CCE_EMBEDDING_MODEL_DIRECTORY")]
    embedding_model_directory: Option<PathBuf>,
    #[arg(long, env = "CCE_EMBEDDING_CACHE_DIR")]
    embedding_cache_dir: Option<PathBuf>,
    #[arg(long, env = "CCE_ONNX_RUNTIME_LIBRARY")]
    onnx_runtime_library: Option<PathBuf>,
    #[arg(long, env = "CCE_EMBEDDING_ALLOW_DOWNLOAD")]
    embedding_allow_download: bool,
    #[arg(long, env = "CCE_EMBEDDING_ALLOW_HIGH_MEMORY")]
    embedding_allow_high_memory: bool,
    #[arg(long, default_value_t = 512)]
    embedding_max_length: usize,
    #[arg(long)]
    embedding_threads: Option<usize>,
    #[arg(long, default_value_t = 4)]
    embedding_batch_size: usize,
    #[arg(long, env = "CCE_EMBEDDING_SESSIONS", default_value_t = 1)]
    embedding_sessions: usize,
    #[arg(long)]
    embedding_query_prefix: Option<String>,
    #[arg(long)]
    embedding_document_prefix: Option<String>,
    #[arg(long)]
    embedding_dimensions: Option<usize>,
    #[arg(long, value_enum, default_value_t = RerankerMode::Disabled)]
    reranker: RerankerMode,
    #[arg(long, env = "CCE_RERANKER_MODEL")]
    reranker_model: Option<String>,
    #[arg(long, env = "CCE_RERANKER_REVISION")]
    reranker_revision: Option<String>,
    #[arg(long, env = "CCE_RERANKER_MODEL_DIRECTORY")]
    reranker_model_directory: Option<PathBuf>,
    #[arg(long, env = "CCE_RERANKER_CACHE_DIR")]
    reranker_cache_dir: Option<PathBuf>,
    #[arg(long, env = "CCE_RERANKER_ALLOW_DOWNLOAD")]
    reranker_allow_download: bool,
    #[arg(long, env = "CCE_RERANKER_MAX_LENGTH", default_value_t = 512)]
    reranker_max_length: usize,
    #[arg(long, env = "CCE_RERANKER_THREADS")]
    reranker_threads: Option<usize>,
    #[arg(long, env = "CCE_RERANKER_BATCH_SIZE", default_value_t = 8)]
    reranker_batch_size: usize,
    #[arg(long, env = "CCE_RERANKER_SESSIONS", default_value_t = 1)]
    reranker_sessions: usize,
    #[arg(long, env = "CCE_SCIP_AUTO", conflicts_with = "scip_index")]
    scip_auto: bool,
    #[arg(long, env = "CCE_SCIP_INDEX")]
    scip_index: Option<PathBuf>,
    #[arg(long, env = "CCE_RUST_ANALYZER", default_value = "rust-analyzer")]
    rust_analyzer: PathBuf,
    #[arg(long, env = "CCE_SCIP_TYPESCRIPT", default_value = "scip-typescript")]
    scip_typescript: PathBuf,
    #[arg(long)]
    scip_threads: Option<usize>,
    #[arg(long, default_value_t = 900)]
    scip_timeout_seconds: u64,
    #[arg(long, env = "CCE_DATAFLOW_INDEX", conflicts_with = "dataflow_joern")]
    dataflow_index: Option<PathBuf>,
    #[arg(long, env = "CCE_DATAFLOW_JOERN")]
    dataflow_joern: bool,
    #[arg(long, env = "CCE_JOERN_PARSE", default_value = "joern-parse")]
    joern_parse: PathBuf,
    #[arg(long, env = "CCE_JOERN_EXPORT", default_value = "joern-export")]
    joern_export: PathBuf,
    #[arg(long, env = "CCE_JOERN_LANGUAGE")]
    joern_language: Option<String>,
    #[arg(long, env = "CCE_JOERN_TIMEOUT_SECONDS", default_value_t = 1_800)]
    joern_timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    Local,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RerankerMode {
    Disabled,
    Local,
}

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cce=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let arguments = Arguments::parse();
    let mut config = EngineConfig::for_repository(&arguments.repository);
    if let Some(data_dir) = arguments.data_dir {
        config.data_root = data_dir;
    }
    config.dense = match arguments.dense {
        DenseMode::Disabled => DenseBackendConfig::Disabled,
        DenseMode::Baseline => DenseBackendConfig::DeterministicBaseline {
            dimensions: arguments.embedding_dimensions.unwrap_or(512),
        },
        DenseMode::Local => {
            let requested_model = arguments
                .embedding_model
                .unwrap_or_else(|| DEFAULT_LOCAL_EMBEDDING_MODEL.to_owned());
            let preset = local_embedding_preset(&requested_model);
            DenseBackendConfig::LocalFastEmbed {
                model: preset.map_or(requested_model, |value| value.model.to_owned()),
                revision: arguments
                    .embedding_revision
                    .or_else(|| preset.map(|value| value.revision.to_owned()))
                    .ok_or_else(|| {
                        anyhow::anyhow!("--embedding-revision is required for a custom local model")
                    })?,
                model_directory: arguments.embedding_model_directory,
                cache_dir: arguments
                    .embedding_cache_dir
                    .unwrap_or_else(|| config.data_root.join("models")),
                runtime_library: arguments.onnx_runtime_library.clone().unwrap_or_else(|| {
                    config
                        .data_root
                        .join("runtime/lib/libonnxruntime.so.1.28.0")
                }),
                allow_download: arguments.embedding_allow_download,
                allow_high_memory: arguments.embedding_allow_high_memory,
                max_length: arguments.embedding_max_length,
                threads: arguments.embedding_threads,
                batch_size: arguments.embedding_batch_size,
                sessions: arguments.embedding_sessions,
                query_prefix: arguments.embedding_query_prefix.unwrap_or_else(|| {
                    preset.map_or_else(String::new, |value| value.query_prefix.to_owned())
                }),
                document_prefix: arguments.embedding_document_prefix.unwrap_or_else(|| {
                    preset.map_or_else(String::new, |value| value.document_prefix.to_owned())
                }),
            }
        }
    };
    config.reranker = match arguments.reranker {
        RerankerMode::Disabled => RerankerBackendConfig::Disabled,
        RerankerMode::Local => {
            let model = arguments
                .reranker_model
                .unwrap_or_else(|| DEFAULT_LOCAL_RERANKER_MODEL.to_owned());
            let revision = arguments
                .reranker_revision
                .or_else(|| {
                    model
                        .eq_ignore_ascii_case(DEFAULT_LOCAL_RERANKER_MODEL)
                        .then(|| DEFAULT_LOCAL_RERANKER_REVISION.to_owned())
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("--reranker-revision is required for a custom local reranker")
                })?;
            RerankerBackendConfig::LocalFastEmbed {
                model,
                revision,
                model_directory: arguments.reranker_model_directory,
                cache_dir: arguments
                    .reranker_cache_dir
                    .unwrap_or_else(|| config.data_root.join("rerankers")),
                runtime_library: arguments.onnx_runtime_library.clone().unwrap_or_else(|| {
                    config
                        .data_root
                        .join("runtime/lib/libonnxruntime.so.1.28.0")
                }),
                allow_download: arguments.reranker_allow_download,
                max_length: arguments.reranker_max_length,
                threads: arguments.reranker_threads,
                batch_size: arguments.reranker_batch_size,
                sessions: arguments.reranker_sessions,
            }
        }
    };
    config.scip = if arguments.scip_auto {
        ScipBackendConfig::auto(
            arguments.rust_analyzer,
            arguments.scip_typescript,
            arguments.scip_threads,
            arguments.scip_timeout_seconds,
        )?
    } else if let Some(path) = arguments.scip_index {
        ScipBackendConfig::Supplied { path }
    } else {
        ScipBackendConfig::Disabled
    };
    config.dataflow = if arguments.dataflow_joern {
        DataflowBackendConfig::joern_with_language(
            &arguments.joern_parse,
            &arguments.joern_export,
            arguments.joern_timeout_seconds,
            arguments.joern_language.as_deref(),
        )?
    } else {
        arguments
            .dataflow_index
            .map_or(DataflowBackendConfig::Disabled, |path| {
                DataflowBackendConfig::Supplied { path }
            })
    };
    serve(Arc::new(CceEngine::open(config)?)).await
}

async fn serve(engine: Arc<CceEngine>) -> anyhow::Result<()> {
    let input = BufReader::new(tokio::io::stdin());
    let mut lines = input.lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.len() > MAX_REQUEST_BYTES {
            write_response(
                &mut output,
                error_response(Value::Null, -32600, "request exceeds 1 MiB"),
            )
            .await?;
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut output,
                    error_response(Value::Null, -32700, &format!("parse error: {error}")),
                )
                .await?;
                continue;
            }
        };
        let Some(id) = request.id.clone() else {
            continue;
        };
        let response = dispatch(&engine, id, request).await;
        write_response(&mut output, response).await?;
    }
    Ok(())
}

async fn dispatch(engine: &CceEngine, id: Value, request: Request) -> Response {
    let result = match request.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": request.params.get("protocolVersion").and_then(Value::as_str).unwrap_or(LATEST_PROTOCOL),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "cce", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Use cce_context for bounded source-linked context. Generated summaries are navigation aids; source citations are authoritative."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => call_tool(engine, &request.params).await,
        method => Err((-32601, format!("method not found: {method}"))),
    };
    match result {
        Ok(result) => Response {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        },
        Err((code, message)) => error_response(id, code, &message),
    }
}

async fn call_tool(engine: &CceEngine, params: &Value) -> Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tool name is required".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let value = match name {
        "cce_index" => serde_json::to_value(engine.index().await.map_err(tool_error)?)
            .map_err(|error| (-32603, error.to_string()))?,
        "cce_status" => serde_json::to_value(engine.status().map_err(tool_error)?)
            .map_err(|error| (-32603, error.to_string()))?,
        "cce_search" => {
            let query = required_string(&arguments, "query")?;
            let intent = optional_intent(&arguments)?;
            let limit = optional_usize(&arguments, "limit")
                .unwrap_or(20)
                .clamp(1, 200);
            serde_json::to_value(
                engine
                    .search(SearchRequest {
                        repository_id: String::new(),
                        snapshot_id: String::new(),
                        query,
                        intent,
                        limit,
                        require_fresh: true,
                        routes: Vec::new(),
                    })
                    .await
                    .map_err(tool_error)?,
            )
            .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_context" => {
            let query = required_string(&arguments, "query")?;
            let budget = optional_usize(&arguments, "budgetTokens")
                .unwrap_or(8_192)
                .clamp(256, 128_000);
            let mut request = ContextRequest::new(query, budget);
            request.intent = optional_intent(&arguments)?;
            request.max_candidates = optional_usize(&arguments, "maxCandidates")
                .unwrap_or(50)
                .clamp(1, 500);
            serde_json::to_value(engine.context(request).await.map_err(tool_error)?)
                .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_symbol" => {
            let name = required_string(&arguments, "name")?;
            serde_json::to_value(
                engine
                    .search(SearchRequest {
                        repository_id: String::new(),
                        snapshot_id: String::new(),
                        query: name,
                        intent: Some(QueryIntent::ExactEntity),
                        limit: optional_usize(&arguments, "limit")
                            .unwrap_or(20)
                            .clamp(1, 200),
                        require_fresh: true,
                        routes: Vec::new(),
                    })
                    .await
                    .map_err(tool_error)?,
            )
            .map_err(|error| (-32603, error.to_string()))?
        }
        _ => return Err((-32602, format!("unknown tool: {name}"))),
    };
    let text = serde_json::to_string_pretty(&value).map_err(|error| (-32603, error.to_string()))?;
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": value,
        "isError": false
    }))
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "cce_index",
            "Index or incrementally refresh the repository",
            json!({"type": "object", "additionalProperties": false}),
        ),
        tool(
            "cce_status",
            "Return per-view freshness and capabilities",
            json!({"type": "object", "additionalProperties": false}),
        ),
        tool(
            "cce_search",
            "Run intent-aware repository retrieval",
            query_schema(true),
        ),
        tool(
            "cce_context",
            "Build a source-linked context pack under a token budget",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "minLength": 1},
                    "intent": intent_schema(),
                    "budgetTokens": {"type": "integer", "minimum": 256, "maximum": 128000},
                    "maxCandidates": {"type": "integer", "minimum": 1, "maximum": 500}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_symbol",
            "Find an exact symbol and source-linked definitions",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "minLength": 1}, "limit": {"type": "integer", "minimum": 1, "maximum": 200}},
                "required": ["name"],
                "additionalProperties": false
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": input_schema})
}

fn query_schema(include_limit: bool) -> Value {
    let mut properties = serde_json::Map::from_iter([
        (
            "query".to_owned(),
            json!({"type": "string", "minLength": 1}),
        ),
        ("intent".to_owned(), intent_schema()),
    ]);
    if include_limit {
        properties.insert(
            "limit".to_owned(),
            json!({"type": "integer", "minimum": 1, "maximum": 200}),
        );
    }
    json!({"type": "object", "properties": properties, "required": ["query"], "additionalProperties": false})
}

fn intent_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["exact_entity", "natural_language_behavior", "issue_localization", "trace", "impact", "architecture", "history", "precise_dataflow"]
    })
}

fn required_string(arguments: &Value, name: &str) -> Result<String, (i32, String)> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| (-32602, format!("{name} must be a non-empty string")))
}

fn optional_usize(arguments: &Value, name: &str) -> Option<usize> {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn optional_intent(arguments: &Value) -> Result<Option<QueryIntent>, (i32, String)> {
    arguments
        .get("intent")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| (-32602, format!("invalid intent: {error}")))
}

fn tool_error(error: cce_core::CceError) -> (i32, String) {
    (-32603, error.to_string())
}

fn error_response(id: Value, code: i32, message: &str) -> Response {
    Response {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.to_owned(),
        }),
    }
}

async fn write_response(output: &mut tokio::io::Stdout, response: Response) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    output.flush().await?;
    Ok(())
}
