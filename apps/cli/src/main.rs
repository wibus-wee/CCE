#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Context;
use cce_core::{QueryIntent, SearchRequest};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DEFAULT_LOCAL_RERANKER_MODEL,
    DEFAULT_LOCAL_RERANKER_REVISION, DataflowBackendConfig, DenseBackendConfig, EngineConfig,
    RerankerBackendConfig, ScipBackendConfig, local_embedding_preset,
};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "cce", version, about = "Local-first codebase context engine")]
struct Arguments {
    #[arg(long, global = true, env = "CCE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, value_enum, default_value_t = DenseMode::Disabled)]
    dense: DenseMode,
    #[arg(long, global = true, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long, global = true, env = "CCE_EMBEDDING_REVISION")]
    embedding_revision: Option<String>,
    #[arg(long, global = true, env = "CCE_EMBEDDING_MODEL_DIRECTORY")]
    embedding_model_directory: Option<PathBuf>,
    #[arg(long, global = true, env = "CCE_EMBEDDING_CACHE_DIR")]
    embedding_cache_dir: Option<PathBuf>,
    #[arg(long, global = true, env = "CCE_ONNX_RUNTIME_LIBRARY")]
    onnx_runtime_library: Option<PathBuf>,
    #[arg(long, global = true, env = "CCE_EMBEDDING_ALLOW_DOWNLOAD")]
    embedding_allow_download: bool,
    #[arg(long, global = true, env = "CCE_EMBEDDING_ALLOW_HIGH_MEMORY")]
    embedding_allow_high_memory: bool,
    #[arg(long, global = true, default_value_t = 512)]
    embedding_max_length: usize,
    #[arg(long, global = true)]
    embedding_threads: Option<usize>,
    #[arg(long, global = true, default_value_t = 4)]
    embedding_batch_size: usize,
    #[arg(
        long,
        global = true,
        env = "CCE_EMBEDDING_SESSIONS",
        default_value_t = 1
    )]
    embedding_sessions: usize,
    #[arg(long, global = true)]
    embedding_query_prefix: Option<String>,
    #[arg(long, global = true)]
    embedding_document_prefix: Option<String>,
    #[arg(long, global = true)]
    embedding_dimensions: Option<usize>,
    #[arg(long, global = true, value_enum, default_value_t = RerankerMode::Disabled)]
    reranker: RerankerMode,
    #[arg(long, global = true, env = "CCE_RERANKER_MODEL")]
    reranker_model: Option<String>,
    #[arg(long, global = true, env = "CCE_RERANKER_REVISION")]
    reranker_revision: Option<String>,
    #[arg(long, global = true, env = "CCE_RERANKER_MODEL_DIRECTORY")]
    reranker_model_directory: Option<PathBuf>,
    #[arg(long, global = true, env = "CCE_RERANKER_CACHE_DIR")]
    reranker_cache_dir: Option<PathBuf>,
    #[arg(long, global = true, env = "CCE_RERANKER_ALLOW_DOWNLOAD")]
    reranker_allow_download: bool,
    #[arg(
        long,
        global = true,
        env = "CCE_RERANKER_MAX_LENGTH",
        default_value_t = 512
    )]
    reranker_max_length: usize,
    #[arg(long, global = true, env = "CCE_RERANKER_THREADS")]
    reranker_threads: Option<usize>,
    #[arg(
        long,
        global = true,
        env = "CCE_RERANKER_BATCH_SIZE",
        default_value_t = 8
    )]
    reranker_batch_size: usize,
    #[arg(
        long,
        global = true,
        env = "CCE_RERANKER_SESSIONS",
        default_value_t = 1
    )]
    reranker_sessions: usize,
    #[arg(
        long,
        global = true,
        env = "CCE_SCIP_AUTO",
        conflicts_with = "scip_index"
    )]
    scip_auto: bool,
    #[arg(long, global = true, env = "CCE_SCIP_INDEX")]
    scip_index: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        env = "CCE_RUST_ANALYZER",
        default_value = "rust-analyzer"
    )]
    rust_analyzer: PathBuf,
    #[arg(
        long,
        global = true,
        env = "CCE_SCIP_TYPESCRIPT",
        default_value = "scip-typescript"
    )]
    scip_typescript: PathBuf,
    #[arg(long, global = true)]
    scip_threads: Option<usize>,
    #[arg(long, global = true, default_value_t = 900)]
    scip_timeout_seconds: u64,
    #[arg(
        long,
        global = true,
        env = "CCE_DATAFLOW_INDEX",
        conflicts_with = "dataflow_joern"
    )]
    dataflow_index: Option<PathBuf>,
    #[arg(long, global = true, env = "CCE_DATAFLOW_JOERN")]
    dataflow_joern: bool,
    #[arg(
        long,
        global = true,
        env = "CCE_JOERN_PARSE",
        default_value = "joern-parse"
    )]
    joern_parse: PathBuf,
    #[arg(
        long,
        global = true,
        env = "CCE_JOERN_EXPORT",
        default_value = "joern-export"
    )]
    joern_export: PathBuf,
    #[arg(long, global = true, env = "CCE_JOERN_LANGUAGE")]
    joern_language: Option<String>,
    #[arg(
        long,
        global = true,
        env = "CCE_JOERN_TIMEOUT_SECONDS",
        default_value_t = 1_800
    )]
    joern_timeout_seconds: u64,
    #[command(subcommand)]
    command: Command,
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

