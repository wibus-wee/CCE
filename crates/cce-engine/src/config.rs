use std::path::{Path, PathBuf};

use cce_core::{CceError, DATA_FORMAT_VERSION, IndexProfile, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_LOCAL_EMBEDDING_MODEL: &str = "JinaEmbeddingsV2BaseCode";
pub const DEFAULT_LOCAL_EMBEDDING_REVISION: &str = "516f4baf13dec4ddddda8631e019b5737c8bc250";
pub const DEFAULT_LOCAL_EMBEDDING_QUERY_PREFIX: &str = "";
pub const DEFAULT_LOCAL_EMBEDDING_DOCUMENT_PREFIX: &str = "";
pub const DEFAULT_LOCAL_RERANKER_MODEL: &str = "jinaai/jina-reranker-v1-turbo-en";
pub const DEFAULT_LOCAL_RERANKER_REVISION: &str = "5bcd26bbe1913aff3cc1983d91f2c3ac9ee91cbf";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalEmbeddingPreset {
    pub model: &'static str,
    pub revision: &'static str,
    pub query_prefix: &'static str,
    pub document_prefix: &'static str,
}

#[must_use]
pub fn local_embedding_preset(value: &str) -> Option<LocalEmbeddingPreset> {
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "default"
        | "quality"
        | "jina"
        | "jina-code"
        | "jinaembeddingsv2basecode"
        | "jinaai/jina-embeddings-v2-base-code" => Some(LocalEmbeddingPreset {
            model: DEFAULT_LOCAL_EMBEDDING_MODEL,
            revision: DEFAULT_LOCAL_EMBEDDING_REVISION,
            query_prefix: DEFAULT_LOCAL_EMBEDDING_QUERY_PREFIX,
            document_prefix: DEFAULT_LOCAL_EMBEDDING_DOCUMENT_PREFIX,
        }),
        "balanced"
        | "multilingual-e5-small"
        | "multilinguale5small"
        | "intfloat/multilingual-e5-small" => Some(LocalEmbeddingPreset {
            model: "MultilingualE5Small",
            revision: "614241f622f53c4eeff9890bdc4f31cfecc418b3",
            query_prefix: "query: ",
            document_prefix: "passage: ",
        }),
        "fast" | "bge-small-q" | "bgesmallenv15q" | "qdrant/bge-small-en-v1.5-onnx-q" => {
            Some(LocalEmbeddingPreset {
                model: "BGESmallENV15Q",
                revision: "52398278842ec682c6f32300af41344b1c0b0bb2",
                query_prefix: "Represent this sentence for searching relevant passages: ",
                document_prefix: "",
            })
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexOptions {
    pub max_file_bytes: u64,
    pub max_unit_bytes: usize,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipBackendConfig {
    Disabled,
    Supplied {
        path: PathBuf,
    },
    RustAnalyzer {
        executable: PathBuf,
        version: String,
        threads: Option<usize>,
        timeout_seconds: u64,
    },
    Auto {
        rust_analyzer: PathBuf,
        typescript_indexer: PathBuf,
        threads: Option<usize>,
        timeout_seconds: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataflowBackendConfig {
    Disabled,
    Supplied {
        path: PathBuf,
    },
    Joern {
        parse_executable: PathBuf,
        export_executable: PathBuf,
        version: String,
        timeout_seconds: u64,
        language: Option<String>,
    },
}

impl DataflowBackendConfig {
    pub fn joern(
        parse_executable: impl Into<PathBuf>,
        export_executable: impl Into<PathBuf>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        Self::joern_with_language(parse_executable, export_executable, timeout_seconds, None)
    }

    pub fn joern_with_language(
        parse_executable: impl Into<PathBuf>,
        export_executable: impl Into<PathBuf>,
        timeout_seconds: u64,
        language: Option<&str>,
    ) -> Result<Self> {
        if !(30..=86_400).contains(&timeout_seconds) {
            return Err(CceError::Configuration(format!(
                "Joern timeout must be 30..=86400 seconds, got {timeout_seconds}"
            )));
        }
        let parse_executable = resolve_executable(parse_executable.into())?;
        let export_executable = resolve_executable(export_executable.into())?;
        let language = language
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                if value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
                {
                    Ok(value.to_ascii_uppercase())
                } else {
                    Err(CceError::Configuration(format!(
                        "invalid Joern language {value:?}; expected an alphanumeric frontend name"
                    )))
                }
            })
            .transpose()?;
        let output = std::process::Command::new(&export_executable)
            .arg("--version")
            .output()
            .map_err(|error| {
                CceError::Configuration(format!(
                    "failed to execute Joern exporter {}: {error}",
                    export_executable.display()
                ))
            })?;
        let reported_version = if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            [stdout, stderr]
                .into_iter()
                .find(|value| {
                    !value.is_empty()
                        && !value.to_ascii_lowercase().contains("unknown option")
                        && !value.to_ascii_lowercase().starts_with("error:")
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let version = if reported_version.is_empty() {
            format!(
                "local-launchers-blake3:{}:{}",
                file_digest(&parse_executable).unwrap_or_else(|| "unavailable".to_owned()),
                file_digest(&export_executable).unwrap_or_else(|| "unavailable".to_owned())
            )
        } else {
            reported_version
        };
        Ok(Self::Joern {
            parse_executable,
            export_executable,
            version,
            timeout_seconds,
            language,
        })
    }

    fn profile_name(&self) -> String {
        match self {
            Self::Disabled => "disabled".to_owned(),
            Self::Supplied { path } => format!(
                "supplied:{}:{}",
                path.display(),
                file_digest(path).unwrap_or_else(|| "unavailable".to_owned())
            ),
            Self::Joern {
                parse_executable,
                export_executable,
                version,
                language,
                ..
            } => format!(
                "joern:{}:{}:{}:{}",
                parse_executable.display(),
                export_executable.display(),
                short_hash(version),
                language.as_deref().unwrap_or("auto")
            ),
        }
    }
}

impl ScipBackendConfig {
    pub fn auto(
        rust_analyzer: impl Into<PathBuf>,
        typescript_indexer: impl Into<PathBuf>,
        threads: Option<usize>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        validate_scip_limits(threads, timeout_seconds)?;
        Ok(Self::Auto {
            rust_analyzer: rust_analyzer.into(),
            typescript_indexer: typescript_indexer.into(),
            threads,
            timeout_seconds,
        })
    }

    pub fn rust_analyzer(
        executable: impl Into<PathBuf>,
        threads: Option<usize>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        validate_scip_limits(threads, timeout_seconds)?;
        let executable = resolve_executable(executable.into())?;
        let output = std::process::Command::new(&executable)
            .arg("--version")
            .output()
            .map_err(|error| {
                CceError::Configuration(format!(
                    "failed to execute SCIP indexer {}: {error}",
                    executable.display()
                ))
            })?;
        if !output.status.success() {
            return Err(CceError::Configuration(format!(
                "SCIP indexer {} --version exited with {}: {}",
                executable.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let version = String::from_utf8(output.stdout)
            .map_err(|error| {
                CceError::Configuration(format!("SCIP indexer version is not UTF-8: {error}"))
            })?
            .trim()
            .to_owned();
        if version.is_empty() {
            return Err(CceError::Configuration(
                "SCIP indexer returned an empty version".to_owned(),
            ));
        }
        Ok(Self::RustAnalyzer {
            executable,
            version,
            threads,
            timeout_seconds,
        })
    }

    fn profile_name(&self) -> String {
        match self {
            Self::Disabled => "disabled".to_owned(),
            Self::Supplied { path } => format!(
                "supplied:{}:{}",
                path.display(),
                file_digest(path).unwrap_or_else(|| "unavailable".to_owned())
            ),
            Self::RustAnalyzer {
                executable,
                version,
                threads,
                ..
            } => format!(
                "rust-analyzer:{}:{}:threads{}",
                executable.display(),
                short_hash(version),
                threads.map_or_else(|| "auto".to_owned(), |value| value.to_string())
            ),
            Self::Auto {
                rust_analyzer,
                typescript_indexer,
                threads,
                ..
            } => format!(
                "auto:rust={}:{}:typescript={}:{}:threads{}",
                rust_analyzer.display(),
                executable_digest(rust_analyzer),
                typescript_indexer.display(),
                executable_digest(typescript_indexer),
                threads.map_or_else(|| "auto".to_owned(), |value| value.to_string())
            ),
        }
    }
}

fn validate_scip_limits(threads: Option<usize>, timeout_seconds: u64) -> Result<()> {
    if threads == Some(0) {
        return Err(CceError::Configuration(
            "SCIP indexer thread count must be positive".to_owned(),
        ));
    }
    if !(10..=86_400).contains(&timeout_seconds) {
        return Err(CceError::Configuration(format!(
            "SCIP indexer timeout must be 10..=86400 seconds, got {timeout_seconds}"
        )));
    }
    Ok(())
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
#[allow(clippy::large_enum_variant)] // One engine-owned serving config; boxing fields obscures CLI wiring.
pub enum DenseBackendConfig {
    Disabled,
    DeterministicBaseline {
        dimensions: usize,
    },
    LocalFastEmbed {
        model: String,
        revision: String,
        model_directory: Option<PathBuf>,
        cache_dir: PathBuf,
        runtime_library: PathBuf,
        allow_download: bool,
        allow_high_memory: bool,
        max_length: usize,
        threads: Option<usize>,
        batch_size: usize,
        sessions: usize,
        query_prefix: String,
        document_prefix: String,
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
            Self::LocalFastEmbed {
                model,
                revision,
                model_directory,
                max_length,
                query_prefix,
                document_prefix,
                ..
            } => Some(format!(
                "local-fastembed:{model}@{revision}:bundle{}:max{max_length}:q{}:d{}",
                model_directory.as_ref().map_or_else(
                    || "registry".to_owned(),
                    |directory| file_digest(&directory.join("cce-model-manifest.json"))
                        .unwrap_or_else(|| "unavailable".to_owned())
                ),
                short_hash(query_prefix),
                short_hash(document_prefix)
            )),
        }
    }

    #[must_use]
    pub const fn is_production(&self) -> bool {
        matches!(self, Self::LocalFastEmbed { .. })
    }

    #[must_use]
    pub fn revision(&self) -> Option<String> {
        match self {
            Self::LocalFastEmbed { revision, .. } => Some(revision.clone()),
            Self::Disabled | Self::DeterministicBaseline { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankerBackendConfig {
    Disabled,
    LocalFastEmbed {
        model: String,
        revision: String,
        model_directory: Option<PathBuf>,
        cache_dir: PathBuf,
        runtime_library: PathBuf,
        allow_download: bool,
        max_length: usize,
        threads: Option<usize>,
        batch_size: usize,
        sessions: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub repository_root: PathBuf,
    pub data_root: PathBuf,
    pub index: IndexOptions,
    pub dense: DenseBackendConfig,
    pub reranker: RerankerBackendConfig,
    pub scip: ScipBackendConfig,
    pub dataflow: DataflowBackendConfig,
    pub capture_learning_data: bool,
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
            reranker: RerankerBackendConfig::Disabled,
            scip: ScipBackendConfig::Disabled,
            dataflow: DataflowBackendConfig::Disabled,
            capture_learning_data: std::env::var("CCE_CAPTURE_LEARNING_DATA").is_ok_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "all"
                )
            }),
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
        options.insert("scip".to_owned(), self.scip.profile_name());
        options.insert("dataflow".to_owned(), self.dataflow.profile_name());
        IndexProfile {
            schema_version: DATA_FORMAT_VERSION,
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            parser_profile: "tree-sitter-v1".to_owned(),
            chunk_policy: format!("ast-l0-l1-l2-max-{}", self.index.max_unit_bytes),
            embedding_model: self.dense.profile_name(),
            embedding_revision: self.dense.revision(),
            options,
        }
    }
}

fn short_hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex()[..12].to_owned()
}

fn file_digest(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

fn executable_digest(path: &Path) -> String {
    if path.components().count() == 1 {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .map(|directory| directory.join(path))
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| file_digest(&candidate))
            .unwrap_or_else(|| "unavailable".to_owned())
    } else {
        file_digest(path).unwrap_or_else(|| "unavailable".to_owned())
    }
}

fn resolve_executable(path: PathBuf) -> Result<PathBuf> {
    if path.components().count() == 1 {
        if let Some(resolved) = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .map(|directory| directory.join(&path))
            .find(|candidate| candidate.is_file())
        {
            return Ok(resolved);
        }
    } else if path.is_file() {
        return if path.is_absolute() {
            Ok(path)
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(path))
                .map_err(|error| CceError::Configuration(error.to_string()))
        };
    }
    Err(CceError::Configuration(format!(
        "SCIP indexer {} was not found; install rust-analyzer or pass --rust-analyzer with an explicit path",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_presets_couple_model_revision_and_training_prefixes() {
        let quality = local_embedding_preset("quality").expect("quality preset");
        assert_eq!(quality.model, "JinaEmbeddingsV2BaseCode");
        assert_eq!(quality.query_prefix, "");

        let balanced = local_embedding_preset("MultilingualE5Small").expect("balanced preset");
        assert_eq!(balanced.query_prefix, "query: ");
        assert_eq!(balanced.document_prefix, "passage: ");

        let fast = local_embedding_preset("fast").expect("fast preset");
        assert_eq!(fast.model, "BGESmallENV15Q");
        assert_eq!(fast.revision.len(), 40);
    }
}
