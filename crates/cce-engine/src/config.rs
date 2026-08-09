use std::path::{Path, PathBuf};

use cce_core::{DATA_FORMAT_VERSION, IndexProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexOptions {
    pub max_file_bytes: u64,
    pub max_unit_bytes: usize,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024,
            max_unit_bytes: 64 * 1024,
            include_hidden: true,
            respect_gitignore: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseBackendConfig {
    Disabled,
    DeterministicBaseline {
        dimensions: usize,
    },
    OpenAiCompatible {
        base_url: String,
        model: String,
        api_key_environment: String,
        dimensions: Option<usize>,
        batch_size: usize,
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
            Self::OpenAiCompatible {
                model, dimensions, ..
            } => Some(format!(
                "openai-compatible:{model}:{}",
                dimensions.map_or_else(|| "native".to_owned(), |value| value.to_string())
            )),
        }
    }

    #[must_use]
    pub const fn is_production(&self) -> bool {
        matches!(self, Self::OpenAiCompatible { .. })
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
            .map(PathBuf::from)
            .unwrap_or_else(|| repository_root.join(".cce"));
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
