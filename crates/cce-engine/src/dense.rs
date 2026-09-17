use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cce_core::{CceError, Result};
use cce_store::DocumentContent;

use crate::DenseBackendConfig;

const MAGIC: &[u8; 8] = b"CCEVEC1\0";
const FORMAT_VERSION: u32 = 1;

/// Whether an embedding input is an indexed document or a user query. Some
/// model families (E5) require different prefixes for each role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedRole {
    /// Embedding a document for the index.
    Document,
    /// Embedding a user query.
    Query,
}

/// An embedding model behind a small async surface.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Profile string persisted with the index (query/index must match).
    fn profile(&self) -> &str;
    /// Whether vectors are benchmark-meaningful (real model vs baseline).
    fn production_ready(&self) -> bool;
    /// Embed `inputs`; returns one vector per input, normalized.
    async fn embed(&self, inputs: &[String], role: EmbedRole) -> Result<Vec<Vec<f32>>>;
}

/// The selected dense backend implementation.
#[derive(Debug, Clone)]
pub enum EmbeddingBackend {
    /// Hash-based offline baseline.
    Deterministic(DeterministicEmbedder),
    /// In-process ONNX model via fastembed.
    Local(LocalEmbedder),
}

impl EmbeddingBackend {
    /// Build the backend for a config; `None` when dense is disabled.
    /// `Local` may download model files into `model_cache_dir` (opt-in
    /// network) then runs fully offline.
    ///
    /// # Errors
    /// Configuration/embedding errors on invalid config or model init.
    pub fn from_config(
        config: &DenseBackendConfig,
        model_cache_dir: &Path,
    ) -> Result<Option<Self>> {
        match config {
            DenseBackendConfig::Disabled => Ok(None),
            DenseBackendConfig::DeterministicBaseline { dimensions } => Ok(Some(
                Self::Deterministic(DeterministicEmbedder::new(*dimensions)?),
            )),
            DenseBackendConfig::Local { model } => Ok(Some(Self::Local(LocalEmbedder::new(
                model,
                model_cache_dir,
            )?))),
        }
    }
}

#[async_trait]
impl Embedder for EmbeddingBackend {
    fn profile(&self) -> &str {
        match self {
            Self::Deterministic(value) => value.profile(),
            Self::Local(value) => value.profile(),
        }
    }

    fn production_ready(&self) -> bool {
        match self {
            Self::Deterministic(value) => value.production_ready(),
            Self::Local(value) => value.production_ready(),
        }
    }

    async fn embed(&self, inputs: &[String], role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        match self {
            Self::Deterministic(value) => value.embed(inputs, role).await,
            Self::Local(value) => value.embed(inputs, role).await,
        }
    }
}

/// Hash-of-token baseline embedder — deterministic, offline, used for
/// tests and pipeline plumbing checks; never production.
#[derive(Debug, Clone)]
pub struct DeterministicEmbedder {
    dimensions: usize,
    profile: String,
}

impl DeterministicEmbedder {
    /// Create a baseline embedder with `dimensions` (32..=16384).
    ///
    /// # Errors
    /// `Configuration` on out-of-range dimensions.
    pub fn new(dimensions: usize) -> Result<Self> {
        if !(32..=16_384).contains(&dimensions) {
            return Err(CceError::Configuration(format!(
                "deterministic embedding dimensions must be 32..=16384, got {dimensions}"
            )));
        }
        Ok(Self {
            dimensions,
            profile: format!("deterministic-baseline-{dimensions}"),
        })
    }
}

#[async_trait]
impl Embedder for DeterministicEmbedder {
    fn profile(&self) -> &str {
        &self.profile
    }

    fn production_ready(&self) -> bool {
        false
    }

    async fn embed(&self, inputs: &[String], _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        Ok(inputs
            .iter()
            .map(|input| {
                let mut vector = vec![0.0_f32; self.dimensions];
                for token in tokens(input) {
                    let hash = blake3::hash(token.as_bytes());
                    let bytes = hash.as_bytes();
                    let slot =
                        u64::from_le_bytes(bytes.first_chunk::<8>().copied().unwrap_or_default())
                            as usize
                            % self.dimensions;
                    let sign = if bytes.get(8).copied().unwrap_or(0) & 1 == 0 {
                        1.0
                    } else {
                        -1.0
                    };
                    if let Some(value) = vector.get_mut(slot) {
                        *value += sign;
                    }
                }
                normalize(&mut vector);
                vector
            })
            .collect())
    }
}

