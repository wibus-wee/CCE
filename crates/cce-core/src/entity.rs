use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SourceAddress;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Repository,
    Directory,
    File,
    Package,
    Module,
    Namespace,
    Class,
    Interface,
    Trait,
    Struct,
    Enum,
    Function,
    Method,
    Field,
    Constant,
    Route,
    Schema,
    Test,
    Configuration,
    Concept,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodeEntity {
    pub id: String,
    pub kind: EntityKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Canonical region this entity occupies, when it maps to a source range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub attributes: serde_json::Map<String, serde_json::Value>,
}
