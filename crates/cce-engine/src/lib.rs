#![forbid(unsafe_code)]

//! CCE indexing and query engine.

mod atlas;
mod browse;
mod config;
mod context;
mod dense;
mod diff;
mod engine;
mod grep;
mod history;
mod ignore;
mod landmarks;
mod lock;
mod packages;
mod parser;
mod planner;
mod providers;
mod relations;
mod repository;
mod rerank;
mod retrieval;
mod scip;
mod zoekt;

pub use atlas::{
    CodebaseMap, ComponentExplanation, DefinitionHit, DefinitionsReport, ImpactReport,
    ImpactedEntity, PackageNode, ReferenceHit, ReferencesReport,
};
pub use browse::{FileContent, FileListEntry, FileListReport};
pub use cce_core::GraphPolicy;
pub use config::{
    DEFAULT_LOCAL_EMBEDDING_MODEL, DEFAULT_LOCAL_RERANKER_MODEL, DenseBackendConfig, EngineConfig,
    IndexOptions,
};
pub use context::{ContextPacker, ContextRequest};
pub use dense::{DenseIndex, DenseSearchHit, EmbedRole, Embedder, EmbeddingBackend, LocalEmbedder};
pub use diff::DEFAULT_DIFF_LIMIT;
pub use engine::{CceEngine, IndexReport, RetrievalCounters, RetrievalCountersSnapshot};
pub use grep::{DEFAULT_GREP_LIMIT, GrepHit, GrepReport, GrepRequest};
pub use parser::{ParsedFile, ParsedUnit, SourceParser};
pub use planner::{QueryPlan, QueryPlanner};
pub use providers::{ProviderReport, ProviderState};
pub use repository::{RepositoryScanner, ScannedFile, ScannedRepository};
pub use rerank::LocalReranker;
pub use retrieval::SearchResult;
pub use zoekt::provision as provision_zoekt;
