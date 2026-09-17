//! cce-gateway: the multi-repo front door.
//!
//! Owns three jobs, all stateless except a small on-disk registry:
//! - **Registry**: repo name → worker URL + materialized source dir.
//! - **Ingest**: content-addressed blob upload (`check`/`blobs`/`push`) that
//!   materializes a pushed worktree into `sources/<slug>/` for the repo's
//!   worker to index — the push model: what the service sees is what the
//!   pusher last committed to it.
//! - **Routing**: `/{repo}/v1/*` proxies to that repo's worker verbatim.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::any;
use clap::Parser;
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi as _, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

mod mcp;

#[derive(Parser)]
#[command(name = "cce-gateway", version)]
struct Arguments {
    /// State root: registry, blob store, materialized sources.
    #[arg(long, env = "CCE_GATEWAY_DATA", default_value = "./gateway-data")]
    data_dir: PathBuf,
    #[arg(long, default_value = "127.0.0.1:7735", env = "CCE_GATEWAY_BIND")]
    bind: SocketAddr,
    #[arg(long)]
    allow_non_loopback: bool,
    /// Static web UI directory to serve at /.
    #[arg(long)]
    web_root: Option<PathBuf>,
    /// Worker URL assigned to repos auto-registered via /v1/repos/ensure —
    /// the single-worker deployment's "just push it" switch. Repos can
    /// always be registered explicitly with their own `worker_url`.
    #[arg(long, env = "CCE_DEFAULT_WORKER")]
    default_worker: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RepoEntry {
    id: String,
    name: String,
    slug: String,
    worker_url: String,
    created_at: String,
    last_push: Option<PushRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PushRecord {
    push_id: String,
    at: String,
    revision: Option<String>,
    file_count: u64,
    bytes: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    name: String,
    worker_url: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct FileEntry {
    path: String,
    /// Declared blob size — part of the wire contract; reserved for
    /// size-limit enforcement.
    #[allow(dead_code)]
    size: u64,
    digest: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CheckRequest {
    files: Vec<FileEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CheckResponse {
    missing: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PushFile {
    path: String,
    digest: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PushRequest {
    revision: Option<String>,
    files: Vec<PushFile>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PushResponse {
    push_id: String,
    materialized: u64,
    bytes: u64,
}

#[derive(Debug)]
struct GatewayState {
    data_dir: PathBuf,
    client: reqwest::Client,
    registry: parking_lot::Mutex<HashMap<String, RepoEntry>>,
    /// Serializes pushes per repo: staging + the source-dir swap are not
    /// re-entrant — two pushes to one repo would otherwise remove each
    /// other's staging dir mid-fill. Different repos still push in
    /// parallel; blob presence checks stay outside the lock (CAS is
    /// immutable, the check never goes stale).
    push_locks: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    default_worker: Option<String>,
}

/// Error envelope returned by every gateway endpoint.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: String,
}

#[derive(Debug)]
struct ApiError(StatusCode, String);

#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "cce-gateway",
        description = "Multi-repo front door: repo registry, content-addressed ingest, worker routing. \
                       Every repo's worker API is reachable under `/{repo}/v1/*` — the same surface a \
                       cce-daemon serves directly."
    ),
    // The proxy is documented via its path annotation but routed as a real
    // wildcard (`/{repo}/v1/{*path}`) — axum refuses to register both the
    // documented single-segment template and the wildcard.
    paths(proxy)
)]
struct ApiDoc;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct HealthStatus {
    status: &'static str,
}

/// `POST /v1/repos/ensure` body — resolve a repo by name, auto-registering
/// it on `--default-worker` when unregistered.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EnsureRequest {
    name: String,
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

fn not_found(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, message.into())
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let arguments = Arguments::parse();
    if !arguments.allow_non_loopback && !arguments.bind.ip().is_loopback() {
        anyhow::bail!(
            "refusing to bind {} — pass --allow-non-loopback for non-loopback binds",
            arguments.bind
        );
    }
    let data_dir = arguments
        .data_dir
        .canonicalize()
        .unwrap_or(arguments.data_dir);
    std::fs::create_dir_all(data_dir.join("blobs"))?;
    std::fs::create_dir_all(data_dir.join("sources"))?;
    let mut registry = HashMap::new();
    let registry_path = data_dir.join("repos.json");
    if let Ok(bytes) = std::fs::read(&registry_path) {
        registry = serde_json::from_slice::<Vec<RepoEntry>>(&bytes)?
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        tracing::info!(repos = registry.len(), "loaded repo registry");
    }
    let state = Arc::new(GatewayState {
        data_dir,
        client: reqwest::Client::new(),
        registry: parking_lot::Mutex::new(registry),
        push_locks: parking_lot::Mutex::new(HashMap::new()),
        default_worker: arguments
            .default_worker
            .map(|url| url.trim_end_matches('/').to_owned()),
    });
    let (api_router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(list_repos))
        .routes(routes!(register_repo))
        .routes(routes!(ensure_repo))
        .routes(routes!(repo_detail))
        .routes(routes!(check_blobs))
        .routes(routes!(put_blob))
        .routes(routes!(push))
        .routes(routes!(mcp::endpoint))
        .split_for_parts();
    let mut router = api_router
        .merge(SwaggerUi::new("/docs").url("/openapi.json", api))
        .route("/{repo}/v1/{*path}", any(proxy))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state.clone());
    if let Some(web_root) = arguments.web_root {
        router = router.nest_service(
            "/",
            tower_http::services::ServeDir::new(web_root).append_index_html_on_directories(true),
        );
    }
    let listener = tokio::net::TcpListener::bind(arguments.bind).await?;
    tracing::info!(bind = %arguments.bind, "cce-gateway listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[utoipa::path(get, path = "/healthz", tag = "meta",
    responses((status = 200, description = "liveness probe", body = HealthStatus)))]
async fn health() -> Json<HealthStatus> {
    Json(HealthStatus { status: "ok" })
}

fn persist_registry(state: &GatewayState) -> anyhow::Result<()> {
    let entries: Vec<RepoEntry> = state.registry.lock().values().cloned().collect();
    let bytes = serde_json::to_vec_pretty(&entries)?;
    let tmp = state.data_dir.join("repos.json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, state.data_dir.join("repos.json"))?;
    Ok(())
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "repo".to_owned()
    } else {
        slug
    }
}

/// Repo-relative paths must stay inside the materialized source dir —
/// anything else would let a push write outside its sandbox.
fn safe_relpath(path: &str) -> Result<PathBuf, ApiError> {
    let rel = Path::new(path);
    if path.is_empty()
        || rel.is_absolute()
        || rel
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(bad_request(format!(
            "unsafe path in push manifest: {path:?}"
        )));
    }
    Ok(rel.to_path_buf())
}

fn blob_path(state: &GatewayState, digest: &str) -> Result<PathBuf, ApiError> {
    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad_request(format!("invalid blob digest: {digest:?}")));
    }
    Ok(state.data_dir.join("blobs").join(&digest[..2]).join(digest))
}