/// Local in-process embedding model served by fastembed/ONNX Runtime.
///
/// Model files are downloaded once into `cache_dir` on first use — selecting
/// `--dense local` is the explicit network opt-in — after which inference is
/// fully offline.
#[derive(Clone)]
pub struct LocalEmbedder {
    inner: Arc<Mutex<fastembed::TextEmbedding>>,
    profile: String,
    prefix: Option<(&'static str, &'static str)>,
}

impl std::fmt::Debug for LocalEmbedder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalEmbedder")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

impl LocalEmbedder {
    /// Load a fastembed model by code, downloading to `cache_dir` on first
    /// use (explicit network opt-in at `--dense local` selection time).
    ///
    /// # Errors
    /// `Configuration` for unknown models; `Embedding` on init failure.
    pub fn new(model_code: &str, cache_dir: &Path) -> Result<Self> {
        let model = fastembed::TextEmbedding::list_supported_models()
            .into_iter()
            .find(|info| info.model_code == model_code)
            .map(|info| info.model)
            .ok_or_else(|| {
                CceError::Configuration(format!(
                    "unknown local embedding model {model_code:?}; run `cce models`"
                ))
            })?;
        let dimensions = fastembed::TextEmbedding::get_model_info(&model)
            .map(|info| info.dim)
            .map_err(|error| CceError::Configuration(error.to_string()))?;
        let options = fastembed::TextInitOptions::new(model)
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(true);
        let session = fastembed::TextEmbedding::try_new(options)
            .map_err(|error| CceError::Embedding(format!("local model init failed: {error}")))?;
        // E5-family models require "query: "/"passage: " prefixes; fastembed
        // does not add them itself.
        let prefix = model_code
            .starts_with("intfloat/")
            .then_some(("query: ", "passage: "));
        Ok(Self {
            inner: Arc::new(Mutex::new(session)),
            profile: format!("local:{model_code}:{dimensions}"),
            prefix,
        })
    }

    /// List the Hugging Face model codes that can be passed to `--embedding-model`.
    #[must_use]
    pub fn supported_model_codes() -> Vec<String> {
        fastembed::TextEmbedding::list_supported_models()
            .into_iter()
            .map(|info| info.model_code)
            .collect()
    }
}

#[async_trait]
impl Embedder for LocalEmbedder {
    fn profile(&self) -> &str {
        &self.profile
    }

    fn production_ready(&self) -> bool {
        true
    }

    async fn embed(&self, inputs: &[String], role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let prefixed = match (self.prefix, role) {
            (Some((query, _)), EmbedRole::Query) => inputs
                .iter()
                .map(|input| format!("{query}{input}"))
                .collect(),
            (Some((_, passage)), EmbedRole::Document) => inputs
                .iter()
                .map(|input| format!("{passage}{input}"))
                .collect(),
            _ => inputs.to_vec(),
        };
        let inner = Arc::clone(&self.inner);
        let mut vectors = tokio::task::spawn_blocking(move || {
            inner
                .lock()
                .map_err(|_| CceError::Embedding("local embedder poisoned".to_owned()))?
                .embed(prefixed, None)
                .map_err(|error| CceError::Embedding(format!("local embedding failed: {error}")))
        })
        .await
        .map_err(|error| CceError::Embedding(format!("local embedding task failed: {error}")))??;
        if vectors.len() != inputs.len() {
            return Err(CceError::Embedding(format!(
                "local model returned {} vectors for {} inputs",
                vectors.len(),
                inputs.len()
            )));
        }
        validate_and_normalize(&mut vectors)?;
        Ok(vectors)
    }
}

/// One dense-index match.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseSearchHit {
    /// Matched document id.
    pub document_id: String,
    /// Cosine similarity score.
    pub score: f32,
}

