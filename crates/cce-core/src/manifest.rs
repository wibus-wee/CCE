use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    ToSchema,
)]
#[serde(rename_all = "snake_case")]
/// One independently-built index view over a snapshot.
pub enum ViewKind {
    /// Canonical source content (worktree truth).
    Source,
    /// Full-text lexical index (`SQLite` FTS5).
    Lexical,
    /// Dense embedding index.
    Dense,
    /// Parsed symbol table.
    Symbols,
    /// Typed relation graph.
    Graph,
    /// Commit/history index.
    History,
    /// Generated knowledge pages.
    Knowledge,
    /// Dataflow/taint evidence graph.
    Dataflow,
}

impl std::fmt::Display for ViewKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned())
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
/// Freshness/completeness state of a view — stale and unavailable are
/// first-class states, never silently hidden.
pub enum ViewState {
    /// Indexing in progress.
    Building,
    /// Fully built and current.
    Ready,
    /// Built with known coverage gaps.
    Partial,
    /// Built against an older snapshot; usable but not current.
    Stale,
    /// Not built (missing tool/backend); remediation is reported.
    Unavailable,
    /// Build attempted and failed; diagnostics are reported.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
/// A named capability a view provides, at a stated trust level.
pub struct Capability {
    /// Capability identifier (e.g. `sqlite_fts5`, `scip:rust-analyzer`).
    pub name: String,
    /// Trust level (`authoritative`, `compiler_derived`, `syntax_only`, …).
    pub level: String,
    /// Optional qualifier explaining the level or coverage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Reported state of one view for a snapshot, including capabilities,
/// the backing artifact, and any diagnostic message.
pub struct ViewStatus {
    /// Lifecycle state.
    pub state: ViewState,
    /// Snapshot this status applies to.
    pub snapshot_id: String,
    /// Index profile hash the view was built under.
    pub profile_hash: String,
    /// When the status was last written.
    pub updated_at: DateTime<Utc>,
    /// Capabilities the view currently provides.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Digest of the backing artifact (provider output, …) when relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
    /// Human-readable diagnostic or remediation note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Per-snapshot status of all index views.
pub struct ViewManifest {
    /// Owning repository.
    pub repository_id: String,
    /// Snapshot the manifest describes.
    pub snapshot_id: String,
    /// View states keyed by view kind.
    pub views: BTreeMap<ViewKind, ViewStatus>,
}
