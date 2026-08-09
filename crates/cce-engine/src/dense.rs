use async_trait::async_trait;
use cce_core::{CceError, Result};
use cce_store::DocumentContent;
use serde::{Deserialize, Serialize};

use crate::DenseBackendConfig;

const MAGIC: &[u8; 8] = b"CCEVEC1\0";
const FORMAT_VERSION: u32 = 1;

#[async_trait]
pub trait Embedder: Send + Sync {
    fn profile(&self) -> &str;
    fn production_ready(&self) -> bool;
    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[derive(Debug, Clone)]
pub enum EmbeddingBackend {
    Deterministic(DeterministicEmbedder),
    OpenAiCompatible(OpenAiCompatibleEmbedder),
}

impl EmbeddingBackend {
    pub fn from_config(config: &DenseBackendConfig) -> Result<Option<Self>> {
        match config {
            DenseBackendConfig::Disabled => Ok(None),
            DenseBackendConfig::DeterministicBaseline { dimensions } => Ok(Some(
                Self::Deterministic(DeterministicEmbedder::new(*dimensions)?),
            )),
            DenseBackendConfig::OpenAiCompatible {
                base_url,
                model,
                api_key_environment,
                dimensions,
                ..
            } => {
                let api_key = std::env::var(api_key_environment).map_err(|_| {
                    CceError::Configuration(format!(
                        "embedding API key environment variable {api_key_environment} is not set"
                    ))
                })?;
                Ok(Some(Self::OpenAiCompatible(OpenAiCompatibleEmbedder::new(
                    base_url.clone(),
                    model.clone(),
                    api_key,
                    *dimensions,
                )?)))
            }
        }
    }
}

#[async_trait]
impl Embedder for EmbeddingBackend {
    fn profile(&self) -> &str {
        match self {
            Self::Deterministic(value) => value.profile(),
            Self::OpenAiCompatible(value) => value.profile(),
        }
    }

    fn production_ready(&self) -> bool {
        match self {
            Self::Deterministic(value) => value.production_ready(),
            Self::OpenAiCompatible(value) => value.production_ready(),
        }
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        match self {
            Self::Deterministic(value) => value.embed(inputs).await,
            Self::OpenAiCompatible(value) => value.embed(inputs).await,
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

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
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
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleEmbedder {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
    dimensions: Option<usize>,
    profile: String,
}

impl OpenAiCompatibleEmbedder {
    pub fn new(
        base_url: String,
        model: String,
        api_key: String,
        dimensions: Option<usize>,
    ) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/');
        let endpoint = if base_url.ends_with("/embeddings") {
            base_url.to_owned()
        } else {
            format!("{base_url}/embeddings")
        };
        reqwest::Url::parse(&endpoint)
            .map_err(|error| CceError::Configuration(format!("invalid embedding URL: {error}")))?;
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|error| CceError::Configuration(error.to_string()))?;
        let profile = format!(
            "openai-compatible:{model}:{}",
            dimensions.map_or_else(|| "native".to_owned(), |value| value.to_string())
        );
        Ok(Self {
            client,
            endpoint,
            model,
            api_key,
            dimensions,
            profile,
        })
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}

#[async_trait]
impl Embedder for OpenAiCompatibleEmbedder {
    fn profile(&self) -> &str {
        &self.profile
    }

    fn production_ready(&self) -> bool {
        true
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&EmbeddingRequest {
                model: &self.model,
                input: inputs,
                encoding_format: "float",
                dimensions: self.dimensions,
            })
            .send()
            .await
            .map_err(|error| CceError::Provider(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(CceError::Provider(format!(
                "embedding provider returned {status}: {}",
                truncate(&message, 1_024)
            )));
        }
        let mut data = response
            .json::<EmbeddingResponse>()
            .await
            .map_err(|error| CceError::Provider(error.to_string()))?
            .data;
        data.sort_by_key(|item| item.index);
        if data.len() != inputs.len() {
            return Err(CceError::Provider(format!(
                "embedding provider returned {} vectors for {} inputs",
                data.len(),
                inputs.len()
            )));
        }
        let mut vectors = data
            .into_iter()
            .map(|item| item.embedding)
            .collect::<Vec<_>>();
        validate_and_normalize(&mut vectors)?;
        Ok(vectors)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DenseSearchHit {
    pub document_id: String,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DenseIndex {
    profile: String,
    dimensions: usize,
    ids: Vec<String>,
    vectors: Vec<f32>,
}

impl DenseIndex {
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
            let mut embedded = embedder.embed(&inputs).await?;
            validate_and_normalize(&mut embedded)?;
            for (document, vector) in batch.iter().zip(embedded) {
                let expected = *dimensions.get_or_insert(vector.len());
                if vector.len() != expected {
                    return Err(CceError::Provider(
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
        let vectors = embedder.embed(&[query.to_owned()]).await?;
        let query = vectors.into_iter().next().ok_or_else(|| {
            CceError::Provider("embedding provider returned no query vector".to_owned())
        })?;
        if query.len() != self.dimensions {
            return Err(CceError::Provider(format!(
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

fn truncate(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
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
                representation: cce_core::RetrievalRepresentation::RawCode,
                address: None,
                evidence: Vec::new(),
                text: "resume cursor persistence".to_owned(),
            },
            DocumentContent {
                document_id: "b".to_owned(),
                entity_id: "eb".to_owned(),
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
