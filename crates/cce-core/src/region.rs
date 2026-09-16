use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::EntityKind;

/// Canonical code-range identity for one snapshot. Every index — lexical
/// documents, dense vectors, graph nodes, and citations — joins on this id,
/// so a `RegionId` is the single object all views describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    /// Whole-file region; also the architecture-routing unit.
    File,
    /// A tree-sitter symbol (function, type, impl block, …).
    Symbol,
    /// A bounded slice of an oversized symbol or of an unparsed file.
    Subregion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodeRegion {
    pub id: String,
    pub snapshot_id: String,
    pub path: String,
    pub kind: RegionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<EntityKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_region_id: Option<String>,
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u32,
    pub end_line: u32,
}
