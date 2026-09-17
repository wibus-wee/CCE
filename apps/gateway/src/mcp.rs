//! MCP (Model Context Protocol) over HTTP, mounted per repository.
//!
//! `POST /{repo}/mcp` accepts JSON-RPC 2.0 messages — a single object or a
//! batch array — and executes `tools/call` by forwarding to the repo's
//! worker `/v1/*` API. This is the Streamable-HTTP transport in its
//! stateless form: responses are plain `application/json`, sessions carry
//! no server-side state, and the `Mcp-Session-Id` issued at `initialize`
//! is advisory only. That keeps the endpoint horizontally scalable like
//! the rest of the gateway — no sticky sessions, no SSE fan-out.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::{GatewayState, require_repo};

const LATEST_PROTOCOL: &str = "2025-11-25";
const MAX_BATCH: usize = 16;

#[utoipa::path(post, path = "/{repo}/mcp", tag = "mcp",
    params(("repo" = String, Path, description = "repo id or slug")),
    summary = "MCP JSON-RPC endpoint for this repo's worker",
    request_body = Value,
    responses(
        (status = 200, description = "JSON-RPC response object or batch array"),
        (status = 202, description = "notifications accepted, no response body"),
        (status = "4XX", description = "unknown repo or malformed request", body = crate::ErrorBody)
    ))]
pub(crate) async fn endpoint(
    State(state): State<Arc<GatewayState>>,
    AxumPath(repo): AxumPath<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let entry = match require_repo(&state, &repo) {
        Ok(entry) => entry,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error.1),
    };

    let (requests, batch) = match &body {
        Value::Array(items) => {
            if items.len() > MAX_BATCH {
                return error_response(StatusCode::BAD_REQUEST, "batch exceeds 16 requests");
            }
            (items.clone(), true)
        }
        Value::Object(_) => (vec![body.clone()], false),
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "expected a JSON-RPC object or array",
            );
        }
    };

    let mut responses = Vec::new();
    let mut session_header: Option<HeaderValue> = None;
    for request in requests {
        match dispatch(&state, &entry.worker_url, request).await {
            Dispatch::Response(response) => responses.push(response),
            Dispatch::Initialized(response) => {
                session_header = HeaderValue::from_str(&uuid::Uuid::now_v7().to_string()).ok();
                responses.push(response);
            }
            Dispatch::Notification => {}
        }
    }

    if responses.is_empty() {
        // Pure notifications get a bare 202 per the transport spec.
        return StatusCode::ACCEPTED.into_response();
    }
    let payload = if batch || responses.len() > 1 {
        Value::Array(responses)
    } else {
        responses.into_iter().next().unwrap_or(Value::Null)
    };
    let mut response = Json(payload).into_response();
    if let Some(session) = session_header.or_else(|| headers.get("mcp-session-id").cloned()) {
        response.headers_mut().insert("mcp-session-id", session);
    }
    response
}

enum Dispatch {
    Response(Value),
    /// `initialize` answered — carries its response and earns a session id.
    Initialized(Value),
    /// Notification swallowed; no response object.
    Notification,
}

async fn dispatch(state: &GatewayState, worker_url: &str, request: Value) -> Dispatch {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = id else {
        return Dispatch::Notification;
    };
    if method.starts_with("notifications/") {
        return Dispatch::Notification;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": request.pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LATEST_PROTOCOL),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "cce-gateway", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Use cce_context for bounded source-linked context; cce_search for retrieval, cce_grep/cce_diff for regex over worktree and history. Generated summaries are navigation aids; source citations are authoritative."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => call_tool(state, worker_url, request.get("params")).await,
        method => Err((-32601, format!("method not found: {method}"))),
    };
    let response = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    };
    if method == "initialize" && response.get("error").is_none() {
        Dispatch::Initialized(response)
    } else {
        Dispatch::Response(response)
    }
}

