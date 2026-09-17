use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{GraphPolicy, QueryIntent, SearchRoute, SourceAddress};

/// The role a packed context item plays for the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemKind {
    /// High-level repository/task orientation.
    Orientation,
    /// Likely entry point for the task.
    EntryPoint,
    /// Verbatim source code.
    Source,
    /// API/type contract or signature.
    Contract,
    /// A path through the relation graph.
    RelationPath,
    /// Test code or behavior evidence.
    Test,
    /// Configuration or build metadata.
    Config,
    /// Commit/history evidence.
    History,
    /// Generated knowledge content.
    Knowledge,
}

/// Why and how a context item was retrieved, with source linkage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextProvenance {
    /// Human-readable reason this item was included.
    pub why_retrieved: String,
    /// Route that produced the item.
    pub route: SearchRoute,
    /// Rank in the underlying result list.
    pub rank: usize,
    /// Score assigned by the producing route.
    pub score: f64,
    /// Snapshot the item was retrieved from.
    pub snapshot_id: String,
    /// Whether content was verified against the current worktree.
    pub verified_current: bool,
    /// Symbol name when the item is symbol-shaped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    /// Primary source address backing the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_address: Option<SourceAddress>,
    /// Additional supporting source addresses.
    #[serde(default)]
    pub evidence_addresses: Vec<SourceAddress>,
}

/// A single packed context item with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    /// Item identifier (document id).
    pub id: String,
    /// Role the item plays.
    pub kind: ContextItemKind,
    /// Short display title.
    pub title: String,
    /// Item body text.
    pub body: String,
    /// Estimated token cost of the item.
    pub estimated_tokens: usize,
    /// Retrieval provenance.
    pub provenance: ContextProvenance,
}

/// A capability gap surfaced to the caller instead of silently degrading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Uncertainty {
    /// Name of the missing/partial capability.
    pub capability: String,
    /// Explanation of what is missing and why.
    pub message: String,
    /// What the caller could do to satisfy the capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<String>,
}

/// A token-budgeted bundle of context items plus explicit uncertainty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextPack {
    /// Repository the pack was built from.
    pub repository_id: String,
    /// Snapshot the pack was built against.
    pub snapshot_id: String,
    /// Original query text.
    pub query: String,
    /// Classified query intent.
    pub intent: QueryIntent,
    /// Routes the executed plan selected; empty when unknown.
    #[serde(default)]
    pub plan_routes: Vec<SearchRoute>,
    /// Graph expansion policy applied, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_policy: Option<GraphPolicy>,
    /// Token budget requested.
    pub budget_tokens: usize,
    /// Tokens actually consumed by the packed items.
    pub used_tokens: usize,
    /// Engine-side wall time for the underlying search plus packing, in ms.
    /// Distinct from caller-observed latency, which also includes transport.
    #[serde(default)]
    pub latency_ms: u64,
    /// Packed items in priority order.
    pub items: Vec<ContextItem>,
    /// Explicit capability gaps affecting this pack.
    #[serde(default)]
    pub uncertainties: Vec<Uncertainty>,
    /// Capability names that could not be satisfied at all.
    #[serde(default)]
    pub missing_capabilities: Vec<String>,
}
