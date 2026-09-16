use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SourceAddress;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryIntent {
    ExactEntity,
    NaturalLanguageBehavior,
    IssueLocalization,
    Trace,
    Impact,
    Architecture,
    History,
    PreciseDataflow,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphPolicy {
    None,
    OutgoingTrace,
    IncomingImpact,
    ArchitectureBoundary,
    DataflowRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchRoute {
    NoRetrieval,
    ExactSymbol,
    Lexical,
    DenseRaw,
    DenseSummary,
    Hybrid,
    Structural,
    Knowledge,
    History,
    Reranked,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalRepresentation {
    RawCode,
    Signature,
    /// Compact per-file descriptor (path, imports, top-level signatures) used
    /// for architecture routing instead of whole-file text.
    FileDescriptor,
    SymbolSummary,
    RoleSummary,
    ModuleSummary,
    FlowSummary,
    TestBehavior,
    CommitSummary,
    KnowledgePage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalDocument {
    pub id: String,
    pub entity_id: String,
    pub snapshot_id: String,
    pub representation: RetrievalRepresentation,
    pub body_artifact_digest: String,
    /// Canonical region this document's content is drawn from, when it maps
    /// to a concrete source range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    #[serde(default)]
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub repository_id: String,
    pub snapshot_id: String,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<QueryIntent>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub require_fresh: bool,
    #[serde(default)]
    pub routes: Vec<SearchRoute>,
}

const fn default_limit() -> usize {
    20
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub document_id: String,
    pub entity_id: String,
    /// Canonical region the hit's content was drawn from, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    pub representation: RetrievalRepresentation,
    pub route: SearchRoute,
    pub rank: usize,
    pub score: f64,
    #[serde(default)]
    pub contributing_routes: Vec<SearchRoute>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    pub snippet: String,
    pub verified_current: bool,
    #[serde(default)]
    pub explanation: Vec<String>,
}
