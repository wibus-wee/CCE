//! Local cross-encoder reranking via fastembed/ONNX.
//!
//! The fused candidate order is a coarse relevance prior built from
//! route-level rank fusion; a cross-encoder re-scores (query, document)
//! pairs jointly and reorders the head of the list. Model files download
//! once into the data root's `models/` directory on first use — selecting
//! a reranker is the same explicit network opt-in as `--dense local`.

use std::{path::Path, sync::Arc, sync::Mutex};

use cce_core::{CceError, Result};

/// Local in-process cross-encoder reranker.
#[derive(Clone)]
pub struct LocalReranker {
    inner: Arc<Mutex<fastembed::TextRerank>>,
    model_code: String,
}

impl std::fmt::Debug for LocalReranker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalReranker")
            .field("model_code", &self.model_code)
            .finish_non_exhaustive()
    }
}

impl LocalReranker {
    pub fn new(model_code: &str, cache_dir: &Path) -> Result<Self> {
        let model = fastembed::TextRerank::list_supported_models()
            .into_iter()
            .find(|info| info.model_code == model_code)
            .map(|info| info.model)
            .ok_or_else(|| {
                CceError::Configuration(format!(
                    "unknown local reranker model {model_code:?}; supported: {}",
                    Self::supported_model_codes().join(", ")
                ))
            })?;
        let options = fastembed::RerankInitOptions::new(model)
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(true);
        let session = fastembed::TextRerank::try_new(options)
            .map_err(|error| CceError::Embedding(format!("local reranker init failed: {error}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(session)),
            model_code: model_code.to_owned(),
        })
    }

    /// List the model codes accepted by `new`.
    #[must_use]
    pub fn supported_model_codes() -> Vec<String> {
        fastembed::TextRerank::list_supported_models()
            .into_iter()
            .map(|info| info.model_code)
            .collect()
    }

    #[must_use]
    pub fn model_code(&self) -> &str {
        &self.model_code
    }

    /// Score (query, document) pairs. Returns `(document_index, score)`
    /// sorted by score descending. Runs on the blocking pool: the ONNX
    /// session is synchronous and must not stall the executor.
    pub async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<(usize, f32)>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let inner = Arc::clone(&self.inner);
        let query = query.to_owned();
        let documents: Vec<String> = documents.to_vec();
        let results = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = documents.iter().map(String::as_str).collect();
            inner
                .lock()
                .map_err(|_| "reranker session poisoned".to_owned())?
                .rerank(query.as_str(), refs.as_slice(), false, None)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| CceError::Embedding(format!("reranker task failed: {error}")))?
        .map_err(CceError::Embedding)?;
        Ok(results
            .into_iter()
            .map(|result| (result.index, result.score))
            .collect())
    }
}
