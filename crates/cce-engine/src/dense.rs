use std::{
    collections::HashMap,
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use cce_core::{CceError, Result};
use cce_store::DocumentContent;
use fastembed::{
    EmbeddingModel, InitOptionsUserDefined, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use fs4::FileExt;
use rayon::prelude::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use usearch::{Index as AnnIndex, IndexOptions, MetricKind, ScalarKind};

use crate::DenseBackendConfig;

const EXACT_MAGIC: &[u8; 8] = b"CCEVEC1\0";
const ANN_MAGIC: &[u8; 8] = b"CCEVEC2\0";
const FORMAT_VERSION: u32 = 1;
const ANN_FORMAT_VERSION: u32 = 2;
const ANN_MIN_DOCUMENTS: usize = 50_000;
const ANN_CONNECTIVITY: usize = 32;
const ANN_EXPANSION_ADD: usize = 256;
const ANN_EXPANSION_SEARCH: usize = 512;
const ANN_RERANK_MULTIPLIER: usize = 48;
const MAX_SAFE_ATTENTION_CELLS: usize = 2 * 1_024 * 1_024;

#[async_trait]
pub trait Embedder: Send + Sync {
    fn profile(&self) -> &str;
    fn production_ready(&self) -> bool;
    async fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
    async fn embed_query(&self, input: &str) -> Result<Vec<f32>>;
}

#[derive(Debug, Clone)]
pub enum EmbeddingBackend {
    Deterministic(DeterministicEmbedder),
    LocalFastEmbed(LocalFastEmbedder),
}

impl EmbeddingBackend {
    pub async fn from_config(config: &DenseBackendConfig) -> Result<Option<Self>> {
        match config {
            DenseBackendConfig::Disabled => Ok(None),
            DenseBackendConfig::DeterministicBaseline { dimensions } => Ok(Some(
                Self::Deterministic(DeterministicEmbedder::new(*dimensions)?),
            )),
            DenseBackendConfig::LocalFastEmbed {
                model,
                revision,
                model_directory,
                cache_dir,
                runtime_library,
                allow_download,
                allow_high_memory,
                max_length,
                threads,
                batch_size,
                query_prefix,
                document_prefix,
            } => {
                let model = model.clone();
                let revision = revision.clone();
                let model_directory = model_directory.clone();
                let cache_dir = cache_dir.clone();
                let runtime_library = runtime_library.clone();
                let allow_download = *allow_download;
                let allow_high_memory = *allow_high_memory;
                let max_length = *max_length;
                let threads = *threads;
                let batch_size = *batch_size;
                let query_prefix = query_prefix.clone();
                let document_prefix = document_prefix.clone();
                let embedder = tokio::task::spawn_blocking(move || {
                    LocalFastEmbedder::new(
                        &model,
                        &revision,
                        model_directory.as_deref(),
                        &cache_dir,
                        &runtime_library,
                        allow_download,
                        allow_high_memory,
                        max_length,
                        threads,
                        batch_size,
                        &query_prefix,
                        &document_prefix,
                    )
                })
                .await
                .map_err(|error| {
                    CceError::Provider(format!("local model initialization task failed: {error}"))
                })??;
                Ok(Some(Self::LocalFastEmbed(embedder)))
            }
        }
    }
}

#[async_trait]
impl Embedder for EmbeddingBackend {
    fn profile(&self) -> &str {
        match self {
            Self::Deterministic(value) => value.profile(),
            Self::LocalFastEmbed(value) => value.profile(),
        }
    }

    fn production_ready(&self) -> bool {
        match self {
            Self::Deterministic(value) => value.production_ready(),
            Self::LocalFastEmbed(value) => value.production_ready(),
        }
    }

    async fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        match self {
            Self::Deterministic(value) => value.embed_documents(inputs).await,
            Self::LocalFastEmbed(value) => value.embed_documents(inputs).await,
        }
    }

    async fn embed_query(&self, input: &str) -> Result<Vec<f32>> {
        match self {
            Self::Deterministic(value) => value.embed_query(input).await,
            Self::LocalFastEmbed(value) => value.embed_query(input).await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeterministicEmbedder {
    dimensions: usize,
    profile: String,
}

impl DeterministicEmbedder {
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

    async fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(inputs
            .iter()
            .map(|input| {
                let mut vector = vec![0.0_f32; self.dimensions];
                for token in tokens(input) {
                    let hash = blake3::hash(token.as_bytes());
                    let bytes = hash.as_bytes();
                    let slot = u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8])) as usize
                        % self.dimensions;
                    let sign = if bytes[8] & 1 == 0 { 1.0 } else { -1.0 };
                    vector[slot] += sign;
                }
                normalize(&mut vector);
                vector
            })
            .collect())
    }

    async fn embed_query(&self, input: &str) -> Result<Vec<f32>> {
        self.embed_documents(&[input.to_owned()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| CceError::Provider("embedder returned no query vector".to_owned()))
    }
}

#[derive(Clone)]
pub struct LocalFastEmbedder {
    model: Arc<Mutex<TextEmbedding>>,
    profile: String,
    batch_size: usize,
    query_prefix: String,
    document_prefix: String,
}

impl fmt::Debug for LocalFastEmbedder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalFastEmbedder")
            .field("profile", &self.profile)
            .field("batch_size", &self.batch_size)
            .finish_non_exhaustive()
    }
}

