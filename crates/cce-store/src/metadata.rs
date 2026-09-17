use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use cce_core::{
    CceError, CodeEntity, CodeRegion, Relation, RepositoryIdentity, Result, RetrievalDocument,
    RetrievalRepresentation, SnapshotIdentity, SourceAddress, ViewKind, ViewManifest, ViewState,
    ViewStatus,
};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::{ArtifactRecord, ArtifactStore};

#[derive(Debug, Clone)]
pub struct SourceFileRecord {
    pub path: String,
    pub language: Option<String>,
    pub content_hash: String,
    pub artifact: ArtifactRecord,
    pub byte_count: u64,
    pub line_count: u64,
    pub analysis_artifact_digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IndexedDocument {
    pub document: RetrievalDocument,
    pub path: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotRecords {
    pub artifacts: Vec<ArtifactRecord>,
    pub files: Vec<SourceFileRecord>,
    pub regions: Vec<CodeRegion>,
    pub entities: Vec<CodeEntity>,
    pub relations: Vec<Relation>,
    pub documents: Vec<IndexedDocument>,
}

#[derive(Debug, Clone)]
pub struct LexicalHit {
    pub document_id: String,
    pub entity_id: String,
    pub region_id: Option<String>,
    pub symbol_name: String,
    pub representation: RetrievalRepresentation,
    pub address: Option<SourceAddress>,
    pub evidence: Vec<SourceAddress>,
    pub score: f64,
    pub snippet: String,
}

/// A file's stat fingerprint observed during a repository scan.
#[derive(Debug, Clone)]
pub struct ScanCacheEntry {
    pub path: String,
    pub size_bytes: u64,
    pub mtime_ms: i64,
    pub line_count: u64,
    pub content_hash: String,
}

/// Outcome of a garbage-collection pass over snapshots and the artifact store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcReport {
    pub pruned_snapshots: usize,
    pub removed_digests: usize,
    pub removed_orphan_files: usize,
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct DocumentContent {
    pub document_id: String,
    pub entity_id: String,
    pub region_id: Option<String>,
    pub representation: RetrievalRepresentation,
    pub address: Option<SourceAddress>,
    pub evidence: Vec<SourceAddress>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationDirection {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreHealth {
    pub sqlite_ok: bool,
    pub sqlite_messages: Vec<String>,
    pub artifacts_checked: usize,
    pub missing_artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_scan_error: Option<String>,
}

#[derive(Clone)]
pub struct MetadataStore {
    connection: Arc<Mutex<Connection>>,
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

impl MetadataStore {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self> {
        let data_root = data_root.as_ref();
        std::fs::create_dir_all(data_root).map_err(|error| CceError::io(data_root, error))?;
        let database_path = data_root.join("metadata.sqlite");
        let connection = Connection::open(&database_path)
            .map_err(|error| CceError::Storage(error.to_string()))?;
        connection
            .busy_timeout(Duration::from_secs(10))
            .map_err(|error| CceError::Storage(error.to_string()))?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;",
            )
            .map_err(|error| CceError::Storage(error.to_string()))?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version > 4 {
            return Err(CceError::UnsupportedFormat {
                found: version,
                supported: 4,
            });
        }
        if version == 0 {
            connection
                .execute_batch(include_str!("migrations/0001_initial.sql"))
                .map_err(|error| CceError::Storage(format!("migration 1 failed: {error}")))?;
        }
        if version < 2 {
            connection
                .execute_batch(include_str!("migrations/0002_parse_cache.sql"))
                .map_err(|error| CceError::Storage(format!("migration 2 failed: {error}")))?;
        }
        if version < 3 {
            connection
                .execute_batch(include_str!("migrations/0003_scan_cache.sql"))
                .map_err(|error| CceError::Storage(format!("migration 3 failed: {error}")))?;
        }
        if version < 4 {
            connection
                .execute_batch(include_str!("migrations/0004_regions.sql"))
                .map_err(|error| CceError::Storage(format!("migration 4 failed: {error}")))?;
        }
        let artifacts = ArtifactStore::open(data_root)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            artifacts,
            database_path,
        })
    }

    #[must_use]
    pub const fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

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

    pub fn health(&self) -> Result<StoreHealth> {
        let connection = self.connection.lock();
        let messages = match connection.prepare("PRAGMA quick_check") {
            Ok(mut statement) => match statement.query_map([], |row| row.get::<_, String>(0)) {
                Ok(rows) => rows
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap_or_else(|error| vec![error.to_string()]),
                Err(error) => vec![error.to_string()],
            },
            Err(error) => vec![error.to_string()],
        };
        let sqlite_ok = messages.len() == 1 && messages[0] == "ok";
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
        }

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
                        indexed.name,
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

    pub fn current_snapshot(&self, repository_id: &str) -> Result<Option<String>> {
        self.connection
            .lock()
            .query_row(
                "SELECT snapshot_id FROM current_snapshots WHERE repository_id=?1",
                [repository_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn snapshot_is_complete(&self, snapshot_id: &str) -> Result<bool> {
        self.connection
            .lock()
            .query_row(
                "SELECT complete FROM snapshots WHERE id=?1",
                [snapshot_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(storage_error)
    }

    pub fn cached_analysis_digest(
        &self,
        repository_id: &str,
        path: &str,
        content_hash: &str,
    ) -> Result<Option<String>> {
        self.connection
            .lock()
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
    pub fn scan_cache_lookup(
        &self,
        repository_id: &str,
        path: &str,
        size_bytes: u64,
        mtime_ms: i64,
    ) -> Result<Option<(String, u64)>> {
        self.connection
            .lock()
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
        drop(connection);
        let mut report = self.artifacts.retain(&keep)?;
        report.removed_digests = unreferenced.len();
        Ok(report)
    }

    pub fn view_manifest(&self, repository_id: &str, snapshot_id: &str) -> Result<ViewManifest> {
        let connection = self.connection.lock();
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

    /// FTS5 retrieval over the snapshot's documents. Runs a strictness
    /// cascade — all terms exact, then all terms as prefixes, then prefix
    /// pairs — so AND-matched documents always rank ahead of fallbacks, and
    /// a fallback hit must cover at least two distinct query terms to count
    /// as evidence. Results are deduplicated by document id and capped at
    /// `limit`.
    pub fn lexical_search(
        &self,
        snapshot_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LexicalHit>> {
        let terms = fts_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT f.document_id, f.entity_id, d.region_id, e.name, d.representation,
                 d.address_json, d.evidence_json,
                 bm25(documents_fts, 0.0, 0.0, 0.0, 3.0, 5.0, 2.0, 1.0) AS rank,
                 snippet(documents_fts, 6, '<mark>', '</mark>', ' … ', 24)
                 FROM documents_fts f JOIN retrieval_documents d
                 ON d.snapshot_id=f.snapshot_id AND d.id=f.document_id
                 JOIN entities e ON e.snapshot_id=f.snapshot_id AND e.id=f.entity_id
                 WHERE documents_fts MATCH ?1 AND f.snapshot_id=?2
                 ORDER BY rank LIMIT ?3",
            )
            .map_err(storage_error)?;
        let mut hits = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut position = 0_usize;
        for match_query in fts_match_queries(&terms) {
            if hits.len() >= limit {
                break;
            }
            let rows = statement
                .query_map(
                    params![match_query, snapshot_id, usize_to_i64(limit)?],
                    |row| {
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
                    },
                )
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

    pub fn entity_by_name(
        &self,
        snapshot_id: &str,
        name: &str,
        limit: usize,
    ) -> Result<Vec<CodeEntity>> {
        let connection = self.connection.lock();
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
    pub fn entities_by_kind(
        &self,
        snapshot_id: &str,
        kind: &cce_core::EntityKind,
    ) -> Result<Vec<CodeEntity>> {
        let kind_json = serde_json::to_string(kind)?;
        let connection = self.connection.lock();
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

    pub fn entity_by_id(&self, snapshot_id: &str, id: &str) -> Result<Option<CodeEntity>> {
        let connection = self.connection.lock();
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

    /// Regions of one kind for a snapshot — the canonical code-range join
    /// target shared by documents, entities, and citations.
    pub fn regions_for_path(&self, snapshot_id: &str, path: &str) -> Result<Vec<CodeRegion>> {
        let connection = self.connection.lock();
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
        let connection = self.connection.lock();
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
    pub fn relations_by_kind(
        &self,
        snapshot_id: &str,
        kind: &cce_core::RelationKind,
        limit: usize,
    ) -> Result<Vec<Relation>> {
        let kind_json = serde_json::to_string(kind)?;
        let connection = self.connection.lock();
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
    pub fn entity_relation_degree(&self, snapshot_id: &str, entity_id: &str) -> Result<usize> {
        self.connection
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM relations
                 WHERE snapshot_id=?1 AND (source_entity_id=?2 OR target_entity_id=?2)",
                params![snapshot_id, entity_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)
            .map(|count| usize::try_from(count).unwrap_or(0))
    }

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

    pub fn documents_for_snapshot(&self, snapshot_id: &str) -> Result<Vec<DocumentContent>> {
        let rows = {
            let connection = self.connection.lock();
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

fn json(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(Into::into)
}

fn optional_json<T: serde::Serialize>(value: Option<&T>) -> Result<Option<String>> {
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
/// hit must cover at least two distinct content terms. With fewer than three
/// terms the prefix-AND stage already enforces the floor (a two-term query
/// requires both terms, a one-term query its only term), so no third stage
/// is emitted.
fn fts_match_queries(terms: &[String]) -> Vec<String> {
    let quoted = |term: &str| format!("\"{term}\"");
    let mut queries = vec![
        terms
            .iter()
            .map(|term| quoted(term))
            .collect::<Vec<_>>()
            .join(" AND "),
        terms
            .iter()
            .map(|term| format!("{}*", quoted(term)))
            .collect::<Vec<_>>()
            .join(" AND "),
    ];
    if terms.len() >= 3 {
        let mut pairs = Vec::new();
        for (index, left) in terms.iter().enumerate() {
            for right in &terms[index + 1..] {
                pairs.push(format!("({}* AND {}*)", quoted(left), quoted(right)));
            }
        }
        queries.push(pairs.join(" OR "));
    }
    queries
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
        let queries = fts_match_queries(&terms);
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
        assert_eq!(
            fts_match_queries(&fts_terms("freshness")),
            ["\"freshness\"", "\"freshness\"*"]
        );
        assert_eq!(
            fts_match_queries(&fts_terms("snapshot freshness")),
            [
                "\"snapshot\" AND \"freshness\"",
                "\"snapshot\"* AND \"freshness\"*",
            ]
        );
        // A repeated term is deduplicated so it cannot satisfy the two-term
        // floor by matching the same word twice.
        assert_eq!(
            fts_match_queries(&fts_terms("freshness freshness")),
            ["\"freshness\"", "\"freshness\"*"]
        );
    }
}
