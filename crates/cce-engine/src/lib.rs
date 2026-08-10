#![forbid(unsafe_code)]

//! CCE indexing and query engine.

mod config;
mod context;
mod dataflow;
mod dense;
mod engine;
mod history;
mod lock;
mod parser;
mod planner;
mod repository;
mod rerank;
mod retrieval;
mod scip_graph;

pub use config::{
    DEFAULT_LOCAL_EMBEDDING_DOCUMENT_PREFIX, DEFAULT_LOCAL_EMBEDDING_MODEL,
    DEFAULT_LOCAL_EMBEDDING_QUERY_PREFIX, DEFAULT_LOCAL_EMBEDDING_REVISION,
    DEFAULT_LOCAL_RERANKER_MODEL, DEFAULT_LOCAL_RERANKER_REVISION, DataflowBackendConfig,
    DenseBackendConfig, EngineConfig, IndexOptions, LocalEmbeddingPreset, RerankerBackendConfig,
    ScipBackendConfig, local_embedding_preset,
};
pub use context::{ContextPacker, ContextRequest};
pub use dense::{DenseIndex, DenseSearchHit, Embedder, EmbeddingBackend, LocalFastEmbedder};
pub use engine::{CceEngine, IndexReport};
pub use parser::{ParsedFile, ParsedUnit, SourceParser};
pub use planner::{GraphPolicy, QueryPlan, QueryPlanner};
pub use repository::{RepositoryScanner, ScannedFile, ScannedRepository};
pub use rerank::LocalReranker;
pub use retrieval::SearchResult;
