use std::path::{Path, PathBuf};

use cce_core::{DATA_FORMAT_VERSION, IndexProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Tunables for the indexing pipeline; hashed into the index profile.
pub struct IndexOptions {
    /// Files larger than this are skipped.
    pub max_file_bytes: u64,
    /// Source units larger than this are split into subregions.
    pub max_unit_bytes: usize,
    /// Whether dotfiles/hidden paths are indexed.
    pub include_hidden: bool,
    /// Index files whose names typically hold credentials (.env, *.key, …).
    /// Off by default: secrets never enter the index, artifacts, or models.
    pub include_sensitive: bool,
    /// Whether gitignore/ignore rules are honored during the scan.
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

/// Default local embedding model for `--dense local`.
///
/// Chosen on the cce-self v5.1 benchmark (same corpus, daemon session): vs
/// `intfloat/multilingual-e5-small` this model improves nDCG@10 +0.09
/// (Holm-significant) and recall@5 +0.10, at a small recall@20 tail cost.
pub const DEFAULT_LOCAL_EMBEDDING_MODEL: &str = "jinaai/jina-embeddings-v2-base-code";

/// Default local cross-encoder reranker for `--reranker`. Multilingual
/// (the vocabulary-gap suite includes CJK queries); scored pairs are
/// (query, source snippet).
pub const DEFAULT_LOCAL_RERANKER_MODEL: &str = "rozgo/bge-reranker-v2-m3";

/// Which dense embedding backend to build during indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseBackendConfig {
    /// No dense index; lexical/structural views only.
    Disabled,
    /// Deterministic hash-based baseline for offline tests/benchmarks.
    DeterministicBaseline {
        /// Vector dimensionality.
        dimensions: usize,
    },
    /// Local ONNX embedding model identified by its fastembed model code,
    /// e.g. `intfloat/multilingual-e5-small`. Model files are downloaded once
    /// into the data root's `models/` directory; inference is fully offline.
    Local {
        /// fastembed model code (`jinaai/jina-embeddings-v2-base-code`, …).
        model: String,
    },
}

impl DenseBackendConfig {
    /// Identifier folded into the index profile hash, or `None` when dense
    /// indexing is disabled.
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

    /// Whether this backend produces benchmark-meaningful embeddings.
    #[must_use]
    pub const fn is_production(&self) -> bool {
        matches!(self, Self::Local { .. })
    }
}

/// External code-intelligence providers (SCIP indexers). Running them is
/// local and offline; provisioning downloads are a separate opt-in layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Whether `index()` invokes providers at all.
    pub enabled: bool,
    /// Per-provider subprocess timeout.
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_secs: 600,
        }
    }
}

/// Which build product a scan+commit produces. Runtime-only — callers
/// flip it per call, never persist it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexVariant {
    /// Full pipeline: parse, providers, dense, zoekt, history.
    #[default]
    Full,
    /// Shallow pass: parse + relations + artifacts only — the verify-loop
    /// snapshot. A distinct profile hash means a distinct snapshot id for
    /// identical content, so a checkpoint can never satisfy or shadow a
    /// full index of the same worktree.
    Checkpoint,
}

/// Root configuration for one engine instance (single repository scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Repository root being indexed/served.
    pub repository_root: PathBuf,
    /// Where `.cce` state lives (`SQLite`, artifacts, provider work dirs).
    pub data_root: PathBuf,
    /// Snapshot build variant — `index()` runs `Full`; `checkpoint()`
    /// clones this config with `Checkpoint` for the scan, so the minted
    /// snapshot carries the shallow profile in its identity.
    pub variant: IndexVariant,
    /// Indexing tunables.
    pub index: IndexOptions,
    /// Dense embedding backend selection.
    pub dense: DenseBackendConfig,
    /// Local cross-encoder reranker model code, e.g.
    /// `rozgo/bge-reranker-v2-m3`. `None` keeps fused-order ranking only.
    pub reranker_model: Option<String>,
    /// External provider (SCIP indexer) orchestration settings.
    pub providers: ProviderConfig,
}

impl EngineConfig {
    /// Default configuration for a repository: data dir at `.cce` (or
    /// `CCE_DATA_DIR`), dense disabled, providers enabled.
    #[must_use]
    pub fn for_repository(root: impl AsRef<Path>) -> Self {
        let repository_root = root.as_ref().to_path_buf();
        let data_root = std::env::var_os("CCE_DATA_DIR")
            .map_or_else(|| repository_root.join(".cce"), PathBuf::from);
        Self {
            repository_root,
            data_root,
            variant: IndexVariant::Full,
            index: IndexOptions::default(),
            dense: DenseBackendConfig::Disabled,
            reranker_model: None,
            providers: ProviderConfig::default(),
        }
    }

