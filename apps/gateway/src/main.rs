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
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use clap::Parser;
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi as _, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

mod breaker;
mod cache;
mod fanout;
mod mcp;
mod metrics;

/// In-flight upstream calls the gateway allows across proxy and fan-out;
/// requests beyond it queue for at most `UPSTREAM_QUEUE_WAIT` before a
/// 503 shed — bounded work instead of unbounded piling onto workers.
const UPSTREAM_CONCURRENCY: usize = 64;
/// Per-worker in-flight bound: one slow or popular worker queues its own
/// callers but cannot consume every global slot and starve other repos.
const WORKER_CONCURRENCY: usize = 16;
const UPSTREAM_QUEUE_WAIT: Duration = Duration::from_secs(2);

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

/// `GET /v1/repos/{id}` response: the registry entry plus a live probe of
/// its worker's liveness endpoint. `flatten` keeps the wire shape
/// identical to `RepoEntry` with one additive field.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RepoDetail {
    #[serde(flatten)]
    entry: RepoEntry,
    /// `healthy` | `unreachable` | `timeout` — see `probe_worker`.
    worker_status: &'static str,
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
    /// Search-response cache: replay is only valid because pushes are
    /// the sole content-mutation channel — see `cache` module docs.
    search_cache: cache::SearchCache,
    /// Bounds in-flight upstream calls: every proxied request and every
    /// fan-out branch must hold a permit before calling a worker. A
    /// request that cannot queue within `UPSTREAM_QUEUE_WAIT` is shed
    /// with 503 rather than piling unbounded work onto the workers.
    upstream_permits: Arc<tokio::sync::Semaphore>,
    /// Per-worker semaphores (`WORKER_CONCURRENCY` each) created lazily
    /// by repo id — noisy-neighbor isolation ahead of the global bound.
    /// Entries live as long as the process; the map is bounded by the
    /// registry's repo count.
    worker_permits: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
    /// Per-worker circuits: repeated reachability failures fast-fail
    /// upstream calls instead of burning a timeout per request. Cache
    /// hits bypass the circuit — a dead worker still serves stale-but-
    /// valid cached content.
    breakers: breaker::Breakers,
    /// Process metrics — atomic counters; see `metrics` module docs.
    metrics: metrics::Metrics,
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

impl IntoResponse for ApiError {
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

/// Milliseconds since `started`, for tracing fields — saturates instead
/// of truncating `Duration`'s u128 millisecond count.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
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
        // Upstream calls must never hang the front door: 5s to establish
        // the connection, 120s end-to-end for proxied requests and the
        // index kick. Per-request `.timeout()` (e.g. the 2s worker health
        // probe) overrides the overall cap. Idle keep-alive connections
        // are pooled per worker so steady traffic skips TCP/TLS setup.
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_mins(2))
            .pool_max_idle_per_host(32)
            .tcp_keepalive(Duration::from_mins(1))
            .build()?,
        registry: parking_lot::Mutex::new(registry),
        push_locks: parking_lot::Mutex::new(HashMap::new()),
        search_cache: cache::SearchCache::new(Duration::from_mins(5), 512),
        upstream_permits: Arc::new(tokio::sync::Semaphore::new(UPSTREAM_CONCURRENCY)),
        worker_permits: parking_lot::Mutex::new(HashMap::new()),
        // Three consecutive reachability failures open a worker's circuit
        // for 30s; a single half-open probe then decides reopen-or-close.
        breakers: breaker::Breakers::new(3, Duration::from_secs(30)),
        metrics: metrics::Metrics::new(),
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
        .routes(routes!(fanout::search_all))
        .routes(routes!(mcp::endpoint))
        .split_for_parts();
    let mut router = api_router
        .merge(SwaggerUi::new("/docs").url("/openapi.json", api))
        .route("/metrics", axum::routing::get(metrics_prometheus))
        .route("/v1/metrics", axum::routing::get(metrics_json))
        .route("/{repo}/v1/{*path}", any(proxy))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        // Gzip responses for clients that ask — search JSON is verbose
        // and repeats, so compression buys 5-10x wire reduction. Applied
        // after body-limit so request sizes are unaffected.
        .layer(tower_http::compression::CompressionLayer::new())
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