/// An in-memory flat dense index (brute-force dot product over normalized
/// vectors); serialized to the artifact store as a versioned blob.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseIndex {
    profile: String,
    dimensions: usize,
    ids: Vec<String>,
    vectors: Vec<f32>,
}

impl DenseIndex {
    /// Embed all documents and build the index.
    ///
    /// # Errors
    /// `Embedding` on model failure or inconsistent dimensions;
    /// `Configuration` when `batch_size` is 0.
    pub async fn build(
        documents: &[DocumentContent],
        embedder: &impl Embedder,
        batch_size: usize,
    ) -> Result<Self> {
        if batch_size == 0 {
            return Err(CceError::Configuration(
                "embedding batch size must be positive".to_owned(),
            ));
        }
        let mut ids = Vec::with_capacity(documents.len());
        let mut vectors = Vec::new();
        let mut dimensions = None;
        for batch in documents.chunks(batch_size) {
            let inputs = batch
                .iter()
                .map(|document| document.text.clone())
                .collect::<Vec<_>>();
            let mut embedded = embedder.embed(&inputs, EmbedRole::Document).await?;
            validate_and_normalize(&mut embedded)?;
            for (document, vector) in batch.iter().zip(embedded) {
                let expected = *dimensions.get_or_insert(vector.len());
                if vector.len() != expected {
                    return Err(CceError::Embedding(
                        "embedding dimensions changed within an index".to_owned(),
                    ));
                }
                ids.push(document.document_id.clone());
                vectors.extend(vector);
            }
        }
        Ok(Self {
            profile: embedder.profile().to_owned(),
            dimensions: dimensions.unwrap_or_default(),
            ids,
            vectors,
        })
    }

