#![forbid(unsafe_code)]

//! `cce-mcp` Model Context Protocol adapter: exposes the engine's index,
//! status, search, context, symbol, map, explain, impact, providers,
//! definitions, references, grep, diff, architecture-diff, export, and
//! file operations as MCP tools over stdio JSON-RPC.

use std::{path::PathBuf, sync::Arc};

use cce_core::{QueryFilters, QueryIntent, SearchRequest, SearchRoute};
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
    /// Dense embedding backend; `local` (default) downloads the model once
    /// on first index, `disabled` stays fully offline.
    #[arg(long, value_enum, env = "CCE_DENSE", default_value_t = DenseMode::Local)]
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
        "cce_index" => serde_json::to_value(
            engine
                .index_with_origin(
                    optional_string(&arguments, "origin").or_else(|| Some("mcp".to_owned())),
                )
                .await
                .map_err(tool_error)?,
        )
        .map_err(|error| (-32603, error.to_string()))?,
        "cce_checkpoint" => serde_json::to_value(
            engine
                .checkpoint(
                    optional_string(&arguments, "origin").or_else(|| Some("mcp".to_owned())),
                )
                .await
                .map_err(tool_error)?,
        )
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
                        filters: QueryFilters::default(),
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
                        filters: QueryFilters::default(),
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
        "cce_providers" => {
            serde_json::to_value(engine.providers()).map_err(|error| (-32603, error.to_string()))?
        }
        "cce_definitions" => {
            let name = required_string(&arguments, "name")?;
            serde_json::to_value(engine.definitions(&name).map_err(tool_error)?)
                .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_references" => {
            let name = required_string(&arguments, "name")?;
            serde_json::to_value(engine.references(&name).map_err(tool_error)?)
                .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_grep" => {
            let pattern = required_string(&arguments, "pattern")?;
            serde_json::to_value(
                engine
                    .grep(&cce_engine::GrepRequest {
                        pattern,
                        filters: QueryFilters {
                            path_prefix: optional_string(&arguments, "pathPrefix"),
                            language: optional_string(&arguments, "language")
                                .map(|value| value.to_lowercase()),
                            hit_type: None,
                            pattern: None,
                        },
                        limit: optional_usize(&arguments, "limit")
                            .unwrap_or(200)
                            .clamp(1, 5_000),
                        ignore_case: arguments
                            .get("ignoreCase")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                    .map_err(tool_error)?,
            )
            .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_diff" => {
            let pattern = required_string(&arguments, "pattern")?;
            serde_json::to_value(
                engine
                    .search(SearchRequest {
                        repository_id: String::new(),
                        snapshot_id: String::new(),
                        query: pattern,
                        intent: None,
                        limit: optional_usize(&arguments, "limit")
                            .unwrap_or(100)
                            .clamp(1, 500),
                        require_fresh: true,
                        routes: vec![SearchRoute::Diff],
                        filters: QueryFilters::default(),
                    })
                    .await
                    .map_err(tool_error)?,
            )
            .map_err(|error| (-32603, error.to_string()))?
        }
        "cce_diff_architecture" => serde_json::to_value(
            engine
                .architecture_diff(
                    optional_string(&arguments, "base").as_deref(),
                    optional_string(&arguments, "head").as_deref(),
                )
                .map_err(tool_error)?,
        )
        .map_err(|error| (-32603, error.to_string()))?,
        "cce_export" => {
            let kind = required_string(&arguments, "kind")?;
            let snapshot = optional_string(&arguments, "snapshot");
            let cursor = optional_string(&arguments, "cursor");
            let limit = optional_usize(&arguments, "limit")
                .unwrap_or(500)
                .clamp(1, 1_000);
            let page = match kind.as_str() {
                "entities" => serde_json::to_value(
                    engine
                        .export_entities(snapshot.as_deref(), cursor.as_deref(), limit)
                        .map_err(tool_error)?,
                ),
                "relations" => serde_json::to_value(
                    engine
                        .export_relations(snapshot.as_deref(), cursor.as_deref(), limit)
                        .map_err(tool_error)?,
                ),
                "regions" => serde_json::to_value(
                    engine
                        .export_regions(snapshot.as_deref(), cursor.as_deref(), limit)
                        .map_err(tool_error)?,
                ),
                _ => {
                    return Err((
                        -32602,
                        format!("kind must be entities|relations|regions, got {kind}"),
                    ));
                }
            };
            page.map_err(|error| (-32603, error.to_string()))?
        }
        "cce_files" => serde_json::to_value(engine.files().map_err(tool_error)?)
            .map_err(|error| (-32603, error.to_string()))?,
        "cce_file" => {
            let path = required_string(&arguments, "path")?;
            serde_json::to_value(engine.file(&path).map_err(tool_error)?)
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
            json!({
                "type": "object",
                "properties": {
                    "origin": {"type": "string", "minLength": 1,
                        "description": "who is running this index — recorded on the snapshot"}
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_checkpoint",
            "Commit a detached parse+relations snapshot of the worktree for \
             diffing — no providers/dense/zoekt, never promoted to current. \
             Feed its snapshot id to cce_diff_architecture as `head` to see \
             what a session changed.",
            json!({
                "type": "object",
                "properties": {
                    "origin": {"type": "string", "minLength": 1,
                        "description": "who is checkpointing — recorded on the snapshot"}
                },
                "additionalProperties": false
            }),
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
            name_schema(),
        ),
        tool(
            "cce_providers",
            "Language intelligence providers and their last-run outcomes",
            json!({"type": "object", "additionalProperties": false}),
        ),
        tool(
            "cce_definitions",
            "Compiler-truth definitions for a symbol (SCIP providers)",
            name_schema(),
        ),
        tool(
            "cce_references",
            "Compiler-truth references for a symbol (SCIP providers)",
            name_schema(),
        ),
        tool(
            "cce_grep",
            "Regex search over the worktree — always fresh, not index-bound",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "minLength": 1},
                    "pathPrefix": {"type": "string"},
                    "language": {"type": "string"},
                    "ignoreCase": {"type": "boolean"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 5_000}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_diff",
            "Regex over stored commit patches (type:diff) — bounded by indexed history",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "minLength": 1},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_diff_architecture",
            "Entity/relation delta between two committed snapshots — what an edit session changed: files appeared, edges wired, edges cut",
            json!({
                "type": "object",
                "properties": {
                    "base": {"type": "string", "description": "base snapshot id; defaults to the snapshot committed before head"},
                    "head": {"type": "string", "description": "head snapshot id; defaults to the current committed snapshot"}
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_export",
            "One id-ordered page of a snapshot's entities, relations, or regions — bulk machine access, pass the cursor from the previous page",
            json!({
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": ["entities", "relations", "regions"]},
                    "snapshot": {"type": "string", "description": "snapshot id; defaults to the current committed snapshot"},
                    "cursor": {"type": "string", "description": "last id of the previous page"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 1_000}
                },
                "required": ["kind"],
                "additionalProperties": false
            }),
        ),
        tool(
            "cce_files",
            "List indexed files of the current snapshot",
            json!({"type": "object", "additionalProperties": false}),
        ),
        tool(
            "cce_file",
            "Read one file's content from the committed source view",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string", "minLength": 1}},
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": input_schema})
}

fn name_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"name": {"type": "string", "minLength": 1}},
        "required": ["name"],
        "additionalProperties": false
    })
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

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
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