/// SIGINT (Ctrl-C) or SIGTERM (docker stop, K8s pod termination) both
/// drain in-flight connections — a hard kill mid-proxy would surface as
/// truncated responses to callers.
async fn shutdown_signal() {
    let control_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl-C handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = control_c => {},
        () = terminate => {},
    }
}

/// Releases a singleflight leader slot on drop: every proxy exit path
/// after claiming leadership — early `?` returns included — wakes its
/// followers instead of stranding them.
struct FlightDone<'a> {
    cache: &'a cache::SearchCache,
    key: (String, String),
}

impl Drop for FlightDone<'_> {
    fn drop(&mut self) {
        self.cache.finish_flight(&self.key.0, &self.key.1);
    }
}

/// Guard holding both admission permits; drops release in reverse order
/// (global first, then the worker slot).
struct UpstreamPermits {
    _worker: tokio::sync::OwnedSemaphorePermit,
    _global: tokio::sync::OwnedSemaphorePermit,
}

/// Wait up to `UPSTREAM_QUEUE_WAIT` for an upstream permit — first the
/// target worker's own bound, then the global one. On timeout the
/// request is shed with 503 + `Retry-After` so callers can tell gateway
/// overload apart from worker failure. Worker-first ordering keeps one
/// saturated worker from holding global slots while its callers queue.
async fn acquire_upstream(state: &GatewayState, worker: &str) -> Result<UpstreamPermits, Response> {
    let shed = |error: &str| {
        state.metrics.record_proxy("shed");
        Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("retry-after", "2")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&ErrorBody {
                    error: error.to_owned(),
                })
                .unwrap_or_default(),
            ))
            .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
    };
    let worker_semaphore = {
        let mut map = state.worker_permits.lock();
        map.entry(worker.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(WORKER_CONCURRENCY)))
            .clone()
    };
    let Ok(Ok(worker)) =
        tokio::time::timeout(UPSTREAM_QUEUE_WAIT, worker_semaphore.acquire_owned()).await
    else {
        return Err(shed("worker busy: too many in-flight calls for this repo"));
    };
    let Ok(Ok(global)) = tokio::time::timeout(
        UPSTREAM_QUEUE_WAIT,
        state.upstream_permits.clone().acquire_owned(),
    )
    .await
    else {
        return Err(shed("gateway saturated: too many upstream calls"));
    };
    Ok(UpstreamPermits {
        _worker: worker,
        _global: global,
    })
}

#[utoipa::path(get, path = "/healthz", tag = "meta",
    responses((status = 200, description = "liveness probe", body = HealthStatus)))]
async fn health() -> Json<HealthStatus> {
    Json(HealthStatus { status: "ok" })
}

