use std::path::{Path, PathBuf};

use cce_core::{DATA_FORMAT_VERSION, IndexProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexOptions {
    pub max_file_bytes: u64,
    pub max_unit_bytes: usize,
    pub include_hidden: bool,
    /// Index files whose names typically hold credentials (.env, *.key, …).
    /// Off by default: secrets never enter the index, artifacts, or models.
    pub include_sensitive: bool,
    pub respect_gitignore: bool,
    /// Completed snapshots retained besides the current one per index run.
    pub snapshot_retention: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024,
            max_unit_bytes: 64 * 1024,
            include_hidden: true,
            include_sensitive: false,
            respect_gitignore: true,
            snapshot_retention: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseBackendConfig {
    Disabled,
    DeterministicBaseline {
        dimensions: usize,
    },
    /// Local ONNX embedding model identified by its fastembed model code,
    /// e.g. `intfloat/multilingual-e5-small`. Model files are downloaded once
    /// into the data root's `models/` directory; inference is fully offline.
    Local {
        model: String,
    },
}

impl DenseBackendConfig {
    #[must_use]
    pub fn profile_name(&self) -> Option<String> {
        match self {
            Self::Disabled => None,
            Self::DeterministicBaseline { dimensions } => {
                Some(format!("deterministic-baseline-{dimensions}"))
            }
            Self::Local { model } => Some(format!("local:{model}")),
        }
    }

    #[must_use]
    pub const fn is_production(&self) -> bool {
        matches!(self, Self::Local { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub repository_root: PathBuf,
    pub data_root: PathBuf,
    pub index: IndexOptions,
    pub dense: DenseBackendConfig,
}

impl EngineConfig {
    #[must_use]
    pub fn for_repository(root: impl AsRef<Path>) -> Self {
        let repository_root = root.as_ref().to_path_buf();
        let data_root = std::env::var_os("CCE_DATA_DIR")
            .map_or_else(|| repository_root.join(".cce"), PathBuf::from);
        Self {
            repository_root,
            data_root,
            index: IndexOptions::default(),
            dense: DenseBackendConfig::Disabled,
        }
    }

    #[must_use]
    pub fn profile(&self) -> IndexProfile {
        let mut options = std::collections::BTreeMap::new();
        options.insert(
            "max_file_bytes".to_owned(),
            self.index.max_file_bytes.to_string(),
        );
        options.insert(
            "max_unit_bytes".to_owned(),
            self.index.max_unit_bytes.to_string(),
        );
        options.insert(
            "include_hidden".to_owned(),
            self.index.include_hidden.to_string(),
        );
        options.insert(
            "include_sensitive".to_owned(),
            self.index.include_sensitive.to_string(),
        );
        options.insert(
            "respect_gitignore".to_owned(),
            self.index.respect_gitignore.to_string(),
        );
        IndexProfile {
            schema_version: DATA_FORMAT_VERSION,
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            parser_profile: "tree-sitter-v1".to_owned(),
            chunk_policy: format!("ast-l0-l1-l2-max-{}", self.index.max_unit_bytes),
            embedding_model: self.dense.profile_name(),
            embedding_revision: None,
            options,
        }
    }
}
