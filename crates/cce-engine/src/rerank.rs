use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use cce_core::{CceError, Result};
use fastembed::{
    RerankInitOptionsUserDefined, RerankerModel, TextRerank, TokenizerFiles,
    UserDefinedRerankingModel,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::dense::{
    initialize_onnx_runtime, is_pinned_revision, required_model_files, resolve_model_files,
    safe_model_path,
};

#[derive(Clone)]
pub struct LocalReranker {
    pool: Arc<RerankerPool>,
    profile: String,
    batch_size: usize,
}

struct RerankerPool {
    models: Vec<Mutex<TextRerank>>,
    next: AtomicUsize,
}

impl fmt::Debug for LocalReranker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalReranker")
            .field("profile", &self.profile)
            .field("batch_size", &self.batch_size)
            .field("sessions", &self.pool.models.len())
            .finish_non_exhaustive()
    }
}

impl LocalReranker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_name: &str,
        revision: &str,
        model_directory: Option<&Path>,
        cache_dir: &Path,
        runtime_library: &Path,
        allow_download: bool,
        max_length: usize,
        threads: Option<usize>,
        batch_size: usize,
        sessions: usize,
    ) -> Result<Self> {
        if !is_pinned_revision(revision) {
            return Err(CceError::Configuration(
                "local reranker revision must be an immutable 40-64 character hexadecimal commit"
                    .to_owned(),
            ));
        }
        if !(64..=8_192).contains(&max_length)
            || batch_size == 0
            || threads == Some(0)
            || !(1..=16).contains(&sessions)
        {
            return Err(CceError::Configuration(
                "reranker max length must be 64..=8192, thread/batch counts must be positive, and sessions must be 1..=16"
                    .to_owned(),
            ));
        }
        initialize_onnx_runtime(runtime_library)?;
        let model_kind = model_name.parse::<RerankerModel>().map_err(|error| {
            CceError::Configuration(format!("unsupported local reranker: {error}"))
        })?;
        let model_info = TextRerank::get_model_info(&model_kind);
        let required_files =
            required_model_files(&model_info.model_file, &model_info.additional_files);
        let paths = if let Some(directory) = model_directory {
            if allow_download {
                return Err(CceError::Configuration(
                    "--reranker-allow-download cannot be combined with a local reranker directory"
                        .to_owned(),
                ));
            }
            resolve_local_reranker_bundle(
                directory,
                &model_info.model_code,
                revision,
                &required_files,
            )?
        } else {
            resolve_model_files(
                cache_dir,
                &model_info.model_code,
                revision,
                &required_files,
                allow_download,
                "--reranker-allow-download",
            )?
        };
        let mut digest = blake3::Hasher::new();
        let read = |name: &str, digest: &mut blake3::Hasher| -> Result<Vec<u8>> {
            let path = paths.get(name).ok_or_else(|| {
                CceError::ArtifactCorrupt(format!("resolved reranker file disappeared: {name}"))
            })?;
            let bytes = std::fs::read(path).map_err(|error| CceError::io(path, error))?;
            digest.update(name.as_bytes());
            digest.update(&bytes);
            Ok(bytes)
        };
        let tokenizer = TokenizerFiles {
            tokenizer_file: read("tokenizer.json", &mut digest)?,
            config_file: read("config.json", &mut digest)?,
            special_tokens_map_file: read("special_tokens_map.json", &mut digest)?,
            tokenizer_config_file: read("tokenizer_config.json", &mut digest)?,
        };
        for file in &model_info.additional_files {
            let _ = read(file, &mut digest)?;
        }
        let onnx_path = paths.get(&model_info.model_file).cloned().ok_or_else(|| {
            CceError::ArtifactCorrupt("resolved reranker ONNX file disappeared".to_owned())
        })?;
        let _ = read(&model_info.model_file, &mut digest)?;
        let mut options = RerankInitOptionsUserDefined::new().with_max_length(max_length);
        if let Some(threads) = threads {
            options = options.with_intra_threads(threads);
        }
        let models = (0..sessions)
            .map(|_| {
                TextRerank::try_new_from_user_defined(
                    UserDefinedRerankingModel::new(onnx_path.clone(), tokenizer.clone()),
                    options.clone(),
                )
                .map(Mutex::new)
                .map_err(|error| {
                    CceError::Provider(format!("failed to load local reranker: {error}"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            pool: Arc::new(RerankerPool {
                models,
                next: AtomicUsize::new(0),
            }),
            profile: format!(
                "local-reranker:{}@{}:{}:max{}:sessions{}",
                model_info.model_code,
                revision,
                &digest.finalize().to_hex()[..16],
                max_length,
                sessions,
            ),
            batch_size,
        })
    }

    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<(usize, f32)>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let query = query.to_owned();
        let documents = documents.to_vec();
        let pool = Arc::clone(&self.pool);
        let batch_size = self.batch_size;
        tokio::task::spawn_blocking(move || {
            let index = pool.next.fetch_add(1, Ordering::Relaxed) % pool.models.len();
            let mut model = pool.models[index]
                .lock()
                .map_err(|_| CceError::Provider("local reranker model lock poisoned".to_owned()))?;
            model
                .rerank(query, documents, false, Some(batch_size))
                .map(|results| {
                    results
                        .into_iter()
                        .map(|result| (result.index, result.score))
                        .collect()
                })
                .map_err(|error| CceError::Provider(format!("local reranking failed: {error}")))
        })
        .await
        .map_err(|error| CceError::Provider(format!("local reranking task failed: {error}")))?
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RerankerBundleManifest {
    bundle_revision: String,
    runtime_model: String,
    files: HashMap<String, String>,
}

fn resolve_local_reranker_bundle(
    directory: &Path,
    model_name: &str,
    revision: &str,
    required_files: &[String],
) -> Result<HashMap<String, PathBuf>> {
    let root = directory.canonicalize().map_err(|error| {
        CceError::Configuration(format!(
            "local reranker directory {} is unavailable: {error}",
            directory.display()
        ))
    })?;
    let manifest_path = root.join("cce-reranker-manifest.json");
    let manifest: RerankerBundleManifest = serde_json::from_slice(
        &std::fs::read(&manifest_path).map_err(|error| CceError::io(&manifest_path, error))?,
    )
    .map_err(|error| {
        CceError::Configuration(format!(
            "invalid local reranker manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    if manifest.bundle_revision != revision
        || !manifest.runtime_model.eq_ignore_ascii_case(model_name)
    {
        return Err(CceError::Configuration(
            "local reranker bundle revision or runtime model does not match the request".to_owned(),
        ));
    }
    let mut resolved = HashMap::new();
    for file in required_files {
        let relative = safe_model_path(file)?;
        let path = root.join(&relative).canonicalize().map_err(|error| {
            CceError::Configuration(format!(
                "local reranker file {} is unavailable: {error}",
                root.join(&relative).display()
            ))
        })?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(CceError::Configuration(format!(
                "local reranker file escaped its bundle: {}",
                path.display()
            )));
        }
        let expected = manifest.files.get(file).ok_or_else(|| {
            CceError::Configuration(format!("reranker manifest has no SHA-256 for {file}"))
        })?;
        let bytes = std::fs::read(&path).map_err(|error| CceError::io(&path, error))?;
        if format!("{:x}", Sha256::digest(&bytes)) != *expected {
            return Err(CceError::Configuration(format!(
                "local reranker file {file} failed SHA-256 verification"
            )));
        }
        resolved.insert(file.clone(), path);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_bundle_rejects_tampered_reranker_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("config.json"), b"original").expect("config");
        let revision = "a".repeat(40);
        std::fs::write(
            directory.path().join("cce-reranker-manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "bundleRevision": revision,
                "runtimeModel": "jinaai/jina-reranker-v1-turbo-en",
                "files": {"config.json": format!("{:x}", Sha256::digest(b"different"))}
            }))
            .expect("manifest"),
        )
        .expect("manifest file");
        let error = resolve_local_reranker_bundle(
            directory.path(),
            "jinaai/jina-reranker-v1-turbo-en",
            &revision,
            &["config.json".to_owned()],
        )
        .expect_err("tampered reranker must fail");
        assert!(error.to_string().contains("SHA-256"));
    }
}
