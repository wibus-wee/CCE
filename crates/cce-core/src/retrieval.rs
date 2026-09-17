use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SourceAddress;

/// Coarse classification of what a query is trying to accomplish, used by
/// the planner to pick routes and graph policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryIntent {
    /// Looking up a specific named entity (symbol, type, file).
    ExactEntity,
    /// Free-form "where/how does X behave" question.
    NaturalLanguageBehavior,
    /// Localizing the code responsible for a reported issue.
    IssueLocalization,
    /// Following a call/data path forward from a starting point.
    Trace,
    /// Finding what would be affected by changing an entity.
    Impact,
    /// Understanding module/architecture structure.
    Architecture,
    /// Asking about when or why code changed.
    History,
    /// Requires source-to-sink dataflow evidence; explicitly refused when
    /// no dataflow view is available.
    PreciseDataflow,
    /// Intent could not be classified.
    Unknown,
}

/// How much graph expansion the planner is allowed to apply to a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphPolicy {
    /// No graph expansion.
    None,
    /// Follow outgoing edges from matched entities.
    OutgoingTrace,
    /// Follow incoming edges to matched entities.
    IncomingImpact,
    /// Expand along module/architecture boundaries.
    ArchitectureBoundary,
    /// Requires evidence-backed dataflow edges; refused when unavailable.
    DataflowRequired,
}

/// Which retrieval channel produced (or should produce) a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchRoute {
    /// Planner decided no retrieval is needed (e.g. abstention cases).
    NoRetrieval,
    /// Exact symbol/entity lookup.
    ExactSymbol,
    /// Full-text (`SQLite` FTS5) search.
    Lexical,
    /// Dense embedding over raw code documents.
    DenseRaw,
    /// Dense embedding over generated summary documents.
    DenseSummary,
    /// Fusion of lexical and dense channels.
    Hybrid,
    /// Structural/graph-driven retrieval.
    Structural,
    /// Retrieval over generated knowledge pages.
    Knowledge,
    /// Retrieval over commit/history documents.
    History,
    /// Result ordering produced by the reranker stage.
    Reranked,
}

/// What form a retrieval document's content takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalRepresentation {
    /// Verbatim source text.
    RawCode,
    /// A symbol signature without its body.
    Signature,
    /// Compact per-file descriptor (path, imports, top-level signatures) used
    /// for architecture routing instead of whole-file text.
    FileDescriptor,
    /// Summary of a single symbol.
    SymbolSummary,
    /// Summary of a file's role.
    RoleSummary,
    /// Summary of a module.
    ModuleSummary,
    /// Summary of a call/data flow.
    FlowSummary,
    /// Summary of test behavior.
    TestBehavior,
    /// Summary derived from a commit.
    CommitSummary,
    /// A generated knowledge page.
    KnowledgePage,
}

/// A single retrievable document: content plus provenance linking it back to
/// source addresses and (for generated content) its producing model/step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalDocument {
    /// Document identifier.
    pub id: String,
    /// Entity this document is attached to.
    pub entity_id: String,
    /// Snapshot the document was built against.
    pub snapshot_id: String,
    /// Which representation the body carries.
    pub representation: RetrievalRepresentation,
    /// Content-addressed digest of the body payload.
    pub body_artifact_digest: String,
    /// Canonical region this document's content is drawn from, when it maps
    /// to a concrete source range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    /// Source address of the document's primary range, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// Embedding profile used for dense indexing, when embedded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_profile: Option<String>,
    /// Identity of the model or deterministic step that generated the body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    /// Additional source addresses supporting this document.
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    /// Precomputed search terms (identifier splits, folds) for lexical match.
    #[serde(default)]
    pub terms: Vec<String>,
}

/// A search request scoped to one repository snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    /// Repository to search in.
    pub repository_id: String,
    /// Snapshot to search against.
    pub snapshot_id: String,
    /// Raw query text.
    pub query: String,
    /// Optional pre-classified intent; the planner classifies when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<QueryIntent>,
    /// Maximum number of hits to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// When true, refuse results from stale snapshots.
    #[serde(default)]
    pub require_fresh: bool,
    /// Route allowlist; empty means planner chooses.
    #[serde(default)]
    pub routes: Vec<SearchRoute>,
    /// Structured hit filters. `key:value` tokens in `query`
    /// (`lang:rust`, `path:crates/…`) are parsed into this field; callers
    /// may also set it directly, in which case the query is not parsed.
    #[serde(default)]
    pub filters: QueryFilters,
}

/// Conjunctive hit filters parsed from `key:value` query tokens or set
/// directly by API callers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryFilters {
    /// Repository-relative path prefix (`path:crates/cce-engine`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    /// Entity language (`lang:rust`) matched exactly, case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl QueryFilters {
    /// Whether any filter is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.path_prefix.is_none() && self.language.is_none()
    }
}

/// Extracts `lang:`/`path:` tokens from a query string, returning the
/// cleaned query text and the structured filters. Unknown `key:` tokens
/// stay in the query text untouched.
#[must_use]
pub fn parse_query_filters(query: &str) -> (String, QueryFilters) {
    let mut filters = QueryFilters::default();
    let mut kept = Vec::new();
    for token in query.split_whitespace() {
        let Some((key, value)) = token.split_once(':') else {
            kept.push(token);
            continue;
        };
        if value.is_empty() {
            kept.push(token);
            continue;
        }
        match key {
            "lang" => filters.language = Some(value.to_ascii_lowercase()),
            "path" => filters.path_prefix = Some(value.to_owned()),
            _ => kept.push(token),
        }
    }
    (kept.join(" "), filters)
}

const fn default_limit() -> usize {
    20
}

/// One scored retrieval result with full provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Identifier of the retrieved document.
    pub document_id: String,
    /// Entity the document belongs to.
    pub entity_id: String,
    /// Canonical region the hit's content was drawn from, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    /// Symbol name when the hit is symbol-shaped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    /// Representation of the hit's content.
    pub representation: RetrievalRepresentation,
    /// Route that produced (or last re-ranked) this hit.
    pub route: SearchRoute,
    /// 1-based position in the returned list.
    pub rank: usize,
    /// Final score used for ordering.
    pub score: f64,
    /// All routes that contributed to this hit under fusion.
    #[serde(default)]
    pub contributing_routes: Vec<SearchRoute>,
    /// Primary source address of the hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// Supporting evidence addresses.
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    /// Display snippet for the hit.
    pub snippet: String,
    /// Whether the hit's content was verified against the current worktree.
    pub verified_current: bool,
    /// Human-readable reasons explaining why this hit was returned.
    #[serde(default)]
    pub explanation: Vec<String>,
}
