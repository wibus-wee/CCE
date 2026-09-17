use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
/// Stable identity of an indexed repository.
pub struct RepositoryIdentity {
    /// Deterministic repository id derived from the canonical root.
    pub id: String,
    /// Canonicalized absolute path of the repository root.
    pub canonical_root: String,
    /// Primary remote URL when the repository has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
/// Everything that distinguishes one index build configuration from
/// another; hashed into `index_profile_hash` so config changes yield
/// distinct snapshots.
pub struct IndexProfile {
    /// On-disk schema version.
    pub schema_version: u32,
    /// Engine version that produced the index.
    pub engine_version: String,
    /// Parser configuration identifier.
    pub parser_profile: String,
    /// Chunking policy identifier.
    pub chunk_policy: String,
    /// Embedding model id when dense indexing is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Embedding model revision/hash when pinned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_revision: Option<String>,
    /// Additional profile-affecting options (provider config, …).
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
/// Immutable identity of one indexed snapshot: repository + base revision +
/// worktree overlay + index profile.
pub struct SnapshotIdentity {
    /// Content-derived snapshot id.
    pub id: String,
    /// Owning repository.
    pub repository_id: String,
    /// Git commit the worktree is based on, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    /// Hash of worktree contents overlaid on the base revision.
    pub workspace_overlay_hash: String,
    /// Hash of the index profile used for the build.
    pub index_profile_hash: String,
    /// When the snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Number of files captured.
    pub file_count: u64,
    /// Total source bytes captured.
    pub source_bytes: u64,
}
