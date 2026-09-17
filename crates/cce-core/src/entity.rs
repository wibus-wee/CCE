use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::SourceAddress;

/// Structural classification of a code entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// The repository root.
    Repository,
    /// A filesystem directory.
    Directory,
    /// A source file.
    File,
    /// A package (crate, npm package, …).
    Package,
    /// A module.
    Module,
    /// A namespace.
    Namespace,
    /// A class.
    Class,
    /// An interface.
    Interface,
    /// A trait (Rust) or equivalent contract.
    Trait,
    /// A struct/record type.
    Struct,
    /// An enum type.
    Enum,
    /// A free function.
    Function,
    /// A method on a type.
    Method,
    /// A field/member.
    Field,
    /// A constant/static.
    Constant,
    /// An HTTP/IPC route definition.
    Route,
    /// A schema definition.
    Schema,
    /// A test entity.
    Test,
    /// A configuration entity.
    Configuration,
    /// A conceptual entity (documentation/knowledge).
    Concept,
    /// A version-control commit — historical evidence, not current-source
    /// truth.
    Commit,
    /// Could not be classified.
    Unknown,
}

/// A node in the entity graph — a file, symbol, or structural element with a
/// canonical source address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodeEntity {
    /// Deterministic entity identifier.
    pub id: String,
    /// Structural classification.
    pub kind: EntityKind,
    /// Short name.
    pub name: String,
    /// Fully qualified name (path + symbol) when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    /// Declaration signature when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Programming language of the entity's definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Canonical region this entity occupies, when it maps to a source range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    /// Source address of the entity's extent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// Capability tags attached to the entity.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Free-form extractor-specific attributes.
    #[serde(default)]
    pub attributes: serde_json::Map<String, serde_json::Value>,
}
