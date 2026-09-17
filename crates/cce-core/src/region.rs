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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
/// A canonical code range within a snapshot — the join key across views.
pub struct CodeRegion {
    /// Deterministic region id.
    pub id: String,
    /// Snapshot the region belongs to.
    pub snapshot_id: String,
    /// Repository-relative file path.
    pub path: String,
    /// Region granularity.
    pub kind: RegionKind,
    /// Language of the file when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Symbol name for symbol regions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    /// Entity kind of the symbol when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<EntityKind>,
    /// Qualified symbol name when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    /// Enclosing region (e.g. symbol inside file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_region_id: Option<String>,
    /// Inclusive start byte offset.
    pub start_byte: u64,
    /// Exclusive end byte offset.
    pub end_byte: u64,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line (inclusive).
    pub end_line: u32,
}
