use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SourceAddress;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Contains,
    Defines,
    Imports,
    Exports,
    References,
    Implements,
    Extends,
    TypeUses,
    Instantiates,
    Calls,
    Tests,
    BuildDependsOn,
    RouteHandledBy,
    PublishesEvent,
    SubscribesEvent,
    ReadsStore,
    WritesStore,
    UsesHook,
    SerializesAs,
    PersistsTo,
    ChangedWith,
    Semantic,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationOrigin {
    Compiler,
    Scip,
    Lsp,
    TreeSitter,
    BuildSystem,
    FrameworkRule,
    ModelInference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub id: String,
    pub source_entity_id: String,
    pub target_entity_id: String,
    pub kind: RelationKind,
    pub origin: RelationOrigin,
    pub confidence: f32,
    pub snapshot_id: String,
    pub extractor: String,
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    #[serde(default)]
    pub attributes: serde_json::Map<String, serde_json::Value>,
}