    /// The index profile hashed into snapshot identity — any config that
    /// can change index output must appear here.
    #[must_use]
    pub fn profile(&self) -> IndexProfile {
        let mut options = std::collections::BTreeMap::new();
        // Bumping this forces fresh snapshots when the FTS document
        // materialization changes (e.g. identifier split/fold forms in the
        // name column) without a config change on the user's side.
        options.insert(
            "lexical_tokenizer".to_owned(),
            "identifier-forms-v1".to_owned(),
        );
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
        // History diff extraction changes document content; bump when the
        // extractor changes shape.
        options.insert("history_diff".to_owned(), "v1".to_owned());
        // Provider output changes the snapshot; toggling or upgrading the
        // ingest must produce a fresh profile hash. v2 scopes `local`
        // symbols to their owning document.
        options.insert(
            "scip_ingest".to_owned(),
            if self.providers.enabled {
                "v2".to_owned()
            } else {
                "off".to_owned()
            },
        );
        // Call-edge materialization versioning, independent of SCIP:
        // v2 dedups on (caller, callee) so distinct callers to one callee
        // all survive. Snapshots built under v1 silently dropped edges.
        options.insert("call_edges".to_owned(), "v2".to_owned());
        // Same rule for type references: v2 dedups on (source, target).
        options.insert("type_refs".to_owned(), "v2".to_owned());
        // Descriptor materialization: v2 persists FileDescriptor and
        // SymbolSummary bodies as their own artifacts (read dispatch keys on
        // generated_by -v2). v1 pointed those documents at the source file
        // artifact, so restored/dense-embedded text was raw source — snapshots
        // built under v1 must not satisfy a v2 profile.
        options.insert("descriptor_bodies".to_owned(), "v2".to_owned());
        // A checkpoint commits parse+relations only — no providers, dense,
        // or zoekt. The marker lives in the profile so checkpoint snapshots
        // get distinct ids from full snapshots of identical content.
        if self.variant == IndexVariant::Checkpoint {
            options.insert("snapshot_variant".to_owned(), "checkpoint".to_owned());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_hash(profile: &IndexProfile) -> String {
        blake3::hash(&serde_json::to_vec(profile).expect("profile json"))
            .to_hex()
            .to_string()
    }

    /// Graph materialization versions ride the profile: a snapshot built
    /// under v1 keying/dedup rules must not hash-match the corrected v2
    /// profile, or its wrong edges would silently satisfy new queries.
    #[test]
    fn graph_materialization_versions_are_profiled() {
        let config = EngineConfig::for_repository("/tmp/repo");
        let profile = config.profile();
        assert_eq!(
            profile.options.get("scip_ingest").map(String::as_str),
            Some("v2")
        );
        assert_eq!(
            profile.options.get("call_edges").map(String::as_str),
            Some("v2")
        );
        assert_eq!(
            profile.options.get("type_refs").map(String::as_str),
            Some("v2")
        );
        assert_eq!(
            profile.options.get("descriptor_bodies").map(String::as_str),
            Some("v2")
        );

        let mut legacy = profile.clone();
        legacy
            .options
            .insert("scip_ingest".to_owned(), "v1".to_owned());
        legacy
            .options
            .insert("call_edges".to_owned(), "v1".to_owned());
        legacy
            .options
            .insert("type_refs".to_owned(), "v1".to_owned());
        legacy
            .options
            .insert("descriptor_bodies".to_owned(), "v1".to_owned());
        assert_ne!(
            profile_hash(&profile),
            profile_hash(&legacy),
            "v1 graph materializations must not hash-match the v2 profile"
        );
    }

    #[test]
    fn disabled_providers_profile_as_off() {
        let mut config = EngineConfig::for_repository("/tmp/repo");
        config.providers.enabled = false;
        let profile = config.profile();
        assert_eq!(
            profile.options.get("scip_ingest").map(String::as_str),
            Some("off")
        );
        // Call-edge extraction is tree-sitter based — unaffected by the
        // provider switch.
        assert_eq!(
            profile.options.get("call_edges").map(String::as_str),
            Some("v2")
        );
    }
}