/// Map a tool call onto the worker's `/v1/*` API. Execution failures come
/// back as `isError` content — the tool ran, the tool failed — while
/// argument and routing problems stay JSON-RPC errors.
async fn call_tool(
    state: &GatewayState,
    worker_url: &str,
    params: Option<&Value>,
) -> Result<Value, (i32, String)> {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tool name is required".to_owned()))?;
    let arguments = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let outcome = match name {
        "cce_index" => post(state, worker_url, "index", json!({})).await,
        "cce_status" => get(state, worker_url, "status").await,
        "cce_providers" => get(state, worker_url, "providers").await,
        "cce_map" => get(state, worker_url, "map").await,
        "cce_files" => get(state, worker_url, "files").await,
        "cce_file" => {
            let path = required_string(&arguments, "path")?;
            get(
                state,
                worker_url,
                &format!("file?path={}", urlencoded(&path)),
            )
            .await
        }
        "cce_explain" => {
            let name = required_string(&arguments, "name")?;
            get(state, worker_url, &format!("explain/{}", urlencoded(&name))).await
        }
        "cce_impact" => {
            let name = required_string(&arguments, "name")?;
            get(state, worker_url, &format!("impact/{}", urlencoded(&name))).await
        }
        "cce_definitions" => {
            let name = required_string(&arguments, "name")?;
            get(state, worker_url, &format!("def/{}", urlencoded(&name))).await
        }
        "cce_references" => {
            let name = required_string(&arguments, "name")?;
            get(state, worker_url, &format!("refs/{}", urlencoded(&name))).await
        }
        "cce_search" | "cce_symbol" => {
            let query = required_string(
                &arguments,
                if name == "cce_symbol" {
                    "name"
                } else {
                    "query"
                },
            )?;
            let mut body = json!({
                "query": query,
                "limit": optional_usize(&arguments, "limit").unwrap_or(20).clamp(1, 200),
            });
            if name == "cce_symbol" {
                set(&mut body, "intent", json!("exact_entity"));
            } else if let Some(intent) = arguments.get("intent") {
                set(&mut body, "intent", intent.clone());
            }
            post(state, worker_url, "search", body).await
        }
        "cce_context" => {
            let query = required_string(&arguments, "query")?;
            let mut body = json!({
                "query": query,
                "budgetTokens": optional_usize(&arguments, "budgetTokens")
                    .unwrap_or(8_192)
                    .clamp(256, 128_000),
                "maxCandidates": optional_usize(&arguments, "maxCandidates")
                    .unwrap_or(50)
                    .clamp(1, 500),
                "requireFresh": true,
            });
            if let Some(intent) = arguments.get("intent") {
                set(&mut body, "intent", intent.clone());
            }
            post(state, worker_url, "context", body).await
        }
        "cce_grep" => {
            let pattern = required_string(&arguments, "pattern")?;
            let mut body = json!({"pattern": pattern});
            for key in ["pathPrefix", "language", "limit", "ignoreCase"] {
                if let Some(value) = arguments.get(key) {
                    set(&mut body, key, value.clone());
                }
            }
            post(state, worker_url, "grep", body).await
        }
        "cce_diff" => {
            let pattern = required_string(&arguments, "pattern")?;
            let mut body = json!({"pattern": pattern});
            if let Some(limit) = arguments.get("limit") {
                set(&mut body, "limit", limit.clone());
            }
            post(state, worker_url, "diff", body).await
        }
        _ => return Err((-32602, format!("unknown tool: {name}"))),
    };

    match outcome {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value)
                .map_err(|error| (-32603, error.to_string()))?;
            Ok(json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": value,
                "isError": false
            }))
        }
        Err(message) => Ok(json!({
            "content": [{"type": "text", "text": message}],
            "isError": true
        })),
    }
}

async fn get(state: &GatewayState, worker_url: &str, path: &str) -> Result<Value, String> {
    send(state.client.get(format!("{worker_url}/v1/{path}"))).await
}

async fn post(
    state: &GatewayState,
    worker_url: &str,
    path: &str,
    body: Value,
) -> Result<Value, String> {
    send(
        state
            .client
            .post(format!("{worker_url}/v1/{path}"))
            .json(&body),
    )
    .await
}

async fn send(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let response = request
        .send()
        .await
        .map_err(|error| format!("worker unreachable: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("worker returned a non-JSON response: {error}"))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(body
            .get("error")
            .and_then(Value::as_str)
            .map_or_else(|| format!("worker returned {status}"), str::to_owned))
    }
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "cce_index",
            "Index or incrementally refresh the repository",
            empty_schema(),
        ),
        tool(
            "cce_status",
            "Return per-view freshness and capabilities",
            empty_schema(),
        ),
        tool(
            "cce_search",
            "Run intent-aware repository retrieval",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "minLength": 1},
                    "intent": intent_schema(),
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
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
            name_schema(),
        ),
        tool(
            "cce_map",
            "Package-level architecture map: build boundaries and dependency direction",
            empty_schema(),
        ),
        tool(
            "cce_explain",
            "Explain a package or symbol: members, dependencies, dependents, tests",
            name_schema(),
        ),
        tool(
            "cce_impact",
            "Blast radius of a symbol or package through persisted impact edges",
            name_schema(),
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
            "cce_providers",
            "Language intelligence providers and their last-run outcomes",
            empty_schema(),
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
                    "limit": {"type": "integer", "minimum": 1, "maximum": 5000}
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
            "cce_files",
            "List indexed files of the current snapshot",
            empty_schema(),
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

fn empty_schema() -> Value {
    json!({"type": "object", "additionalProperties": false})
}

fn name_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"name": {"type": "string", "minLength": 1}},
        "required": ["name"],
        "additionalProperties": false
    })
}

fn intent_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["exact_entity", "natural_language_behavior", "issue_localization", "trace", "impact", "architecture", "history", "precise_dataflow"]
    })
}

fn set(body: &mut Value, key: &str, value: Value) {
    if let Some(map) = body.as_object_mut() {
        map.insert(key.to_owned(), value);
    }
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

fn urlencoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}
