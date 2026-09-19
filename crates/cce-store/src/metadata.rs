use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};

use cce_core::{
    CceError, CodeEntity, CodeRegion, Relation, RepositoryIdentity, Result, RetrievalDocument,
    RetrievalRepresentation, SnapshotIdentity, SourceAddress, ViewKind, ViewManifest, ViewState,
    ViewStatus, has_cjk,
};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::{ArtifactRecord, ArtifactStore};

#[derive(Debug, Clone)]
/// One indexed source file: identity plus the artifact holding its bytes.
pub struct SourceFileRecord {
    /// Repository-relative path.
    pub path: String,
    /// Detected language, when known.
    pub language: Option<String>,
    /// BLAKE3 content hash of the file bytes.
    pub content_hash: String,
    /// Artifact storing the file's bytes.
    pub artifact: ArtifactRecord,
    /// File size.
    pub byte_count: u64,
    /// Number of lines.
    pub line_count: u64,
    /// Digest of cached per-file analysis output, when present.
    pub analysis_artifact_digest: Option<String>,
}

#[derive(Debug, Clone)]
/// A retrieval document with its materialized body for FTS indexing.
pub struct IndexedDocument {
    /// The document record.
    pub document: RetrievalDocument,
    /// Source path the document is drawn from.
    pub path: String,
    /// Display/symbol name used for exact-match lookups.
    pub name: String,
    /// Text body fed to the FTS index.
    pub body: String,
}

#[derive(Debug, Clone, Default)]
/// Everything committed atomically for one snapshot.
pub struct SnapshotRecords {
    /// Artifact records to register.
    pub artifacts: Vec<ArtifactRecord>,
    /// Source file records.
    pub files: Vec<SourceFileRecord>,
    /// Canonical code regions.
    pub regions: Vec<CodeRegion>,
    /// Entity graph nodes.
    pub entities: Vec<CodeEntity>,
    /// Typed relations (already merged across extractors).
    pub relations: Vec<Relation>,
    /// Retrieval documents for FTS.
    pub documents: Vec<IndexedDocument>,
}

#[derive(Debug, Clone)]
/// Lightweight per-file row for post-commit view repair — the full
/// `ArtifactRecord` is not joined since repair only needs the analysis
/// digest to reload cached parse results.
pub struct SourceFileRow {
    /// Repository-relative path.
    pub path: String,
    /// Detected language, when known.
    pub language: Option<String>,
    /// Digest of cached per-file analysis output, when present.
    pub analysis_artifact_digest: Option<String>,
}

#[derive(Debug, Clone)]
/// One FTS5 match with its entity linkage and score.
pub struct LexicalHit {
    /// Retrieved document id.
    pub document_id: String,
    /// Entity owning the document.
    pub entity_id: String,
    /// Canonical region, when mappable.
    pub region_id: Option<String>,
    /// Symbol/display name.
    pub symbol_name: String,
    /// Document representation.
    pub representation: RetrievalRepresentation,
    /// Primary source address.
    pub address: Option<SourceAddress>,
    /// Evidence addresses.
    pub evidence: Vec<SourceAddress>,
    /// FTS rank score.
    pub score: f64,
    /// Match snippet.
    pub snippet: String,
}

/// One pairwise evidence-floor clause: two query terms `AND`ed together.
pub(crate) type FloorPair = (String, String);

/// A file's stat fingerprint observed during a repository scan.
#[derive(Debug, Clone)]
/// Cached file metadata for incremental scans.
pub struct ScanCacheEntry {
    /// Repository-relative path.
    pub path: String,
    /// File size at scan time.
    pub size_bytes: u64,
    /// Modification time in epoch milliseconds.
    pub mtime_ms: i64,
    /// Line count at scan time.
    pub line_count: u64,
    /// Content hash at scan time.
    pub content_hash: String,
}

/// Outcome of a garbage-collection pass over snapshots and the artifact store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Result of a garbage-collection pass.
pub struct GcReport {
    /// Snapshots dropped.
    pub pruned_snapshots: usize,
    /// Artifact rows dereferenced.
    pub removed_digests: usize,
    /// Unreferenced object files deleted.
    pub removed_orphan_files: usize,
    /// Bytes reclaimed.
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone)]
/// A document's materialized text plus linkage, for packing/snippet use.
pub struct DocumentContent {
    /// Document id.
    pub document_id: String,
    /// Owning entity.
    pub entity_id: String,
    /// Canonical region when mappable.
    pub region_id: Option<String>,
    /// Document representation.
    pub representation: RetrievalRepresentation,
    /// Primary source address.
    pub address: Option<SourceAddress>,
    /// Evidence addresses.
    pub evidence: Vec<SourceAddress>,
    /// Full document text.
    pub text: String,
}

/// Edge traversal direction for relation queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationDirection {
    /// Edges where the entity is the source.
    Outgoing,
    /// Edges where the entity is the target.
    Incoming,
    /// Both directions.
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Result of a store health check.
pub struct StoreHealth {
    /// Whether `SQLite` answered its self-check.
    pub sqlite_ok: bool,
    /// PRAGMA integrity/foreign-key messages.
    pub sqlite_messages: Vec<String>,
    /// How many referenced artifacts were verified on disk.
    pub artifacts_checked: usize,
    /// Referenced digests missing from the object store.
    pub missing_artifacts: Vec<String>,
    /// Error encountered while scanning the object store, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_scan_error: Option<String>,
}

/// Read connections alongside the writer — WAL permits any number of
/// readers next to one serialized writer, so search traffic fans out
/// instead of queueing on a single connection mutex.
const READ_POOL_SIZE: usize = 4;

/// Connection pool keyed to WAL's concurrency model. `lock()` returns
/// the serialized WRITER — every legacy `self.connection.read()` call
/// site keeps its exact semantics (multi-statement transactions stay
/// per-method on one connection). `read()` hands out a round-robin
/// READ-ONLY connection (`PRAGMA query_only`) — a write reaching it
/// fails loudly instead of silently running on the wrong connection.
struct ConnectionPool {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl ConnectionPool {
    fn open(
        database_path: &Path,
        reader_count: usize,
        configure: impl Fn(&Connection) -> Result<()>,
    ) -> Result<Self> {
        let writer = Connection::open(database_path)
            .map_err(|error| CceError::Storage(error.to_string()))?;
        writer
            .busy_timeout(Duration::from_secs(10))
            .map_err(|error| CceError::Storage(error.to_string()))?;
        configure(&writer)?;
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            let reader = Connection::open(database_path)
                .map_err(|error| CceError::Storage(error.to_string()))?;
            reader
                .busy_timeout(Duration::from_secs(10))
                .map_err(|error| CceError::Storage(error.to_string()))?;
            reader
                .execute_batch(
                    "PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY; PRAGMA query_only=ON;",
                )
                .map_err(|error| CceError::Storage(error.to_string()))?;
            readers.push(Mutex::new(reader));
        }
        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            next: AtomicUsize::new(0),
        })
    }

    /// The serialized writer — same guard the single-connection store
    /// handed every caller before pooling.
    fn lock(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.writer.lock()
    }

    /// A read-only pooled connection for provably-read-only paths.
    /// Falls back to the writer if the pool is somehow empty — correct,
    /// just serialized.
    fn read(&self) -> parking_lot::MutexGuard<'_, Connection> {
        if self.readers.is_empty() {
            return self.writer.lock();
        }
        let index =
            self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.readers.len();
        self.readers
            .get(index)
            .map_or_else(|| self.writer.lock(), Mutex::lock)
    }
}

/// One conjunction-binding row: the artifact path plus the entity
/// carrying the matched document, when it resolves. The caller
/// classifies the binding's scope from `entity_kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermBinding {
    /// Repository-relative path of the binding artifact.
    pub path: String,
    /// Name of the entity whose document carries every term.
    pub entity_name: Option<String>,
    /// Kind of that entity — `None` when the document's entity does
    /// not resolve (treated as file-level binding).
    pub entity_kind: Option<cce_core::EntityKind>,
}

/// The canonical metadata store: one `SQLite` database (entities, relations,
/// FTS, view status) plus the content-addressed artifact store.
#[derive(Clone)]
pub struct MetadataStore {
    connection: Arc<ConnectionPool>,
    artifacts: ArtifactStore,
    database_path: PathBuf,
}

impl std::fmt::Debug for MetadataStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetadataStore")
            .field("database_path", &self.database_path)
            .field("artifacts", &self.artifacts)
            .finish_non_exhaustive()
    }
}

// The connection guard is held for the whole query body; tightening its
// drop buys nothing on a single-writer store and complicates statement
// borrows, so the nursery lint is allowed here deliberately.
#[allow(clippy::significant_drop_tightening)]
impl MetadataStore {
    /// Open (creating/migrating if needed) the store under `data_root`.
    ///
    /// # Errors
    /// `UnsupportedFormat` if the database is newer than this build; I/O or
    /// storage errors on open/migration failure.
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self> {
        let data_root = data_root.as_ref();
        std::fs::create_dir_all(data_root).map_err(|error| CceError::io(data_root, error))?;
        let database_path = data_root.join("metadata.sqlite");
        let pool = ConnectionPool::open(&database_path, READ_POOL_SIZE, |connection| {
            connection
                .execute_batch(
                    "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;",
                )
                .map_err(|error| CceError::Storage(error.to_string()))
        })?;
        let version: u32 = pool
            .lock()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version > 5 {
            return Err(CceError::UnsupportedFormat {
                found: version,
                supported: 5,
            });
        }
        if version == 0 {
            pool.lock()
                .execute_batch(include_str!("migrations/0001_initial.sql"))
                .map_err(|error| CceError::Storage(format!("migration 1 failed: {error}")))?;
        }
        if version < 2 {
            pool.lock()
                .execute_batch(include_str!("migrations/0002_parse_cache.sql"))
                .map_err(|error| CceError::Storage(format!("migration 2 failed: {error}")))?;
        }
        if version < 3 {
            pool.lock()
                .execute_batch(include_str!("migrations/0003_scan_cache.sql"))
                .map_err(|error| CceError::Storage(format!("migration 3 failed: {error}")))?;
        }
        if version < 4 {
            pool.lock()
                .execute_batch(include_str!("migrations/0004_regions.sql"))
                .map_err(|error| CceError::Storage(format!("migration 4 failed: {error}")))?;
        }
        if version < 5 {
            pool.lock()
                .execute_batch(include_str!("migrations/0005_entity_name_trigrams.sql"))
                .map_err(|error| CceError::Storage(format!("migration 5 failed: {error}")))?;
        }
        let artifacts = ArtifactStore::open(data_root)?;
        Ok(Self {
            connection: Arc::new(pool),
            artifacts,
            database_path,
        })
    }

