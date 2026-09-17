#![forbid(unsafe_code)]

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cce_core::{QueryIntent, SearchRequest, SearchRoute};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DenseBackendConfig, EngineConfig,
};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::ServeDir,
    trace::TraceLayer,
};

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
    #[arg(long, value_enum, default_value_t = DenseMode::Disabled)]
    dense: DenseMode,
    /// Local embedding model code, used with --dense local.
    #[arg(long, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long)]
    embedding_dimensions: Option<usize>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    /// In-process ONNX model via fastembed; downloads model files once, then offline.
    Local,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchInput {
    query: String,
    intent: Option<QueryIntent>,
    #[serde(default = "default_search_limit")]
    limit: usize,
    /// Route pins for ablation-style queries; empty follows the planner.
    #[serde(default)]
    routes: Vec<SearchRoute>,
}

const fn default_search_limit() -> usize {
    20
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
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
    let state = Arc::new(CceEngine::open(config)?);
    let request_id = HeaderName::from_static("x-request-id");
    let mut router = Router::new()
        .route("/healthz", get(health))
        .route("/v1/index", post(index))
        .route("/v1/status", get(status))
        .route("/v1/search", post(search))
        .route("/v1/context", post(context))
        .route("/v1/map", get(codebase_map))
        .route("/v1/explain/{name}", get(explain))
        .route("/v1/impact/{name}", get(impact))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    if let Some(web_root) = arguments.web_root {
        router =
            router.fallback_service(ServeDir::new(web_root).append_index_html_on_directories(true));
    }
    let listener = tokio::net::TcpListener::bind(arguments.bind).await?;
    tracing::info!(address = %arguments.bind, "CCE daemon listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn index(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<cce_engine::IndexReport>, ApiError> {
    Ok(Json(engine.index().await?))
}

async fn status(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<cce_core::ViewManifest>, ApiError> {
    Ok(Json(engine.status()?))
}

async fn search(
    State(engine): State<Arc<CceEngine>>,
    Json(input): Json<SearchInput>,
) -> Result<Json<cce_engine::SearchResult>, ApiError> {
    Ok(Json(
        engine
            .search(SearchRequest {
                repository_id: String::new(),
                snapshot_id: String::new(),
                query: input.query,
                intent: input.intent,
                limit: input.limit.clamp(1, 200),
                require_fresh: true,
                routes: input.routes,
            })
            .await?,
    ))
}

async fn context(
    State(engine): State<Arc<CceEngine>>,
    Json(mut request): Json<ContextRequest>,
) -> Result<Json<cce_core::ContextPack>, ApiError> {
    request.budget_tokens = request.budget_tokens.clamp(256, 128_000);
    request.max_candidates = request.max_candidates.clamp(1, 500);
    Ok(Json(engine.context(request).await?))
}

async fn codebase_map(
    State(engine): State<Arc<CceEngine>>,
) -> Result<Json<cce_engine::CodebaseMap>, ApiError> {
    Ok(Json(engine.codebase_map()?))
}

async fn explain(
    State(engine): State<Arc<CceEngine>>,
    Path(name): Path<String>,
) -> Result<Json<cce_engine::ComponentExplanation>, ApiError> {
    Ok(Json(engine.explain_component(&name)?))
}

async fn impact(
    State(engine): State<Arc<CceEngine>>,
    Path(name): Path<String>,
) -> Result<Json<cce_engine::ImpactReport>, ApiError> {
    Ok(Json(engine.impact_analysis(&name)?))
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