#[utoipa::path(get, path = "/v1/repos", tag = "repos",
    summary = "list registered repositories",
    responses(
        (status = 200, body = Vec<RepoEntry>),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 500, description = "internal error", body = ErrorBody)
    ))]
async fn list_repos(State(state): State<Arc<GatewayState>>) -> Json<Vec<RepoEntry>> {
    let mut entries: Vec<RepoEntry> = { state.registry.lock().values().cloned().collect() };
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Json(entries)
}

#[utoipa::path(get, path = "/v1/repos/{id}", tag = "repos", params(("id" = String, Path, description = "repo id or slug")),
    summary = "repo detail incl. last push",
    responses(
        (status = 200, body = RepoEntry),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 500, description = "internal error", body = ErrorBody)
    ))]
async fn repo_detail(
    State(state): State<Arc<GatewayState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RepoEntry>, ApiError> {
    let registry = state.registry.lock();
    registry
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| not_found(format!("unknown repo {id}")))
}

#[utoipa::path(post, path = "/v1/repos", tag = "repos",
    request_body = RegisterRequest,
    summary = "register (or update) a repository → worker mapping",
    responses(
        (status = 200, body = RepoEntry),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 500, description = "internal error", body = ErrorBody)
    ))]
async fn register_repo(
    State(state): State<Arc<GatewayState>>,
    Json(input): Json<RegisterRequest>,
) -> Result<Json<RepoEntry>, ApiError> {
    if input.name.is_empty() || input.name.len() > 200 {
        return Err(bad_request("repo name must be 1..200 chars"));
    }
    let worker_url = input.worker_url.trim_end_matches('/').to_owned();
    if !(worker_url.starts_with("http://") || worker_url.starts_with("https://")) {
        return Err(bad_request("workerUrl must be http(s)"));
    }
    let slug = slugify(&input.name);
    let id = format!(
        "repo_{}",
        &blake3::hash(input.name.as_bytes()).to_hex()[..16]
    );
    let mut registry = state.registry.lock();
    // Idempotent on name: re-registering updates the worker URL.
    if let Some(existing) = registry
        .values_mut()
        .find(|entry| entry.name == input.name || entry.id == id)
    {
        existing.worker_url = worker_url;
        let entry = existing.clone();
        drop(registry);
        persist_registry(&state)?;
        return Ok(Json(entry));
    }
    let entry = RepoEntry {
        id: id.clone(),
        name: input.name,
        slug,
        worker_url,
        created_at: chrono::Utc::now().to_rfc3339(),
        last_push: None,
    };
    std::fs::create_dir_all(state.data_dir.join("sources").join(&entry.slug))
        .map_err(ApiError::from)?;
    registry.insert(id, entry.clone());
    drop(registry);
    persist_registry(&state)?;
    Ok(Json(entry))
}

