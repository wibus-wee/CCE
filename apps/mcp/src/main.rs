#![forbid(unsafe_code)]

//! `cce-mcp` Model Context Protocol adapter: exposes the engine's index,
//! status, search, context, map, explain, and impact operations as MCP tools
//! over stdio JSON-RPC.

use std::{path::PathBuf, sync::Arc};

use cce_core::{QueryIntent, SearchRequest};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DenseBackendConfig, EngineConfig,
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
    /// Local embedding model code, used with --dense local.
    #[arg(long, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long)]
    embedding_dimensions: Option<usize>,
    /// Local cross-encoder reranker model code; enables reranking. Bare
    /// `--reranker` uses the default model.
    #[arg(
        long,
        env = "CCE_RERANKER_MODEL",
        num_args = 0..=1,
        default_missing_value = cce_engine::DEFAULT_LOCAL_RERANKER_MODEL
    )]
    reranker: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    /// In-process ONNX model via fastembed; downloads model files once, then offline.
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
        DenseMode::Local => DenseBackendConfig::Local {
            model: arguments
                .embedding_model
                .unwrap_or_else(|| DEFAULT_LOCAL_EMBEDDING_MODEL.to_owned()),
        },
    };
    config.reranker_model = arguments.reranker;
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
                        filters: cce_core::QueryFilters::default(),
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
                        filters: cce_core::QueryFilters::default(),
                    })
                    .await
                    .map_err(tool_error)?,
            )
            .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_map" => serde_json::to_value(engine.codebase_map().map_err(tool_error)?)
            .map_err(|error| (-32603, error.to_string()))?,
        "cce_explain" => {
            let name = required_string(&arguments, "name")?;
            serde_json::to_value(engine.explain_component(&name).map_err(tool_error)?)
                .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_impact" => {
            let name = required_string(&arguments, "name")?;
            serde_json::to_value(engine.impact_analysis(&name).map_err(tool_error)?)
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
                    "budgetTokens": {"type": "integer", "minimum": 256, "maximum": 128_000},
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
        tool(
            "cce_map",
            "Package-level architecture map: build boundaries and dependency direction",
            json!({"type": "object", "additionalProperties": false}),
        ),
        tool(
            "cce_explain",
            "Explain a package or symbol: members, dependencies, dependents, tests",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "minLength": 1}},
                "required": ["name"],
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_impact",
            "Blast radius of a symbol or package through persisted impact edges",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "minLength": 1}},
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
