#![forbid(unsafe_code)]

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cce_core::{QueryIntent, SearchRequest};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DEFAULT_LOCAL_RERANKER_MODEL,
    DEFAULT_LOCAL_RERANKER_REVISION, DataflowBackendConfig, DenseBackendConfig, EngineConfig,
    RerankerBackendConfig, ScipBackendConfig, local_embedding_preset,
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
#[serde(rename_all = "camelCase")]
struct SearchInput {
    query: String,
    intent: Option<QueryIntent>,
    #[serde(default = "default_search_limit")]
    limit: usize,
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
    tracing::debug!(data_root = %config.data_root.display(), "opening CCE engine");
    let state = Arc::new(CceEngine::open(config)?);
    tracing::debug!("CCE engine opened; building HTTP router");
    let request_id = HeaderName::from_static("x-request-id");
    let mut router = Router::new()
        .route("/healthz", get(health))
        .route("/v1/index", post(index))
        .route("/v1/status", get(status))
        .route("/v1/search", post(search))
        .route("/v1/context", post(context))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    if let Some(web_root) = arguments.web_root {
        router =
            router.fallback_service(ServeDir::new(web_root).append_index_html_on_directories(true));
    }
    tracing::debug!(address = %arguments.bind, "binding CCE daemon listener");
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
                routes: Vec::new(),
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
