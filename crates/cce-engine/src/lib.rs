#![forbid(unsafe_code)]

//! CCE indexing and query engine.

mod atlas;
mod config;
mod context;
mod dense;
mod engine;
mod history;
mod lock;
mod packages;
mod parser;
mod planner;
mod relations;
mod repository;
mod retrieval;

pub use atlas::{CodebaseMap, ComponentExplanation, ImpactReport, ImpactedEntity, PackageNode};
pub use cce_core::GraphPolicy;
pub use config::{DenseBackendConfig, EngineConfig, IndexOptions};
pub use context::{ContextPacker, ContextRequest};
pub use dense::{DenseIndex, DenseSearchHit, EmbedRole, Embedder, EmbeddingBackend, LocalEmbedder};
pub use engine::{CceEngine, IndexReport};
pub use parser::{ParsedFile, ParsedUnit, SourceParser};
pub use planner::{QueryPlan, QueryPlanner};
pub use repository::{RepositoryScanner, ScannedFile, ScannedRepository};
pub use retrieval::SearchResult;
