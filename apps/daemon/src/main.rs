#![forbid(unsafe_code)]

//! `cce-daemon` HTTP service: the engine API over `/v1/*` plus the web
//! dashboard's static assets, bound to loopback by default. `--watch`
//! reindexes automatically when the worktree or HEAD moves.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{FromRef, Path, Query, State},
    http::{HeaderName, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use cce_core::{QueryIntent, SearchRequest, SearchRoute};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DenseBackendConfig, EngineConfig,
};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

mod watch;

/// Request state. Most handlers need only the engine — the `FromRef`
/// impl keeps every `State<Arc<CceEngine>>` extractor working; handlers
/// whose freshness behaviour depends on `--watch` take the whole state.
#[derive(Clone)]
struct DaemonState {
    engine: Arc<CceEngine>,
    /// `--watch` active: a background poller reindexes when the snapshot
    /// identity moves, so freshness-demanding requests may serve the
    /// committed snapshot instead of rescanning the worktree per query.
    watch_keeps_fresh: bool,
}

impl FromRef<DaemonState> for Arc<CceEngine> {
    fn from_ref(state: &DaemonState) -> Self {
        Self::clone(&state.engine)
    }
}

#[derive(Debug, Parser)]
#[command(name = "cce-daemon", version)]
struct Arguments {
    #[arg(default_value = ".")]
    repository: PathBuf,
    #[arg(long, env = "CCE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:7734")]
    bind: SocketAddr,
    #[arg(long)]
    allow_non_loopback: bool,
    #[arg(long)]
    web_root: Option<PathBuf>,
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
    /// Skip external code-intelligence providers (SCIP indexers) during
    /// indexing.
    #[arg(long, env = "CCE_NO_PROVIDERS")]
    no_providers: bool,
    /// Keep the index fresh: rescan every --watch-interval and reindex when
    /// the snapshot identity (worktree content + HEAD + index profile)
    /// moves.
    #[arg(long, env = "CCE_WATCH")]
    watch: bool,
    /// Poll interval for --watch, in seconds.
    #[arg(long, env = "CCE_WATCH_INTERVAL", default_value_t = 15)]
    watch_interval: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    /// In-process ONNX model via fastembed; downloads model files once, then offline.
    Local,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SearchInput {
    query: String,
    intent: Option<QueryIntent>,
    #[serde(default = "default_search_limit")]
    limit: usize,
    /// Route pins for ablation-style queries; empty follows the planner.
    #[serde(default)]
    routes: Vec<SearchRoute>,
    /// Structured `lang:`/`path:` filters; when unset the daemon parses
    /// them out of the query text like the CLI does.
    #[serde(default)]
    filters: cce_core::QueryFilters,
}

const fn default_search_limit() -> usize {
    20
}

/// `POST /v1/grep` request: a worktree regex, not an index query.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct GrepInput {
    /// Rust regex pattern.
    pattern: String,
    /// Optional path-prefix filter.
    #[serde(default)]
    path_prefix: Option<String>,
    /// Optional language filter.
    #[serde(default)]
    language: Option<String>,
    /// ASCII case-insensitive matching.
    #[serde(default)]
    ignore_case: bool,
    #[serde(default = "default_grep_limit")]
    limit: usize,
}

const fn default_grep_limit() -> usize {
    cce_engine::DEFAULT_GREP_LIMIT
}

/// `POST /v1/diff` request: a query-time regex over stored commit patches
/// (Sourcegraph `type:diff`); bounded by history indexing, not the worktree.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct DiffInput {
    /// Rust regex pattern matched against `+`/`-` patch lines.
    pattern: String,
    #[serde(default = "default_diff_limit")]
    limit: usize,
}

const fn default_diff_limit() -> usize {
    cce_engine::DEFAULT_DIFF_LIMIT
}

