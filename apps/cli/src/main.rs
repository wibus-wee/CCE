#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Context;
use cce_core::{QueryIntent, SearchRequest};
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
    #[arg(long, global = true, env = "CCE_EMBEDDING_BASE_URL")]
    embedding_base_url: Option<String>,
    #[arg(long, global = true, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long, global = true, default_value = "CCE_EMBEDDING_API_KEY")]
    embedding_api_key_environment: String,
    #[arg(long, global = true)]
    embedding_dimensions: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    Provider,
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
        DenseMode::Provider => DenseBackendConfig::OpenAiCompatible {
            base_url: arguments
                .embedding_base_url
                .clone()
                .context("--embedding-base-url is required for --dense provider")?,
            model: arguments
                .embedding_model
                .clone()
                .context("--embedding-model is required for --dense provider")?,
            api_key_environment: arguments.embedding_api_key_environment.clone(),
            dimensions: arguments.embedding_dimensions,
            batch_size: 32,
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
