use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryIdentity {
    pub id: String,
    pub canonical_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IndexProfile {
    pub schema_version: u32,
    pub engine_version: String,
    pub parser_profile: String,
    pub chunk_policy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_revision: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotIdentity {
    pub id: String,
    pub repository_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    pub workspace_overlay_hash: String,
    pub index_profile_hash: String,
    pub created_at: DateTime<Utc>,
    pub file_count: u64,
    pub source_bytes: u64,
}