/// `GET /metrics` — Prometheus text exposition. Lives on the outer
/// router next to the proxy wildcard, not in the `OpenAPI` surface.
async fn metrics_prometheus(State(state): State<Arc<GatewayState>>) -> Response {
    Response::builder()
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )
        .body(axum::body::Body::from(state.metrics.render_prometheus()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `GET /v1/metrics` — the same series as JSON for dashboards/scripts,
/// plus the per-worker circuit-breaker states (`open`/`half_open`/`closed`).
async fn metrics_json(State(state): State<Arc<GatewayState>>) -> Json<serde_json::Value> {
    let mut snapshot = state.metrics.snapshot();
    let breakers: serde_json::Map<String, serde_json::Value> = state
        .registry
        .lock()
        .keys()
        .map(|id| {
            (
                id.clone(),
                serde_json::Value::from(state.breakers.state_name(id)),
            )
        })
        .collect();
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("breakers".to_owned(), serde_json::Value::Object(breakers));
    }
    Json(snapshot)
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

/// One manifest entry resolved for ingest: `rel` is the validated
/// repo-relative target path, `blob` its content-addressed source. Built
/// once per file — the same plan feeds the presence check and
/// materialization, so validation never runs twice.
#[derive(Debug)]
struct StagedFile {
    rel: PathBuf,
    blob: PathBuf,
    digest: String,
}

fn plan_entry(state: &GatewayState, path: &str, digest: &str) -> Result<StagedFile, ApiError> {
    Ok(StagedFile {
        rel: safe_relpath(path)?,
        blob: blob_path(state, digest)?,
        digest: digest.to_owned(),
    })
}

/// Stat every blob in `plan`, chunked across up to 16 OS threads — a
/// serial `exists()` loop over a 500k-file manifest can take seconds on a
/// cold page cache. Must run on `spawn_blocking`, never on the executor.
/// Returns the missing digests, sorted + deduped.
fn missing_blobs(plan: &[StagedFile]) -> Result<Vec<String>, ApiError> {
    const MAX_WORKERS: usize = 16;
    if plan.is_empty() {
        return Ok(Vec::new());
    }
    let chunk_size = plan.len().div_ceil(MAX_WORKERS.min(plan.len()));
    let mut missing = std::thread::scope(|scope| {
        let handles: Vec<_> = plan
            .chunks(chunk_size)
            .map(|files| {
                scope.spawn(move || {
                    files
                        .iter()
                        .filter(|file| !file.blob.exists())
                        .map(|file| file.digest.clone())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut missing = Vec::new();
        for handle in handles {
            // The panic itself was already reported by the panic hook;
            // surface it to the caller as a 500 rather than unwrap.
            let found = handle.join().map_err(|_| {
                ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "blob scan worker panicked".to_owned(),
                )
            })?;
            missing.extend(found);
        }
        Ok::<_, ApiError>(missing)
    })?;
    missing.sort();
    missing.dedup();
    Ok(missing)
}

/// Fill `staging` with links to CAS blobs, then swap it over `source_dir`
/// — the worker never sees a half-written source tree. CAS objects are
/// immutable, so same-filesystem hardlinks are safe and O(1) (the `OSTree`
/// pattern); fall back to a byte copy when the blob store lives on a
/// different filesystem. Runs on a `spawn_blocking` thread.
fn materialize(plan: &[StagedFile], staging: &Path, source_dir: &Path) -> Result<u64, ApiError> {
    if staging.exists() {
        std::fs::remove_dir_all(staging).map_err(ApiError::from)?;
    }
    std::fs::create_dir_all(staging).map_err(ApiError::from)?;
    let mut bytes_total = 0_u64;
    for file in plan {
        let target = staging.join(&file.rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(ApiError::from)?;
        }
        if std::fs::hard_link(&file.blob, &target).is_err() {
            std::fs::copy(&file.blob, &target).map_err(ApiError::from)?;
        }
        bytes_total += std::fs::metadata(&target).map_err(ApiError::from)?.len();
    }
    if source_dir.exists() {
        std::fs::remove_dir_all(source_dir).map_err(ApiError::from)?;
    }
    std::fs::rename(staging, source_dir).map_err(ApiError::from)?;
    Ok(bytes_total)
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
    summary = "repo detail incl. last push + worker liveness",
    responses(
        (status = 200, body = RepoDetail),
        (status = "4XX", description = "client error", body = ErrorBody),
        (status = 500, description = "internal error", body = ErrorBody)
    ))]
async fn repo_detail(
    State(state): State<Arc<GatewayState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RepoDetail>, ApiError> {
    // Registry lock is released before the network probe — never hold a
    // sync mutex across .await.
    let entry = state
        .registry
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| not_found(format!("unknown repo {id}")))?;
    let worker_status = probe_worker(&state.client, &entry.worker_url).await;
    Ok(Json(RepoDetail {
        entry,
        worker_status,
    }))
}

/// Best-effort liveness probe of a repo's worker — never fails the detail
/// request, the verdict is reported in the `workerStatus` field instead.
/// cce-daemon serves liveness at `/healthz`; the 2s per-request timeout
/// caps the probe so a dead worker can't stall the detail call.
async fn probe_worker(client: &reqwest::Client, worker_url: &str) -> &'static str {
    match client
        .get(format!("{worker_url}/healthz"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => "healthy",
        // A non-2xx answer proves reachability but means the worker is
        // not serving normally — report it in the failure bucket.
        Ok(_) => "unreachable",
        Err(error) if error.is_timeout() => "timeout",
        Err(_) => "unreachable",
    }
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
    let plan: Vec<StagedFile> = input
        .files
        .iter()
        .map(|file| plan_entry(&state, &file.path, &file.digest))
        .collect::<Result<_, _>>()?;
    // The stat scan is parallelized filesystem I/O — keep it off the
    // async executor.
    let missing = tokio::task::spawn_blocking(move || missing_blobs(&plan))
        .await
        .map_err(ApiError::from)??;
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
    let path = blob_path(&state, &digest)?;
    // Hashing a blob up to the 64MB body limit plus the CAS write are
    // blocking work — run the whole store op on a blocking thread.
    tokio::task::spawn_blocking(move || -> Result<StatusCode, ApiError> {
        let actual = blake3::hash(&body).to_hex().to_string();
        if actual != digest {
            return Err(bad_request(format!(
                "blob digest mismatch: declared {digest}, content hashes to {actual}"
            )));
        }
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
    })
    .await
    .map_err(ApiError::from)?
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
    // Outcome + wall time are recorded for every push — validation
    // rejections count as failures with zero files.
    let started = Instant::now();
    let result = push_inner(&state, &id, &input).await;
    let files = result.as_ref().map_or(0, |ok| ok.materialized);
    state
        .metrics
        .record_push(result.is_ok(), files, elapsed_ms(started));
    result
}

async fn push_inner(
    state: &GatewayState,
    id: &str,
    input: &PushRequest,
) -> Result<Json<PushResponse>, ApiError> {
    let repo = require_repo(state, id)?;
    if input.files.len() > 500_000 {
        return Err(bad_request("push manifest exceeds 500k files"));
    }
    let started = Instant::now();
    // Validate the manifest once — the resolved plan feeds both the
    // presence check and materialization below.
    let plan = Arc::new(
        input
            .files
            .iter()
            .map(|file| plan_entry(state, &file.path, &file.digest))
            .collect::<Result<Vec<_>, _>>()?,
    );
    // Verify every blob is present before touching the source dir — a
    // partial materialization would index a truncated worktree. The stat
    // scan runs on blocking threads: 500k serial exists() calls can take
    // seconds and would stall the async executor.
    let missing = {
        let plan = Arc::clone(&plan);
        tokio::task::spawn_blocking(move || missing_blobs(&plan))
            .await
            .map_err(ApiError::from)??
    };
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
    // Staging fill + swap happen inside one blocking call — hardlink
    // fan-out over a huge manifest is fast but still synchronous fs I/O.
    let bytes_total = {
        let plan = Arc::clone(&plan);
        tokio::task::spawn_blocking(move || materialize(&plan, &staging, &source_dir))
            .await
            .map_err(ApiError::from)??
    };
    let record = PushRecord {
        push_id: format!("push_{}", uuid::Uuid::now_v7().simple()),
        at: chrono::Utc::now().to_rfc3339(),
        revision: input.revision.clone(),
        file_count: input.files.len() as u64,
        bytes: bytes_total,
    };
    {
        let mut registry = state.registry.lock();
        if let Some(entry) = registry.get_mut(id) {
            entry.last_push = Some(record.clone());
        }
    }
    persist_registry(state)?;
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
    tracing::info!(
        repo = %repo.id,
        files = record.file_count,
        bytes = record.bytes,
        elapsed_ms = elapsed_ms(started),
        "push materialized"
    );
    // New content committed — every cached search response for this repo
    // is stale as of now. This is the cache's exact invalidation point.
    state.search_cache.invalidate_repo(&repo.id);
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
        (status = 502, description = "worker unreachable", body = ErrorBody),
        (status = 504, description = "worker timed out", body = ErrorBody)
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
    // Search-response cache: POST /v1/search only — the expensive engine
    // path — keyed on the exact request body. A hit replays the stored
    // response verbatim; push invalidates the repo's entries (see
    // `cache` module docs for why that is exact, not heuristic).
    let cacheable = method == axum::http::Method::POST && path == "search" && query.is_empty();
    let body_digest = cacheable.then(|| blake3::hash(&body).to_hex().to_string());
    // Singleflight: a cacheable miss claims the in-flight slot or follows
    // the leader already running it. Followers wait for the leader's
    // finish, then replay what it stored — a stampede on a hot query
    // costs one upstream call. The leader path calls `finish_flight` on
    // every exit below so followers can never hang.
    // `Some` only while this request holds the singleflight leader slot;
    // the guard's Drop wakes followers on every exit path below.
    let _flight_done = if let Some(digest) = &body_digest {
        if let Some((cached_body, cached_type)) = state.search_cache.get(&entry.id, digest) {
            state.metrics.record_proxy("cache_hit");
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header("x-cce-cache", "hit");
            if let Some(content_type) = cached_type {
                builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
            }
            return builder
                .body(axum::body::Body::from(cached_body))
                .map_err(ApiError::from);
        }
        match state.search_cache.begin_flight(&entry.id, digest) {
            cache::Flight::Leader => Some(FlightDone {
                cache: &state.search_cache,
                key: (entry.id.clone(), digest.clone()),
            }),
            cache::Flight::Follower(mut receiver) => {
                // Sender dropped at finish_flight resolves changed() as
                // Err — Ok and Err mean the same here: re-check the cache.
                let _ = receiver.changed().await;
                if let Some((cached_body, cached_type)) = state.search_cache.get(&entry.id, digest)
                {
                    state.metrics.record_proxy("singleflight");
                    let mut builder = Response::builder()
                        .status(StatusCode::OK)
                        .header("x-cce-cache", "hit")
                        .header("x-cce-flight", "follower");
                    if let Some(content_type) = cached_type {
                        builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
                    }
                    return builder
                        .body(axum::body::Body::from(cached_body))
                        .map_err(ApiError::from);
                }
                // Leader failed — this follower fetches upstream itself.
                None
            }
        }
    } else {
        None
    };
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
    // A cacheable request reaching this line is fetching upstream —
    // leader or self-relying follower; both count as misses.
    if body_digest.is_some() {
        state.metrics.record_proxy("cache_miss");
    }
    // Bound in-flight upstream work: queue briefly, then shed — the
    // permit covers the whole upstream call including the body read.
    // The permit comes BEFORE the circuit check: an admit obligates an
    // outcome report, so shedding first keeps a queued-out request from
    // stranding a half-open probe permit.
    let _permits = match acquire_upstream(&state, &entry.id).await {
        Ok(permit) => permit,
        Err(shed) => return Ok(shed),
    };
    // Circuit check after the cache: a worker whose circuit is open
    // fails fast — no timeout wait per request. Cache hits already
    // returned above, so a broken worker still serves cached content.
    if !state.breakers.admit(&entry.id) {
        state.metrics.record_proxy("breaker");
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("x-cce-breaker", "open")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&ErrorBody {
                    error: format!("worker {} circuit open — failing fast", entry.worker_url),
                })
                .unwrap_or_default(),
            ))
            .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response()));
    }
    let started = Instant::now();
    let response = upstream.send().await.map_err(|error| {
        state.breakers.on_failure(&entry.id);
        state.metrics.record_upstream_ms(elapsed_ms(started));
        tracing::warn!(
            repo = %entry.id,
            latency_ms = elapsed_ms(started),
            %error,
            "worker upstream request failed"
        );
        if error.is_timeout() {
            state.metrics.record_proxy("timeout");
            ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                format!("worker {} timed out: {error}", entry.worker_url),
            )
        } else if error.is_connect() {
            state.metrics.record_proxy("connect");
            ApiError(
                StatusCode::BAD_GATEWAY,
                format!("worker {} connection failed: {error}", entry.worker_url),
            )
        } else {
            state.metrics.record_proxy("other");
            ApiError(
                StatusCode::BAD_GATEWAY,
                format!("worker {} unreachable: {error}", entry.worker_url),
            )
        }
    })?;
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // A worker that dies mid-response is an upstream failure too — same
    // classification as the send path above, and it counts against the
    // circuit even though the status line already arrived.
    let body = response.bytes().await.map_err(|error| {
        state.breakers.on_failure(&entry.id);
        state.metrics.record_upstream_ms(elapsed_ms(started));
        tracing::warn!(
            repo = %entry.id,
            latency_ms = elapsed_ms(started),
            %error,
            "worker response body failed"
        );
        if error.is_timeout() {
            state.metrics.record_proxy("timeout");
            ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                format!(
                    "worker {} timed out mid-response: {error}",
                    entry.worker_url
                ),
            )
        } else {
            state.metrics.record_proxy("other");
            ApiError(
                StatusCode::BAD_GATEWAY,
                format!("worker {} dropped the response: {error}", entry.worker_url),
            )
        }
    })?;
    // One outcome report per admitted call: a reached response reports
    // breaker health by status — anything under 500 means the worker
    // answered, 4xx included; a 5xx counts as failure even when its
    // body parsed fine.
    if status.is_server_error() {
        state.breakers.on_failure(&entry.id);
    } else {
        state.breakers.on_success(&entry.id);
    }
    state.metrics.record_upstream_ms(elapsed_ms(started));
    state.metrics.record_proxy("ok");
    if let Some(digest) = &body_digest {
        if status == StatusCode::OK {
            state
                .search_cache
                .put(&entry.id, digest, body.clone(), content_type.clone());
        }
    }
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
    }
    if body_digest.is_some() {
        builder = builder.header("x-cce-cache", "miss");
    }
    builder
        .body(axum::body::Body::from(body))
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cce-gateway-{tag}-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// The detail response must be the `RepoEntry` shape plus one
    /// additive field — no nesting, no renamed keys.
    #[test]
    fn repo_detail_serializes_flat_with_worker_status() {
        let detail = RepoDetail {
            entry: RepoEntry {
                id: "repo_abc".to_owned(),
                name: "Demo".to_owned(),
                slug: "demo".to_owned(),
                worker_url: "http://127.0.0.1:7700".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                last_push: None,
            },
            worker_status: "healthy",
        };
        let value = serde_json::to_value(&detail).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object["workerStatus"], serde_json::json!("healthy"));
        assert_eq!(
            object["workerUrl"],
            serde_json::json!("http://127.0.0.1:7700")
        );
        assert_eq!(object["id"], serde_json::json!("repo_abc"));
        assert!(!object.contains_key("entry"));
    }

    #[test]
    fn safe_relpath_rejects_traversal() {
        assert!(safe_relpath("").is_err());
        assert!(safe_relpath("/absolute").is_err());
        assert!(safe_relpath("../escape").is_err());
        assert!(safe_relpath("a/../b").is_err());
        assert!(safe_relpath("src/main.rs").is_ok());
    }

    #[test]
    fn missing_blobs_reports_absent_digests() {
        let root = temp_root("missing");
        let present = root.join("present");
        std::fs::write(&present, b"blob").unwrap();
        let plan = vec![
            StagedFile {
                rel: PathBuf::from("a.rs"),
                blob: present,
                digest: "aa".to_owned(),
            },
            StagedFile {
                rel: PathBuf::from("b.rs"),
                blob: root.join("absent"),
                digest: "bb".to_owned(),
            },
        ];
        assert_eq!(missing_blobs(&plan).unwrap(), vec!["bb".to_owned()]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Materialization hardlinks CAS blobs (same inode on unix) and swaps
    /// the staging dir over the source dir atomically.
    #[test]
    fn materialize_links_blobs_into_source() {
        let root = temp_root("materialize");
        let blob = root.join("blob");
        std::fs::write(&blob, b"fn main() {}\n").unwrap();
        let plan = vec![StagedFile {
            rel: PathBuf::from("src").join("main.rs"),
            blob: blob.clone(),
            digest: "dd".to_owned(),
        }];
        let staging = root.join(".staging");
        let source = root.join("source");
        let bytes = materialize(&plan, &staging, &source).unwrap();
        let target = source.join("src").join("main.rs");
        assert_eq!(std::fs::read(&target).unwrap(), b"fn main() {}\n");
        assert_eq!(bytes, 13);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&target).unwrap().ino(),
                std::fs::metadata(&blob).unwrap().ino()
            );
        }
        // Re-materializing swaps over the existing source dir cleanly.
        materialize(&plan, &staging, &source).unwrap();
        assert!(target.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