/// Push-side discovery: a pusher names a repo, the gateway resolves (or
/// auto-registers with the default worker) and returns the entry.
#[utoipa::path(post, path = "/v1/repos/ensure", tag = "repos",
    request_body = EnsureRequest,
    responses(
        (status = 200, body = RepoEntry),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 404, description = "unknown repo and no default worker", body = ErrorBody)
    ))]
async fn ensure_repo(
    State(state): State<Arc<GatewayState>>,
    Json(input): Json<EnsureRequest>,
) -> Result<Json<RepoEntry>, ApiError> {
    let name = input.name;
    if name.is_empty() {
        return Err(bad_request("ensure requires a repo name"));
    }
    let found = state
        .registry
        .lock()
        .values()
        .find(|entry| entry.name == name)
        .cloned();
    if let Some(entry) = found {
        return Ok(Json(entry));
    }
    let Some(worker_url) = state.default_worker.clone() else {
        return Err(not_found(format!(
            "repo {name:?} is not registered and no default worker is configured"
        )));
    };
    register_repo(State(state), Json(RegisterRequest { name, worker_url })).await
}

#[utoipa::path(post, path = "/v1/repos/{id}/check", tag = "repos", params(("id" = String, Path, description = "repo id or slug")),
    request_body = CheckRequest,
    summary = "which manifest digests the CAS is missing",
    responses(
        (status = 200, body = CheckResponse),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 500, description = "internal error", body = ErrorBody)
    ))]
async fn check_blobs(
    State(state): State<Arc<GatewayState>>,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<CheckRequest>,
) -> Result<Json<CheckResponse>, ApiError> {
    require_repo(&state, &id)?;
    let mut missing = Vec::new();
    for file in &input.files {
        safe_relpath(&file.path)?;
        let path = blob_path(&state, &file.digest)?;
        if !path.exists() {
            missing.push(file.digest.clone());
        }
    }
    missing.sort();
    missing.dedup();
    Ok(Json(CheckResponse { missing }))
}

#[utoipa::path(put, path = "/v1/repos/{id}/blobs/{digest}", tag = "repos", params(("id" = String, Path, description = "repo id or slug"), ("digest" = String, Path, description = "blake3 hex digest")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "raw blob bytes — must hash to `digest`"),
    summary = "upload one content-addressed blob",
    responses(
        (status = 201, description = "blob stored"),
        (status = 204, description = "blob already present"),
        (status = "4XX", description = "client error", body = ErrorBody)
    ))]
async fn put_blob(
    State(state): State<Arc<GatewayState>>,
    AxumPath((id, digest)): AxumPath<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    require_repo(&state, &id)?;
    let actual = blake3::hash(&body).to_hex().to_string();
    if actual != digest {
        return Err(bad_request(format!(
            "blob digest mismatch: declared {digest}, content hashes to {actual}"
        )));
    }
    let path = blob_path(&state, &digest)?;
    if path.exists() {
        return Ok(StatusCode::NO_CONTENT);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(ApiError::from)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &body).map_err(ApiError::from)?;
    std::fs::rename(tmp, path).map_err(ApiError::from)?;
    Ok(StatusCode::CREATED)
}

#[utoipa::path(post, path = "/v1/repos/{id}/push", tag = "repos", params(("id" = String, Path, description = "repo id or slug")),
    request_body = PushRequest,
    summary = "commit a manifest: materialize source + kick worker index",
    responses(
        (status = 200, body = PushResponse),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 500, description = "internal error", body = ErrorBody)
    ))]