impl LocalFastEmbedder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_name: &str,
        revision: &str,
        model_directory: Option<&Path>,
        cache_dir: &Path,
        runtime_library: &Path,
        allow_download: bool,
        allow_high_memory: bool,
        max_length: usize,
        threads: Option<usize>,
        batch_size: usize,
        query_prefix: &str,
        document_prefix: &str,
    ) -> Result<Self> {
        if !is_pinned_revision(revision) {
            return Err(CceError::Configuration(
                "local embedding revision must be an immutable 40-64 character hexadecimal commit"
                    .to_owned(),
            ));
        }
        validate_local_resource_budget(max_length, batch_size, allow_high_memory)?;
        if threads == Some(0) {
            return Err(CceError::Configuration(
                "local embedding thread count must be positive".to_owned(),
            ));
        }
        initialize_onnx_runtime(runtime_library)?;
        let model_name = normalize_model_name(model_name);
        let model_kind = model_name.parse::<EmbeddingModel>().map_err(|error| {
            CceError::Configuration(format!("unsupported local embedding model: {error}"))
        })?;
        let model_info = TextEmbedding::get_model_info(&model_kind).map_err(|error| {
            CceError::Configuration(format!("unsupported local embedding model: {error}"))
        })?;
        let required_files =
            required_model_files(&model_info.model_file, &model_info.additional_files);
        let paths = if let Some(directory) = model_directory {
            if allow_download {
                return Err(CceError::Configuration(
                    "--embedding-allow-download cannot be combined with a local model directory"
                        .to_owned(),
                ));
            }
            resolve_local_model_bundle(directory, model_name, revision, &required_files)?
        } else {
            resolve_model_files(
                cache_dir,
                &model_info.model_code,
                revision,
                &required_files,
                allow_download,
                "--embedding-allow-download",
            )?
        };
        let mut digest = blake3::Hasher::new();
        let read = |name: &str, digest: &mut blake3::Hasher| -> Result<Vec<u8>> {
            let path = paths.get(name).ok_or_else(|| {
                CceError::ArtifactCorrupt(format!("resolved model file disappeared: {name}"))
            })?;
            let bytes = std::fs::read(path).map_err(|error| CceError::io(path, error))?;
            digest.update(name.as_bytes());
            digest.update(&bytes);
            Ok(bytes)
        };
        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read("tokenizer.json", &mut digest)?,
            config_file: read("config.json", &mut digest)?,
            special_tokens_map_file: read("special_tokens_map.json", &mut digest)?,
            tokenizer_config_file: read("tokenizer_config.json", &mut digest)?,
        };
        let mut user_model = UserDefinedEmbeddingModel::new(
            read(&model_info.model_file, &mut digest)?,
            tokenizer_files,
        );
        if let Some(pooling) = TextEmbedding::get_default_pooling_method(&model_kind) {
            user_model = user_model.with_pooling(pooling);
        }
        user_model =
            user_model.with_quantization(TextEmbedding::get_quantization_mode(&model_kind));
        user_model.output_key.clone_from(&model_info.output_key);
        for file in &model_info.additional_files {
            let file_name = Path::new(file)
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    CceError::Configuration(format!("invalid model file name: {file}"))
                })?;
            user_model = user_model
                .with_external_initializer(file_name.to_owned(), read(file, &mut digest)?);
        }
        let mut options = InitOptionsUserDefined::new().with_max_length(max_length);
        if let Some(threads) = threads {
            options = options.with_intra_threads(threads);
        }
        let model = TextEmbedding::try_new_from_user_defined(user_model, options)
            .map_err(|error| CceError::Provider(format!("failed to load local model: {error}")))?;
        let artifact_digest = digest.finalize().to_hex();
        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            profile: format!(
                "local-fastembed:{}@{}:{}:max{}:q{}:d{}",
                model_info.model_code,
                revision,
                &artifact_digest[..16],
                max_length,
                short_hash(query_prefix),
                short_hash(document_prefix)
            ),
            batch_size,
            query_prefix: query_prefix.to_owned(),
            document_prefix: document_prefix.to_owned(),
        })
    }

    async fn embed_with_prefix(&self, inputs: &[String], prefix: &str) -> Result<Vec<Vec<f32>>> {
        let inputs = inputs
            .iter()
            .map(|input| format!("{prefix}{input}"))
            .collect::<Vec<_>>();
        let model = Arc::clone(&self.model);
        let batch_size = self.batch_size;
        tokio::task::spawn_blocking(move || {
            let mut model = model.lock().map_err(|_| {
                CceError::Provider("local embedding model lock poisoned".to_owned())
            })?;
            model
                .embed(inputs, Some(batch_size))
                .map_err(|error| CceError::Provider(format!("local embedding failed: {error}")))
        })
        .await
        .map_err(|error| CceError::Provider(format!("local embedding task failed: {error}")))?
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalModelBundleManifest {
    bundle_revision: String,
    runtime_model: String,
    files: HashMap<String, String>,
}

fn resolve_local_model_bundle(
    directory: &Path,
    model_name: &str,
    revision: &str,
    required_files: &[String],
) -> Result<HashMap<String, PathBuf>> {
    let root = directory.canonicalize().map_err(|error| {
        CceError::Configuration(format!(
            "local embedding model directory {} is unavailable: {error}",
            directory.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(CceError::Configuration(format!(
            "local embedding model path {} is not a directory",
            root.display()
        )));
    }
    let manifest_path = root.join("cce-model-manifest.json");
    let manifest: LocalModelBundleManifest = serde_json::from_slice(
        &std::fs::read(&manifest_path).map_err(|error| CceError::io(&manifest_path, error))?,
    )
    .map_err(|error| {
        CceError::Configuration(format!(
            "invalid local embedding manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    if manifest.bundle_revision != revision {
        return Err(CceError::Configuration(format!(
            "local model bundle revision {} does not match requested {revision}",
            manifest.bundle_revision
        )));
    }
    if normalize_model_name(&manifest.runtime_model) != model_name {
        return Err(CceError::Configuration(format!(
            "local model runtime {} does not match requested {model_name}",
            manifest.runtime_model
        )));
    }
    let mut resolved = HashMap::new();
    for file in required_files {
        let relative = safe_model_path(file)?;
        let path = root.join(&relative).canonicalize().map_err(|error| {
            CceError::Configuration(format!(
                "local model file {} is unavailable: {error}",
                root.join(&relative).display()
            ))
        })?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(CceError::Configuration(format!(
                "local model file escaped its bundle: {}",
                path.display()
            )));
        }
        let expected = manifest.files.get(file).ok_or_else(|| {
            CceError::Configuration(format!("local model manifest has no SHA-256 for {file}"))
        })?;
        let bytes = std::fs::read(&path).map_err(|error| CceError::io(&path, error))?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if &actual != expected {
            return Err(CceError::Configuration(format!(
                "local model file {file} failed SHA-256 verification"
            )));
        }
        resolved.insert(file.clone(), path);
    }
    Ok(resolved)
}

#[async_trait]
impl Embedder for LocalFastEmbedder {
    fn profile(&self) -> &str {
        &self.profile
    }

    fn production_ready(&self) -> bool {
        true
    }

    async fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_with_prefix(inputs, &self.document_prefix).await
    }

    async fn embed_query(&self, input: &str) -> Result<Vec<f32>> {
        self.embed_with_prefix(&[input.to_owned()], &self.query_prefix)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| CceError::Provider("local embedder returned no query vector".to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DenseSearchHit {
    pub document_id: String,
    pub score: f32,
}

pub struct DenseIndex {
    profile: String,
    dimensions: usize,
    ids: Vec<String>,
    vectors: Vec<f32>,
    ann: Option<AnnIndex>,
}

impl fmt::Debug for DenseIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DenseIndex")
            .field("profile", &self.profile)
            .field("dimensions", &self.dimensions)
            .field("documents", &self.ids.len())
            .field("capability", &self.capability_name())
            .finish()
    }
}

impl DenseIndex {
    pub async fn build(
        documents: &[DocumentContent],
        embedder: &impl Embedder,
        batch_size: usize,
    ) -> Result<Self> {
        Self::build_incremental(documents, embedder, batch_size, None)
            .await
            .map(|(index, _)| index)
    }

    pub async fn build_incremental(
        documents: &[DocumentContent],
        embedder: &impl Embedder,
        batch_size: usize,
        previous: Option<&Self>,
    ) -> Result<(Self, usize)> {
        if batch_size == 0 {
            return Err(CceError::Configuration(
                "embedding batch size must be positive".to_owned(),
            ));
        }
        let previous = previous.filter(|index| index.profile == embedder.profile());
        let previous_positions = previous.map(|index| {
            index
                .ids
                .iter()
                .enumerate()
                .map(|(position, id)| (id.as_str(), position))
                .collect::<HashMap<_, _>>()
        });
        let missing = documents
            .iter()
            .filter(|document| {
                !previous_positions
                    .as_ref()
                    .is_some_and(|positions| positions.contains_key(document.document_id.as_str()))
            })
            .collect::<Vec<_>>();
        let mut new_vectors = HashMap::<&str, Vec<f32>>::with_capacity(missing.len());
        let mut dimensions = previous.map(|index| index.dimensions);
        for batch in missing.chunks(batch_size) {
            let inputs = batch
                .iter()
                .map(|document| dense_document_text(document))
                .collect::<Vec<_>>();
            let mut embedded = embedder.embed_documents(&inputs).await?;
            validate_and_normalize(&mut embedded)?;
            if embedded.len() != batch.len() {
                return Err(CceError::Provider(format!(
                    "embedder returned {} vectors for {} documents",
                    embedded.len(),
                    batch.len()
                )));
            }
            for (document, vector) in batch.iter().zip(embedded) {
                let expected = *dimensions.get_or_insert(vector.len());
                if vector.len() != expected {
                    return Err(CceError::Provider(
                        "embedding dimensions changed within an index".to_owned(),
                    ));
                }
                new_vectors.insert(document.document_id.as_str(), vector);
            }
        }
        let mut ids = Vec::with_capacity(documents.len());
        let mut vectors = Vec::with_capacity(
            documents
                .len()
                .checked_mul(dimensions.unwrap_or_default())
                .unwrap_or_default(),
        );
        let mut reused = 0_usize;
        for document in documents {
            ids.push(document.document_id.clone());
            if let (Some(index), Some(positions)) = (previous, previous_positions.as_ref())
                && let Some(position) = positions.get(document.document_id.as_str())
            {
                let start = position.checked_mul(index.dimensions).ok_or_else(|| {
                    CceError::ArtifactCorrupt("vector position overflow".to_owned())
                })?;
                let end = start.checked_add(index.dimensions).ok_or_else(|| {
                    CceError::ArtifactCorrupt("vector position overflow".to_owned())
                })?;
                vectors.extend_from_slice(index.vectors.get(start..end).ok_or_else(|| {
                    CceError::ArtifactCorrupt("reused vector range is invalid".to_owned())
                })?);
                reused += 1;
            } else {
                vectors.extend(
                    new_vectors
                        .remove(document.document_id.as_str())
                        .ok_or_else(|| {
                            CceError::Provider("new document embedding disappeared".to_owned())
                        })?,
                );
            }
        }
        let index = Self {
            profile: embedder.profile().to_owned(),
            dimensions: dimensions.unwrap_or_default(),
            ids,
            vectors,
            ann: None,
        }
        .with_ann_threshold(ANN_MIN_DOCUMENTS)?;
        Ok((index, reused))
    }

    fn with_ann_threshold(mut self, threshold: usize) -> Result<Self> {
        if self.ids.len() >= threshold && !self.ids.is_empty() {
            self.ann = Some(build_ann_index(
                &self.vectors,
                self.dimensions,
                self.ids.len(),
            )?);
        }
        Ok(self)
    }

    #[must_use]
    pub const fn capability_name(&self) -> &'static str {
        if self.ann.is_some() {
            "hnsw_i8_exact_f32_rerank"
        } else {
            "parallel_exact_inner_product"
        }
    }

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
        let query = embedder.embed_query(query).await?;
        if query.len() != self.dimensions {
            return Err(CceError::Provider(format!(
                "query dimension {} does not match index dimension {}",
                query.len(),
                self.dimensions
            )));
        }
        if limit == 0 || self.ids.is_empty() {
            return Ok(Vec::new());
        }
        let hits = if let Some(index) = &self.ann {
            let candidate_count = limit
                .saturating_mul(ANN_RERANK_MULTIPLIER)
                .max(64)
                .min(self.ids.len());
            let candidates = index
                .search(&query, candidate_count)
                .map_err(|error| CceError::Provider(format!("local ANN search failed: {error}")))?;
            candidates
                .keys
                .into_iter()
                .map(|key| {
                    let position = usize::try_from(key).map_err(|_| {
                        CceError::ArtifactCorrupt("ANN key exceeds platform size".to_owned())
                    })?;
                    let start = position.checked_mul(self.dimensions).ok_or_else(|| {
                        CceError::ArtifactCorrupt("ANN vector position overflow".to_owned())
                    })?;
                    let end = start.checked_add(self.dimensions).ok_or_else(|| {
                        CceError::ArtifactCorrupt("ANN vector position overflow".to_owned())
                    })?;
                    Ok(DenseSearchHit {
                        document_id: self.ids.get(position).cloned().ok_or_else(|| {
                            CceError::ArtifactCorrupt("ANN key has no document".to_owned())
                        })?,
                        score: dot(
                            &query,
                            self.vectors.get(start..end).ok_or_else(|| {
                                CceError::ArtifactCorrupt(
                                    "ANN rerank vector is truncated".to_owned(),
                                )
                            })?,
                        ),
                    })
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            self.ids
                .par_iter()
                .zip(self.vectors.par_chunks_exact(self.dimensions))
                .map(|(id, vector)| DenseSearchHit {
                    document_id: id.clone(),
                    score: dot(&query, vector),
                })
                .collect()
        };
        Ok(top_k(hits, limit))
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let dimension = u32::try_from(self.dimensions)
            .map_err(|_| CceError::Configuration("vector dimension exceeds format".to_owned()))?;
        let count = u64::try_from(self.ids.len())
            .map_err(|_| CceError::Configuration("vector count exceeds format".to_owned()))?;
        let mut ann_bytes = if let Some(index) = &self.ann {
            let mut bytes = vec![0_u8; index.serialized_length()];
            index.save_to_buffer(&mut bytes).map_err(|error| {
                CceError::Provider(format!("failed to serialize local ANN index: {error}"))
            })?;
            Some(bytes)
        } else {
            None
        };
        let extra = ann_bytes.as_ref().map_or(0, Vec::len);
        let mut bytes = Vec::with_capacity(72 + self.vectors.len() * 4 + extra);
        bytes.extend_from_slice(if self.ann.is_some() {
            ANN_MAGIC
        } else {
            EXACT_MAGIC
        });
        bytes.extend_from_slice(
            &(if self.ann.is_some() {
                ANN_FORMAT_VERSION
            } else {
                FORMAT_VERSION
            })
            .to_le_bytes(),
        );
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
        if let Some(ann) = ann_bytes.take() {
            bytes.extend_from_slice(
                &u64::try_from(ann.len())
                    .map_err(|_| CceError::Configuration("ANN artifact exceeds format".to_owned()))?
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(&ann);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = SliceReader::new(bytes);
        let ann_format = match reader.take(8)? {
            magic if magic == EXACT_MAGIC => false,
            magic if magic == ANN_MAGIC => true,
            _ => return Err(CceError::ArtifactCorrupt("invalid vector magic".to_owned())),
        };
        let version = reader.u32()?;
        let supported = if ann_format {
            ANN_FORMAT_VERSION
        } else {
            FORMAT_VERSION
        };
        if version != supported {
            return Err(CceError::UnsupportedFormat {
                found: version,
                supported,
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
        let ann = if ann_format {
            let ann_length = usize::try_from(reader.u64()?)
                .map_err(|_| CceError::ArtifactCorrupt("ANN length overflow".to_owned()))?;
            let index =
                AnnIndex::restore_from_buffer(reader.take(ann_length)?).map_err(|error| {
                    CceError::ArtifactCorrupt(format!("invalid local ANN index: {error}"))
                })?;
            if index.dimensions() != dimensions || index.size() != count {
                return Err(CceError::ArtifactCorrupt(
                    "ANN metadata does not match its CCE envelope".to_owned(),
                ));
            }
            Some(index)
        } else {
            None
        };
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
            ann,
        })
    }
}

fn build_ann_index(vectors: &[f32], dimensions: usize, count: usize) -> Result<AnnIndex> {
    if dimensions == 0 || vectors.len() != count.saturating_mul(dimensions) {
        return Err(CceError::Configuration(
            "ANN vector matrix has an invalid shape".to_owned(),
        ));
    }
    let options = IndexOptions {
        dimensions,
        metric: MetricKind::IP,
        quantization: ScalarKind::I8,
        connectivity: ANN_CONNECTIVITY,
        expansion_add: ANN_EXPANSION_ADD,
        expansion_search: ANN_EXPANSION_SEARCH,
        multi: false,
    };
    let index = AnnIndex::new(&options).map_err(|error| {
        CceError::Provider(format!("failed to create local ANN index: {error}"))
    })?;
    index
        .reserve_capacity_and_threads(count, 1)
        .map_err(|error| {
            CceError::Provider(format!("failed to reserve local ANN index: {error}"))
        })?;
    for (position, vector) in vectors.chunks_exact(dimensions).enumerate() {
        let key = u64::try_from(position)
            .map_err(|_| CceError::Provider("ANN key overflow".to_owned()))?;
        index.add(key, vector).map_err(|error| {
            CceError::Provider(format!("failed to build local ANN index: {error}"))
        })?;
    }
    Ok(index)
}

fn top_k(mut hits: Vec<DenseSearchHit>, limit: usize) -> Vec<DenseSearchHit> {
    let compare = |left: &DenseSearchHit, right: &DenseSearchHit| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.document_id.cmp(&right.document_id))
    };
    if limit < hits.len() {
        hits.select_nth_unstable_by(limit, compare);
        hits.truncate(limit);
    }
    hits.sort_unstable_by(compare);
    hits
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

pub(crate) fn required_model_files(model_file: &str, additional_files: &[String]) -> Vec<String> {
    let mut files = vec![
        model_file.to_owned(),
        "tokenizer.json".to_owned(),
        "config.json".to_owned(),
        "special_tokens_map.json".to_owned(),
        "tokenizer_config.json".to_owned(),
    ];
    files.extend(additional_files.iter().cloned());
    files.sort();
    files.dedup();
    files
}

pub(crate) fn initialize_onnx_runtime(runtime_library: &Path) -> Result<()> {
    static RUNTIME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let runtime_library = runtime_library.canonicalize().map_err(|error| {
        CceError::Configuration(format!(
            "ONNX Runtime library {} is unavailable ({error}); install the pinned local runtime and pass --onnx-runtime-library",
            runtime_library.display()
        ))
    })?;
    if let Some(loaded) = RUNTIME.get() {
        if loaded != &runtime_library {
            return Err(CceError::Configuration(format!(
                "ONNX Runtime is already initialized from {}; cannot switch to {} in the same process",
                loaded.display(),
                runtime_library.display()
            )));
        }
        return Ok(());
    }
    ort::init_from(&runtime_library)
        .map_err(|error| {
            CceError::Configuration(format!(
                "failed to load ONNX Runtime {}: {error}",
                runtime_library.display()
            ))
        })?
        .commit();
    let _ = RUNTIME.set(runtime_library);
    Ok(())
}

pub(crate) fn resolve_model_files(
    cache_dir: &Path,
    model_code: &str,
    revision: &str,
    files: &[String],
    allow_download: bool,
    download_flag: &str,
) -> Result<HashMap<String, PathBuf>> {
    if !safe_repository_id(model_code) {
        return Err(CceError::Configuration(format!(
            "unsafe model repository id: {model_code}"
        )));
    }
    let snapshot_dir = cache_dir
        .join("models")
        .join(model_code.replace('/', "--"))
        .join(revision);
    std::fs::create_dir_all(&snapshot_dir).map_err(|error| CceError::io(&snapshot_dir, error))?;
    let client = if allow_download {
        let mut builder = reqwest::blocking::Client::builder()
            .user_agent(concat!("cce/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(30 * 60));
        if let Some(proxy) = model_registry_proxy() {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|error| {
                CceError::Configuration(format!("invalid model registry proxy: {error}"))
            })?);
        }
        if let Some(certificate_path) = model_registry_ca_certificate() {
            let certificate_bytes = std::fs::read(&certificate_path)
                .map_err(|error| CceError::io(&certificate_path, error))?;
            let certificate =
                reqwest::Certificate::from_pem(&certificate_bytes).map_err(|error| {
                    CceError::Configuration(format!(
                        "invalid model registry CA certificate {}: {error}",
                        certificate_path.display()
                    ))
                })?;
            builder = builder.add_root_certificate(certificate);
        }
        Some(builder.build().map_err(|error| {
            CceError::Provider(format!("failed to initialize model downloader: {error}"))
        })?)
    } else {
        None
    };
    let mut resolved = HashMap::new();
    for file in files {
        let relative = safe_model_path(file)?;
        let path = snapshot_dir.join(&relative);
        if !path.is_file() {
            let Some(client) = &client else {
                return Err(CceError::Configuration(format!(
                    "local model file {file} is not cached under {}; run with {download_flag} once, then disable downloads for offline operation",
                    cache_dir.display(),
                )));
            };
            download_model_file(client, model_code, revision, file, &path)?;
        }
        resolved.insert(file.clone(), path);
    }
    Ok(resolved)
}

fn model_registry_proxy() -> Option<String> {
    [
        "CCE_MODEL_REGISTRY_PROXY",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .into_iter()
    .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

fn model_registry_ca_certificate() -> Option<PathBuf> {
    ["CCE_MODEL_REGISTRY_CA_CERT", "CODEX_PROXY_CERT"]
        .into_iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn download_model_file(
    client: &reqwest::blocking::Client,
    model_code: &str,
    revision: &str,
    file: &str,
    destination: &Path,
) -> Result<()> {
    const MAX_MODEL_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
    let parent = destination.parent().ok_or_else(|| {
        CceError::Configuration(format!(
            "model destination has no parent: {}",
            destination.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|error| CceError::io(parent, error))?;
    let lock_path = parent.join(format!(
        ".{}.lock",
        destination
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("model")
    ));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| CceError::io(&lock_path, error))?;
    FileExt::lock(&lock).map_err(|error| CceError::io(&lock_path, error))?;
    if destination.is_file() {
        return Ok(());
    }
    let mut url = reqwest::Url::parse("https://huggingface.co")
        .map_err(|error| CceError::Configuration(format!("invalid model registry URL: {error}")))?;
    {
        let mut segments = url.path_segments_mut().map_err(|()| {
            CceError::Configuration("model registry URL cannot accept paths".to_owned())
        })?;
        segments.extend(model_code.split('/'));
        segments.push("resolve");
        segments.push(revision);
        segments.extend(file.split('/'));
    }
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| {
            CceError::Provider(format!(
                "failed to download pinned model file {file}: {error}; details: {error:?}"
            ))
        })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MODEL_FILE_BYTES)
    {
        return Err(CceError::Provider(format!(
            "model file {file} exceeds the 8 GiB safety limit"
        )));
    }
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| CceError::io(parent, error))?;
    let written = std::io::copy(
        &mut response.take(MAX_MODEL_FILE_BYTES + 1),
        temporary.as_file_mut(),
    )
    .map_err(|error| CceError::io(temporary.path(), error))?;
    if written > MAX_MODEL_FILE_BYTES {
        return Err(CceError::Provider(format!(
            "model file {file} exceeds the 8 GiB safety limit"
        )));
    }
    temporary
        .as_file_mut()
        .flush()
        .map_err(|error| CceError::io(temporary.path(), error))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| CceError::io(temporary.path(), error))?;
    temporary
        .persist(destination)
        .map_err(|error| CceError::io(destination, error.error))?;
    Ok(())
}

fn safe_repository_id(value: &str) -> bool {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    parts.next().is_none()
        && !owner.is_empty()
        && !name.is_empty()
        && [owner, name].into_iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
}

pub(crate) fn safe_model_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(CceError::Configuration(format!(
            "unsafe model file path: {value}"
        )));
    }
    Ok(path.to_path_buf())
}

pub(crate) fn is_pinned_revision(revision: &str) -> bool {
    (40..=64).contains(&revision.len()) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_local_resource_budget(
    max_length: usize,
    batch_size: usize,
    allow_high_memory: bool,
) -> Result<()> {
    if !(32..=8_192).contains(&max_length) {
        return Err(CceError::Configuration(format!(
            "local embedding max length must be 32..=8192, got {max_length}"
        )));
    }
    if batch_size == 0 {
        return Err(CceError::Configuration(
            "local embedding batch size must be positive".to_owned(),
        ));
    }
    let attention_cells = batch_size
        .checked_mul(max_length)
        .and_then(|value| value.checked_mul(max_length))
        .ok_or_else(|| {
            CceError::Configuration("local embedding resource estimate overflowed".to_owned())
        })?;
    if attention_cells > MAX_SAFE_ATTENTION_CELLS && !allow_high_memory {
        return Err(CceError::Configuration(format!(
            "local embedding batch_size ({batch_size}) × max_length² ({max_length}²) estimates {attention_cells} attention cells, above the safe default {MAX_SAFE_ATTENTION_CELLS}; reduce batch/length or explicitly pass --embedding-allow-high-memory"
        )));
    }
    Ok(())
}

fn normalize_model_name(value: &str) -> &str {
    match value.to_ascii_lowercase().as_str() {
        "default" | "quality" | "jina" | "jina-code" | "jinaai/jina-embeddings-v2-base-code" => {
            "JinaEmbeddingsV2BaseCode"
        }
        "bge-small" | "baai/bge-small-en-v1.5" => "BGESmallENV15",
        "fast" | "bge-small-q" => "BGESmallENV15Q",
        "balanced" | "multilingual-e5-small" | "intfloat/multilingual-e5-small" => {
            "MultilingualE5Small"
        }
        _ => value,
    }
}

fn short_hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex()[..12].to_owned()
}

fn dense_document_text(document: &DocumentContent) -> String {
    let path = document
        .address
        .as_ref()
        .map_or("<derived>", |address| address.path.as_str());
    format!(
        "path: {path}\nrepresentation: {:?}\n{}",
        document.representation, document.text
    )
}

fn validate_and_normalize(vectors: &mut [Vec<f32>]) -> Result<()> {
    let dimensions = vectors.first().map(Vec::len).unwrap_or_default();
    for vector in vectors {
        if vector.len() != dimensions
            || vector.is_empty()
            || vector.iter().any(|value| !value.is_finite())
        {
            return Err(CceError::Provider("invalid embedding vector".to_owned()));
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
                symbol_name: Some("alpha".to_owned()),
                representation: cce_core::RetrievalRepresentation::RawCode,
                address: None,
                evidence: Vec::new(),
                text: "resume cursor persistence".to_owned(),
            },
            DocumentContent {
                document_id: "b".to_owned(),
                entity_id: "eb".to_owned(),
                symbol_name: Some("beta".to_owned()),
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

        let mut changed = documents.clone();
        changed.push(DocumentContent {
            document_id: "c".to_owned(),
            entity_id: "ec".to_owned(),
            symbol_name: Some("gamma".to_owned()),
            representation: cce_core::RetrievalRepresentation::RawCode,
            address: None,
            evidence: Vec::new(),
            text: "incremental vector reuse".to_owned(),
        });
        let (incremental, reused) =
            DenseIndex::build_incremental(&changed, &embedder, 8, Some(&decoded))
                .await
                .expect("incremental index");
        assert_eq!(reused, 2);
        assert_eq!(incremental.ids, vec!["a", "b", "c"]);
        let ann = incremental.with_ann_threshold(1).expect("ANN index");
        assert_eq!(ann.capability_name(), "hnsw_i8_exact_f32_rerank");
        let ann_decoded =
            DenseIndex::decode(&ann.encode().expect("encode ANN")).expect("decode ANN");
        let ann_hits = ann_decoded
            .search("cursor resume", &embedder, 1)
            .await
            .expect("ANN search");
        assert_eq!(ann_hits[0].document_id, "a");
    }

    #[tokio::test]
    async fn quantized_ann_preserves_exact_top_ten_recall() {
        let embedder = DeterministicEmbedder::new(96).expect("embedder");
        let documents = (0..2_500)
            .map(|index| DocumentContent {
                document_id: format!("doc-{index:04}"),
                entity_id: format!("entity-{index:04}"),
                symbol_name: Some(format!("symbol_{index}")),
                representation: cce_core::RetrievalRepresentation::RawCode,
                address: None,
                evidence: Vec::new(),
                text: format!(
                    "fn symbol_{index}() {{ subsystem_{} operation_{} storage_{}; }}",
                    index % 31,
                    index % 47,
                    index % 67
                ),
            })
            .collect::<Vec<_>>();
        let exact = DenseIndex::build(&documents, &embedder, 128)
            .await
            .expect("exact index");
        let ann = DenseIndex::decode(&exact.encode().expect("encode exact"))
            .expect("decode exact")
            .with_ann_threshold(1)
            .expect("ANN index");
        let mut found = 0_usize;
        let mut expected = 0_usize;
        for index in (0..2_500).step_by(83) {
            let query = format!(
                "subsystem_{} operation_{} storage_{}",
                index % 31,
                index % 47,
                index % 67
            );
            let exact_ids = exact
                .search(&query, &embedder, 10)
                .await
                .expect("exact search")
                .into_iter()
                .map(|hit| hit.document_id)
                .collect::<std::collections::HashSet<_>>();
            let ann_ids = ann
                .search(&query, &embedder, 10)
                .await
                .expect("ANN search")
                .into_iter()
                .map(|hit| hit.document_id)
                .collect::<std::collections::HashSet<_>>();
            found += exact_ids.intersection(&ann_ids).count();
            expected += exact_ids.len();
        }
        let recall = found as f64 / expected as f64;
        println!("synthetic_ann_recall_at_10={recall:.4} queries=31 corpus=2500");
        assert!(recall >= 0.99, "ANN Recall@10 regressed to {recall:.4}");
    }

    #[test]
    fn local_resource_budget_rejects_previous_oom_configuration() {
        let error = validate_local_resource_budget(1_024, 32, false).expect_err("unsafe budget");
        assert!(error.to_string().contains("attention cells"));
        validate_local_resource_budget(1_024, 32, true).expect("explicit override");
        validate_local_resource_budget(512, 4, false).expect("safe default");
    }

    #[test]
    fn local_model_bundle_binds_revision_runtime_and_file_hashes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = b"{\"hidden_size\":384}";
        std::fs::write(directory.path().join("config.json"), config).expect("model config");
        let revision = "a".repeat(64);
        std::fs::write(
            directory.path().join("cce-model-manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "bundleRevision": revision,
                "runtimeModel": "MultilingualE5Small",
                "files": {"config.json": format!("{:x}", Sha256::digest(config))}
            }))
            .expect("manifest JSON"),
        )
        .expect("manifest");
        let resolved = resolve_local_model_bundle(
            directory.path(),
            "MultilingualE5Small",
            &revision,
            &["config.json".to_owned()],
        )
        .expect("verified local bundle");
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn model_identifiers_and_paths_are_strictly_validated() {
        assert!(is_pinned_revision(
            "52398278842ec682c6f32300af41344b1c0b0bb2"
        ));
        assert!(!is_pinned_revision("main"));
        assert!(safe_repository_id("Qdrant/bge-small-en-v1.5-onnx-Q"));
        assert!(!safe_repository_id("owner/repo/extra"));
        assert!(safe_model_path("onnx/model.onnx").is_ok());
        assert!(safe_model_path("../model.onnx").is_err());
    }

    #[test]
    fn dense_document_representation_includes_path_and_kind() {
        let document = DocumentContent {
            document_id: "doc".to_owned(),
            entity_id: "entity".to_owned(),
            symbol_name: Some("search".to_owned()),
            representation: cce_core::RetrievalRepresentation::RawCode,
            address: None,
            evidence: Vec::new(),
            text: "fn search() {}".to_owned(),
        };
        let text = dense_document_text(&document);
        assert!(text.starts_with("path: <derived>\nrepresentation: RawCode\n"));
        assert!(text.ends_with("fn search() {}"));
    }
}