    /// Brute-force top-`limit` search; `embedder` profile must match the
    /// index's build profile.
    ///
    /// # Errors
    /// `Configuration` on profile mismatch; `Embedding` on query failure.
    pub async fn search(
        &self,
        query: &str,
        embedder: &impl Embedder,
        limit: usize,
    ) -> Result<Vec<DenseSearchHit>> {
        if embedder.profile() != self.profile {
            return Err(CceError::Configuration(format!(
                "dense index profile {} does not match query profile {}",
                self.profile,
                embedder.profile()
            )));
        }
        let vectors = embedder
            .embed(&[query.to_owned()], EmbedRole::Query)
            .await?;
        let query = vectors
            .into_iter()
            .next()
            .ok_or_else(|| CceError::Embedding("embedder returned no query vector".to_owned()))?;
        if query.len() != self.dimensions {
            return Err(CceError::Embedding(format!(
                "query dimension {} does not match index dimension {}",
                query.len(),
                self.dimensions
            )));
        }
        let mut hits = self
            .ids
            .iter()
            .zip(self.vectors.chunks_exact(self.dimensions))
            .map(|(id, vector)| DenseSearchHit {
                document_id: id.clone(),
                score: dot(&query, vector),
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Serialize to the `CCEVEC1` binary format for artifact storage.
    ///
    /// # Errors
    /// `Configuration` when sizes exceed format limits.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let dimension = u32::try_from(self.dimensions)
            .map_err(|_| CceError::Configuration("vector dimension exceeds format".to_owned()))?;
        let count = u64::try_from(self.ids.len())
            .map_err(|_| CceError::Configuration("vector count exceeds format".to_owned()))?;
        let mut bytes = Vec::with_capacity(64 + self.vectors.len() * 4);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&dimension.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        let profile = self.profile.as_bytes();
        bytes.extend_from_slice(
            &u32::try_from(profile.len())
                .map_err(|_| CceError::Configuration("profile name too long".to_owned()))?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(profile);
        for (id, vector) in self
            .ids
            .iter()
            .zip(self.vectors.chunks_exact(self.dimensions))
        {
            let id = id.as_bytes();
            bytes.extend_from_slice(
                &u32::try_from(id.len())
                    .map_err(|_| CceError::Configuration("document id too long".to_owned()))?
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(id);
            for value in vector {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        Ok(bytes)
    }

    /// Parse a `CCEVEC1` blob back into an index, with integrity checks.
    ///
    /// # Errors
    /// `ArtifactCorrupt`/`UnsupportedFormat` on malformed input.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = SliceReader::new(bytes);
        if reader.take(8)? != MAGIC {
            return Err(CceError::ArtifactCorrupt("invalid vector magic".to_owned()));
        }
        let version = reader.u32()?;
        if version != FORMAT_VERSION {
            return Err(CceError::UnsupportedFormat {
                found: version,
                supported: FORMAT_VERSION,
            });
        }
        let dimensions = reader.u32()? as usize;
        let count = usize::try_from(reader.u64()?)
            .map_err(|_| CceError::ArtifactCorrupt("vector count overflow".to_owned()))?;
        if dimensions == 0 && count != 0 {
            return Err(CceError::ArtifactCorrupt(
                "zero-dimensional non-empty index".to_owned(),
            ));
        }
        let profile_len = reader.u32()? as usize;
        let profile = std::str::from_utf8(reader.take(profile_len)?)
            .map_err(|_| CceError::ArtifactCorrupt("invalid vector profile".to_owned()))?
            .to_owned();
        let mut ids = Vec::with_capacity(count);
        let vector_capacity = count
            .checked_mul(dimensions)
            .ok_or_else(|| CceError::ArtifactCorrupt("vector allocation overflow".to_owned()))?;
        let mut vectors = Vec::with_capacity(vector_capacity);
        for _ in 0..count {
            let id_len = reader.u32()? as usize;
            let id = std::str::from_utf8(reader.take(id_len)?)
                .map_err(|_| CceError::ArtifactCorrupt("invalid document id".to_owned()))?
                .to_owned();
            ids.push(id);
            for _ in 0..dimensions {
                vectors.push(f32::from_le_bytes(reader.take(4)?.try_into().map_err(
                    |_| CceError::ArtifactCorrupt("truncated float".to_owned()),
                )?));
            }
        }
        if !reader.is_empty() {
            return Err(CceError::ArtifactCorrupt(
                "trailing bytes in vector index".to_owned(),
            ));
        }
        Ok(Self {
            profile,
            dimensions,
            ids,
            vectors,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct SliceReader<'a> {
    remaining: &'a [u8],
}

impl<'a> SliceReader<'a> {
    const fn new(remaining: &'a [u8]) -> Self {
        Self { remaining }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let (value, remaining) = self
            .remaining
            .split_at_checked(length)
            .ok_or_else(|| CceError::ArtifactCorrupt("truncated vector index".to_owned()))?;
        self.remaining = remaining;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(
            |_| CceError::ArtifactCorrupt("truncated u32".to_owned()),
        )?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(
            |_| CceError::ArtifactCorrupt("truncated u64".to_owned()),
        )?))
    }

    const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

fn validate_and_normalize(vectors: &mut [Vec<f32>]) -> Result<()> {
    let dimensions = vectors.first().map(Vec::len).unwrap_or_default();
    for vector in vectors {
        if vector.len() != dimensions
            || vector.is_empty()
            || vector.iter().any(|value| !value.is_finite())
        {
            return Err(CceError::Embedding("invalid embedding vector".to_owned()));
        }
        normalize(vector);
    }
    Ok(())
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector {
            *value /= norm;
        }
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binary_index_round_trips_and_searches() {
        let embedder = DeterministicEmbedder::new(64).expect("embedder");
        let documents = vec![
            DocumentContent {
                document_id: "a".to_owned(),
                entity_id: "ea".to_owned(),
                region_id: None,
                representation: cce_core::RetrievalRepresentation::RawCode,
                address: None,
                evidence: Vec::new(),
                text: "resume cursor persistence".to_owned(),
            },
            DocumentContent {
                document_id: "b".to_owned(),
                entity_id: "eb".to_owned(),
                region_id: None,
                representation: cce_core::RetrievalRepresentation::RawCode,
                address: None,
                evidence: Vec::new(),
                text: "unrelated CSS styles".to_owned(),
            },
        ];
        let index = DenseIndex::build(&documents, &embedder, 8)
            .await
            .expect("index");
        let decoded = DenseIndex::decode(&index.encode().expect("encode")).expect("decode");
        let hits = decoded
            .search("cursor resume", &embedder, 1)
            .await
            .expect("search");
        assert_eq!(hits[0].document_id, "a");
    }
}
