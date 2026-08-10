use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use cce_core::{
    CceError, CodeEntity, LearningEventStage, LearningFeedback, LearningReceipt, QueryIntent,
    Relation, RepositoryIdentity, Result, RetrievalDocument, RetrievalRepresentation,
    SnapshotIdentity, SourceAddress, ViewKind, ViewManifest, ViewState, ViewStatus,
};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter, types::Value,
};
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
    pub entities: Vec<CodeEntity>,
    pub relations: Vec<Relation>,
    pub documents: Vec<IndexedDocument>,
}

#[derive(Debug, Clone)]
pub struct LexicalHit {
    pub document_id: String,
    pub entity_id: String,
    pub symbol_name: String,
    pub representation: RetrievalRepresentation,
    pub address: Option<SourceAddress>,
    pub evidence: Vec<SourceAddress>,
    pub score: f64,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct DocumentContent {
    pub document_id: String,
    pub entity_id: String,
    pub symbol_name: Option<String>,
    pub representation: RetrievalRepresentation,
    pub address: Option<SourceAddress>,
    pub evidence: Vec<SourceAddress>,
    pub text: String,
}

type DocumentRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
);

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
        if version > 3 {
            return Err(CceError::UnsupportedFormat {
                found: version,
                supported: 3,
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
                .execute_batch(include_str!("migrations/0003_learning_events.sql"))
                .map_err(|error| CceError::Storage(format!("migration 3 failed: {error}")))?;
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

    pub fn record_trajectory(
        &self,
        id: &str,
        repository_id: &str,
        snapshot_id: &str,
        query: &str,
        intent: QueryIntent,
        artifact: &ArtifactRecord,
    ) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction().map_err(storage_error)?;
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
        transaction
            .execute(
                "INSERT INTO trajectories(id, repository_id, snapshot_id, query, intent,
                 artifact_digest, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    repository_id,
                    snapshot_id,
                    query,
                    json(&intent)?,
                    artifact.digest,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn trajectory_bytes(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let digest = self
            .connection
            .lock()
            .query_row(
                "SELECT artifact_digest FROM trajectories WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        digest
            .map(|digest| self.artifacts.read(&digest))
            .transpose()
    }

    pub fn record_learning_feedback(
        &self,
        feedback: &LearningFeedback,
        receipt: &LearningReceipt,
    ) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "INSERT INTO learning_events(id, trajectory_id, stage, document_id, dwell_ms,
                 metadata_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    receipt.event_id,
                    feedback.trajectory_id,
                    json(&feedback.stage)?,
                    feedback.document_id,
                    feedback.dwell_ms.map(u64_to_i64).transpose()?,
                    json(&feedback.metadata)?,
                    receipt.recorded_at,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn record_shown_documents(
        &self,
        trajectory_id: &str,
        document_ids: &[String],
    ) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction().map_err(storage_error)?;
        let now = Utc::now().to_rfc3339();
        for document_id in document_ids {
            transaction
                .execute(
                    "INSERT INTO learning_events(id, trajectory_id, stage, document_id, dwell_ms,
                     metadata_json, created_at) VALUES (?1, ?2, ?3, ?4, NULL, '{}', ?5)",
                    params![
                        uuid::Uuid::now_v7().to_string(),
                        trajectory_id,
                        json(&LearningEventStage::ShownToModel)?,
                        document_id,
                        now,
                    ],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
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

        for entity in &records.entities {
            transaction
                .execute(
                    "INSERT INTO entities(id, snapshot_id, kind, name, qualified_name, signature,
                     language, address_json, capabilities_json, attributes_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        entity.id,
                        snapshot.id,
                        json(&entity.kind)?,
                        entity.name,
                        entity.qualified_name,
                        entity.signature,
                        entity.language,
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
                    "INSERT INTO retrieval_documents(id, snapshot_id, entity_id, representation,
                     body_artifact_digest, address_json, embedding_profile, generated_by,
                     evidence_json, terms_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        document.id,
                        snapshot.id,
                        document.entity_id,
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

    pub fn snapshot_identity(&self, snapshot_id: &str) -> Result<Option<SnapshotIdentity>> {
        let row = self
            .connection
            .lock()
            .query_row(
                "SELECT id, repository_id, base_revision, workspace_overlay_hash,
                 index_profile_hash, created_at, file_count, source_bytes
                 FROM snapshots WHERE id=?1 AND complete=1",
                [snapshot_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        row.map(
            |(
                id,
                repository_id,
                base_revision,
                workspace_overlay_hash,
                index_profile_hash,
                created_at,
                file_count,
                source_bytes,
            )| {
                Ok(SnapshotIdentity {
                    id,
                    repository_id,
                    base_revision,
                    workspace_overlay_hash,
                    index_profile_hash,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                        .map_err(|error| CceError::Storage(error.to_string()))?
                        .with_timezone(&Utc),
                    file_count: u64::try_from(file_count).map_err(|_| {
                        CceError::Storage("snapshot file count is negative".to_owned())
                    })?,
                    source_bytes: u64::try_from(source_bytes).map_err(|_| {
                        CceError::Storage("snapshot source byte count is negative".to_owned())
                    })?,
                })
            },
        )
        .transpose()
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

    pub fn lexical_search(
        &self,
        snapshot_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LexicalHit>> {
        let query = fts_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.lexical_search_fts(snapshot_id, &query, &[], limit)
    }

    pub fn lexical_search_representations(
        &self,
        snapshot_id: &str,
        query: &str,
        representations: &[RetrievalRepresentation],
        limit: usize,
    ) -> Result<Vec<LexicalHit>> {
        let query = fts_query(query);
        if query.is_empty() || representations.is_empty() {
            return Ok(Vec::new());
        }
        self.lexical_search_fts(snapshot_id, &query, representations, limit)
    }

    pub fn lexical_path_search(
        &self,
        snapshot_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LexicalHit>> {
        let query = fts_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.lexical_search_fts(snapshot_id, &format!("path : ({query})"), &[], limit)
    }

    fn lexical_search_fts(
        &self,
        snapshot_id: &str,
        query: &str,
        representations: &[RetrievalRepresentation],
        limit: usize,
    ) -> Result<Vec<LexicalHit>> {
        let connection = self.connection.lock();
        let representation_filter = if representations.is_empty() {
            String::new()
        } else {
            format!(
                " AND d.representation IN ({})",
                std::iter::repeat_n("?", representations.len())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let limit_parameter = representations.len() + 3;
        let sql = format!(
            "SELECT f.document_id, f.entity_id,
             CASE d.representation WHEN '\"commit_summary\"' THEN f.name ELSE e.name END,
             d.representation, d.address_json,
             d.evidence_json,
             bm25(documents_fts, 0.0, 0.0, 0.0, 3.0, 5.0, 2.0, 1.0) AS rank,
             snippet(documents_fts, 6, '<mark>', '</mark>', ' … ', 24)
             FROM documents_fts f JOIN retrieval_documents d
             ON d.snapshot_id=f.snapshot_id AND d.id=f.document_id
             JOIN entities e ON e.snapshot_id=f.snapshot_id AND e.id=f.entity_id
             WHERE documents_fts MATCH ?1 AND f.snapshot_id=?2{representation_filter}
             ORDER BY rank LIMIT ?{limit_parameter}"
        );
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let mut parameters = Vec::with_capacity(representations.len() + 3);
        parameters.push(Value::Text(query.to_owned()));
        parameters.push(Value::Text(snapshot_id.to_owned()));
        for representation in representations {
            parameters.push(Value::Text(json(representation)?));
        }
        parameters.push(Value::Integer(usize_to_i64(limit)?));
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(storage_error)?;
        let mut hits = Vec::new();
        for row in rows {
            let (
                document_id,
                entity_id,
                symbol_name,
                representation,
                address,
                evidence,
                rank,
                snippet,
            ) = row.map_err(storage_error)?;
            hits.push(LexicalHit {
                document_id,
                entity_id,
                symbol_name,
                representation: parse_json(&representation)?,
                address: address.as_deref().map(parse_json).transpose()?,
                evidence: parse_json(&evidence)?,
                score: 1.0 / (1.0 + rank.abs()),
                snippet,
            });
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
                "SELECT id, kind, name, qualified_name, signature, language, address_json,
                 capabilities_json, attributes_json FROM entities
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
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(storage_error)?;
        let mut entities = Vec::new();
        for row in rows {
            let (
                id,
                kind,
                name,
                qualified_name,
                signature,
                language,
                address,
                capabilities,
                attributes,
            ) = row.map_err(storage_error)?;
            entities.push(CodeEntity {
                id,
                kind: parse_json(&kind)?,
                name,
                qualified_name,
                signature,
                language,
                address: address.as_deref().map(parse_json).transpose()?,
                capabilities: parse_json(&capabilities)?,
                attributes: parse_json(&attributes)?,
            });
        }
        Ok(entities)
    }

    pub fn entity_by_id(&self, snapshot_id: &str, id: &str) -> Result<Option<CodeEntity>> {
        let connection = self.connection.lock();
        let row = connection
            .query_row(
                "SELECT id, kind, name, qualified_name, signature, language, address_json,
                 capabilities_json, attributes_json FROM entities WHERE snapshot_id=?1 AND id=?2",
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
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        row.map(
            |(
                id,
                kind,
                name,
                qualified_name,
                signature,
                language,
                address,
                capabilities,
                attributes,
            )| {
                Ok(CodeEntity {
                    id,
                    kind: parse_json(&kind)?,
                    name,
                    qualified_name,
                    signature,
                    language,
                    address: address.as_deref().map(parse_json).transpose()?,
                    capabilities: parse_json(&capabilities)?,
                    attributes: parse_json(&attributes)?,
                })
            },
        )
        .transpose()
    }

    pub fn entities_by_ids(
        &self,
        snapshot_id: &str,
        entity_ids: &[String],
    ) -> Result<Vec<CodeEntity>> {
        const IDS_PER_QUERY: usize = 400;
        let mut entities = Vec::with_capacity(entity_ids.len());
        for chunk in entity_ids.chunks(IDS_PER_QUERY) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT id, kind, name, qualified_name, signature, language, address_json,
                 capabilities_json, attributes_json FROM entities
                 WHERE snapshot_id=? AND id IN ({placeholders})"
            );
            let mut parameters = Vec::with_capacity(chunk.len() + 1);
            parameters.push(Value::Text(snapshot_id.to_owned()));
            parameters.extend(chunk.iter().cloned().map(Value::Text));
            let rows = {
                let connection = self.connection.lock();
                let mut statement = connection.prepare(&sql).map_err(storage_error)?;
                statement
                    .query_map(params_from_iter(parameters.iter()), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    })
                    .map_err(storage_error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(storage_error)?
            };
            for (
                id,
                kind,
                name,
                qualified_name,
                signature,
                language,
                address,
                capabilities,
                attributes,
            ) in rows
            {
                entities.push(CodeEntity {
                    id,
                    kind: parse_json(&kind)?,
                    name,
                    qualified_name,
                    signature,
                    language,
                    address: address.as_deref().map(parse_json).transpose()?,
                    capabilities: parse_json(&capabilities)?,
                    attributes: parse_json(&attributes)?,
                });
            }
        }
        Ok(entities)
    }

    pub fn relations_for_entity(
        &self,
        snapshot_id: &str,
        entity_id: &str,
        direction: RelationDirection,
        limit: usize,
    ) -> Result<Vec<Relation>> {
        Ok(self
            .relations_for_entities(snapshot_id, &[entity_id.to_owned()], direction, limit)?
            .remove(entity_id)
            .unwrap_or_default())
    }

    pub fn relations_for_entities(
        &self,
        snapshot_id: &str,
        entity_ids: &[String],
        direction: RelationDirection,
        limit_per_entity: usize,
    ) -> Result<std::collections::HashMap<String, Vec<Relation>>> {
        let mut grouped = entity_ids
            .iter()
            .map(|entity_id| (entity_id.clone(), Vec::new()))
            .collect::<std::collections::HashMap<_, _>>();
        if entity_ids.is_empty() || limit_per_entity == 0 {
            return Ok(grouped);
        }
        let predicate = match direction {
            RelationDirection::Outgoing => "relation.source_entity_id=frontier.entity_id",
            RelationDirection::Incoming => "relation.target_entity_id=frontier.entity_id",
            RelationDirection::Both => {
                "(relation.source_entity_id=frontier.entity_id OR relation.target_entity_id=frontier.entity_id)"
            }
        };
        let values = std::iter::repeat_n("(?)", entity_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "WITH frontier(entity_id) AS (VALUES {values}), ranked AS (
               SELECT frontier.entity_id AS frontier_entity_id, relation.id,
                      relation.source_entity_id, relation.target_entity_id, relation.kind,
                      relation.origin, relation.confidence, relation.extractor,
                      relation.evidence_json, relation.attributes_json,
                      ROW_NUMBER() OVER (
                        PARTITION BY frontier.entity_id
                        ORDER BY relation.confidence DESC, relation.id
                      ) AS relation_rank
               FROM frontier JOIN relations AS relation ON {predicate}
               WHERE relation.snapshot_id=?
             )
             SELECT frontier_entity_id, id, source_entity_id, target_entity_id, kind, origin,
                    confidence, extractor, evidence_json, attributes_json
             FROM ranked WHERE relation_rank<=? ORDER BY frontier_entity_id, relation_rank"
        );
        let mut parameters = entity_ids
            .iter()
            .cloned()
            .map(Value::Text)
            .collect::<Vec<_>>();
        parameters.push(Value::Text(snapshot_id.to_owned()));
        parameters.push(Value::Integer(usize_to_i64(limit_per_entity)?));
        let connection = self.connection.lock();
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })
            .map_err(storage_error)?;
        for row in rows {
            let (
                frontier_entity_id,
                id,
                source_entity_id,
                target_entity_id,
                kind,
                origin,
                confidence,
                extractor,
                evidence,
                attributes,
            ) = row.map_err(storage_error)?;
            let relation = Relation {
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
            };
            grouped
                .entry(frontier_entity_id)
                .or_default()
                .push(relation);
        }
        Ok(grouped)
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

    pub fn source_texts(&self, addresses: &[SourceAddress]) -> Result<Vec<Option<String>>> {
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot_id = &addresses[0].snapshot_id;
        if addresses
            .iter()
            .any(|address| address.snapshot_id != *snapshot_id)
        {
            return Err(CceError::Configuration(
                "batched source addresses must belong to one snapshot".to_owned(),
            ));
        }
        let mut paths = addresses
            .iter()
            .map(|address| address.path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        let mut digests_by_path = std::collections::HashMap::<String, String>::new();
        for chunk in paths.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT path, artifact_digest FROM source_files
                 WHERE snapshot_id=? AND path IN ({placeholders})"
            );
            let mut parameters = Vec::with_capacity(chunk.len() + 1);
            parameters.push(Value::Text(snapshot_id.clone()));
            parameters.extend(chunk.iter().cloned().map(Value::Text));
            let rows = {
                let connection = self.connection.lock();
                let mut statement = connection.prepare(&sql).map_err(storage_error)?;
                statement
                    .query_map(params_from_iter(parameters.iter()), |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(storage_error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(storage_error)?
            };
            digests_by_path.extend(rows);
        }
        let mut bytes_by_digest = std::collections::HashMap::<String, Vec<u8>>::new();
        for digest in digests_by_path.values() {
            if !bytes_by_digest.contains_key(digest) {
                bytes_by_digest.insert(digest.clone(), self.artifacts.read(digest)?);
            }
        }
        addresses
            .iter()
            .map(|address| {
                let Some(digest) = digests_by_path.get(&address.path) else {
                    return Ok(None);
                };
                let bytes = &bytes_by_digest[digest];
                let start = usize::try_from(address.start_byte)
                    .map_err(|_| CceError::ArtifactCorrupt(digest.clone()))?;
                let end = usize::try_from(address.end_byte)
                    .map_err(|_| CceError::ArtifactCorrupt(digest.clone()))?;
                let slice = bytes
                    .get(start..end)
                    .ok_or_else(|| CceError::ArtifactCorrupt(digest.clone()))?;
                String::from_utf8(slice.to_vec())
                    .map(Some)
                    .map_err(|_| CceError::ArtifactCorrupt(digest.clone()))
            })
            .collect()
    }

    pub fn documents_for_snapshot(&self, snapshot_id: &str) -> Result<Vec<DocumentContent>> {
        let rows = {
            let connection = self.connection.lock();
            let mut statement = connection
                .prepare(
                    "SELECT document.id, document.entity_id, entity.name, document.representation,
                     document.address_json, document.body_artifact_digest, document.evidence_json
                     FROM retrieval_documents AS document
                     LEFT JOIN entities AS entity ON entity.snapshot_id=document.snapshot_id
                                                  AND entity.id=document.entity_id
                     WHERE document.snapshot_id=?1 ORDER BY document.id",
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
        self.materialize_documents(rows)
    }

    pub fn documents_by_ids(
        &self,
        snapshot_id: &str,
        document_ids: &[String],
    ) -> Result<Vec<DocumentContent>> {
        const IDS_PER_QUERY: usize = 400;
        let mut rows = Vec::with_capacity(document_ids.len());
        for chunk in document_ids.chunks(IDS_PER_QUERY) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT document.id, document.entity_id, entity.name, document.representation,
                 document.address_json, document.body_artifact_digest, document.evidence_json
                 FROM retrieval_documents AS document
                 LEFT JOIN entities AS entity ON entity.snapshot_id=document.snapshot_id
                                              AND entity.id=document.entity_id
                 WHERE document.snapshot_id=? AND document.id IN ({placeholders})"
            );
            let mut parameters = Vec::with_capacity(chunk.len() + 1);
            parameters.push(Value::Text(snapshot_id.to_owned()));
            parameters.extend(chunk.iter().cloned().map(Value::Text));
            let chunk_rows = {
                let connection = self.connection.lock();
                let mut statement = connection.prepare(&sql).map_err(storage_error)?;
                statement
                    .query_map(params_from_iter(parameters.iter()), |row| {
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
            rows.extend(chunk_rows);
        }
        self.materialize_documents(rows)
    }

    fn materialize_documents(&self, rows: Vec<DocumentRow>) -> Result<Vec<DocumentContent>> {
        rows.into_iter()
            .map(
                |(
                    document_id,
                    entity_id,
                    symbol_name,
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
                        symbol_name,
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

fn fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| !term.is_empty())
        .filter(|term| !is_query_stopword(term))
        .take(32)
        .flat_map(|term| {
            let exact = format!("\"{}\"", term.replace('"', "\"\""));
            if term.chars().count() >= 4 {
                vec![exact.clone(), format!("{exact}*")]
            } else {
                vec![exact]
            }
        })
        .collect::<Vec<_>>()
        .join(" OR ")
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
    use crate::ArtifactKind;

    #[test]
    fn opens_and_migrates_database() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("metadata store");
        assert!(directory.path().join("metadata.sqlite").is_file());
        let version: u32 = store
            .connection
            .lock()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, 3);
    }

    #[test]
    fn records_content_addressed_learning_trajectory_and_feedback() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("metadata store");
        let repository = RepositoryIdentity {
            id: "repo".to_owned(),
            canonical_root: "/repo".to_owned(),
            remote: None,
        };
        store
            .register_repository(&repository)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snapshot".to_owned(),
            repository_id: repository.id.clone(),
            base_revision: Some("revision".to_owned()),
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");
        store
            .commit_snapshot(&snapshot, &SnapshotRecords::default())
            .expect("commit snapshot");
        let artifact = store
            .artifacts()
            .put_bytes(ArtifactKind::Trace, br#"{"hits":[]}"#)
            .expect("trace artifact");
        store
            .record_trajectory(
                "trajectory",
                &repository.id,
                &snapshot.id,
                "find the implementation",
                QueryIntent::Architecture,
                &artifact,
            )
            .expect("trajectory");
        let feedback = LearningFeedback {
            trajectory_id: "trajectory".to_owned(),
            stage: LearningEventStage::Accepted,
            document_id: None,
            dwell_ms: Some(12),
            metadata: std::collections::BTreeMap::new(),
        };
        let receipt = LearningReceipt {
            event_id: "event".to_owned(),
            trajectory_id: "trajectory".to_owned(),
            recorded_at: Utc::now().to_rfc3339(),
        };
        store
            .record_learning_feedback(&feedback, &receipt)
            .expect("feedback");

        assert_eq!(
            store
                .trajectory_bytes("trajectory")
                .expect("trajectory bytes")
                .expect("trajectory exists"),
            br#"{"hits":[]}"#
        );
        let count: u64 = store
            .connection
            .lock()
            .query_row("SELECT COUNT(*) FROM learning_events", [], |row| row.get(0))
            .expect("event count");
        assert_eq!(count, 1);
    }

    #[test]
    fn reads_only_complete_snapshot_identities() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("metadata store");
        let repository = RepositoryIdentity {
            id: "repo".to_owned(),
            canonical_root: "/repo".to_owned(),
            remote: None,
        };
        store
            .register_repository(&repository)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snapshot".to_owned(),
            repository_id: repository.id,
            base_revision: Some("revision".to_owned()),
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 7,
            source_bytes: 42,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");
        assert!(
            store
                .snapshot_identity(&snapshot.id)
                .expect("read incomplete")
                .is_none()
        );
        store
            .commit_snapshot(&snapshot, &SnapshotRecords::default())
            .expect("commit snapshot");
        let loaded = store
            .snapshot_identity(&snapshot.id)
            .expect("read complete")
            .expect("complete snapshot");
        assert_eq!(loaded.id, snapshot.id);
        assert_eq!(loaded.index_profile_hash, snapshot.index_profile_hash);
        assert_eq!(loaded.file_count, 7);
        assert_eq!(loaded.source_bytes, 42);
    }

    #[test]
    fn fts_query_is_bounded_and_quoted() {
        assert_eq!(
            fts_query("resumeAttempt cursor"),
            "\"resumeAttempt\" OR \"resumeAttempt\"* OR \"cursor\" OR \"cursor\"*"
        );
        assert_eq!(
            fts_query("Where is the snapshot freshness decided?"),
            "\"snapshot\" OR \"snapshot\"* OR \"freshness\" OR \"freshness\"* OR \"decided\" OR \"decided\"*"
        );
    }

    #[test]
    fn lexical_representation_filter_has_an_independent_candidate_pool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("metadata store");
        let repository = RepositoryIdentity {
            id: "repo".to_owned(),
            canonical_root: "/repo".to_owned(),
            remote: None,
        };
        store
            .register_repository(&repository)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snapshot".to_owned(),
            repository_id: repository.id.clone(),
            base_revision: None,
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");
        let entity = CodeEntity {
            id: "repository-entity".to_owned(),
            kind: cce_core::EntityKind::Repository,
            name: "repo".to_owned(),
            qualified_name: None,
            signature: None,
            language: None,
            address: None,
            capabilities: Vec::new(),
            attributes: serde_json::Map::new(),
        };
        let mut records = SnapshotRecords::default();
        records.entities.push(entity);
        for (id, representation, body) in [
            (
                "source-document",
                RetrievalRepresentation::RawCode,
                "cache invalidation cache invalidation implementation",
            ),
            (
                "history-document",
                RetrievalRepresentation::CommitSummary,
                "cache invalidation fix",
            ),
        ] {
            let artifact = store
                .artifacts()
                .put_bytes(ArtifactKind::Knowledge, body.as_bytes())
                .expect("document artifact");
            records.artifacts.push(artifact.clone());
            records.documents.push(IndexedDocument {
                document: RetrievalDocument {
                    id: id.to_owned(),
                    entity_id: "repository-entity".to_owned(),
                    snapshot_id: snapshot.id.clone(),
                    representation,
                    body_artifact_digest: artifact.digest,
                    address: None,
                    embedding_profile: None,
                    generated_by: None,
                    evidence: Vec::new(),
                    terms: vec!["cache".to_owned(), "invalidation".to_owned()],
                },
                path: id.to_owned(),
                name: id.to_owned(),
                body: body.to_owned(),
            });
        }
        store
            .commit_snapshot(&snapshot, &records)
            .expect("commit snapshot");

        let generic = store
            .lexical_search(&snapshot.id, "cache invalidation", 1)
            .expect("generic search");
        assert_eq!(generic[0].representation, RetrievalRepresentation::RawCode);
        let history = store
            .lexical_search_representations(
                &snapshot.id,
                "cache invalidation",
                &[RetrievalRepresentation::CommitSummary],
                1,
            )
            .expect("history search");
        assert_eq!(history[0].document_id, "history-document");
    }

    #[test]
    fn batches_entities_and_reuses_source_artifacts_for_ranges() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("metadata store");
        let repository = RepositoryIdentity {
            id: "repo".to_owned(),
            canonical_root: "/repo".to_owned(),
            remote: None,
        };
        store
            .register_repository(&repository)
            .expect("register repository");
        let source = b"alpha beta gamma\n";
        let snapshot = SnapshotIdentity {
            id: "snapshot".to_owned(),
            repository_id: repository.id.clone(),
            base_revision: None,
            workspace_overlay_hash: "overlay".to_owned(),
            index_profile_hash: "profile".to_owned(),
            created_at: Utc::now(),
            file_count: 1,
            source_bytes: source.len() as u64,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");
        let artifact = store
            .artifacts()
            .put_bytes(ArtifactKind::Source, source)
            .expect("source artifact");
        let address = |id: &str, start: u64, end: u64| {
            SourceAddress::new(
                &repository.id,
                &snapshot.id,
                "src/lib.rs",
                start..end,
                1..=1,
            )
            .expect("source address")
            .with_symbol(id)
        };
        let entities = [
            ("alpha", address("alpha", 0, 5)),
            ("beta", address("beta", 6, 10)),
        ]
        .into_iter()
        .map(|(id, address)| CodeEntity {
            id: id.to_owned(),
            kind: cce_core::EntityKind::Function,
            name: id.to_owned(),
            qualified_name: None,
            signature: None,
            language: Some("rust".to_owned()),
            address: Some(address),
            capabilities: Vec::new(),
            attributes: serde_json::Map::new(),
        })
        .collect::<Vec<_>>();
        let records = SnapshotRecords {
            artifacts: vec![artifact.clone()],
            files: vec![SourceFileRecord {
                path: "src/lib.rs".to_owned(),
                language: Some("rust".to_owned()),
                content_hash: blake3::hash(source).to_hex().to_string(),
                artifact,
                byte_count: source.len() as u64,
                line_count: 1,
                analysis_artifact_digest: None,
            }],
            entities: entities.clone(),
            relations: Vec::new(),
            documents: Vec::new(),
        };
        store
            .commit_snapshot(&snapshot, &records)
            .expect("commit snapshot");

        let mut loaded = store
            .entities_by_ids(&snapshot.id, &["beta".to_owned(), "alpha".to_owned()])
            .expect("batched entities");
        loaded.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(
            loaded
                .iter()
                .map(|entity| entity.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        let texts = store
            .source_texts(
                &loaded
                    .iter()
                    .filter_map(|entity| entity.address.clone())
                    .collect::<Vec<_>>(),
            )
            .expect("batched source ranges");
        assert_eq!(
            texts,
            vec![Some("alpha".to_owned()), Some("beta".to_owned())]
        );
    }

    #[test]
    fn batches_frontier_relations_with_a_limit_per_entity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = MetadataStore::open(directory.path()).expect("metadata store");
        {
            let connection = store.connection.lock();
            connection
                .execute(
                    "INSERT INTO repositories(id, canonical_root, created_at, updated_at)
                     VALUES ('repo', '/repo', ?1, ?1)",
                    [Utc::now().to_rfc3339()],
                )
                .expect("repository");
            connection
                .execute(
                    "INSERT INTO snapshots(id, repository_id, workspace_overlay_hash,
                     index_profile_hash, created_at, file_count, source_bytes, complete)
                     VALUES ('snapshot', 'repo', 'overlay', 'profile', ?1, 0, 0, 1)",
                    [Utc::now().to_rfc3339()],
                )
                .expect("snapshot");
            for (id, source, target, confidence) in [
                ("r1", "a", "b", 0.9),
                ("r2", "a", "c", 0.8),
                ("r3", "d", "a", 0.7),
            ] {
                connection
                    .execute(
                        "INSERT INTO relations(id, snapshot_id, source_entity_id, target_entity_id,
                         kind, origin, confidence, extractor, evidence_json, attributes_json)
                         VALUES (?1, 'snapshot', ?2, ?3, ?4, ?5, ?6, 'test', '[]', '{}')",
                        params![
                            id,
                            source,
                            target,
                            json(&cce_core::RelationKind::Calls).expect("kind"),
                            json(&cce_core::RelationOrigin::Compiler).expect("origin"),
                            confidence,
                        ],
                    )
                    .expect("relation");
            }
        }
        let grouped = store
            .relations_for_entities(
                "snapshot",
                &["a".to_owned(), "d".to_owned()],
                RelationDirection::Outgoing,
                1,
            )
            .expect("batched relations");
        assert_eq!(grouped["a"][0].id, "r1");
        assert_eq!(grouped["d"][0].id, "r3");
        assert_eq!(grouped["a"].len(), 1);
        assert_eq!(grouped["d"].len(), 1);

        let both = store
            .relations_for_entities("snapshot", &["a".to_owned()], RelationDirection::Both, 3)
            .expect("bidirectional indexed relations");
        assert_eq!(
            both["a"]
                .iter()
                .map(|relation| relation.id.as_str())
                .collect::<Vec<_>>(),
            vec!["r1", "r2", "r3"]
        );
    }
}