    /// The content-addressed artifact store.
    #[must_use]
    pub const fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// Insert or update a repository record.
    ///
    /// # Errors
    /// Storage error on write failure.
    pub fn register_repository(&self, repository: &RepositoryIdentity) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.connection
            .lock()
            .execute(
                "INSERT INTO repositories(id, canonical_root, remote, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(id) DO UPDATE SET canonical_root=excluded.canonical_root,
                 remote=excluded.remote, updated_at=excluded.updated_at",
                params![
                    repository.id,
                    repository.canonical_root,
                    repository.remote,
                    now
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// Run integrity checks over `SQLite` and referenced artifacts.
    ///
    /// # Errors
    /// Storage error if the checks themselves cannot run.
    pub fn health(&self) -> Result<StoreHealth> {
        let connection = self.connection.read();
        let messages = match connection.prepare("PRAGMA quick_check") {
            Ok(mut statement) => match statement.query_map([], |row| row.get::<_, String>(0)) {
                Ok(rows) => rows
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap_or_else(|error| vec![error.to_string()]),
                Err(error) => vec![error.to_string()],
            },
            Err(error) => vec![error.to_string()],
        };
        let sqlite_ok = messages.len() == 1 && messages.first().is_some_and(|m| m == "ok");
        let messages = messages.into_iter().map(bound_health_message).collect();
        let (digests, artifact_scan_error) =
            match connection.prepare("SELECT digest FROM artifacts ORDER BY digest") {
                Ok(mut statement) => match statement.query_map([], |row| row.get::<_, String>(0)) {
                    Ok(rows) => match rows.collect::<std::result::Result<Vec<_>, _>>() {
                        Ok(digests) => (digests, None),
                        Err(error) => (Vec::new(), Some(error.to_string())),
                    },
                    Err(error) => (Vec::new(), Some(error.to_string())),
                },
                Err(error) => (Vec::new(), Some(error.to_string())),
            };
        let mut missing_artifacts = Vec::new();
        for digest in &digests {
            if !self.artifacts.contains(digest)? {
                missing_artifacts.push(digest.clone());
            }
        }
        Ok(StoreHealth {
            sqlite_ok,
            sqlite_messages: messages,
            artifacts_checked: digests.len(),
            missing_artifacts,
            artifact_scan_error,
        })
    }

    /// Record an artifact row so it is known to health checks and GC.
    ///
    /// # Errors
    /// Storage error on write failure.
    pub fn register_artifact(&self, artifact: &ArtifactRecord) -> Result<()> {
        let connection = self.connection.lock();
        connection
            .execute(
                "INSERT OR IGNORE INTO artifacts(digest, kind, size_bytes, relative_path, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    artifact.digest,
                    artifact.kind.to_string(),
                    u64_to_i64(artifact.size_bytes)?,
                    artifact.relative_path,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// Insert a snapshot row before its records are committed.
    ///
    /// # Errors
    /// Storage error on write failure.
    pub fn begin_snapshot(&self, snapshot: &SnapshotIdentity) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "INSERT OR REPLACE INTO snapshots(
                  id, repository_id, base_revision, workspace_overlay_hash, index_profile_hash,
                  created_at, file_count, source_bytes, complete
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)",
                params![
                    snapshot.id,
                    snapshot.repository_id,
                    snapshot.base_revision,
                    snapshot.workspace_overlay_hash,
                    snapshot.index_profile_hash,
                    snapshot.created_at.to_rfc3339(),
                    u64_to_i64(snapshot.file_count)?,
                    u64_to_i64(snapshot.source_bytes)?,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// Atomically commit all records for a snapshot (files, regions,
    /// entities, relations, documents) in one transaction.
    ///
    /// # Errors
    /// Storage error; on failure nothing is committed.
    pub fn commit_snapshot(
        &self,
        snapshot: &SnapshotIdentity,
        records: &SnapshotRecords,
    ) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM documents_fts WHERE snapshot_id=?1",
                [&snapshot.id],
            )
            .map_err(storage_error)?;
        transaction
            .execute("DELETE FROM relations WHERE snapshot_id=?1", [&snapshot.id])
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM retrieval_documents WHERE snapshot_id=?1",
                [&snapshot.id],
            )
            .map_err(storage_error)?;
        transaction
            .execute("DELETE FROM entities WHERE snapshot_id=?1", [&snapshot.id])
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM entity_name_trigrams WHERE snapshot_id=?1",
                [&snapshot.id],
            )
            .map_err(storage_error)?;
        transaction
            .execute("DELETE FROM regions WHERE snapshot_id=?1", [&snapshot.id])
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM source_files WHERE snapshot_id=?1",
                [&snapshot.id],
            )
            .map_err(storage_error)?;

        for artifact in &records.artifacts {
            insert_artifact(&transaction, artifact)?;
        }