#[derive(Debug, Subcommand)]
enum Command {
    Index {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    Status {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    Doctor {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    Rebuild {
        #[arg(default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        confirm: bool,
    },
    Search {
        repository: PathBuf,
        query: String,
        #[arg(long, value_enum)]
        intent: Option<IntentArgument>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Context {
        repository: PathBuf,
        query: String,
        #[arg(long, value_enum)]
        intent: Option<IntentArgument>,
        #[arg(long, default_value_t = 8_192)]
        budget: usize,
        #[arg(long, default_value_t = 50)]
        candidates: usize,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum IntentArgument {
    #[value(name = "exact_entity")]
    Exact,
    #[value(name = "natural_language_behavior")]
    Behavior,
    #[value(name = "issue_localization")]
    Issue,
    #[value(name = "trace")]
    Trace,
    #[value(name = "impact")]
    Impact,
    #[value(name = "architecture")]
    Architecture,
    #[value(name = "history")]
    History,
    #[value(name = "precise_dataflow")]
    Dataflow,
}

impl From<IntentArgument> for QueryIntent {
    fn from(value: IntentArgument) -> Self {
        match value {
            IntentArgument::Exact => Self::ExactEntity,
            IntentArgument::Behavior => Self::NaturalLanguageBehavior,
            IntentArgument::Issue => Self::IssueLocalization,
            IntentArgument::Trace => Self::Trace,
            IntentArgument::Impact => Self::Impact,
            IntentArgument::Architecture => Self::Architecture,
            IntentArgument::History => Self::History,
            IntentArgument::Dataflow => Self::PreciseDataflow,
        }
    }
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
    match &arguments.command {
        Command::Index { repository } => {
            let engine = engine(&arguments, repository)?;
            let report = engine.index().await?;
            print_value(&report)?;
        }
        Command::Status { repository } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.status()?)?;
        }
        Command::Doctor { repository } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.doctor()?)?;
        }
        Command::Rebuild {
            repository,
            confirm,
        } => {
            anyhow::ensure!(
                *confirm,
                "rebuild requires --confirm; metadata is quarantined, not deleted"
            );
            let data_dir = data_dir(&arguments, repository);
            let quarantine = quarantine_metadata(&data_dir)?;
            let engine = engine(&arguments, repository)?;
            let report = engine.index().await?;
            eprintln!("quarantined prior SQLite files at {}", quarantine.display());
            print_value(&report)?;
        }
        Command::Search {
            repository,
            query,
            intent,
            limit,
        } => {
            let engine = engine(&arguments, repository)?;
            let result = engine
                .search(SearchRequest {
                    repository_id: String::new(),
                    snapshot_id: String::new(),
                    query: query.clone(),
                    intent: intent.map(Into::into),
                    limit: *limit,
                    require_fresh: true,
                    routes: Vec::new(),
                })
                .await?;
            print_value(&result)?;
        }
        Command::Context {
            repository,
            query,
            intent,
            budget,
            candidates,
        } => {
            let engine = engine(&arguments, repository)?;
            let mut request = ContextRequest::new(query, *budget);
            request.intent = intent.map(Into::into);
            request.max_candidates = *candidates;
            let pack = engine.context(request).await?;
            if arguments.json {
                print_value(&pack)?;
            } else {
                print_context(&pack);
            }
        }
    }
    Ok(())
}

fn engine(arguments: &Arguments, repository: &PathBuf) -> anyhow::Result<CceEngine> {
    let mut config = EngineConfig::for_repository(repository);
    config.data_root = data_dir(arguments, repository);
    config.dense = match arguments.dense {
        DenseMode::Disabled => DenseBackendConfig::Disabled,
        DenseMode::Baseline => DenseBackendConfig::DeterministicBaseline {
            dimensions: arguments.embedding_dimensions.unwrap_or(512),
        },
        DenseMode::Local => {
            let requested_model = arguments
                .embedding_model
                .clone()
                .unwrap_or_else(|| DEFAULT_LOCAL_EMBEDDING_MODEL.to_owned());
            let preset = local_embedding_preset(&requested_model);
            DenseBackendConfig::LocalFastEmbed {
                model: preset.map_or(requested_model, |value| value.model.to_owned()),
                revision: arguments
                    .embedding_revision
                    .clone()
                    .or_else(|| preset.map(|value| value.revision.to_owned()))
                    .context("--embedding-revision is required for a custom local model")?,
                model_directory: arguments.embedding_model_directory.clone(),
                cache_dir: arguments
                    .embedding_cache_dir
                    .clone()
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
                query_prefix: arguments.embedding_query_prefix.clone().unwrap_or_else(|| {
                    preset.map_or_else(String::new, |value| value.query_prefix.to_owned())
                }),
                document_prefix: arguments
                    .embedding_document_prefix
                    .clone()
                    .unwrap_or_else(|| {
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
                .clone()
                .unwrap_or_else(|| DEFAULT_LOCAL_RERANKER_MODEL.to_owned());
            let revision = arguments
                .reranker_revision
                .clone()
                .or_else(|| {
                    model
                        .eq_ignore_ascii_case(DEFAULT_LOCAL_RERANKER_MODEL)
                        .then(|| DEFAULT_LOCAL_RERANKER_REVISION.to_owned())
                })
                .context("--reranker-revision is required for a custom local reranker")?;
            RerankerBackendConfig::LocalFastEmbed {
                model,
                revision,
                model_directory: arguments.reranker_model_directory.clone(),
                cache_dir: arguments
                    .reranker_cache_dir
                    .clone()
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
            &arguments.rust_analyzer,
            &arguments.scip_typescript,
            arguments.scip_threads,
            arguments.scip_timeout_seconds,
        )?
    } else if let Some(path) = &arguments.scip_index {
        ScipBackendConfig::Supplied { path: path.clone() }
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
            .as_ref()
            .map_or(DataflowBackendConfig::Disabled, |path| {
                DataflowBackendConfig::Supplied { path: path.clone() }
            })
    };
    CceEngine::open(config).map_err(Into::into)
}

fn data_dir(arguments: &Arguments, repository: &PathBuf) -> PathBuf {
    arguments
        .data_dir
        .clone()
        .unwrap_or_else(|| EngineConfig::for_repository(repository).data_root)
}

fn quarantine_metadata(data_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let database = data_dir.join("metadata.sqlite");
    anyhow::ensure!(
        database.is_file(),
        "no metadata database exists at {}",
        database.display()
    );
    let quarantine = data_dir.join("quarantine").join(format!(
        "metadata-{}",
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::create_dir_all(&quarantine)?;
    for name in [
        "metadata.sqlite",
        "metadata.sqlite-wal",
        "metadata.sqlite-shm",
    ] {
        let source = data_dir.join(name);
        if source.exists() {
            std::fs::rename(&source, quarantine.join(name))?;
        }
    }
    Ok(quarantine)
}

fn print_value(value: &impl serde::Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_context(pack: &cce_core::ContextPack) {
    println!("# CCE Context Pack");
    println!("\n- Snapshot: `{}`", pack.snapshot_id);
    println!("- Intent: `{:?}`", pack.intent);
    println!(
        "- Budget: {}/{} estimated tokens",
        pack.used_tokens, pack.budget_tokens
    );
    for item in &pack.items {
        println!("\n## {}\n", item.title);
        println!("{}", item.body);
        println!(
            "\n_Provenance: route `{:?}`, rank {}, score {:.6}, current {}_",
            item.provenance.route,
            item.provenance.rank,
            item.provenance.score,
            item.provenance.verified_current
        );
    }
    if !pack.uncertainties.is_empty() {
        println!("\n## Uncertainties");
        for uncertainty in &pack.uncertainties {
            println!(
                "\n- **{}**: {}",
                uncertainty.capability, uncertainty.message
            );
        }
    }
}