async fn push(
    State(state): State<Arc<GatewayState>>,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let repo = require_repo(&state, &id)?;
    if input.files.len() > 500_000 {
        return Err(bad_request("push manifest exceeds 500k files"));
    }
    // Verify every blob is present before touching the source dir — a
    // partial materialization would index a truncated worktree.
    let mut missing = Vec::new();
    for file in &input.files {
        safe_relpath(&file.path)?;
        if !blob_path(&state, &file.digest)?.exists() {
            missing.push(file.digest.clone());
        }
    }
    if !missing.is_empty() {
        return Err(bad_request(format!(
            "{} blobs missing — upload them via PUT /v1/repos/{id}/blobs/{{digest}} first",
            missing.len()
        )));
    }
    // Serialize materialization per repo: the shared staging dir and the
    // remove+rename swap are not safe under concurrent pushes.
    let push_lock = {
        let mut locks = state.push_locks.lock();
        locks.entry(repo.id.clone()).or_default().clone()
    };
    let _push_permit = push_lock.lock().await;
    let source_dir = state.data_dir.join("sources").join(&repo.slug);
    let staging = state
        .data_dir
        .join("sources")
        .join(format!(".{}", repo.slug));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(ApiError::from)?;
    }
    std::fs::create_dir_all(&staging).map_err(ApiError::from)?;
    let mut bytes_total = 0_u64;
    for file in &input.files {
        let rel = safe_relpath(&file.path)?;
        let target = staging.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(ApiError::from)?;
        }
        std::fs::copy(blob_path(&state, &file.digest)?, &target).map_err(ApiError::from)?;
        bytes_total += std::fs::metadata(&target).map_err(ApiError::from)?.len();
    }
    if source_dir.exists() {
        std::fs::remove_dir_all(&source_dir).map_err(ApiError::from)?;
    }
    std::fs::rename(&staging, &source_dir).map_err(ApiError::from)?;
    let record = PushRecord {
        push_id: format!("push_{}", uuid::Uuid::now_v7().simple()),
        at: chrono::Utc::now().to_rfc3339(),
        revision: input.revision.clone(),
        file_count: input.files.len() as u64,
        bytes: bytes_total,
    };
    {
        let mut registry = state.registry.lock();
        if let Some(entry) = registry.get_mut(&id) {
            entry.last_push = Some(record.clone());
        }
    }
    persist_registry(&state)?;
    // Kick the worker's index in the background — push returns once the
    // source is materialized; indexing reports via the worker's own status.
    let client = state.client.clone();
    let worker_url = repo.worker_url;
    tokio::spawn(async move {
        match client.post(format!("{worker_url}/v1/index")).send().await {
            Ok(response) if response.status().is_success() => {
                tracing::info!(worker = %worker_url, "worker index triggered");
            }
            Ok(response) => {
                tracing::warn!(worker = %worker_url, status = %response.status(), "worker index rejected");
            }
            Err(error) => {
                tracing::warn!(worker = %worker_url, %error, "worker index trigger failed");
            }
        }
    });
    Ok(Json(PushResponse {
        push_id: record.push_id,
        materialized: record.file_count,
        bytes: record.bytes,
    }))
}

/// Repo lookup accepts the canonical id or the human-readable slug — proxy
/// URLs like `/my-project/v1/search` stay readable.
fn require_repo(state: &GatewayState, id: &str) -> Result<RepoEntry, ApiError> {
    let registry = state.registry.lock();
    if let Some(entry) = registry.get(id) {
        return Ok(entry.clone());
    }
    registry
        .values()
        .find(|entry| entry.slug == id || entry.name == id)
        .cloned()
        .ok_or_else(|| not_found(format!("unknown repo {id}")))
}

/// `/{repo}/v1/*` → `{worker_url}/v1/*` verbatim — method, body, and
/// content-type pass through untouched so every daemon endpoint works
/// unchanged behind the gateway.
#[utoipa::path(get, path = "/{repo}/v1/{path}", tag = "repos", params(("repo" = String, Path, description = "repo id, slug, or name"), ("path" = String, Path, description = "worker API path — `{path}` is a suffix wildcard, e.g. `search`, `status`, `def/{name}`")),
    summary = "worker API proxy — every method and path depth forwards verbatim to the repo's cce-daemon (GET shown here as representative; see the cce-daemon OpenAPI doc for upstream request/response schemas)",
    responses(
        (status = 200, description = "upstream worker response"),
        (status = 404, description = "unknown repo", body = ErrorBody),
        (status = 502, description = "worker unreachable", body = ErrorBody)
    ))]
async fn proxy(
    State(state): State<Arc<GatewayState>>,
    AxumPath((repo, path)): AxumPath<(String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let entry = require_repo(&state, &repo)?;
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let url = format!("{}/v1/{path}{query}", entry.worker_url);
    let method = request.method().clone();
    let headers = request.headers().clone();
    let body = axum::body::to_bytes(request.into_body(), 64 * 1024 * 1024)
        .await
        .map_err(ApiError::from)?;
    let mut upstream = state.client.request(method, &url).body(body);
    for (name, value) in &headers {
        if matches!(
            name.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        ) {
            continue;
        }
        upstream = upstream.header(name, value);
    }
    let response = upstream.send().await.map_err(|error| {
        ApiError(
            StatusCode::BAD_GATEWAY,
            format!("worker {} unreachable: {error}", entry.worker_url),
        )
    })?;
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.bytes().await.map_err(ApiError::from)?;
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
    }
    builder
        .body(axum::body::Body::from(body))
        .map_err(ApiError::from)
}