        for file in &records.files {
            insert_artifact(&transaction, &file.artifact)?;
            transaction
                .execute(
                    "INSERT INTO source_files(snapshot_id, path, language, content_hash,
                     artifact_digest, byte_count, line_count, analysis_artifact_digest)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        snapshot.id,
                        file.path,
                        file.language,
                        file.content_hash,
                        file.artifact.digest,
                        u64_to_i64(file.byte_count)?,
                        u64_to_i64(file.line_count)?,
                        file.analysis_artifact_digest,
                    ],
                )
                .map_err(storage_error)?;
        }

        for region in &records.regions {
            transaction
                .execute(
                    "INSERT INTO regions(id, snapshot_id, path, kind, language, symbol_name,
                     symbol_kind, qualified_name, parent_region_id, start_byte, end_byte,
                     start_line, end_line)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        region.id,
                        snapshot.id,
                        region.path,
                        json(&region.kind)?,
                        region.language,
                        region.symbol_name,
                        optional_json(region.symbol_kind.as_ref())?,
                        region.qualified_name,
                        region.parent_region_id,
                        u64_to_i64(region.start_byte)?,
                        u64_to_i64(region.end_byte)?,
                        i64::from(region.start_line),
                        i64::from(region.end_line),
                    ],
                )
                .map_err(storage_error)?;
        }

        let mut trigram_insert = transaction
            .prepare(
                "INSERT INTO entity_name_trigrams(snapshot_id, trigram, entity_id)
                 VALUES (?1, ?2, ?3)",
            )
            .map_err(storage_error)?;
        for entity in &records.entities {
            transaction
                .execute(
                    "INSERT INTO entities(id, snapshot_id, kind, name, qualified_name, signature,
                     language, region_id, address_json, capabilities_json, attributes_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        entity.id,
                        snapshot.id,
                        json(&entity.kind)?,
                        entity.name,
                        entity.qualified_name,
                        entity.signature,
                        entity.language,
                        entity.region_id,
                        optional_json(entity.address.as_ref())?,
                        json(&entity.capabilities)?,
                        json(&entity.attributes)?,
                    ],
                )
                .map_err(storage_error)?;
            let mut trigrams = name_trigrams(&entity.name);
            if let Some(qualified) = &entity.qualified_name {
                trigrams.extend(name_trigrams(qualified));
            }
            trigrams.sort();
            trigrams.dedup();
            for trigram in &trigrams {
                trigram_insert
                    .execute(params![snapshot.id, trigram, entity.id])
                    .map_err(storage_error)?;
            }
        }
        drop(trigram_insert);

        for relation in &records.relations {
            transaction
                .execute(
                    "INSERT INTO relations(id, snapshot_id, source_entity_id, target_entity_id, kind,
                     origin, confidence, extractor, evidence_json, attributes_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        relation.id,
                        snapshot.id,
                        relation.source_entity_id,
                        relation.target_entity_id,
                        json(&relation.kind)?,
                        json(&relation.origin)?,
                        f64::from(relation.confidence),
                        relation.extractor,
                        json(&relation.evidence)?,
                        json(&relation.attributes)?,
                    ],
                )
                .map_err(storage_error)?;
        }

        for indexed in &records.documents {
            let document = &indexed.document;
            transaction
                .execute(
                    "INSERT INTO retrieval_documents(id, snapshot_id, entity_id, region_id,
                     representation, body_artifact_digest, address_json, embedding_profile,
                     generated_by, evidence_json, terms_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        document.id,
                        snapshot.id,
                        document.entity_id,
                        document.region_id,
                        json(&document.representation)?,
                        document.body_artifact_digest,
                        optional_json(document.address.as_ref())?,
                        document.embedding_profile,
                        document.generated_by,
                        json(&document.evidence)?,
                        json(&document.terms)?,
                    ],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO documents_fts(document_id, snapshot_id, entity_id, path, name, terms, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        document.id,
                        snapshot.id,
                        document.entity_id,
                        indexed.path,
                        identifier_fts_forms(&indexed.name),
                        document.terms.join(" "),
                        indexed.body,
                    ],
                )
                .map_err(storage_error)?;
        }

        transaction
            .execute(
                "UPDATE snapshots SET complete=1 WHERE id=?1",
                [&snapshot.id],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO current_snapshots(repository_id, snapshot_id, updated_at)
                 VALUES (?1, ?2, ?3) ON CONFLICT(repository_id) DO UPDATE SET
                 snapshot_id=excluded.snapshot_id, updated_at=excluded.updated_at",
                params![snapshot.repository_id, snapshot.id, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(())
    }

    /// Upsert the status of one view for a snapshot.
    ///
    /// # Errors
    /// Storage error on write failure.
    pub fn set_view_status(
        &self,
        repository_id: &str,
        snapshot_id: &str,
        kind: ViewKind,
        status: &ViewStatus,
    ) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "INSERT INTO view_status(repository_id, snapshot_id, view_kind, state, profile_hash,
                 updated_at, capabilities_json, artifact_digest, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(snapshot_id, view_kind) DO UPDATE SET state=excluded.state,
                 profile_hash=excluded.profile_hash, updated_at=excluded.updated_at,
                 capabilities_json=excluded.capabilities_json,
                 artifact_digest=excluded.artifact_digest, message=excluded.message",
                params![
                    repository_id,
                    snapshot_id,
                    json(&kind)?,
                    json(&status.state)?,
                    status.profile_hash,
                    status.updated_at.to_rfc3339(),
                    json(&status.capabilities)?,
                    status.artifact_digest,
                    status.message,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// The most recently committed snapshot for a repository, if any.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn current_snapshot(&self, repository_id: &str) -> Result<Option<String>> {
        self.connection
            .read()
            .query_row(
                "SELECT snapshot_id FROM current_snapshots WHERE repository_id=?1",
                [repository_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)
    }

    /// Whether the snapshot finished committing (all rows present).
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn snapshot_is_complete(&self, snapshot_id: &str) -> Result<bool> {
        self.connection
            .read()
            .query_row(
                "SELECT complete FROM snapshots WHERE id=?1",
                [snapshot_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(storage_error)
    }

    /// The most recent *other* completed snapshot under the same index
    /// profile that produced a dense artifact — the incremental-embedding
    /// reuse source. Returns `(snapshot_id, artifact_digest)`.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn prior_dense_artifact(
        &self,
        repository_id: &str,
        exclude_snapshot_id: &str,
        profile_hash: &str,
    ) -> Result<Option<(String, String)>> {
        self.connection
            .read()
            .query_row(
                "SELECT s.id, v.artifact_digest
                 FROM snapshots s
                 JOIN view_status v
                   ON v.snapshot_id = s.id AND v.repository_id = s.repository_id
                 WHERE s.repository_id = ?1 AND s.id != ?2 AND s.complete = 1
                   AND s.index_profile_hash = ?3
                   AND v.view_kind = '\"dense\"' AND v.artifact_digest IS NOT NULL
                 ORDER BY s.created_at DESC LIMIT 1",
                params![repository_id, exclude_snapshot_id, profile_hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_error)
    }

    /// Look up a cached per-file analysis artifact digest for reuse.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn cached_analysis_digest(
        &self,
        repository_id: &str,
        path: &str,
        content_hash: &str,
    ) -> Result<Option<String>> {
        self.connection
            .read()
            .query_row(
                "SELECT sf.analysis_artifact_digest
                 FROM current_snapshots current
                 JOIN source_files sf ON sf.snapshot_id=current.snapshot_id
                 WHERE current.repository_id=?1 AND sf.path=?2 AND sf.content_hash=?3",
                params![repository_id, path, content_hash],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(storage_error)
    }

    /// Look up a cached content hash for a file whose size and mtime are
    /// unchanged since the last scan. Heuristic fast path only.
    /// Return cached scan entries for files whose mtime/size/hash still
    /// match, so unchanged files can skip re-reading.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn scan_cache_lookup(
        &self,
        repository_id: &str,
        path: &str,
        size_bytes: u64,
        mtime_ms: i64,
    ) -> Result<Option<(String, u64)>> {
        self.connection
            .read()
            .query_row(
                "SELECT content_hash, line_count FROM scan_cache
                 WHERE repository_id=?1 AND path=?2 AND size_bytes=?3 AND mtime_ms=?4",
                params![repository_id, path, u64_to_i64(size_bytes)?, mtime_ms],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(storage_error)?
            .map(|(hash, lines)| Ok((hash, u64::try_from(lines).unwrap_or(0))))
            .transpose()
    }

    /// Replace the scan cache for a repository with the entries observed in
    /// the latest scan.
    /// Replace a repository's scan-cache entries after a scan.
    ///
    /// # Errors
    /// Storage error on write failure.
    pub fn update_scan_cache(&self, repository_id: &str, entries: &[ScanCacheEntry]) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM scan_cache WHERE repository_id=?1",
                [repository_id],
            )
            .map_err(storage_error)?;
        for entry in entries {
            transaction
                .execute(
                    "INSERT INTO scan_cache(repository_id, path, size_bytes, mtime_ms, line_count, content_hash)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        repository_id,
                        entry.path,
                        u64_to_i64(entry.size_bytes)?,
                        entry.mtime_ms,
                        u64_to_i64(entry.line_count)?,
                        entry.content_hash,
                    ],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    /// Delete all snapshots for a repository except the current one and the
    /// `keep` most recently created completed snapshots. Returns the pruned
    /// snapshot ids. Callers must hold the index lease.
    /// Drop all but the newest `keep` snapshots, returning removed ids.
    ///
    /// # Errors
    /// Storage error on delete failure.
    pub fn prune_snapshots(&self, repository_id: &str, keep: usize) -> Result<Vec<String>> {
        let connection = self.connection.lock();
        // Query on the held guard — `current_snapshot` would re-lock this
        // non-reentrant mutex.
        let current: String = connection
            .query_row(
                "SELECT snapshot_id FROM current_snapshots WHERE repository_id=?1",
                [repository_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| CceError::Storage("repository has no current snapshot".to_owned()))?;
        let mut statement = connection
            .prepare(
                "SELECT id FROM snapshots WHERE repository_id=?1 AND id != ?2 AND complete=1
                 ORDER BY created_at DESC",
            )
            .map_err(storage_error)?;
        let completed = statement
            .query_map(params![repository_id, current], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let retained = completed.iter().take(keep).cloned().collect::<Vec<_>>();
        let mut statement = connection
            .prepare(
                "SELECT id FROM snapshots WHERE repository_id=?1 AND id != ?2
                 AND (complete=0 OR id NOT IN (SELECT value FROM json_each(?3)))",
            )
            .map_err(storage_error)?;
        let doomed = statement
            .query_map(
                params![
                    repository_id,
                    current.clone(),
                    serde_json::to_string(&retained)?
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        for snapshot_id in &doomed {
            connection
                .execute(
                    "DELETE FROM documents_fts WHERE snapshot_id=?1",
                    [snapshot_id],
                )
                .map_err(storage_error)?;
            connection
                .execute("DELETE FROM snapshots WHERE id=?1", [snapshot_id])
                .map_err(storage_error)?;
        }
        Ok(doomed)
    }

    /// Remove artifact rows and object files that no committed metadata row
    /// references. Returns the removed digests and reclaimed byte count.
    /// Delete artifact objects no longer referenced by metadata.
    ///
    /// # Errors
    /// Storage/I/O error during the sweep.
    pub fn gc_artifacts(&self) -> Result<GcReport> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT digest FROM artifacts
                 WHERE digest NOT IN (SELECT artifact_digest FROM source_files)
                   AND digest NOT IN (SELECT analysis_artifact_digest FROM source_files
                                      WHERE analysis_artifact_digest IS NOT NULL)
                   AND digest NOT IN (SELECT body_artifact_digest FROM retrieval_documents)
                   AND digest NOT IN (SELECT artifact_digest FROM view_status
                                      WHERE artifact_digest IS NOT NULL)
                   AND digest NOT IN (SELECT artifact_digest FROM trajectories)",
            )
            .map_err(storage_error)?;
        let unreferenced = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        for digest in &unreferenced {
            connection
                .execute("DELETE FROM artifacts WHERE digest=?1", [digest])
                .map_err(storage_error)?;
        }
        let mut statement = connection
            .prepare("SELECT digest FROM artifacts")
            .map_err(storage_error)?;
        let keep = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<std::collections::HashSet<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        let mut report = self.artifacts.retain(&keep)?;
        report.removed_digests = unreferenced.len();
        Ok(report)
    }

    /// Load the full view-status manifest for a snapshot.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn view_manifest(&self, repository_id: &str, snapshot_id: &str) -> Result<ViewManifest> {
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT view_kind, state, profile_hash, updated_at, capabilities_json,
                 artifact_digest, message FROM view_status WHERE repository_id=?1 AND snapshot_id=?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![repository_id, snapshot_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(storage_error)?;
        let mut views = std::collections::BTreeMap::new();
        for row in rows {
            let (kind, state, profile_hash, updated_at, capabilities, artifact_digest, message) =
                row.map_err(storage_error)?;
            views.insert(
                parse_json(&kind)?,
                ViewStatus {
                    state: parse_json::<ViewState>(&state)?,
                    snapshot_id: snapshot_id.to_owned(),
                    profile_hash,
                    updated_at: chrono::DateTime::parse_from_rfc3339(&updated_at)
                        .map_err(|error| CceError::Storage(error.to_string()))?
                        .with_timezone(&Utc),
                    capabilities: parse_json(&capabilities)?,
                    artifact_digest,
                    message,
                },
            );
        }
        Ok(ViewManifest {
            repository_id: repository_id.to_owned(),
            snapshot_id: snapshot_id.to_owned(),
            views,
        })
    }

    /// FTS5 query with code-switching cascade (see `fts_match_queries`).
    /// `filters` pushes `path:`/`lang:` constraints into the SQL join so
    /// `limit` is honored against the filtered set rather than post-trimmed.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn lexical_search(
        &self,
        snapshot_id: &str,
        query: &str,
        limit: usize,
        filters: &cce_core::QueryFilters,
    ) -> Result<Vec<LexicalHit>> {
        let terms = fts_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let floor_pairs = self.floor_evidence(snapshot_id, &terms)?;
        let mut sql = "SELECT f.document_id, f.entity_id, d.region_id, e.name, d.representation,
                 d.address_json, d.evidence_json,
                 bm25(documents_fts, 0.0, 0.0, 0.0, 3.0, 5.0, 2.0, 1.0) AS rank,
                 snippet(documents_fts, 6, '<mark>', '</mark>', ' … ', 24)
                 FROM documents_fts f JOIN retrieval_documents d
                 ON d.snapshot_id=f.snapshot_id AND d.id=f.document_id
                 JOIN entities e ON e.snapshot_id=f.snapshot_id AND e.id=f.entity_id
                 WHERE documents_fts MATCH ?1 AND f.snapshot_id=?2"
            .to_owned();
        let mut filter_params: Vec<String> = Vec::new();
        if let Some(prefix) = filters.path_prefix.as_ref() {
            filter_params.push(format!("{}%", like_escape(prefix)));
            let _ = write!(
                sql,
                " AND f.path LIKE ?{} ESCAPE '\\'",
                2 + filter_params.len()
            );
        }
        if let Some(language) = filters.language.as_ref() {
            filter_params.push(language.clone());
            let _ = write!(sql, " AND lower(e.language) = ?{}", 2 + filter_params.len());
        }
        let _ = write!(sql, " ORDER BY rank LIMIT ?{}", 3 + filter_params.len());
        let connection = self.connection.read();
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let mut hits = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut position = 0_usize;
        for match_query in fts_match_queries(&terms, &floor_pairs) {
            if hits.len() >= limit {
                break;
            }
            let limit_i64 = usize_to_i64(limit)?;
            let mut bound: Vec<Box<dyn rusqlite::types::ToSql>> =
                vec![Box::new(match_query), Box::new(snapshot_id.to_owned())];
            bound.extend(
                filter_params
                    .iter()
                    .map(|value| -> Box<dyn rusqlite::types::ToSql> { Box::new(value.clone()) }),
            );
            bound.push(Box::new(limit_i64));
            let rows = statement
                .query_map(rusqlite::params_from_iter(bound.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, f64>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                })
                .map_err(storage_error)?;
            for row in rows {
                let (
                    document_id,
                    entity_id,
                    region_id,
                    symbol_name,
                    representation,
                    address,
                    evidence,
                    _rank,
                    snippet,
                ) = row.map_err(storage_error)?;
                if !seen.insert(document_id.clone()) {
                    continue;
                }
                position += 1;
                hits.push(LexicalHit {
                    document_id,
                    entity_id,
                    region_id,
                    symbol_name,
                    representation: parse_json(&representation)?,
                    address: address.as_deref().map(parse_json).transpose()?,
                    evidence: parse_json(&evidence)?,
                    // Position within the strictness cascade; per-pass bm25
                    // ranks are not comparable across different MATCH queries.
                    score: 1.0 / (1.0 + position as f64),
                    snippet,
                });
            }
        }
        Ok(hits)
    }

    /// Cost-budgeted pair clauses for the evidence floor, measured by
    /// per-term document frequency (one indexed count each — ~1 ms per
    /// term even on large indexes).
    ///
    /// * **Ubiquitous terms are dropped first**: a term appearing in more
    ///   than `docs/UBIQUITY_DOC_FRACTION` documents (absolute floor
    ///   `UBIQUITY_DF_MIN`) carries ~zero IDF. Its pairs are the weakest
    ///   evidence *and* the most expensive — and measured, the decoy
    ///   pipeline: admitting ultra-common pairs moved `decoy_hit_rate@20`
    ///   from .007 to .103.
    /// * **Floor pairs** — every pair of the remaining live terms,
    ///   ordered by clause cost (`df_a + df_b` posting-merge cost),
    ///   capped at `PAIR_CLAUSES_MAX`. Rare and mid-frequency pairs are
    ///   informative *and* cheap, so the cap spends on informative
    ///   combinations first and drops exactly the expensive tail.
    ///
    /// Terms absent from the corpus are dead pair clauses (an AND with a
    /// zero-document term never matches) and never appear. Fully
    /// deterministic: pairs by `(cost, left, right)`.
    ///
    /// # Errors
    /// Storage error on query failure.
    fn floor_evidence(&self, snapshot_id: &str, terms: &[String]) -> Result<Vec<FloorPair>> {
        if terms.len() <= PAIR_TERMS_MAX {
            return Ok(pairs_of(terms));
        }
        let connection = self.connection.read();
        let total_docs: i64 = connection
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE snapshot_id=?1",
                [snapshot_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let mut statement = connection
            .prepare(
                "SELECT count(*) FROM documents_fts
                 WHERE documents_fts MATCH ?1 AND snapshot_id=?2",
            )
            .map_err(storage_error)?;
        let mut scored: Vec<(i64, &str)> = Vec::with_capacity(terms.len());
        for term in terms {
            // Prefix frequency: the pair stage emits `term*`, so rank by
            // what the stage will actually scan. `fts_terms` output is
            // alphanumeric/underscore only, safe unquoted.
            let df: i64 = statement
                .query_row(rusqlite::params![format!("{term}*"), snapshot_id], |row| {
                    row.get(0)
                })
                .unwrap_or(i64::MAX);
            scored.push((df, term.as_str()));
        }
        drop(statement);
        drop(connection);
        // Ubiquitous terms first — see `UBIQUITY_*` constants. Small
        // live sets then pair whole under the clause budget regardless.
        let ubiquity_cutoff = (total_docs / UBIQUITY_DOC_FRACTION).max(UBIQUITY_DF_MIN);
        let mut live: Vec<(i64, &str)> = scored
            .iter()
            .filter(|(df, _)| (1..=ubiquity_cutoff).contains(df))
            .copied()
            .collect();
        if live.len() < 2 {
            // Rescue: the floor needs two distinct terms. When ubiquity
            // and dead terms leave fewer, fall back to the least-common
            // corpus-present terms rather than dropping the floor stage.
            let mut corpus_live: Vec<(i64, &str)> = scored
                .iter()
                .filter(|(df, _)| (1..i64::MAX).contains(df))
                .copied()
                .collect();
            corpus_live.sort_unstable();
            live = corpus_live.into_iter().take(PAIR_TERMS_MAX).collect();
        }
        let mut pairs: Vec<(i64, &str, &str)> = Vec::with_capacity(live.len() * live.len() / 2);
        for (index, (left_df, left)) in live.iter().enumerate() {
            for (right_df, right) in live.get(index + 1..).unwrap_or_default() {
                pairs.push((left_df + right_df, left, right));
            }
        }
        pairs.sort_unstable();
        Ok(pairs
            .into_iter()
            .take(PAIR_CLAUSES_MAX)
            .map(|(_, left, right)| ((*left).to_owned(), (*right).to_owned()))
            .collect())
    }

    /// Exact-match document frequency of one query term — how many
    /// retrieval documents carry the term verbatim. The engine's
    /// literal-evidence veto reads this to prove a named thing absent:
    /// a term the query states verbatim that no document contains cannot
    /// be answered for, and expansion machinery must not fabricate
    /// vicinity for it.
    ///
    /// Deliberately exact, not prefix: `ViewStats` must not match
    /// `ViewStatus` — a near-miss spelling is precisely the case the
    /// veto exists to catch. The term is double-quoted, the same
    /// convention as the lexical cascade, so `::`/`.`-bearing tokens
    /// stay phrase-semantic. A term with no alphanumeric would emit an
    /// empty FTS phrase; callers filter those upstream.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn term_document_frequency(&self, snapshot_id: &str, term: &str) -> Result<i64> {
        let connection = self.connection.read();
        connection
            .query_row(
                "SELECT count(*) FROM documents_fts
                 WHERE documents_fts MATCH ?1 AND snapshot_id=?2",
                rusqlite::params![format!("\"{term}\""), snapshot_id],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    /// Document count in a snapshot — denominates rarity cutoffs.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn document_count(&self, snapshot_id: &str) -> Result<i64> {
        let connection = self.connection.read();
        connection
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE snapshot_id=?1",
                [snapshot_id],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    /// Paths of documents mentioning a term (exact quoted phrase match),
    /// cheapest first by row order — witness-check drill-down material.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn term_mention_paths(
        &self,
        snapshot_id: &str,
        term: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT path FROM documents_fts
                 WHERE documents_fts MATCH ?1 AND snapshot_id=?2 LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![format!("\"{term}\""), snapshot_id, usize_to_i64(limit)?],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(storage_error)
    }

    /// Paths of documents containing EVERY given term — the coherent-
    /// scope binding set for a claim's distinguishing vocabulary. An
    /// empty set means no artifact can witness the claim as asked.
    ///
    /// # Errors
    /// Storage error on query failure, or when `terms` is empty.
    pub fn terms_binding_paths(
        &self,
        snapshot_id: &str,
        terms: &[String],
        limit: usize,
    ) -> Result<Vec<String>> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let matcher = terms
            .iter()
            .map(|term| format!("\"{term}\""))
            .collect::<Vec<_>>()
            .join(" AND ");
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT path FROM documents_fts
                 WHERE documents_fts MATCH ?1 AND snapshot_id=?2 LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![matcher, snapshot_id, usize_to_i64(limit)?],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(storage_error)
    }

    /// Conjunction bindings with entity granularity: same match as
    /// [`terms_binding_paths`](Self::terms_binding_paths) but each row
    /// names the entity owning the matched document, so callers can tell
    /// "all terms inside one symbol" from "same file, unrelated spans".
    ///
    /// # Errors
    /// Storage error on query failure, or when `terms` is empty.
    pub fn terms_binding_scopes(
        &self,
        snapshot_id: &str,
        terms: &[String],
        limit: usize,
    ) -> Result<Vec<TermBinding>> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let matcher = terms
            .iter()
            .map(|term| format!("\"{term}\""))
            .collect::<Vec<_>>()
            .join(" AND ");
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT f.path, e.name, e.kind FROM documents_fts f
                 LEFT JOIN entities e
                   ON e.id = f.entity_id AND e.snapshot_id = f.snapshot_id
                 WHERE documents_fts MATCH ?1 AND f.snapshot_id=?2 LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![matcher, snapshot_id, usize_to_i64(limit)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        rows.map(|row| {
            let (path, entity_name, kind_json) = row.map_err(storage_error)?;
            let entity_kind = kind_json
                .as_deref()
                .map(serde_json::from_str::<cce_core::EntityKind>)
                .transpose()?;
            Ok(TermBinding {
                path,
                entity_name,
                entity_kind,
            })
        })
        .collect()
    }

    /// Which of `document_ids` contain `term` — per-hit coverage checks
    /// for claim vocabulary.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn documents_matching_term(
        &self,
        snapshot_id: &str,
        term: &str,
        document_ids: &[String],
    ) -> Result<Vec<String>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = document_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT document_id FROM documents_fts
             WHERE documents_fts MATCH ?1 AND snapshot_id=?2 AND document_id IN ({placeholders})"
        );
        let connection = self.connection.read();
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let parameters = rusqlite::params_from_iter(
            std::iter::once(format!("\"{term}\""))
                .chain(std::iter::once(snapshot_id.to_owned()))
                .chain(document_ids.iter().cloned()),
        );
        let rows = statement
            .query_map(parameters, |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(storage_error)
    }

    /// Commit-class document ids (commit summary / commit diff) whose
    /// FTS row matches `term` — the history-claim witness surface:
    /// whether the term appears in the recorded change history at all.
    /// Raw patch payloads in the artifact store are authoritative for
    /// `type:diff`; this surface reports the indexed projection.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn history_documents_matching_term(
        &self,
        snapshot_id: &str,
        term: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT f.document_id FROM documents_fts f
                 JOIN retrieval_documents d ON d.id = f.document_id
                 WHERE documents_fts MATCH ?1 AND f.snapshot_id=?2
                   AND d.representation IN (?3, ?4)
                 LIMIT ?5",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    format!("\"{term}\""),
                    snapshot_id,
                    json(&RetrievalRepresentation::CommitSummary)?,
                    json(&RetrievalRepresentation::CommitDiff)?,
                    usize_to_i64(limit)?,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(storage_error)
    }

    /// Entities whose name or qualified name contains `term` — a
    /// case-insensitive LIKE superset callers refine by identifier-part
    /// boundaries. This is the definition-tier candidate set for
    /// witness checks: mention is not definition.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn entity_name_candidates(
        &self,
        snapshot_id: &str,
        term: &str,
        limit: usize,
    ) -> Result<Vec<CodeEntity>> {
        // Match on the folded column — identifiers fold to their
        // alphanumeric stream, so `viewstatus` must reach
        // `view_status.rs`, `WebSocket`, and `a::view::Status` alike.
        // The term folds the same way before patterning.
        let folded: String = term
            .chars()
            .filter(|character| character.is_alphanumeric())
            .collect();
        let escaped = folded.replace('\\', "\\\\").replace('%', "\\%");
        let pattern = format!("%{escaped}%");
        let connection = self.connection.read();

        // The trigram index narrows the verification set: an entity
        // matching the folded pattern must carry EVERY trigram of the
        // term. Snapshots committed before the index existed (and terms
        // shorter than a trigram) have nothing to intersect — those
        // fall back to the folded-LIKE scan below.
        let trigrams = name_trigrams(term);
        let index_rows: i64 = if trigrams.is_empty() {
            0
        } else {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM entity_name_trigrams WHERE snapshot_id=?1",
                    params![snapshot_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?
        };
        if index_rows > 0 {
            let placeholders = trigrams.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT entity_id FROM entity_name_trigrams
                 WHERE snapshot_id=? AND trigram IN ({placeholders})
                 GROUP BY entity_id
                 HAVING COUNT(DISTINCT trigram)=? LIMIT ?"
            );
            let mut parameters: Vec<rusqlite::types::Value> =
                Vec::with_capacity(trigrams.len() + 3);
            parameters.push(snapshot_id.to_owned().into());
            parameters.extend(
                trigrams
                    .iter()
                    .map(|trigram| rusqlite::types::Value::Text(trigram.clone())),
            );
            parameters.push(usize_to_i64(trigrams.len())?.into());
            parameters.push(usize_to_i64(limit.saturating_mul(4).max(64))?.into());
            let mut statement = connection.prepare(&sql).map_err(storage_error)?;
            let candidates = statement
                .query_map(rusqlite::params_from_iter(parameters), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<String>, _>>()
                .map_err(storage_error)?;
            drop(statement);
            if !candidates.is_empty() {
                let id_placeholders = candidates.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let verify_sql = format!(
                    "SELECT id, kind, name, qualified_name, signature, language, region_id,
                     address_json, capabilities_json, attributes_json FROM entities
                     WHERE snapshot_id=? AND id IN ({id_placeholders}) AND (
                       REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(name,'_',''),'-',''),'.',''),'/',''),':','') LIKE ? ESCAPE '\\'
                       OR REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(qualified_name,'_',''),'-',''),'.',''),'/',''),':','') LIKE ? ESCAPE '\\'
                     ) LIMIT ?"
                );
                let mut parameters: Vec<rusqlite::types::Value> =
                    Vec::with_capacity(candidates.len() + 4);
                parameters.push(snapshot_id.to_owned().into());
                parameters.extend(
                    candidates
                        .iter()
                        .map(|id| rusqlite::types::Value::Text(id.clone())),
                );
                parameters.push(pattern.clone().into());
                parameters.push(pattern.into());
                parameters.push(usize_to_i64(limit)?.into());
                let mut statement = connection.prepare(&verify_sql).map_err(storage_error)?;
                let rows = statement
                    .query_map(rusqlite::params_from_iter(parameters), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, String>(9)?,
                        ))
                    })
                    .map_err(storage_error)?;
                return rows
                    .map(|row| entity_from_cols(row.map_err(storage_error)?))
                    .collect();
            }
            // The index carries every entity of this snapshot; an empty
            // intersection is a true miss, not an unindexed snapshot.
            return Ok(Vec::new());
        }

        let mut statement = connection
            .prepare(
                "SELECT id, kind, name, qualified_name, signature, language, region_id,
                 address_json, capabilities_json, attributes_json FROM entities
                 WHERE snapshot_id=?1 AND (
                   REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(name,'_',''),'-',''),'.',''),'/',''),':','') LIKE ?2 ESCAPE '\\'
                   OR REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(qualified_name,'_',''),'-',''),'.',''),'/',''),':','') LIKE ?2 ESCAPE '\\'
                 ) LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![snapshot_id, pattern, usize_to_i64(limit)?], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })
            .map_err(storage_error)?;
        rows.map(|row| entity_from_cols(row.map_err(storage_error)?))
            .collect()
    }

    /// Exact-name entity lookup, optionally filtered to a path prefix.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn entity_by_name(
        &self,
        snapshot_id: &str,
        name: &str,
        limit: usize,
    ) -> Result<Vec<CodeEntity>> {
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT id, kind, name, qualified_name, signature, language, region_id,
                 address_json, capabilities_json, attributes_json FROM entities
                 WHERE snapshot_id=?1 AND (name=?2 OR qualified_name=?2)
                 ORDER BY CASE WHEN name=?2 THEN 0 ELSE 1 END LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![snapshot_id, name, usize_to_i64(limit)?], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })
            .map_err(storage_error)?;
        rows.map(|row| entity_from_cols(row.map_err(storage_error)?))
            .collect()
    }

    /// All entities of one kind in a snapshot — e.g. `Package` nodes for the
    /// architecture map.
    /// List entities of a kind within a snapshot.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn entities_by_kind(
        &self,
        snapshot_id: &str,
        kind: &cce_core::EntityKind,
    ) -> Result<Vec<CodeEntity>> {
        let kind_json = serde_json::to_string(kind)?;
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT id, kind, name, qualified_name, signature, language, region_id,
                 address_json, capabilities_json, attributes_json FROM entities
                 WHERE snapshot_id=?1 AND kind=?2 ORDER BY name",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![snapshot_id, kind_json], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })
            .map_err(storage_error)?;
        rows.map(|row| entity_from_cols(row.map_err(storage_error)?))
            .collect()
    }

    /// Fetch a single entity by id.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn entity_by_id(&self, snapshot_id: &str, id: &str) -> Result<Option<CodeEntity>> {
        let connection = self.connection.read();
        let row = connection
            .query_row(
                "SELECT id, kind, name, qualified_name, signature, language, region_id,
                 address_json, capabilities_json, attributes_json FROM entities
                 WHERE snapshot_id=?1 AND id=?2",
                params![snapshot_id, id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        row.map(entity_from_cols).transpose()
    }

    /// Entities by id, batched: one `IN` query per chunk instead of a
    /// point lookup per id. Retrieval passes that fan out over graph
    /// neighbors resolve hundreds of entities — round-tripping each one
    /// dominates the join's latency.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn entities_by_ids(&self, snapshot_id: &str, ids: &[String]) -> Result<Vec<CodeEntity>> {
        const SQLITE_VARIABLE_LIMIT: usize = 900;
        let connection = self.connection.read();
        let mut entities = Vec::new();
        for chunk in ids.chunks(SQLITE_VARIABLE_LIMIT) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT id, kind, name, qualified_name, signature, language, region_id,
                 address_json, capabilities_json, attributes_json FROM entities
                 WHERE snapshot_id=?1 AND id IN ({placeholders})"
            );
            let mut statement = connection.prepare(&sql).map_err(storage_error)?;
            let mut params: Vec<rusqlite::types::Value> = Vec::with_capacity(chunk.len() + 1);
            params.push(snapshot_id.to_owned().into());
            params.extend(chunk.iter().map(|id| id.clone().into()));
            let rows = statement
                .query_map(rusqlite::params_from_iter(params), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                })
                .map_err(storage_error)?;
            for row in rows {
                entities.push(entity_from_cols(row.map_err(storage_error)?)?);
            }
        }
        Ok(entities)
    }

    /// Regions of one kind for a snapshot — the canonical code-range join
    /// target shared by documents, entities, and citations.
    /// All canonical regions in one file, ordered by position.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn regions_for_path(&self, snapshot_id: &str, path: &str) -> Result<Vec<CodeRegion>> {
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT id, path, kind, language, symbol_name, symbol_kind, qualified_name,
                 parent_region_id, start_byte, end_byte, start_line, end_line
                 FROM regions WHERE snapshot_id=?1 AND path=?2
                 ORDER BY start_byte, end_byte",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![snapshot_id, path], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            })
            .map_err(storage_error)?;
        let mut regions = Vec::new();
        for row in rows {
            let (
                id,
                path,
                kind,
                language,
                symbol_name,
                symbol_kind,
                qualified_name,
                parent_region_id,
                start_byte,
                end_byte,
                start_line,
                end_line,
            ) = row.map_err(storage_error)?;
            regions.push(CodeRegion {
                id,
                snapshot_id: snapshot_id.to_owned(),
                path,
                kind: parse_json(&kind)?,
                language,
                symbol_name,
                symbol_kind: symbol_kind.as_deref().map(parse_json).transpose()?,
                qualified_name,
                parent_region_id,
                start_byte: u64::try_from(start_byte).unwrap_or(0),
                end_byte: u64::try_from(end_byte).unwrap_or(0),
                start_line: u32::try_from(start_line).unwrap_or(0),
                end_line: u32::try_from(end_line).unwrap_or(0),
            });
        }
        Ok(regions)
    }

    /// Edges touching an entity, in the requested direction.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn relations_for_entity(
        &self,
        snapshot_id: &str,
        entity_id: &str,
        direction: RelationDirection,
        limit: usize,
    ) -> Result<Vec<Relation>> {
        let predicate = match direction {
            RelationDirection::Outgoing => "source_entity_id=?2",
            RelationDirection::Incoming => "target_entity_id=?2",
            RelationDirection::Both => "(source_entity_id=?2 OR target_entity_id=?2)",
        };
        let sql = format!(
            "SELECT id, source_entity_id, target_entity_id, kind, origin, confidence,
             extractor, evidence_json, attributes_json FROM relations
             WHERE snapshot_id=?1 AND {predicate} ORDER BY confidence DESC LIMIT ?3"
        );
        let connection = self.connection.read();
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![snapshot_id, entity_id, usize_to_i64(limit)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        let mut relations = Vec::new();
        for row in rows {
            relations.push(relation_from_cols(
                row.map_err(storage_error)?,
                snapshot_id,
            )?);
        }
        Ok(relations)
    }

    /// All relations of one kind in a snapshot — e.g. `BuildDependsOn` for
    /// the package architecture map.
    /// All edges of a kind within a snapshot.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn relations_by_kind(
        &self,
        snapshot_id: &str,
        kind: &cce_core::RelationKind,
        limit: usize,
    ) -> Result<Vec<Relation>> {
        let kind_json = serde_json::to_string(kind)?;
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT id, source_entity_id, target_entity_id, kind, origin, confidence,
                 extractor, evidence_json, attributes_json FROM relations
                 WHERE snapshot_id=?1 AND kind=?2 ORDER BY confidence DESC LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![snapshot_id, kind_json, usize_to_i64(limit)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        let mut relations = Vec::new();
        for row in rows {
            relations.push(relation_from_cols(
                row.map_err(storage_error)?,
                snapshot_id,
            )?);
        }
        Ok(relations)
    }

    /// Total edge count touching an entity — used to penalize hub nodes
    /// during graph expansion.
    /// Total inbound+outbound edge count for an entity.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn entity_relation_degree(&self, snapshot_id: &str, entity_id: &str) -> Result<usize> {
        self.connection
            .read()
            .query_row(
                "SELECT COUNT(*) FROM relations
                 WHERE snapshot_id=?1 AND (source_entity_id=?2 OR target_entity_id=?2)",
                params![snapshot_id, entity_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)
            .map(|count| usize::try_from(count).unwrap_or(0))
    }

    /// Per-file rows for post-commit view repair. The full artifact record
    /// is not joined — repair only needs the analysis digest to reload
    /// cached parse results.
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn source_files_for_snapshot(&self, snapshot_id: &str) -> Result<Vec<SourceFileRow>> {
        let connection = self.connection.read();
        let mut statement = connection
            .prepare(
                "SELECT path, language, analysis_artifact_digest FROM source_files
                 WHERE snapshot_id=?1 ORDER BY path",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([snapshot_id], |row| {
                Ok(SourceFileRow {
                    path: row.get::<_, String>(0)?,
                    language: row.get::<_, Option<String>>(1)?,
                    analysis_artifact_digest: row.get::<_, Option<String>>(2)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    /// Count relations of one kind produced by a given origin — used to
    /// recompute provider-derived view statuses during post-commit repair.
    ///
    /// # Errors
    /// Storage or serialization error on failure.
    pub fn count_relations(
        &self,
        snapshot_id: &str,
        kind: &cce_core::RelationKind,
        origin: &cce_core::RelationOrigin,
    ) -> Result<usize> {
        self.connection
            .read()
            .query_row(
                "SELECT COUNT(*) FROM relations
                 WHERE snapshot_id=?1 AND kind=?2 AND origin=?3",
                params![
                    snapshot_id,
                    serde_json::to_string(kind)?,
                    serde_json::to_string(origin)?
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)
            .map(|count| usize::try_from(count).unwrap_or(0))
    }

    /// Count retrieval documents of one representation — used to recompute
    /// history coverage during post-commit repair.
    ///
    /// # Errors
    /// Storage or serialization error on failure.
    pub fn count_documents(
        &self,
        snapshot_id: &str,
        representation: &RetrievalRepresentation,
    ) -> Result<usize> {
        self.connection
            .read()
            .query_row(
                "SELECT COUNT(*) FROM retrieval_documents
                 WHERE snapshot_id=?1 AND representation=?2",
                params![snapshot_id, json(representation)?],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)
            .map(|count| usize::try_from(count).unwrap_or(0))
    }

    /// Read the source text at a canonical address (with integrity check).
    ///
    /// # Errors
    /// `ArtifactCorrupt` on digest mismatch; storage error otherwise.
    pub fn source_text(&self, address: &SourceAddress) -> Result<String> {
        let digest = self
            .connection
            .lock()
            .query_row(
                "SELECT artifact_digest FROM source_files WHERE snapshot_id=?1 AND path=?2",
                params![address.snapshot_id, address.path],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| {
                CceError::ArtifactCorrupt(format!("{}:{}", address.snapshot_id, address.path))
            })?;
        let bytes = self.artifacts.read(&digest)?;
        let start = usize::try_from(address.start_byte)
            .map_err(|_| CceError::ArtifactCorrupt(digest.clone()))?;
        let end = usize::try_from(address.end_byte)
            .map_err(|_| CceError::ArtifactCorrupt(digest.clone()))?;
        let slice = bytes
            .get(start..end)
            .ok_or_else(|| CceError::ArtifactCorrupt(digest.clone()))?;
        String::from_utf8(slice.to_vec()).map_err(|_| CceError::ArtifactCorrupt(digest))
    }

    /// Whole-file bytes for a `(snapshot, path)` pair — the same artifact
    /// digest `source_text` slices into.
    ///
    /// # Errors
    /// Artifact-corrupt when the path was never indexed on that snapshot;
    /// storage error when the artifact itself is missing or unreadable.
    pub fn source_file_bytes(&self, snapshot_id: &str, path: &str) -> Result<Vec<u8>> {
        let digest = self
            .connection
            .lock()
            .query_row(
                "SELECT artifact_digest FROM source_files WHERE snapshot_id=?1 AND path=?2",
                params![snapshot_id, path],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| CceError::ArtifactCorrupt(format!("{snapshot_id}:{path}")))?;
        self.artifacts.read(&digest)
    }

    /// Materialize all documents for a snapshot (text included).
    ///
    /// # Errors
    /// Storage error on query failure.
    pub fn documents_for_snapshot(&self, snapshot_id: &str) -> Result<Vec<DocumentContent>> {
        let rows = {
            let connection = self.connection.read();
            let mut statement = connection
                .prepare(
                    "SELECT id, entity_id, region_id, representation, address_json,
                     body_artifact_digest, evidence_json
                     FROM retrieval_documents WHERE snapshot_id=?1 ORDER BY id",
                )
                .map_err(storage_error)?;
            statement
                .query_map([snapshot_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        rows.into_iter()
            .map(
                |(
                    document_id,
                    entity_id,
                    region_id,
                    representation,
                    address_json,
                    digest,
                    evidence_json,
                )| {
                    let address: Option<SourceAddress> =
                        address_json.as_deref().map(parse_json).transpose()?;
                    let text = if let Some(address) = &address {
                        self.source_text(address)?
                    } else {
                        let bytes = self.artifacts.read(&digest)?;
                        String::from_utf8(bytes)
                            .map_err(|_| CceError::ArtifactCorrupt(digest.clone()))?
                    };
                    Ok(DocumentContent {
                        document_id,
                        entity_id,
                        region_id,
                        representation: parse_json(&representation)?,
                        address,
                        evidence: parse_json(&evidence_json)?,
                        text,
                    })
                },
            )
            .collect()
    }
}

/// Folded alphanumeric trigrams over a name — the index units of the
/// entity-name inverted index. `view_status`, `ViewStatus`, and
/// `a::view::Status` all fold to the same lowercase stream before
/// windowing, matching `entity_name_candidates`' fold semantics.
fn name_trigrams(name: &str) -> Vec<String> {
    let folded: Vec<char> = name
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let mut trigrams = std::collections::BTreeSet::new();
    for window in folded.windows(3) {
        trigrams.insert(window.iter().collect::<String>());
    }
    trigrams.into_iter().collect()
}

fn insert_artifact(transaction: &Transaction<'_>, artifact: &ArtifactRecord) -> Result<()> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO artifacts(digest, kind, size_bytes, relative_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                artifact.digest,
                artifact.kind.to_string(),
                u64_to_i64(artifact.size_bytes)?,
                artifact.relative_path,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(Into::into)
}

fn optional_json<T: Serialize>(value: Option<&T>) -> Result<Option<String>> {
    value.map(json).transpose()
}

fn parse_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(Into::into)
}

#[allow(clippy::type_complexity)]
type EntityCols = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn entity_from_cols(
    (
        id,
        kind,
        name,
        qualified_name,
        signature,
        language,
        region_id,
        address,
        capabilities,
        attributes,
    ): EntityCols,
) -> Result<CodeEntity> {
    Ok(CodeEntity {
        id,
        kind: parse_json(&kind)?,
        name,
        qualified_name,
        signature,
        language,
        region_id,
        address: address.as_deref().map(parse_json).transpose()?,
        capabilities: parse_json(&capabilities)?,
        attributes: parse_json(&attributes)?,
    })
}

#[allow(clippy::type_complexity)]
type RelationCols = (
    String,
    String,
    String,
    String,
    String,
    f64,
    String,
    String,
    String,
);

fn relation_from_cols(
    (
        id,
        source_entity_id,
        target_entity_id,
        kind,
        origin,
        confidence,
        extractor,
        evidence,
        attributes,
    ): RelationCols,
    snapshot_id: &str,
) -> Result<Relation> {
    Ok(Relation {
        id,
        source_entity_id,
        target_entity_id,
        kind: parse_json(&kind)?,
        origin: parse_json(&origin)?,
        confidence: confidence as f32,
        snapshot_id: snapshot_id.to_owned(),
        extractor,
        evidence: parse_json(&evidence)?,
        attributes: parse_json(&attributes)?,
    })
}

fn storage_error(error: rusqlite::Error) -> CceError {
    let recovery = matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
    .then_some(
        "; run `cce doctor`, then `cce rebuild --confirm` to quarantine and reconstruct metadata",
    );
    CceError::Storage(format!(
        "{}{recovery}",
        error,
        recovery = recovery.unwrap_or_default()
    ))
}

fn bound_health_message(message: String) -> String {
    const MAXIMUM_CHARS: usize = 4_000;
    if message.chars().count() <= MAXIMUM_CHARS {
        message
    } else {
        format!(
            "{}… (truncated)",
            message.chars().take(MAXIMUM_CHARS).collect::<String>()
        )
    }
}

fn u64_to_i64(value: u64) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| CceError::Storage(format!("value {value} exceeds SQLite INTEGER")))
}

fn usize_to_i64(value: usize) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| CceError::Storage(format!("value {value} exceeds SQLite INTEGER")))
}

/// Term budget for the pairwise evidence floor in `fts_match_queries`.
/// The floor ORs every pairwise AND — C(n,2) clauses — so issue-length
/// queries (32 terms → 496 clauses, ~15 s per pass on a mid-size index)
/// cannot pair every term: above this count `floor_evidence` measures
/// per-term document frequency, drops ubiquitous terms (see `UBIQUITY_*`),
/// and emits only the cheapest remaining pairs.
const PAIR_TERMS_MAX: usize = 12;

/// Clause budget for the pairwise evidence floor in `fts_match_queries`.
/// The floor ORs every pairwise AND — C(n,2) clauses — so issue-length
/// queries (32 terms → 496 clauses, ~15 s per pass on a mid-size index)
/// emit only the cheapest pairs by document-frequency cost. The tight
/// bound is load-bearing twice over: it caps posting-merge work *and*
/// concentrates the floor on the rarest evidence, keeping the stage's
/// internal `bm25` ranking clean — measured: widening it admitted
/// mid-frequency pairs that diluted rare-evidence documents out of the
/// top-`limit` window (recall@50 .617 → .593, query time 2.8 s → 6.1 s).
const PAIR_CLAUSES_MAX: usize = 150;

/// Ubiquitous-term cutoff for the evidence floor: a term appearing in
/// more than 1/8 of the corpus carries ~zero IDF, so its pairs are both
/// the weakest evidence and the most expensive to evaluate — and
/// measured: they are the decoy pipeline (`decoy_hit_rate@20` .007 → .103
/// when ultra-common pairs were admitted). The absolute floor keeps small
/// corpora (where nothing is meaningfully ubiquitous) fully pairable.
const UBIQUITY_DOC_FRACTION: i64 = 8;
/// Minimum document count for the ubiquity cutoff — see above.
const UBIQUITY_DF_MIN: i64 = 200;

fn fts_terms(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| !term.is_empty())
        .filter(|term| !is_query_stopword(term))
        // Distinct content terms only: a repeated term must not count twice
        // toward the evidence floor in `fts_match_queries`.
        .filter(|term| seen.insert(term.to_ascii_lowercase()))
        .take(32)
        .map(|term| term.replace('"', "\"\""))
        .collect()
}

/// Every unordered pair over a term list — the short-query floor where the
/// clause count is trivially bounded (C(12,2) = 66 at most).
fn pairs_of(terms: &[String]) -> Vec<FloorPair> {
    let mut pairs = Vec::with_capacity(terms.len() * terms.len() / 2);
    for (index, left) in terms.iter().enumerate() {
        for right in terms.get(index + 1..).unwrap_or_default() {
            pairs.push((left.clone(), right.clone()));
        }
    }
    pairs
}

/// Strictness cascade for one lexical query: exact AND, prefix AND, then a
/// prefix fallback with an evidence floor. Prefixes keep partial identifier
/// matches ("fresh" → "freshness") reachable while exact AND still wins the
/// top ranks.
///
/// The fallback stage used to be a plain prefix OR, which lets a document
/// matching ANY single query term count as evidence — so a zero-evidence
/// query whose only hit is a coincidental prefix ("stored" → "store") still
/// surfaced noise. FTS5 has no "at least k of" operator, so the floor is
/// expressed at the MATCH level as the OR of every pairwise AND: a fallback
/// hit must cover at least two distinct query terms. With fewer than three
/// terms the prefix-AND stage already enforces the floor (a two-term query
/// requires both terms, a one-term query its only term), so no third stage
/// is emitted. Long queries pass precomputed `floor_pairs` — the cheapest
/// `PAIR_CLAUSES_MAX` clauses over non-ubiquitous corpus-present terms
/// (see `floor_evidence`); short queries pass `pairs_of(terms)`, so the
/// cascade is unchanged. The floor pairs run as ONE stage so `bm25`
/// orders admitted documents by match quality rather than by which
/// sub-band reached them.
///
/// Code-switched queries (CJK runs alongside Latin words) get extra stages
/// between the AND stages and the pairwise floor. unicode61 indexes a CJK
/// run as one monolithic token, so a CJK term can only ever match documents
/// carrying the same script — requiring it vetoes every Latin-script source
/// document, which is where mixed-script answers almost always live. The
/// Latin side of a code-switched query is a deliberate anchor (the writer
/// chose not to translate "stale"), so it earns a single-term tail that a
/// homogeneous query does not get.
fn fts_match_queries(terms: &[String], floor_pairs: &[FloorPair]) -> Vec<String> {
    let quoted = |term: &str| format!("\"{term}\"");
    let exact_and = |terms: &[String]| {
        terms
            .iter()
            .map(|term| quoted(term))
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let prefix_and = |terms: &[String]| {
        terms
            .iter()
            .map(|term| format!("{}*", quoted(term)))
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let mut queries = vec![exact_and(terms), prefix_and(terms)];
    let latin: Vec<String> = terms
        .iter()
        .filter(|term| !term.chars().any(has_cjk))
        .cloned()
        .collect();
    let code_switched = !latin.is_empty() && latin.len() < terms.len();
    if code_switched {
        queries.push(exact_and(&latin));
        queries.push(prefix_and(&latin));
    }
    let pair_stage = |pairs: &[(String, String)]| {
        pairs
            .iter()
            .map(|(left, right)| format!("({}* AND {}*)", quoted(left), quoted(right)))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    // Short queries need ≥3 terms for the floor (fewer and the prefix-AND
    // stage already enforces it). Long queries emit whatever the budgeted
    // pair list holds — even a single live pair beats the always-empty
    // full-AND stages.
    if floor_pairs.len() >= 3 || (!floor_pairs.is_empty() && terms.len() > PAIR_TERMS_MAX) {
        queries.push(pair_stage(floor_pairs));
    }
    if code_switched {
        queries.push(
            latin
                .iter()
                .map(|term| format!("{}*", quoted(term)))
                .collect::<Vec<_>>()
                .join(" OR "),
        );
    }
    queries
}

/// Escapes `LIKE` metacharacters so `path:` filters stay literal prefixes
/// under `ESCAPE '\'`.
fn like_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Expands an identifier for the FTS `name` column: raw spelling plus its
/// split-word and folded forms, so `set view status`, `setViewStatus`,
/// `setviewstatus`, and `set_view_status` all reach the same name.
fn identifier_fts_forms(name: &str) -> String {
    let mut forms = name.to_owned();
    let splits = cce_core::split_identifier_terms(name);
    if !splits.is_empty() {
        let _ = write!(forms, " {}", splits.join(" "));
    }
    let folded = cce_core::folded_identifier(name);
    if folded != name.to_ascii_lowercase() {
        let _ = write!(forms, " {folded}");
    }
    forms
}

fn is_query_stopword(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "a" | "an"
            | "and"
            | "are"
            | "does"
            | "how"
            | "in"
            | "is"
            | "located"
            | "of"
            | "on"
            | "the"
            | "to"
            | "what"
            | "where"
            | "which"
            | "who"
            | "why"
            | "怎么"
            | "如何"
            | "什么"
            | "哪里"
            | "在哪"
            | "为什么"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_migrates_database() {
        let directory = tempfile::tempdir().expect("temporary directory");
        MetadataStore::open(directory.path()).expect("metadata store");
        assert!(directory.path().join("metadata.sqlite").is_file());
    }

    #[test]
    fn fts_terms_strip_stopwords_and_queries_cascade() {
        let terms = fts_terms("Where is the snapshot freshness decided?");
        assert_eq!(terms, ["snapshot", "freshness", "decided"]);
        let queries = fts_match_queries(&terms, &pairs_of(&terms));
        assert_eq!(
            queries,
            [
                "\"snapshot\" AND \"freshness\" AND \"decided\"",
                "\"snapshot\"* AND \"freshness\"* AND \"decided\"*",
                "(\"snapshot\"* AND \"freshness\"*) OR (\"snapshot\"* AND \"decided\"*) OR (\"freshness\"* AND \"decided\"*)",
            ]
        );
    }

    #[test]
    fn fts_fallback_requires_two_distinct_terms() {
        // One- and two-term queries stop after the prefix-AND stage: it
        // already requires every term, which is the tightest possible floor.
        // A repeated term is deduplicated so it cannot satisfy the two-term
        // floor by matching the same word twice.
        for terms in [fts_terms("freshness"), fts_terms("freshness freshness")] {
            assert_eq!(
                fts_match_queries(&terms, &pairs_of(&terms)),
                ["\"freshness\"", "\"freshness\"*"]
            );
        }
        let terms = fts_terms("snapshot freshness");
        assert_eq!(
            fts_match_queries(&terms, &pairs_of(&terms)),
            [
                "\"snapshot\" AND \"freshness\"",
                "\"snapshot\"* AND \"freshness\"*",
            ]
        );
    }

    #[test]
    fn fts_code_switched_query_gets_latin_fallback_stages() {
        // CJK runs index as monolithic unicode61 tokens; requiring them in
        // every stage vetoes all Latin-script source. The Latin anchors get
        // their own cascade and a single-term tail.
        let terms = fts_terms("search 偶发返回陈旧结果，哪里把过期视图标记为 stale");
        assert_eq!(
            terms,
            [
                "search",
                "偶发返回陈旧结果",
                "哪里把过期视图标记为",
                "stale"
            ]
        );
        assert_eq!(
            fts_match_queries(&terms, &pairs_of(&terms)),
            [
                "\"search\" AND \"偶发返回陈旧结果\" AND \"哪里把过期视图标记为\" AND \"stale\"",
                "\"search\"* AND \"偶发返回陈旧结果\"* AND \"哪里把过期视图标记为\"* AND \"stale\"*",
                "\"search\" AND \"stale\"",
                "\"search\"* AND \"stale\"*",
                "(\"search\"* AND \"偶发返回陈旧结果\"*) OR (\"search\"* AND \"哪里把过期视图标记为\"*) OR (\"search\"* AND \"stale\"*) OR (\"偶发返回陈旧结果\"* AND \"哪里把过期视图标记为\"*) OR (\"偶发返回陈旧结果\"* AND \"stale\"*) OR (\"哪里把过期视图标记为\"* AND \"stale\"*)",
                "\"search\"* OR \"stale\"*",
            ]
        );
    }

    #[test]
    fn fts_pure_cjk_query_keeps_strict_cascade() {
        // No Latin anchors: nothing extra to fall back on. A single
        // monolithic CJK term still gets exact and prefix stages.
        let terms = fts_terms("多条检索通道的命中是怎么合并成一个排序的");
        assert_eq!(
            fts_match_queries(&terms, &pairs_of(&terms)),
            [
                "\"多条检索通道的命中是怎么合并成一个排序的\"",
                "\"多条检索通道的命中是怎么合并成一个排序的\"*",
            ]
        );
    }

    #[test]
    fn fts_diacritic_words_do_not_trigger_code_switching() {
        // unicode61 strips diacritics on both sides, so a Latin-script word
        // like "café" can still match — it is not a CJK monolith.
        let terms = fts_terms("café search resume");
        assert_eq!(
            fts_match_queries(&terms, &pairs_of(&terms)),
            [
                "\"café\" AND \"search\" AND \"resume\"",
                "\"café\"* AND \"search\"* AND \"resume\"*",
                "(\"café\"* AND \"search\"*) OR (\"café\"* AND \"resume\"*) OR (\"search\"* AND \"resume\"*)",
            ]
        );
    }

    #[test]
    fn fts_pair_floor_uses_budgeted_clauses_for_long_queries() {
        // Issue-length queries pass a cost-budgeted pair list: the strict
        // AND stages keep every term while the floor stage carries only
        // the clauses `floor_evidence` emitted — 3 pairs here, not
        // C(20,2) = 190.
        let terms: Vec<String> = (0..20).map(|index| format!("term{index:02}")).collect();
        let floor_pairs: Vec<FloorPair> = vec![
            ("term00".to_owned(), "term01".to_owned()),
            ("term00".to_owned(), "term02".to_owned()),
            ("term01".to_owned(), "term02".to_owned()),
        ];
        let queries = fts_match_queries(&terms, &floor_pairs);
        assert_eq!(queries.len(), 3);
        assert!(queries[0].contains("\"term19\""));
        assert!(queries[1].contains("\"term19\"*"));
        assert_eq!(
            queries[2],
            "(\"term00\"* AND \"term01\"*) OR (\"term00\"* AND \"term02\"*) OR (\"term01\"* AND \"term02\"*)"
        );
    }

    #[test]
    fn floor_pairs_drop_ubiquitous_terms() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("store");
        store
            .register_repository(&RepositoryIdentity {
                id: "repo_t".to_owned(),
                canonical_root: "/repo".to_owned(),
                remote: None,
            })
            .expect("repository");
        let snapshot = SnapshotIdentity {
            id: "snap_t".to_owned(),
            repository_id: "repo_t".to_owned(),
            base_revision: None,
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");

        // 215 documents: `zephyr` rarest (2 docs), `quux` (3),
        // `mid00..mid10` spanning df 4..=14, and `common`/`shared`/
        // `vocabulary` in every document — past the ubiquity cutoff of
        // max(215/8, 200) = 200. `filler*`/`absent*` terms never appear.
        let mut records = SnapshotRecords::default();
        for index in 0..215_usize {
            let mut body = String::from("common shared vocabulary");
            if index < 2 {
                body.push_str(" zephyr");
            }
            if index < 3 {
                body.push_str(" quux");
            }
            for mid in 0..11_usize {
                if index < mid + 4 {
                    let _ = write!(body, " mid{mid:02}");
                }
            }
            let entity_id = format!("entity:{index}");
            records.artifacts.push(ArtifactRecord {
                digest: format!("digest:{index}"),
                kind: crate::ArtifactKind::Source,
                size_bytes: body.len() as u64,
                relative_path: format!("artifacts/{index}"),
            });
            records.entities.push(CodeEntity {
                id: entity_id.clone(),
                kind: cce_core::EntityKind::Function,
                name: format!("f{index}"),
                qualified_name: None,
                signature: None,
                language: None,
                region_id: None,
                address: None,
                capabilities: Vec::new(),
                attributes: serde_json::Map::new(),
            });
            records.documents.push(IndexedDocument {
                document: RetrievalDocument {
                    id: format!("doc:{index}"),
                    entity_id,
                    snapshot_id: "snap_t".to_owned(),
                    representation: RetrievalRepresentation::RawCode,
                    body_artifact_digest: format!("digest:{index}"),
                    region_id: None,
                    address: None,
                    embedding_profile: None,
                    generated_by: None,
                    evidence: Vec::new(),
                    terms: Vec::new(),
                },
                path: format!("src/f{index}.rs"),
                name: format!("f{index}"),
                body,
            });
        }
        store.commit_snapshot(&snapshot, &records).expect("commit");

        // Issue-length query: 13 dead + all 14 corpus terms. `common` is
        // ubiquitous (215 > 200) and drops out; the remaining 13 live
        // terms pair whole → C(13,2) = 78 clauses ordered cheapest first.
        // Dead `filler*` terms never appear.
        let mut terms: Vec<String> = (0..13).map(|index| format!("filler{index}")).collect();
        terms.extend(["common", "zephyr", "quux"].map(str::to_owned));
        terms.extend((0..11).map(|index| format!("mid{index:02}")));
        let clauses = store.floor_evidence("snap_t", &terms).expect("floor");
        assert_eq!(clauses.len(), 78);
        assert_eq!(clauses[0], ("zephyr".to_owned(), "quux".to_owned()));
        assert!(clauses.iter().all(|(left, _)| !left.starts_with("filler")));
        assert!(
            clauses
                .iter()
                .all(|(left, right)| left != "common" && right != "common")
        );

        // Exactly two corpus terms still emit their single pair.
        let mut two_live: Vec<String> = (0..13).map(|index| format!("absent{index}")).collect();
        two_live.extend(["zephyr", "quux"].map(str::to_owned));
        let pairs = store.floor_evidence("snap_t", &two_live).expect("pairs");
        assert_eq!(pairs, [("zephyr".to_owned(), "quux".to_owned())]);

        // All-ubiquitous query: the rescue keeps the floor alive with the
        // least-common corpus terms — 3 of them → C(3,2) = 3 clauses —
        // instead of silently dropping fallback evidence.
        let mut ubiquitous: Vec<String> = (0..13).map(|index| format!("absent{index}")).collect();
        ubiquitous.extend(["common", "shared", "vocabulary"].map(str::to_owned));
        let pairs = store
            .floor_evidence("snap_t", &ubiquitous)
            .expect("ubiquitous");
        assert_eq!(pairs.len(), 3);

        // End to end: the strict AND stages die on dead terms; the single
        // live pair still surfaces the two documents carrying both rares.
        let query = format!(
            "{} zephyr quux",
            (0..13)
                .map(|index| format!("absent{index}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let hits = store
            .lexical_search("snap_t", &query, 20, &cce_core::QueryFilters::default())
            .expect("lexical");
        assert_eq!(hits.len(), 2);

        // With all 14 terms queried, `doc:12` surfaces through the pure
        // mid-band pair (mid09, mid10). `doc:13` carries only `mid10`
        // plus ubiquitous vocabulary — the decoy shape — and correctly
        // fails the two-term floor.
        let query = format!(
            "zephyr quux common {} {}",
            (0..11)
                .map(|index| format!("mid{index:02}"))
                .collect::<Vec<_>>()
                .join(" "),
            (0..5)
                .map(|index| format!("absent{index}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let hits = store
            .lexical_search("snap_t", &query, 20, &cce_core::QueryFilters::default())
            .expect("lexical");
        let ids: Vec<&str> = hits.iter().map(|hit| hit.document_id.as_str()).collect();
        assert_eq!(hits.len(), 13);
        assert!(ids.contains(&"doc:12"));
        assert!(!ids.contains(&"doc:13"));
    }

    #[test]
    fn term_document_frequency_is_exact_not_prefix() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("store");
        store
            .register_repository(&RepositoryIdentity {
                id: "repo_t".to_owned(),
                canonical_root: "/repo".to_owned(),
                remote: None,
            })
            .expect("repository");
        let snapshot = SnapshotIdentity {
            id: "snap_t".to_owned(),
            repository_id: "repo_t".to_owned(),
            base_revision: None,
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");
        let records = SnapshotRecords {
            artifacts: vec![ArtifactRecord {
                digest: "digest:0".to_owned(),
                kind: crate::ArtifactKind::Source,
                size_bytes: 40,
                relative_path: "artifacts/0".to_owned(),
            }],
            entities: vec![CodeEntity {
                id: "entity:0".to_owned(),
                kind: cce_core::EntityKind::Function,
                name: "ViewStatus".to_owned(),
                qualified_name: None,
                signature: None,
                language: None,
                region_id: None,
                address: None,
                capabilities: Vec::new(),
                attributes: serde_json::Map::new(),
            }],
            documents: vec![IndexedDocument {
                document: RetrievalDocument {
                    id: "doc:0".to_owned(),
                    entity_id: "entity:0".to_owned(),
                    snapshot_id: "snap_t".to_owned(),
                    representation: RetrievalRepresentation::RawCode,
                    body_artifact_digest: "digest:0".to_owned(),
                    region_id: None,
                    address: None,
                    embedding_profile: None,
                    generated_by: None,
                    evidence: Vec::new(),
                    terms: Vec::new(),
                },
                path: "src/view_status.rs".to_owned(),
                name: "ViewStatus".to_owned(),
                body: "pub struct ViewStatus; fn search() {}".to_owned(),
            }],
            ..SnapshotRecords::default()
        };
        store.commit_snapshot(&snapshot, &records).expect("commit");

        // Present spellings count their documents; near-miss spellings
        // are the trap shapes the abstention veto exists to prove absent
        // — a truncated variant must not ride the real prefix, and a
        // pluralized near-miss must not ride its real stem. The fakes
        // are built by mutating the real names so the trap literals
        // never appear in this file: a verbatim trap spelling in
        // committed source indexes into the corpus and falsifies the
        // "provably absent" claim every self-dogfood index relies on.
        assert_eq!(
            store
                .term_document_frequency("snap_t", "ViewStatus")
                .expect("df"),
            1
        );
        assert_eq!(
            store
                .term_document_frequency("snap_t", "viewstatus")
                .expect("df"),
            1,
            "unicode61 folding makes the check case-insensitive"
        );
        let swap_last = |name: &str| -> String {
            let mut chars: Vec<char> = name.chars().collect();
            let last = chars.len() - 1;
            chars.swap(last - 1, last);
            chars.into_iter().collect()
        };
        let pluralized = |name: &str| format!("{name}s");
        let truncated =
            |name: &str| -> String { name.chars().take(name.chars().count() - 1).collect() };
        for absent in [
            swap_last("ViewStatus"),
            pluralized("ViewStatus"),
            truncated("ViewStatus"),
            pluralized("search"),
            "nosuch".to_owned(),
        ] {
            assert_eq!(
                store
                    .term_document_frequency("snap_t", &absent)
                    .expect("df"),
                0,
                "{absent} is provably absent"
            );
        }
    }

    #[test]
    fn witness_apis_cover_mentions_binding_and_coverage() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("store");
        store
            .register_repository(&RepositoryIdentity {
                id: "repo_t".to_owned(),
                canonical_root: "/repo".to_owned(),
                remote: None,
            })
            .expect("repository");
        let snapshot = SnapshotIdentity {
            id: "snap_t".to_owned(),
            repository_id: "repo_t".to_owned(),
            base_revision: None,
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");

        let entity =
            |id: &str, kind: cce_core::EntityKind, name: &str, qualified: &str| CodeEntity {
                id: id.to_owned(),
                kind,
                name: name.to_owned(),
                qualified_name: Some(qualified.to_owned()),
                signature: None,
                language: None,
                region_id: None,
                address: None,
                capabilities: Vec::new(),
                attributes: serde_json::Map::new(),
            };
        let document = |index: usize, entity_id: &str, path: &str, name: &str, body: &str| {
            (
                IndexedDocument {
                    document: RetrievalDocument {
                        id: format!("doc:{index}"),
                        entity_id: entity_id.to_owned(),
                        snapshot_id: "snap_t".to_owned(),
                        representation: RetrievalRepresentation::RawCode,
                        body_artifact_digest: format!("digest:{index}"),
                        region_id: None,
                        address: None,
                        embedding_profile: None,
                        generated_by: None,
                        evidence: Vec::new(),
                        terms: Vec::new(),
                    },
                    path: path.to_owned(),
                    name: name.to_owned(),
                    body: body.to_owned(),
                },
                ArtifactRecord {
                    digest: format!("digest:{index}"),
                    kind: crate::ArtifactKind::Source,
                    size_bytes: body.len() as u64,
                    relative_path: format!("artifacts/{index}"),
                },
            )
        };
        let (doc0, art0) = document(
            0,
            "ent:viewstatus",
            "src/view_status.rs",
            "ViewStatus",
            "pub struct ViewStatus; staleness handling",
        );
        let (doc1, art1) = document(
            1,
            "ent:gateway",
            "src/gateway.rs",
            "gateway",
            "the gateway validates every request token",
        );
        let (doc2, art2) = document(
            2,
            "ent:plan",
            "plans/009.md",
            "plan",
            "a distributed lock design sketch",
        );
        let (doc3, art3) = document(
            3,
            "ent:locks",
            "src/lock_manager.rs",
            "lock_manager",
            "fn lock_manager acquires the lock",
        );
        let records = SnapshotRecords {
            artifacts: vec![art0, art1, art2, art3],
            entities: vec![
                entity(
                    "ent:viewstatus",
                    cce_core::EntityKind::Struct,
                    "ViewStatus",
                    "app::view::ViewStatus",
                ),
                entity(
                    "ent:locks",
                    cce_core::EntityKind::File,
                    "src/lock_manager.rs",
                    "src/lock_manager.rs",
                ),
                entity(
                    "ent:blockchain",
                    cce_core::EntityKind::File,
                    "src/blockchain.rs",
                    "src/blockchain.rs",
                ),
                entity(
                    "ent:gateway",
                    cce_core::EntityKind::File,
                    "src/gateway.rs",
                    "src/gateway.rs",
                ),
            ],
            documents: vec![doc0, doc1, doc2, doc3],
            ..SnapshotRecords::default()
        };
        store.commit_snapshot(&snapshot, &records).expect("commit");

        assert_eq!(store.document_count("snap_t").expect("count"), 4);

        // Name candidates fold identifier separators: `viewstatus`
        // reaches `ViewStatus` and `src/view_status.rs` alike. The LIKE
        // set is a recall superset — `blockchain` legitimately surfaces
        // for `lock` here; the part-boundary check lives in the caller.
        let candidates = store
            .entity_name_candidates("snap_t", "viewstatus", 8)
            .expect("candidates");
        assert!(
            candidates
                .iter()
                .any(|entity| entity.id == "ent:viewstatus"),
            "folded name match must reach the entity: {candidates:?}"
        );
        let candidates = store
            .entity_name_candidates("snap_t", "lock", 8)
            .expect("candidates");
        let ids: Vec<&str> = candidates.iter().map(|entity| entity.id.as_str()).collect();
        assert!(ids.contains(&"ent:locks"));
        assert!(
            ids.contains(&"ent:blockchain"),
            "superset over-recalls: {ids:?}"
        );
        assert!(
            store
                .entity_name_candidates("snap_t", "websocket", 8)
                .expect("candidates")
                .is_empty(),
            "a never-defined term finds no candidates"
        );
        // Terms shorter than a trigram bypass the index and take the
        // folded-LIKE scan — `lo` still reaches `lock` entities.
        let short = store
            .entity_name_candidates("snap_t", "lo", 8)
            .expect("short candidates");
        assert!(
            short.iter().any(|entity| entity.id == "ent:locks"),
            "short terms still match through the LIKE fallback: {short:?}"
        );

        // Mentions report every document carrying the term; binding
        // reports only artifacts carrying ALL of them.
        let mentions = store
            .term_mention_paths("snap_t", "lock", 8)
            .expect("mentions");
        assert_eq!(
            mentions.len(),
            2,
            "prose and code both mention: {mentions:?}"
        );
        let bound = store
            .terms_binding_paths("snap_t", &["distributed".to_owned(), "lock".to_owned()], 8)
            .expect("bound");
        assert_eq!(bound, ["plans/009.md"], "only the plan binds both");
        let bound = store
            .terms_binding_paths("snap_t", &["gateway".to_owned(), "token".to_owned()], 8)
            .expect("bound");
        assert_eq!(bound, ["src/gateway.rs"]);

        // Scope-aware bindings resolve the entity behind each match.
        let bindings = store
            .terms_binding_scopes("snap_t", &["gateway".to_owned(), "token".to_owned()], 8)
            .expect("bindings");
        assert_eq!(bindings.len(), 1, "{bindings:?}");
        assert_eq!(bindings[0].path, "src/gateway.rs");
        assert_eq!(bindings[0].entity_kind, Some(cce_core::EntityKind::File));
        let bindings = store
            .terms_binding_scopes(
                "snap_t",
                &["viewstatus".to_owned(), "staleness".to_owned()],
                8,
            )
            .expect("bindings");
        assert_eq!(bindings.len(), 1, "{bindings:?}");
        assert_eq!(
            bindings[0].entity_kind,
            Some(cce_core::EntityKind::Struct),
            "a symbol entity attests entity scope"
        );
        assert_eq!(bindings[0].entity_name.as_deref(), Some("ViewStatus"));
        // A document whose entity never resolves still reports the
        // path — the caller treats it as file-level binding.
        let bindings = store
            .terms_binding_scopes("snap_t", &["distributed".to_owned(), "lock".to_owned()], 8)
            .expect("bindings");
        assert_eq!(bindings[0].path, "plans/009.md");
        assert_eq!(bindings[0].entity_kind, None);

        // Per-hit coverage: `distributed` lives in no returned hit,
        // `lock` lives in one of them.
        let doc_ids = vec!["doc:0".to_owned(), "doc:3".to_owned()];
        assert!(
            store
                .documents_matching_term("snap_t", "distributed", &doc_ids)
                .expect("coverage")
                .is_empty()
        );
        assert_eq!(
            store
                .documents_matching_term("snap_t", "lock", &doc_ids)
                .expect("coverage"),
            ["doc:3"]
        );
    }

    #[test]
    fn history_documents_matching_term_filters_to_commit_class() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("store");
        store
            .register_repository(&RepositoryIdentity {
                id: "repo_t".to_owned(),
                canonical_root: "/repo".to_owned(),
                remote: None,
            })
            .expect("repository");
        let snapshot = SnapshotIdentity {
            id: "snap_t".to_owned(),
            repository_id: "repo_t".to_owned(),
            base_revision: None,
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");

        let document =
            |index: usize, representation: RetrievalRepresentation, path: &str, body: &str| {
                (
                    IndexedDocument {
                        document: RetrievalDocument {
                            id: format!("doc:{index}"),
                            entity_id: format!("ent:{index}"),
                            snapshot_id: "snap_t".to_owned(),
                            representation,
                            body_artifact_digest: format!("digest:{index}"),
                            region_id: None,
                            address: None,
                            embedding_profile: None,
                            generated_by: None,
                            evidence: Vec::new(),
                            terms: Vec::new(),
                        },
                        path: path.to_owned(),
                        name: path.to_owned(),
                        body: body.to_owned(),
                    },
                    ArtifactRecord {
                        digest: format!("digest:{index}"),
                        kind: crate::ArtifactKind::Source,
                        size_bytes: body.len() as u64,
                        relative_path: format!("artifacts/{index}"),
                    },
                )
            };
        // `grpc` is attested in a commit summary AND in current source —
        // only the commit-class document is a history witness.
        let (doc0, art0) = document(
            0,
            RetrievalRepresentation::CommitSummary,
            "commit:aaa111",
            "added grpc transport for the control plane",
        );
        let (doc1, art1) = document(
            1,
            RetrievalRepresentation::CommitDiff,
            "commit:bbb222",
            "src/net.rs +use tonic::transport::Channel; // grpc client",
        );
        let (doc2, art2) = document(
            2,
            RetrievalRepresentation::RawCode,
            "src/net.rs",
            "fn dial() { /* grpc endpoint */ }",
        );
        let records = SnapshotRecords {
            artifacts: vec![art0, art1, art2],
            documents: vec![doc0, doc1, doc2],
            ..SnapshotRecords::default()
        };
        store.commit_snapshot(&snapshot, &records).expect("commit");

        assert_eq!(
            store
                .history_documents_matching_term("snap_t", "grpc", 8)
                .expect("history"),
            ["doc:0", "doc:1"],
            "both commit classes witness; the RawCode mention does not"
        );
        assert!(
            store
                .history_documents_matching_term("snap_t", "tokio", 8)
                .expect("history")
                .is_empty(),
            "a term absent from commit-class documents has no history witness"
        );
        assert!(
            store
                .history_documents_matching_term("snap_t", "dial", 8)
                .expect("history")
                .is_empty(),
            "current-source-only vocabulary has no history witness"
        );
    }
}
