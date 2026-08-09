#![forbid(unsafe_code)]

//! CCE indexing and query engine.

mod config;
mod context;
mod dense;
mod engine;
mod history;
mod lock;
mod parser;
mod planner;
mod repository;
mod retrieval;

pub use config::{DenseBackendConfig, EngineConfig, IndexOptions};
pub use context::{ContextPacker, ContextRequest};
pub use dense::{DenseIndex, DenseSearchHit, Embedder, EmbeddingBackend};
pub use engine::{CceEngine, IndexReport};
pub use parser::{ParsedFile, ParsedUnit, SourceParser};
pub use planner::{GraphPolicy, QueryPlan, QueryPlanner};
pub use repository::{RepositoryScanner, ScannedFile, ScannedRepository};
pub use retrieval::SearchResult;
