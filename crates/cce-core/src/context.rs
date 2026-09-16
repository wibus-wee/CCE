use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{GraphPolicy, QueryIntent, SearchRoute, SourceAddress};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemKind {
    Orientation,
    EntryPoint,
    Source,
    Contract,
    RelationPath,
    Test,
    Config,
    History,
    Knowledge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextProvenance {
    pub why_retrieved: String,
    pub route: SearchRoute,
    pub rank: usize,
    pub score: f64,
    pub snapshot_id: String,
    pub verified_current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_address: Option<SourceAddress>,
    #[serde(default)]
    pub evidence_addresses: Vec<SourceAddress>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    pub id: String,
    pub kind: ContextItemKind,
    pub title: String,
    pub body: String,
    pub estimated_tokens: usize,
    pub provenance: ContextProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Uncertainty {
    pub capability: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextPack {
    pub repository_id: String,
    pub snapshot_id: String,
    pub query: String,
    pub intent: QueryIntent,
    /// Routes the executed plan selected; empty when unknown.
    #[serde(default)]
    pub plan_routes: Vec<SearchRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_policy: Option<GraphPolicy>,
    pub budget_tokens: usize,
    pub used_tokens: usize,
    pub items: Vec<ContextItem>,
    #[serde(default)]
    pub uncertainties: Vec<Uncertainty>,
    #[serde(default)]
    pub missing_capabilities: Vec<String>,
}
