#![forbid(unsafe_code)]

use std::path::PathBuf;

use cce_core::{QueryIntent, SearchRequest, SearchRoute};
use cce_engine::{CceEngine, ContextRequest, DenseBackendConfig, EngineConfig};
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
    /// Local embedding model code (see `cce models`), used with --dense local.
    #[arg(long, global = true, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long, global = true)]
    embedding_dimensions: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

const DEFAULT_LOCAL_MODEL: &str = "intfloat/multilingual-e5-small";

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    /// In-process ONNX model via fastembed; downloads model files once, then offline.
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
        /// Override the planner's routes; repeatable. Used for route ablations.
        #[arg(long = "route", value_enum)]
        routes: Vec<RouteArgument>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Context {
        repository: PathBuf,
        query: String,
        #[arg(long, value_enum)]
        intent: Option<IntentArgument>,
        /// Override the planner's routes; repeatable. Used for route ablations.
        #[arg(long = "route", value_enum)]
        routes: Vec<RouteArgument>,
        #[arg(long, default_value_t = 8_192)]
        budget: usize,
        #[arg(long, default_value_t = 50)]
        candidates: usize,
    },
    /// Package-level architecture map of the repository.
    Map {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    /// Explain one package or symbol: members, dependencies, dependents, tests.
    Explain { repository: PathBuf, name: String },
    /// Blast radius of a symbol or package: impact edges within two hops.
    Impact { repository: PathBuf, name: String },
    /// List local embedding model codes usable with --dense local.
    Models,
    /// Prune old snapshots and unreferenced artifacts.
    Gc {
        #[arg(default_value = ".")]
        repository: PathBuf,
        /// Number of completed snapshots to retain besides the current one.
        #[arg(long, default_value_t = 8)]
        keep: usize,
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

/// CLI route names mirror the `SearchRoute` `snake_case` wire names.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum RouteArgument {
    #[value(name = "no_retrieval")]
    NoRetrieval,
    #[value(name = "exact_symbol")]
    ExactSymbol,
    #[value(name = "lexical")]
    Lexical,
    #[value(name = "dense_raw")]
    DenseRaw,
    #[value(name = "dense_summary")]
    DenseSummary,
    #[value(name = "hybrid")]
    Hybrid,
    #[value(name = "structural")]
    Structural,
    #[value(name = "knowledge")]
    Knowledge,
    #[value(name = "history")]
    History,
    #[value(name = "reranked")]
    Reranked,
}

impl From<RouteArgument> for SearchRoute {
    fn from(value: RouteArgument) -> Self {
        match value {
            RouteArgument::NoRetrieval => Self::NoRetrieval,
            RouteArgument::ExactSymbol => Self::ExactSymbol,
            RouteArgument::Lexical => Self::Lexical,
            RouteArgument::DenseRaw => Self::DenseRaw,
            RouteArgument::DenseSummary => Self::DenseSummary,
            RouteArgument::Hybrid => Self::Hybrid,
            RouteArgument::Structural => Self::Structural,
            RouteArgument::Knowledge => Self::Knowledge,
            RouteArgument::History => Self::History,
            RouteArgument::Reranked => Self::Reranked,
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
            routes,
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
                    routes: routes.iter().map(|route| (*route).into()).collect(),
                })
                .await?;
            print_value(&result)?;
        }
        Command::Context {
            repository,
            query,
            intent,
            routes,
            budget,
            candidates,
        } => {
            let engine = engine(&arguments, repository)?;
            let mut request = ContextRequest::new(query, *budget);
            request.intent = intent.map(Into::into);
            request.routes = routes.iter().map(|route| (*route).into()).collect();
            request.max_candidates = *candidates;
            let pack = engine.context(request).await?;
            if arguments.json {
                print_value(&pack)?;
            } else {
                print_context(&pack);
            }
        }
        Command::Map { repository } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.codebase_map()?)?;
        }
        Command::Explain { repository, name } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.explain_component(name)?)?;
        }
        Command::Impact { repository, name } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.impact_analysis(name)?)?;
        }
        Command::Models => {
            for code in cce_engine::LocalEmbedder::supported_model_codes() {
                println!("{code}");
            }
        }
        Command::Gc { repository, keep } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.gc(*keep)?)?;
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
        DenseMode::Local => DenseBackendConfig::Local {
            model: arguments
                .embedding_model
                .clone()
                .unwrap_or_else(|| DEFAULT_LOCAL_MODEL.to_owned()),
        },
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
