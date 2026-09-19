#![forbid(unsafe_code)]

//! `cce` command-line client: index, status, search, context packs, and the
//! architecture atlas over a repository's `.cce/` state.

use std::path::PathBuf;

use cce_core::{QueryIntent, SearchRequest, SearchRoute};
use cce_engine::{
    CceEngine, ContextRequest, DEFAULT_LOCAL_EMBEDDING_MODEL, DenseBackendConfig, EngineConfig,
};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "cce", version, about = "Local-first codebase context engine")]
struct Arguments {
    #[arg(long, global = true, env = "CCE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    /// Dense embedding backend. `local` (default) runs the local ONNX
    /// embedder — the first index downloads the model once (see `cce
    /// models`); `disabled` keeps indexing/querying fully offline.
    #[arg(long, global = true, value_enum, env = "CCE_DENSE", default_value_t = DenseMode::Local)]
    dense: DenseMode,
    /// Local embedding model code (see `cce models`), used with --dense local.
    #[arg(long, global = true, env = "CCE_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    #[arg(long, global = true)]
    embedding_dimensions: Option<usize>,
    /// Local cross-encoder reranker model code; enables reranking. Bare
    /// `--reranker` uses the default model.
    #[arg(
        long,
        global = true,
        env = "CCE_RERANKER_MODEL",
        num_args = 0..=1,
        default_missing_value = cce_engine::DEFAULT_LOCAL_RERANKER_MODEL
    )]
    reranker: Option<String>,
    /// Skip external code-intelligence providers (SCIP indexers) during
    /// indexing. Query commands are unaffected.
    #[arg(long, global = true, env = "CCE_NO_PROVIDERS")]
    no_providers: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DenseMode {
    Disabled,
    Baseline,
    /// In-process ONNX model via fastembed; downloads model files once, then offline.
    Local,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build or incrementally refresh the snapshot: scan, parse, run providers,
    /// materialize views, then prune beyond the retention window.
    Index {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    /// Commit a detached parse+relations snapshot for diffing — no
    /// providers/dense/zoekt, never promoted to current. The returned
    /// snapshot id feeds `cce diff --head` for session-change review.
    Checkpoint {
        #[arg(default_value = ".")]
        repository: PathBuf,
        /// Producer label recorded on the snapshot (e.g. a session id).
        #[arg(long)]
        origin: Option<String>,
    },
    /// Read-only view manifest: each view's state and freshness for the
    /// current snapshot. Never writes.
    Status {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    /// Store health: the `SQLite` self-check plus a verification pass over the
    /// artifacts the metadata references.
    Doctor {
        #[arg(default_value = ".")]
        repository: PathBuf,
    },
    /// Quarantine the metadata store and rebuild from scratch. Requires
    /// `--confirm`; prior `SQLite` files are moved aside, never deleted.
    Rebuild {
        #[arg(default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        confirm: bool,
    },
    /// Intent-aware ranked retrieval: the plan, ranked hits, and any capability
    /// the snapshot could not supply.
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
        /// Serve the last committed snapshot without rescanning the
        /// worktree — hits report `verifiedCurrent: false`.
        #[arg(long)]
        no_verify: bool,
    },
    /// Emit the canonical source-linked context pack under a token budget.
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
        /// Serve the last committed snapshot without rescanning the
        /// worktree — hits report `verifiedCurrent: false`.
        #[arg(long)]
        no_verify: bool,
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
    /// Definition sites for a symbol.
    Def { repository: PathBuf, name: String },
    /// Inbound references/calls/implementations for a symbol, origin-tagged.
    Refs { repository: PathBuf, name: String },
    /// Brute-force regex over the live worktree — always fresh, never reads
    /// the snapshot. Honors ignore rules and the sensitive-file policy.
    Grep {
        repository: PathBuf,
        pattern: String,
        /// Path-prefix filter (`path:` equivalent).
        #[arg(long)]
        path: Option<String>,
        /// Language filter (`lang:` equivalent).
        #[arg(long)]
        lang: Option<String>,
        /// ASCII case-insensitive matching.
        #[arg(long, short = 'i')]
        ignore_case: bool,
        #[arg(long, default_value_t = cce_engine::DEFAULT_GREP_LIMIT)]
        limit: usize,
    },
    /// With PATTERN: regex over stored commit patches — Sourcegraph
    /// `type:diff`, historical evidence bounded by the history index (most
    /// recent 512 commits). With `--base`/`--head` — or no PATTERN at all —
    /// the entity/relation delta between two committed snapshots.
    Diff {
        repository: PathBuf,
        /// Patch-search regex (type:diff). Omit to diff snapshots.
        pattern: Option<String>,
        /// Base snapshot; defaults to the snapshot committed before --head.
        #[arg(long)]
        base: Option<String>,
        /// Head snapshot; defaults to the current snapshot.
        #[arg(long)]
        head: Option<String>,
        #[arg(long, default_value_t = cce_engine::DEFAULT_DIFF_LIMIT)]
        limit: usize,
        /// Patch-search only: serve the last committed snapshot without
        /// rescanning the worktree.
        #[arg(long)]
        no_verify: bool,
    },
    /// Probe external code-intelligence providers (SCIP toolchains).
    Providers {
        #[arg(default_value = ".")]
        repository: PathBuf,
        /// Install provisionable toolchains (zoekt) into the managed
        /// cache — explicit network opt-in.
        #[arg(long)]
        provision: bool,
    },
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
    #[value(name = "diff")]
    Diff,
    #[value(name = "zoekt")]
    Zoekt,
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
            RouteArgument::Diff => Self::Diff,
            RouteArgument::Zoekt => Self::Zoekt,
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
            let report = engine.index_with_origin(Some("cli".to_owned())).await?;
            print_value(&report)?;
        }
        Command::Checkpoint { repository, origin } => {
            let engine = engine(&arguments, repository)?;
            let report = engine
                .checkpoint(origin.clone().or_else(|| Some("cli".to_owned())))
                .await?;
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
            no_verify,
        } => {
            let engine = engine(&arguments, repository)?;
            let result = engine
                .search(SearchRequest {
                    repository_id: String::new(),
                    snapshot_id: String::new(),
                    query: query.clone(),
                    intent: intent.map(Into::into),
                    limit: *limit,
                    require_fresh: !no_verify,
                    routes: routes.iter().map(|route| (*route).into()).collect(),
                    filters: cce_core::QueryFilters::default(),
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
            no_verify,
        } => {
            let engine = engine(&arguments, repository)?;
            let mut request = ContextRequest::new(query, *budget);
            request.intent = intent.map(Into::into);
            request.require_fresh = !no_verify;
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
        Command::Def { repository, name } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.definitions(name)?)?;
        }
        Command::Refs { repository, name } => {
            let engine = engine(&arguments, repository)?;
            print_value(&engine.references(name)?)?;
        }
        Command::Grep {
            repository,
            pattern,
            path,
            lang,
            ignore_case,
            limit,
        } => {
            let engine = engine(&arguments, repository)?;
            let request = cce_engine::GrepRequest {
                pattern: pattern.clone(),
                filters: cce_core::QueryFilters {
                    path_prefix: path.clone(),
                    language: lang.clone().map(|value| value.to_lowercase()),
                    hit_type: None,
                    pattern: None,
                },
                limit: *limit,
                ignore_case: *ignore_case,
            };
            let report = engine.grep(&request)?;
            if arguments.json {
                print_value(&report)?;
            } else {
                for hit in &report.matches {
                    println!("{}:{}:{}: {}", hit.path, hit.line, hit.column, hit.text);
                }
                if report.truncated {
                    eprintln!("… truncated at {} matches", report.matches.len());
                }
            }
        }
        Command::Diff {
            repository,
            pattern,
            base,
            head,
            limit,
            no_verify,
        } => {
            let engine = engine(&arguments, repository)?;
            // Snapshot-delta mode: --base/--head given, or no PATTERN at
            // all (defaults to previous-vs-current). A PATTERN combined
            // with snapshot ids is ambiguous and rejected.
            let patch_search = pattern.is_some() && base.is_none() && head.is_none();
            if patch_search {
                let result = engine
                    .search(SearchRequest {
                        repository_id: String::new(),
                        snapshot_id: String::new(),
                        query: pattern.clone().unwrap_or_default(),
                        intent: None,
                        limit: *limit,
                        require_fresh: !no_verify,
                        routes: vec![SearchRoute::Diff],
                        filters: cce_core::QueryFilters::default(),
                    })
                    .await?;
                if arguments.json {
                    print_value(&result)?;
                } else {
                    for hit in &result.hits {
                        let path = hit
                            .address
                            .as_ref()
                            .map_or("?", |address| address.path.as_str());
                        let commit = hit.symbol_name.as_deref().unwrap_or("commit");
                        println!("{commit} {path}:");
                        for line in hit.snippet.lines() {
                            println!("    {line}");
                        }
                    }
                }
            } else {
                anyhow::ensure!(
                    pattern.is_none(),
                    "PATTERN cannot be combined with --base/--head; it selects patch search (type:diff)"
                );
                let report = engine.architecture_diff(base.as_deref(), head.as_deref())?;
                if arguments.json {
                    print_value(&report)?;
                } else {
                    print_architecture_diff(&report);
                }
            }
        }
        Command::Providers {
            repository,
            provision,
        } => {
            if *provision {
                // Blocking network/subprocess work — keep it off the
                // async executor and let the blocking client drop on a
                // blocking thread.
                let root = data_dir(&arguments, repository);
                let report =
                    tokio::task::spawn_blocking(move || cce_engine::provision_zoekt(&root)).await?;
                print_value(&report)?;
            } else {
                let engine = engine(&arguments, repository)?;
                print_value(&engine.providers())?;
            }
        }
        Command::Models => {
            println!("# embedding models (--embedding-model)");
            for code in cce_engine::LocalEmbedder::supported_model_codes() {
                println!("{code}");
            }
            println!("# reranker models (--reranker)");
            for code in cce_engine::LocalReranker::supported_model_codes() {
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
                .unwrap_or_else(|| DEFAULT_LOCAL_EMBEDDING_MODEL.to_owned()),
        },
    };
    config.reranker_model.clone_from(&arguments.reranker);
    config.providers.enabled = !arguments.no_providers;
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

fn entity_line(entity: &cce_engine::DiffEntityRef) -> String {
    let name = entity.qualified_name.as_deref().unwrap_or(&entity.name);
    entity.path.as_deref().map_or_else(
        || format!("{:?} {name}", entity.kind),
        |path| format!("{:?} {name} ({path})", entity.kind),
    )
}

fn relation_line(relation: &cce_engine::DiffRelation) -> String {
    let source = relation
        .source
        .qualified_name
        .as_deref()
        .unwrap_or(&relation.source.name);
    let target = relation
        .target
        .qualified_name
        .as_deref()
        .unwrap_or(&relation.target.name);
    format!(
        "{:?} {source} -> {target} ({:?}, {:.2})",
        relation.kind, relation.origin, relation.confidence
    )
}

fn print_architecture_diff(report: &cce_engine::ArchitectureDiff) {
    println!("{} -> {}", report.base_snapshot_id, report.head_snapshot_id);
    println!(
        "+{} entities  -{} entities  +{} relations  -{} relations  ~{} changed",
        report.counts.added_entities,
        report.counts.removed_entities,
        report.counts.added_relations,
        report.counts.removed_relations,
        report.counts.changed_relations
    );
    for entity in &report.added_entities {
        println!("  + {}", entity_line(entity));
    }
    for entity in &report.removed_entities {
        println!("  - {}", entity_line(entity));
    }
    for relation in &report.added_relations {
        println!("  + {}", relation_line(relation));
    }
    for relation in &report.removed_relations {
        println!("  - {}", relation_line(relation));
    }
    for relation in &report.changed_relations {
        let source = relation
            .source
            .qualified_name
            .as_deref()
            .unwrap_or(&relation.source.name);
        let target = relation
            .target
            .qualified_name
            .as_deref()
            .unwrap_or(&relation.target.name);
        println!(
            "  ~ {:?} {source} -> {target} ({:?}/{:.2} -> {:?}/{:.2})",
            relation.kind,
            relation.base_origin,
            relation.base_confidence,
            relation.head_origin,
            relation.head_confidence
        );
    }
    if report.truncated {
        eprintln!("… lists truncated; see counts for full totals or use --json");
    }
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