#[derive(Debug, Serialize, ToSchema)]
struct Health {
    status: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
struct ApiErrorBody {
    error: String,
}

#[derive(Debug)]
struct ApiError(cce_core::CceError);

impl From<cce_core::CceError> for ApiError {
    fn from(value: cce_core::CceError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            cce_core::CceError::InvalidRepository(_)
            | cce_core::CceError::Configuration(_)
            | cce_core::CceError::InvalidSourceRange { .. } => StatusCode::BAD_REQUEST,
            cce_core::CceError::ViewUnavailable { .. } => StatusCode::CONFLICT,
            cce_core::CceError::Cancelled => StatusCode::REQUEST_TIMEOUT,
            cce_core::CceError::IndexBusy(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ApiErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(info(
    title = "cce-daemon",
    description = "CCE worker API — one repository per process. Served unchanged behind a cce-gateway as `/{repo}/v1/*`."
))]
struct ApiDoc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cce=info,tower_http=info".into()),
        )
        .json()
        .init();
    let arguments = Arguments::parse();
    if !arguments.bind.ip().is_loopback() && !arguments.allow_non_loopback {
        anyhow::bail!("refusing non-loopback bind without --allow-non-loopback");
    }
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
    config.providers.enabled = !arguments.no_providers;
    let engine = Arc::new(CceEngine::open(config)?);
    if arguments.watch {
        // Floor at 1s so --watch-interval 0 cannot hot-spin the scanner.
        let interval = Duration::from_secs(arguments.watch_interval.max(1));
        tracing::info!(interval_secs = interval.as_secs(), "watch mode enabled");
        tokio::spawn(watch::run(Arc::clone(&engine), interval));
    }
    let state = DaemonState {
        engine,
        watch_keeps_fresh: arguments.watch,
    };
    let request_id = HeaderName::from_static("x-request-id");
    let (api_router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(index))
        .routes(routes!(checkpoint))
        .routes(routes!(events))
        .routes(routes!(status))
        .routes(routes!(search))
        .routes(routes!(context))
        .routes(routes!(codebase_map))
        .routes(routes!(explain))
        .routes(routes!(impact))
        .routes(routes!(definitions))
        .routes(routes!(references))
        .routes(routes!(providers))
        .routes(routes!(grep))
        .routes(routes!(diff))
        .routes(routes!(diff_architecture))
        .routes(routes!(export))
        .routes(routes!(export_entities))
        .routes(routes!(export_relations))
        .routes(routes!(export_regions))
        .routes(routes!(files))
        .routes(routes!(file_source))
        .split_for_parts();
    let mut router = api_router
        // SwaggerUi serves the generated OpenAPI JSON itself at this URL —
        // no handwritten spec anywhere in the pipeline.
        .merge(SwaggerUi::new("/docs").url("/openapi.json", api))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    if let Some(web_root) = arguments.web_root {
        // SPA fallback: client-side routes (/query, /browse/<path>, …) have
        // no file on disk — serve index.html and let the router resolve.
        router = router.fallback_service(
            ServeDir::new(&web_root)
                .append_index_html_on_directories(true)
                .not_found_service(ServeFile::new(web_root.join("index.html"))),
        );
    }
    let listener = tokio::net::TcpListener::bind(arguments.bind).await?;
    tracing::info!(address = %arguments.bind, "CCE daemon listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[utoipa::path(get, path = "/healthz", tag = "meta",
    responses((status = 200, description = "liveness probe", body = Health)))]
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// `POST /v1/index` / `/v1/checkpoint` query — who produced the snapshot.
/// Attribution is review context, never snapshot identity.
#[derive(Debug, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
struct OriginQuery {
    /// Producer label — `cli`, `watch`, an MCP client name, `checkpoint`.
    origin: Option<String>,
}

#[utoipa::path(post, path = "/v1/index", tag = "worker",
    summary = "reindex the worktree",
    params(OriginQuery),
    responses(
        (status = 200, body = cce_engine::IndexReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn index(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<OriginQuery>,
) -> Result<Json<cce_engine::IndexReport>, ApiError> {
    Ok(Json(engine.index_with_origin(query.origin).await?))
}

#[utoipa::path(post, path = "/v1/checkpoint", tag = "worker",
    summary = "commit a detached parse+relations snapshot for diffing",
    description = "Indexes the worktree without providers, dense, or zoekt \
        and commits it under a checkpoint profile: a distinct snapshot id \
        that never promotes to `current`. Pair with `GET \
        /v1/diff/architecture?head=<snapshot>` to review what a session \
        changed without moving the serving view.",
    params(OriginQuery),
    responses(
        (status = 200, body = cce_engine::IndexReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn checkpoint(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<OriginQuery>,
) -> Result<Json<cce_engine::IndexReport>, ApiError> {
    Ok(Json(engine.checkpoint(query.origin).await?))
}

#[utoipa::path(get, path = "/v1/events", tag = "worker",
    summary = "snapshot lifecycle stream (SSE)",
    description = "One `snapshot` event per commit or activation made by \
        this daemon — the signal to re-poll status/map/diff instead of a \
        timer. Commits from other processes (a bare `cce index`) do not \
        emit here. A `lagged` event reports how many broadcasts were \
        dropped; receivers should re-fetch state rather than replay.",
    responses(
        (status = 200, description = "text/event-stream of SnapshotEvent")
    ))]
async fn events(
    State(engine): State<Arc<CceEngine>>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = engine.subscribe_snapshots();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some((Ok(Event::default().event("snapshot").data(data)), rx))
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => Some((
                Ok(Event::default().event("lagged").data(skipped.to_string())),
                rx,
            )),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
        }
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

#[utoipa::path(get, path = "/v1/status", tag = "worker",
    summary = "snapshot + view freshness manifest",
    responses(
        (status = 200, body = cce_core::ViewManifest),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn status(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<cce_core::ViewManifest>, ApiError> {
    Ok(Json(engine.status()?))
}

#[utoipa::path(post, path = "/v1/search", tag = "worker",
    request_body = SearchInput,
    summary = "hybrid code search",
    responses(
        (status = 200, body = cce_engine::SearchResult),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn search(
    State(state): State<DaemonState>,
    Json(input): Json<SearchInput>,
) -> Result<Json<cce_engine::SearchResult>, ApiError> {
    Ok(Json(
        state
            .engine
            .search(SearchRequest {
                repository_id: String::new(),
                snapshot_id: String::new(),
                query: input.query,
                intent: input.intent,
                limit: input.limit.clamp(1, 200),
                // Under --watch the poller owns freshness — serve the
                // committed snapshot instead of rescanning per request.
                require_fresh: !state.watch_keeps_fresh,
                routes: input.routes,
                filters: input.filters,
            })
            .await?,
    ))
}

#[utoipa::path(post, path = "/v1/context", tag = "worker",
    request_body = ContextRequest,
    summary = "packed context for an agent prompt",
    responses(
        (status = 200, body = cce_core::ContextPack),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn context(
    State(engine): State<Arc<CceEngine>>,
    Json(mut request): Json<ContextRequest>,
) -> Result<Json<cce_core::ContextPack>, ApiError> {
    request.budget_tokens = request.budget_tokens.clamp(256, 128_000);
    request.max_candidates = request.max_candidates.clamp(1, 500);
    Ok(Json(engine.context(request).await?))
}

#[utoipa::path(get, path = "/v1/map", tag = "worker",
    summary = "package-level architecture map",
    responses(
        (status = 200, body = cce_engine::CodebaseMap),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn codebase_map(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<cce_engine::CodebaseMap>, ApiError> {
    Ok(Json(engine.codebase_map()?))
}

#[utoipa::path(get, path = "/v1/explain/{name}", tag = "worker", params(("name" = String, Path, description = "symbol or component name")),
    summary = "explain a component/package",
    responses(
        (status = 200, body = cce_engine::ComponentExplanation),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn explain(
    State(engine): State<Arc<CceEngine>>,
    Path(name): Path<String>,
) -> Result<Json<cce_engine::ComponentExplanation>, ApiError> {
    Ok(Json(engine.explain_component(&name)?))
}

#[utoipa::path(get, path = "/v1/impact/{name}", tag = "worker", params(("name" = String, Path, description = "symbol or component name")),
    summary = "impact analysis of a symbol",
    responses(
        (status = 200, body = cce_engine::ImpactReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn impact(
    State(engine): State<Arc<CceEngine>>,
    Path(name): Path<String>,
) -> Result<Json<cce_engine::ImpactReport>, ApiError> {
    Ok(Json(engine.impact_analysis(&name)?))
}

#[utoipa::path(get, path = "/v1/def/{name}", tag = "worker", params(("name" = String, Path, description = "symbol or component name")),
    summary = "go-to-definition",
    responses(
        (status = 200, body = cce_engine::DefinitionsReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn definitions(
    State(engine): State<Arc<CceEngine>>,
    Path(name): Path<String>,
) -> Result<Json<cce_engine::DefinitionsReport>, ApiError> {
    Ok(Json(engine.definitions(&name)?))
}

#[utoipa::path(get, path = "/v1/refs/{name}", tag = "worker", params(("name" = String, Path, description = "symbol or component name")),
    summary = "find-references",
    responses(
        (status = 200, body = cce_engine::ReferencesReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn references(
    State(engine): State<Arc<CceEngine>>,
    Path(name): Path<String>,
) -> Result<Json<cce_engine::ReferencesReport>, ApiError> {
    Ok(Json(engine.references(&name)?))
}

#[utoipa::path(get, path = "/v1/providers", tag = "worker",
    summary = "SCIP/provider detection report",
    responses(
        (status = 200, body = Vec<cce_engine::ProviderReport>),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn providers(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<Vec<cce_engine::ProviderReport>>, ApiError> {
    Ok(Json(engine.providers()))
}

#[utoipa::path(post, path = "/v1/grep", tag = "worker",
    request_body = GrepInput,
    summary = "worktree regex search",
    responses(
        (status = 200, body = cce_engine::GrepReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn grep(
    State(engine): State<Arc<CceEngine>>,
    Json(input): Json<GrepInput>,
) -> Result<Json<cce_engine::GrepReport>, ApiError> {
    Ok(Json(engine.grep(&cce_engine::GrepRequest {
        pattern: input.pattern,
        filters: cce_core::QueryFilters {
            path_prefix: input.path_prefix,
            language: input.language.map(|value| value.to_lowercase()),
            hit_type: None,
            pattern: None,
        },
        limit: input.limit.clamp(1, 5_000),
        ignore_case: input.ignore_case,
    })?))
}

#[utoipa::path(post, path = "/v1/diff", tag = "worker",
    request_body = DiffInput,
    summary = "search stored commit patches (type:diff)",
    responses(
        (status = 200, body = cce_engine::SearchResult),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn diff(
    State(state): State<DaemonState>,
    Json(input): Json<DiffInput>,
) -> Result<Json<cce_engine::SearchResult>, ApiError> {
    Ok(Json(
        state
            .engine
            .search(SearchRequest {
                repository_id: String::new(),
                snapshot_id: String::new(),
                query: input.pattern,
                intent: None,
                limit: input.limit.clamp(1, 500),
                // Under --watch the poller owns freshness — serve the
                // committed snapshot instead of rescanning per request.
                require_fresh: !state.watch_keeps_fresh,
                routes: vec![SearchRoute::Diff],
                filters: cce_core::QueryFilters::default(),
            })
            .await?,
    ))
}

/// `GET /v1/diff/architecture` query — the snapshot pair to compare.
#[derive(Debug, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
struct ArchitectureDiffQuery {
    /// Base snapshot id; defaults to the snapshot committed before head.
    base: Option<String>,
    /// Head snapshot id; defaults to the current committed snapshot.
    head: Option<String>,
}

#[utoipa::path(get, path = "/v1/diff/architecture", tag = "worker",
    summary = "entity/relation delta between two committed snapshots",
    params(ArchitectureDiffQuery),
    responses(
        (status = 200, body = cce_engine::ArchitectureDiff),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn diff_architecture(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<ArchitectureDiffQuery>,
) -> Result<Json<cce_engine::ArchitectureDiff>, ApiError> {
    Ok(Json(engine.architecture_diff(
        query.base.as_deref(),
        query.head.as_deref(),
    )?))
}

/// `GET /v1/export*` query — snapshot selection plus cursor pagination.
#[derive(Debug, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
struct ExportQuery {
    /// Snapshot to read; defaults to the current committed snapshot.
    snapshot: Option<String>,
    /// Cursor: rows with `id > cursor` (the last id of the previous page).
    cursor: Option<String>,
    /// Page size (default 1000, clamped to `1..=10_000`).
    limit: Option<usize>,
}

fn export_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(1_000).clamp(1, 10_000)
}

#[utoipa::path(get, path = "/v1/export", tag = "worker",
    summary = "committed entity/relation/region counts of a snapshot",
    params(ExportQuery),
    responses(
        (status = 200, body = cce_engine::ExportSummary),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn export(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<cce_engine::ExportSummary>, ApiError> {
    Ok(Json(engine.export_summary(query.snapshot.as_deref())?))
}

#[utoipa::path(get, path = "/v1/export/entities", tag = "worker",
    summary = "one id-ordered page of a snapshot's entities",
    params(ExportQuery),
    responses(
        (status = 200, body = cce_engine::EntityExportPage),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn export_entities(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<cce_engine::EntityExportPage>, ApiError> {
    Ok(Json(engine.export_entities(
        query.snapshot.as_deref(),
        query.cursor.as_deref(),
        export_limit(query.limit),
    )?))
}

#[utoipa::path(get, path = "/v1/export/relations", tag = "worker",
    summary = "one id-ordered page of a snapshot's relations",
    params(ExportQuery),
    responses(
        (status = 200, body = cce_engine::RelationExportPage),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn export_relations(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<cce_engine::RelationExportPage>, ApiError> {
    Ok(Json(engine.export_relations(
        query.snapshot.as_deref(),
        query.cursor.as_deref(),
        export_limit(query.limit),
    )?))
}

#[utoipa::path(get, path = "/v1/export/regions", tag = "worker",
    summary = "one id-ordered page of a snapshot's canonical regions",
    params(ExportQuery),
    responses(
        (status = 200, body = cce_engine::RegionExportPage),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn export_regions(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<cce_engine::RegionExportPage>, ApiError> {
    Ok(Json(engine.export_regions(
        query.snapshot.as_deref(),
        query.cursor.as_deref(),
        export_limit(query.limit),
    )?))
}

/// `GET /v1/file?path=` query — the repository-relative file path.
#[derive(Debug, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
struct FileQuery {
    /// Repository-relative path, e.g. `src/main.rs`.
    path: String,
}

#[utoipa::path(get, path = "/v1/files", tag = "worker",
    summary = "list indexed files for the current snapshot",
    responses(
        (status = 200, body = cce_engine::FileListReport),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn files(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<cce_engine::FileListReport>, ApiError> {
    Ok(Json(engine.files()?))
}

#[utoipa::path(get, path = "/v1/file", tag = "worker",
    summary = "file content from the committed source view",
    params(FileQuery),
    responses(
        (status = 200, body = cce_engine::FileContent),
        (status = "4XX", description = "client error — invalid request or unavailable view", body = ApiErrorBody),
        (status = 500, description = "internal error", body = ApiErrorBody)
    ))]
async fn file_source(
    State(engine): State<Arc<CceEngine>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<cce_engine::FileContent>, ApiError> {
    Ok(Json(engine.file(&query.path)?))
}

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
