use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use utoipa::ToSchema;

use cce_core::{
    Capability, CceError, CodeEntity, CodeRegion, EntityKind, RegionKind, Relation, RelationKind,
    RelationOrigin, Result, RetrievalDocument, RetrievalRepresentation, SnapshotIdentity,
    SourceAddress, ViewKind, ViewManifest, ViewState, ViewStatus,
};
use cce_store::{
    ArtifactKind, ArtifactRecord, GcReport, IndexedDocument, MetadataStore, SnapshotRecords,
    SourceFileRecord, StoreHealth,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{
    DenseBackendConfig, DenseIndex, Embedder, EmbeddingBackend, EngineConfig, RepositoryScanner,
    ScannedFile, ScannedRepository, SourceParser,
};

/// Everything an `index()` run produced: snapshot identity, counts, skip
/// reasons, provider outcomes, and the resulting view manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IndexReport {
    /// Repository that was indexed.
    pub repository_id: String,
    /// Identity of the committed snapshot.
    pub snapshot: SnapshotIdentity,
    /// True when an identical snapshot already existed (no-op index).
    pub reused_snapshot: bool,
    /// Files captured into the snapshot.
    pub indexed_files: usize,
    /// Files that produced parsed units.
    pub parsed_files: usize,
    /// Files whose analysis was reused from the scan cache.
    pub reused_file_analyses: usize,
    /// Source units (symbols/regions) extracted.
    pub source_units: usize,
    /// Relations committed for this snapshot.
    pub relations: usize,
    /// Retrieval documents committed for FTS.
    pub retrieval_documents: usize,
    /// Files skipped for exceeding `max_file_bytes`.
    pub skipped_large_files: Vec<String>,
    /// Binary files skipped.
    pub skipped_binary_files: Vec<String>,
    /// Sensitive files skipped (`.env`, keys, …).
    pub skipped_sensitive_files: Vec<String>,
    /// Files dropped by the unconditional built-in policy (lockfiles,
    /// minified assets), each paired with its skip reason.
    pub skipped_builtin_files: Vec<(String, String)>,
    /// Per-provider outcomes from this index pass.
    #[serde(default)]
    pub providers: Vec<crate::providers::ProviderReport>,
    /// View manifest after this run.
    pub manifest: ViewManifest,
}

/// The single-repository intelligence engine: indexing, views, retrieval,
/// and provider orchestration for one repository root.
#[derive(Debug)]
pub struct CceEngine {
    config: EngineConfig,
    store: MetadataStore,
    parser: SourceParser,
    /// Lazily initialized dense backend. ONNX session startup (and the
    /// one-time model download for `--dense local`) runs on the blocking
    /// pool so a slow fetch never stalls the executor; the result — including
    /// failures — is cached so a bad model does not retry a download per query.
    embedder: tokio::sync::OnceCell<std::result::Result<Option<EmbeddingBackend>, String>>,
    /// Lazily initialized cross-encoder reranker (`--reranker`). Same
    /// lifecycle as `embedder`: one ONNX session, init result cached.
    reranker: tokio::sync::OnceCell<std::result::Result<Option<crate::LocalReranker>, String>>,
    /// Snapshot ids whose committed-view repair already ran in this process.
    /// Repair is once per snapshot per process: a deterministically failing
    /// step (e.g. a missing model file) reports `Failed` once instead of
    /// re-running on every search; a fixed environment heals on the next
    /// process/index invocation.
    repair_attempts: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl CceEngine {
    /// Open an engine for `config`, initializing/migrating the store.
    ///
    /// # Errors
    /// Storage/format errors on open.
    pub fn open(config: EngineConfig) -> Result<Self> {
        let store = MetadataStore::open(&config.data_root)?;
        Ok(Self {
            config,
            store,
            parser: SourceParser::new(),
            embedder: tokio::sync::OnceCell::new(),
            reranker: tokio::sync::OnceCell::new(),
            repair_attempts: std::sync::Mutex::new(std::collections::HashSet::new()),
        })
    }

    /// Shared reranker backend, initialized on first use. `Ok(None)` when
    /// no reranker model is configured.
    pub async fn reranker(&self) -> Result<Option<crate::LocalReranker>> {
        let model = self.config.reranker_model.clone();
        let cache_dir = self.model_cache_dir();
        let state = self
            .reranker
            .get_or_init(move || async move {
                let Some(model) = model else {
                    return Ok(None);
                };
                tokio::task::spawn_blocking(move || {
                    crate::LocalReranker::new(&model, &cache_dir)
                        .map(Some)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            })
            .await;
        match state {
            Ok(reranker) => Ok(reranker.clone()),
            Err(message) => Err(CceError::Embedding(message.clone())),
        }
    }

    /// Shared dense backend, initialized on first use.
    pub async fn embedder(&self) -> Result<Option<EmbeddingBackend>> {
        let dense = self.config.dense.clone();
        let cache_dir = self.model_cache_dir();
        let state = self
            .embedder
            .get_or_init(move || async move {
                tokio::task::spawn_blocking(move || {
                    EmbeddingBackend::from_config(&dense, &cache_dir)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            })
            .await;
        match state {
            Ok(backend) => Ok(backend.clone()),
            Err(message) => Err(CceError::Embedding(message.clone())),
        }
    }

    /// Build (or rebuild) the dense view for a committed snapshot and record
    /// its outcome in the manifest. Best-effort by contract: a backend that
    /// fails to initialize or a build that fails marks the view `Failed`
    /// with its reason but never fails the index — lexical/structural views
    /// still stand. Called both from the main index path and from the
    /// early-return repair path that heals snapshots interrupted mid-build.
    async fn build_dense_view(
        &self,
        repository_id: &str,
        snapshot: &SnapshotIdentity,
    ) -> Result<()> {
        let embedder = match self.embedder().await {
            Ok(embedder) => embedder,
            Err(error) => {
                self.store.set_view_status(
                    repository_id,
                    &snapshot.id,
                    ViewKind::Dense,
                    &status(
                        snapshot,
                        ViewState::Failed,
                        vec![],
                        Some(format!("embedding backend init failed: {error}")),
                    ),
                )?;
                return Ok(());
            }
        };
        let Some(embedder) = embedder else {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Dense,
                &status(
                    snapshot,
                    ViewState::Unavailable,
                    vec![],
                    Some(
                        "No embedding backend selected; lexical and structural views remain available"
                            .to_owned(),
                    ),
                ),
            )?;
            return Ok(());
        };
        self.store.set_view_status(
            repository_id,
            &snapshot.id,
            ViewKind::Dense,
            &status(
                snapshot,
                ViewState::Building,
                vec![Capability {
                    name: "flat_inner_product".to_owned(),
                    level: "building".to_owned(),
                    reason: None,
                }],
                None,
            ),
        )?;
        let batch_size = match &self.config.dense {
            DenseBackendConfig::Local { .. } => 32,
            DenseBackendConfig::DeterministicBaseline { .. } | DenseBackendConfig::Disabled => 64,
        };
        let dense_result: Result<ArtifactRecord> = async {
            let documents = self.store.documents_for_snapshot(&snapshot.id)?;
            let reusable = self.dense_reuse_map(repository_id, snapshot, &embedder)?;
            let index =
                DenseIndex::build(&documents, &embedder, batch_size, reusable.as_ref()).await?;
            let artifact = self
                .store
                .artifacts()
                .put_bytes(ArtifactKind::VectorIndex, &index.encode()?)?;
            self.store.register_artifact(&artifact)?;
            Ok(artifact)
        }
        .await;
        match dense_result {
            Ok(artifact) => {
                let mut dense_status = status(
                    snapshot,
                    if embedder.production_ready() {
                        ViewState::Ready
                    } else {
                        ViewState::Partial
                    },
                    vec![Capability {
                        name: "flat_inner_product".to_owned(),
                        level: if embedder.production_ready() {
                            "production".to_owned()
                        } else {
                            "benchmark_only".to_owned()
                        },
                        reason: (!embedder.production_ready()).then(|| {
                            "deterministic hash embeddings are a reproducible baseline, not semantic production retrieval"
                                .to_owned()
                        }),
                    }],
                    None,
                );
                dense_status.artifact_digest = Some(artifact.digest);
                self.store.set_view_status(
                    repository_id,
                    &snapshot.id,
                    ViewKind::Dense,
                    &dense_status,
                )?;
            }
            Err(error) => {
                self.store.set_view_status(
                    repository_id,
                    &snapshot.id,
                    ViewKind::Dense,
                    &status(snapshot, ViewState::Failed, vec![], Some(error.to_string())),
                )?;
            }
        }
        Ok(())
    }

    /// Recompute the post-commit view statuses of a complete snapshot whose
    /// index was interrupted. All inputs are derivable from committed
    /// records: file analyses live in the artifact store, provider edges in
    /// `relations`, history coverage in commit entities/documents, and
    /// provider detect-state is re-probed (a report is fresher than a stale
    /// persisted one). The dense view rebuilds in place via
    /// `build_dense_view`. Idempotent — statuses converge, data is never
    /// rewritten.
    async fn repair_committed_views(
        &self,
        scanned: &ScannedRepository,
        manifest: &ViewManifest,
    ) -> Result<()> {
        let snapshot = &scanned.snapshot;
        let repository_id = scanned.identity.id.as_str();
        let stuck = |kind: ViewKind| {
            manifest
                .views
                .get(&kind)
                .is_some_and(|view| matches!(view.state, ViewState::Building | ViewState::Failed))
        };
        let scip_edges = if stuck(ViewKind::Graph) || stuck(ViewKind::Dataflow) {
            self.store.count_relations(
                &snapshot.id,
                &RelationKind::References,
                &RelationOrigin::Scip,
            )?
        } else {
            0
        };
        if stuck(ViewKind::Source) {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Source,
                &source_view_status(snapshot),
            )?;
        }
        if stuck(ViewKind::Lexical) {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Lexical,
                &lexical_view_status(snapshot),
            )?;
        }
        if stuck(ViewKind::Symbols) {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Symbols,
                &symbols_view_status(snapshot, self.committed_syntax_coverage(&snapshot.id)?),
            )?;
        }
        if stuck(ViewKind::Graph) {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Graph,
                &repaired_graph_status(
                    snapshot,
                    &crate::providers::detect_all(&self.config.repository_root),
                    scip_edges,
                ),
            )?;
        }
        if stuck(ViewKind::Dense) && !matches!(self.config.dense, DenseBackendConfig::Disabled) {
            self.build_dense_view(repository_id, snapshot).await?;
        }
        if stuck(ViewKind::Knowledge) {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Knowledge,
                &knowledge_view_status(snapshot),
            )?;
        }
        if stuck(ViewKind::History) {
            let commits = self
                .store
                .entities_by_kind(&snapshot.id, &EntityKind::Commit)?
                .len();
            let history_status = if commits == 0 {
                if self.config.repository_root.join(".git").exists() {
                    status(
                        snapshot,
                        ViewState::Failed,
                        vec![],
                        Some(
                            "snapshot carries no commit records — history extraction failed \
                             or produced nothing"
                                .to_owned(),
                        ),
                    )
                } else {
                    status(
                        snapshot,
                        ViewState::Unavailable,
                        vec![],
                        Some("Repository has no local .git object database".to_owned()),
                    )
                }
            } else {
                let diff_documents = self
                    .store
                    .count_documents(&snapshot.id, &RetrievalRepresentation::CommitDiff)?;
                status(
                    snapshot,
                    ViewState::Ready,
                    vec![
                        Capability {
                            name: "git_commit_messages".to_owned(),
                            level: "historical_evidence".to_owned(),
                            reason: Some(format!(
                                "{commits} reachable commits indexed with gitoxide"
                            )),
                        },
                        Capability {
                            name: "git_diff_hunks".to_owned(),
                            level: "historical_evidence".to_owned(),
                            reason: Some(format!(
                                "{diff_documents} commits contributed extracted diff hunks"
                            )),
                        },
                    ],
                    None,
                )
            };
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::History,
                &history_status,
            )?;
        }
        if stuck(ViewKind::Dataflow) {
            self.store.set_view_status(
                repository_id,
                &snapshot.id,
                ViewKind::Dataflow,
                &dataflow_view_status(snapshot, scip_edges),
            )?;
        }
        Ok(())
    }

    /// Recompute tree-sitter coverage from committed file analyses — the
    /// parse flag lives inside each file's `CachedFileAnalysis` artifact.
    fn committed_syntax_coverage(&self, snapshot_id: &str) -> Result<f64> {
        let mut parsed = 0_usize;
        let mut candidates = 0_usize;
        for row in self.store.source_files_for_snapshot(snapshot_id)? {
            if !row.language.as_deref().is_some_and(SourceParser::supports) {
                continue;
            }
            candidates += 1;
            let Some(digest) = row.analysis_artifact_digest else {
                continue;
            };
            match self.store.artifacts().read(&digest).and_then(|bytes| {
                serde_json::from_slice::<CachedFileAnalysis>(&bytes).map_err(|error| {
                    CceError::Storage(format!("invalid cached analysis {digest}: {error}"))
                })
            }) {
                Ok(cache) => {
                    parsed += usize::from(cache.parsed.parsed);
                }
                Err(error) => {
                    tracing::warn!(path = %row.path, %error, "skipping unreadable file analysis in repair");
                }
            }
        }
        Ok(if candidates == 0 {
            1.0
        } else {
            parsed as f64 / candidates as f64
        })
    }

    /// Vectors reusable for `snapshot`'s dense build, keyed by document-text
    /// digest. Decodes the newest completed snapshot under the same index
    /// profile and joins its vectors with that snapshot's document texts —
    /// unchanged text reuses its embedding verbatim, so a worktree edit only
    /// pays the model for documents that actually changed. `None` on a cold
    /// start, a profile/model change, or a prior build without an artifact.
    fn dense_reuse_map(
        &self,
        repository_id: &str,
        snapshot: &SnapshotIdentity,
        embedder: &EmbeddingBackend,
    ) -> Result<Option<HashMap<String, Vec<f32>>>> {
        let Some((prior_snapshot, digest)) = self.store.prior_dense_artifact(
            repository_id,
            &snapshot.id,
            &snapshot.index_profile_hash,
        )?
        else {
            return Ok(None);
        };
        let bytes = self.store.artifacts().read(&digest)?;
        let (profile, ids, vectors, dimensions) = DenseIndex::decode(&bytes)?.into_parts();
        if profile != embedder.profile() || dimensions == 0 {
            return Ok(None);
        }
        let text_keys: HashMap<String, String> = self
            .store
            .documents_for_snapshot(&prior_snapshot)?
            .into_iter()
            .map(|document| {
                (
                    document.document_id,
                    crate::dense::text_digest(&document.text),
                )
            })
            .collect();
        let mut reusable = HashMap::with_capacity(ids.len());
        for (position, id) in ids.iter().enumerate() {
            let Some(key) = text_keys.get(id) else {
                continue;
            };
            let start = position * dimensions;
            let Some(slice) = vectors.get(start..start + dimensions) else {
                continue;
            };
            reusable.insert(key.clone(), slice.to_vec());
        }
        Ok(Some(reusable))
    }

    /// The underlying metadata store.
    #[must_use]
    pub const fn store(&self) -> &MetadataStore {
        &self.store
    }

    /// The engine's configuration.
    #[must_use]
    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Current view manifest, marking views `Stale` when the worktree has
    /// drifted from the indexed snapshot.
    ///
    /// # Errors
    /// `ViewUnavailable` when the repository was never indexed.
    pub fn status(&self) -> Result<ViewManifest> {
        let scanned = RepositoryScanner::new(self.config.clone()).scan(Some(&self.store))?;
        let current = self
            .store
            .current_snapshot(&scanned.identity.id)?
            .ok_or_else(|| CceError::ViewUnavailable {
                view: "manifest".to_owned(),
                reason: "repository has not been indexed".to_owned(),
            })?;
        let mut manifest = self.store.view_manifest(&scanned.identity.id, &current)?;
        if current != scanned.snapshot.id {
            for view in manifest.views.values_mut() {
                if !matches!(view.state, ViewState::Unavailable | ViewState::Failed) {
                    view.state = ViewState::Stale;
                }
                view.message = Some(format!(
                    "indexed snapshot {} differs from working snapshot {}; run cce index",
                    current, scanned.snapshot.id
                ));
            }
        }
        Ok(manifest)
    }

    /// Run store health checks (`SQLite` integrity, artifact presence).
    ///
    /// # Errors
    /// Storage error when the checks cannot run.
    pub fn doctor(&self) -> Result<StoreHealth> {
        self.store.health()
    }

    /// Prune snapshots beyond the retention window and delete artifact
    /// objects no committed metadata row references. Holds the index lease.
    pub fn gc(&self, keep: usize) -> Result<GcReport> {
        let _lease = crate::lock::IndexLease::acquire(&self.config.data_root)?;
        let anchor = RepositoryScanner::new(self.config.clone()).identify()?;
        let pruned = self.store.prune_snapshots(&anchor.identity.id, keep)?;
        let mut report = self.store.gc_artifacts()?;
        report.pruned_snapshots = pruned.len();
        Ok(report)
    }

    /// Run the full indexing pipeline: scan → parse → providers → ingest →
    /// commit snapshot. Holds the index lease.
    ///
    /// # Errors
    /// `IndexBusy` when another index operation holds the lease; storage,
    /// parse, or provider errors surface in the report/status rather than
    /// aborting where recoverable.
    pub async fn index(&self) -> Result<IndexReport> {
        // Scan before taking the write lease: an unchanged repository returns
        // without ever contending with concurrent readers or writers.
        let mut scanned = RepositoryScanner::new(self.config.clone()).scan(Some(&self.store))?;
        self.store.register_repository(&scanned.identity)?;
        // One lease may be taken early — for the activation gate below — and
        // then shared by view repair, Zoekt repair, or the write path. Inner
        // acquisitions must reuse it: `IndexLease` is not re-entrant.
        let mut lease: Option<crate::lock::IndexLease> = None;
        if self.store.snapshot_is_complete(&scanned.snapshot.id)? {
            // "Data present" is not "snapshot active": reusing a complete
            // historical snapshot must move `current` to it, or status,
            // atlas and non-fresh search keep resolving the stale one.
            let current = self.store.current_snapshot(&scanned.identity.id)?;
            if current.as_deref() != Some(scanned.snapshot.id.as_str()) {
                // Activating is a write. Take the lease, then re-scan under
                // it — the first scan predates serialization, and another
                // writer may have committed a different snapshot meanwhile.
                let held = crate::lock::IndexLease::acquire(&self.config.data_root)?;
                scanned = RepositoryScanner::new(self.config.clone()).scan(Some(&self.store))?;
                if self.store.snapshot_is_complete(&scanned.snapshot.id)? {
                    let current = self.store.current_snapshot(&scanned.identity.id)?;
                    if current.as_deref() != Some(scanned.snapshot.id.as_str()) {
                        self.store.activate_complete_snapshot(
                            &scanned.identity.id,
                            &scanned.snapshot.id,
                        )?;
                    }
                }
                // `refreshed` may map to a snapshot that is not complete —
                // then the reuse return below is skipped and the write path
                // continues with it, holding this same lease.
                lease = Some(held);
            }
        }
        if self.store.snapshot_is_complete(&scanned.snapshot.id)? {
            let mut manifest = self
                .store
                .view_manifest(&scanned.identity.id, &scanned.snapshot.id)?;
            // A snapshot is marked complete when its records transaction
            // commits — post-commit steps (the dense build, view status
            // writes) can still be interrupted, leaving `Building` (or a
            // transient `Failed`) committed in the manifest while the early
            // return above would skip the rebuild forever. Repair in place:
            // every post-commit input is derivable from committed records.
            // Once per snapshot per process — a deterministically failing
            // step reports `Failed` once rather than re-running per search.
            let needs_repair = manifest
                .views
                .values()
                .any(|view| matches!(view.state, ViewState::Building | ViewState::Failed));
            let first_attempt = needs_repair
                && self
                    .repair_attempts
                    .lock()
                    .map_err(|_| CceError::Storage("repair mutex poisoned".to_owned()))?
                    .insert(scanned.snapshot.id.clone());
            if first_attempt {
                if lease.is_none() {
                    lease = Some(crate::lock::IndexLease::acquire(&self.config.data_root)?);
                }
                self.repair_committed_views(&scanned, &manifest).await?;
                manifest = self
                    .store
                    .view_manifest(&scanned.identity.id, &scanned.snapshot.id)?;
            }
            // Zoekt shards are derived content outside the snapshot
            // commit; a reused snapshot may predate the toolchain or
            // carry a stale marker. Rebuild once per mismatch — this
            // branch skips `ingest_providers` entirely.
            let mut providers = Vec::new();
            let zoekt_fresh =
                crate::zoekt::indexed_snapshot(&crate::zoekt::index_dir(&self.config.data_root))
                    == Some(scanned.snapshot.id.clone());
            if !zoekt_fresh || !manifest.views.contains_key(&ViewKind::Zoekt) {
                // A concurrent writer owns freshness — its fresh-index
                // pass rebuilds the shards anyway, so busy means skip.
                if lease.is_none() {
                    match crate::lock::IndexLease::acquire(&self.config.data_root) {
                        Ok(held) => lease = Some(held),
                        Err(CceError::IndexBusy(_)) => {}
                        Err(error) => return Err(error),
                    }
                }
                if lease.is_some() {
                    let repo_root = self.config.repository_root.clone();
                    let data_root = self.config.data_root.clone();
                    let snapshot_id = scanned.snapshot.id.clone();
                    let timeout =
                        std::time::Duration::from_secs(self.config.providers.timeout_secs);
                    let report = tokio::task::spawn_blocking(move || {
                        crate::zoekt::ensure_report(&repo_root, &data_root, &snapshot_id, timeout)
                    })
                    .await
                    .map_err(|error| {
                        CceError::Configuration(format!("provider runner failed: {error}"))
                    })?;
                    self.store.set_view_status(
                        &scanned.identity.id,
                        &scanned.snapshot.id,
                        ViewKind::Zoekt,
                        &zoekt_view_status(&scanned.snapshot, std::slice::from_ref(&report)),
                    )?;
                    providers.push(report);
                    manifest = self
                        .store
                        .view_manifest(&scanned.identity.id, &scanned.snapshot.id)?;
                }
            }
            return Ok(IndexReport {
                repository_id: scanned.identity.id,
                snapshot: scanned.snapshot,
                reused_snapshot: true,
                indexed_files: scanned.files.len(),
                parsed_files: 0,
                reused_file_analyses: scanned.files.len(),
                source_units: 0,
                relations: 0,
                retrieval_documents: 0,
                skipped_large_files: scanned.skipped_large_files,
                skipped_binary_files: scanned.skipped_binary_files,
                skipped_sensitive_files: scanned.skipped_sensitive_files,
                skipped_builtin_files: scanned
                    .skipped_builtin_files
                    .into_iter()
                    .map(|(path, reason)| (path, reason.to_owned()))
                    .collect(),
                providers,
                manifest,
            });
        }

        // Only the write path needs the lease. The scanned hashes pin the
        // snapshot; if the tree changes mid-build the byte-level hash check
        // in `ScannedFile::bytes` fails the index instead of committing a
        // snapshot that misidentifies content. The lease may already be held
        // when the activation gate's rescan landed on an incomplete snapshot.
        let _lease = match lease {
            Some(held) => held,
            None => crate::lock::IndexLease::acquire(&self.config.data_root)?,
        };
        self.store.begin_snapshot(&scanned.snapshot)?;
        for kind in [
            ViewKind::Source,
            ViewKind::Lexical,
            ViewKind::Dense,
            ViewKind::Symbols,
            ViewKind::Graph,
            ViewKind::History,
            ViewKind::Knowledge,
            ViewKind::Dataflow,
        ] {
            self.store.set_view_status(
                &scanned.identity.id,
                &scanned.snapshot.id,
                kind,
                &status(&scanned.snapshot, ViewState::Building, Vec::new(), None),
            )?;
        }

        let mut records = SnapshotRecords::default();
        let repository_entity_id = entity_id(&scanned.identity.id, "", 0, 0, "repository");
        records.entities.push(CodeEntity {
            id: repository_entity_id.clone(),
            kind: EntityKind::Repository,
            name: scanned.identity.canonical_root.clone(),
            qualified_name: scanned.identity.remote.clone(),
            signature: None,
            language: None,
            region_id: None,
            address: None,
            capabilities: vec!["repository_orientation".to_owned()],
            attributes: serde_json::Map::new(),
        });
        let mut parsed_files = 0_usize;
        let mut parse_candidates = 0_usize;
        let mut reused_file_analyses = 0_usize;
        let mut source_units = 0_usize;
        let mut file_entities = HashMap::new();
        let mut file_regions = HashMap::new();
        let mut parsed_by_path = HashMap::new();
        let mut unit_ids_by_path: HashMap<String, Vec<String>> = HashMap::new();
        let mut name_index: HashMap<String, Vec<crate::relations::SymbolCandidate>> =
            HashMap::new();

        // One newest-first history walk feeds both the per-file
        // `lastTouched` attribute below and the co-change pass in
        // `add_derived_relations`.
        let touched_commits = crate::history::changed_paths_per_commit(
            &self.config.repository_root,
            HISTORY_COMMIT_LIMIT,
        );
        let last_touched = crate::history::last_touched(&touched_commits);

        let mut texts_by_path = HashMap::new();
        for file in &scanned.files {
            let bytes = file.bytes()?;
            let file_body = std::str::from_utf8(&bytes)
                .map_err(|error| {
                    CceError::Configuration(format!("{} is not UTF-8: {error}", file.relative_path))
                })?
                .to_owned();
            texts_by_path.insert(file.relative_path.clone(), file_body.clone());
            let artifact = self
                .store
                .artifacts()
                .put_bytes(ArtifactKind::Source, &bytes)?;
            let file_id = entity_id(
                &scanned.identity.id,
                &file.relative_path,
                0,
                bytes.len(),
                "file",
            );
            file_entities.insert(file.relative_path.clone(), file_id.clone());
            let file_region_id =
                region_id(&file.relative_path, 0, bytes.len(), RegionKind::File, "");
            file_regions.insert(file.relative_path.clone(), file_region_id.clone());
            records.regions.push(CodeRegion {
                id: file_region_id.clone(),
                snapshot_id: scanned.snapshot.id.clone(),
                path: file.relative_path.clone(),
                kind: RegionKind::File,
                language: file.language.clone(),
                symbol_name: None,
                symbol_kind: None,
                qualified_name: Some(file.relative_path.clone()),
                parent_region_id: None,
                start_byte: 0,
                end_byte: bytes.len() as u64,
                start_line: 1,
                end_line: file.line_count.max(1) as u32,
            });
            let file_address = address(
                &scanned.identity.id,
                &scanned.snapshot.id,
                &file.relative_path,
                &file_body,
                0,
                bytes.len(),
                Some(&file_id),
            )?;
            records.entities.push(CodeEntity {
                id: file_id.clone(),
                kind: EntityKind::File,
                name: file.relative_path.clone(),
                qualified_name: Some(file.relative_path.clone()),
                signature: None,
                language: file.language.clone(),
                region_id: Some(file_region_id),
                address: Some(file_address),
                capabilities: vec!["source_truth".to_owned()],
                // `lastTouched` is the newest commit timestamp touching this
                // path — recency evidence for fault-localization priors.
                // Files outside the indexed history window carry nothing.
                attributes: last_touched.get(&file.relative_path).map_or_else(
                    serde_json::Map::new,
                    |timestamp| {
                        serde_json::Map::from_iter([(
                            "lastTouched".to_owned(),
                            (*timestamp).into(),
                        )])
                    },
                ),
            });

            if file.language.as_deref().is_some_and(SourceParser::supports) {
                parse_candidates += 1;
            }
            let (parsed, analysis_artifact) = self.parse_with_cache(
                &scanned.identity.id,
                file,
                &bytes,
                &mut reused_file_analyses,
            )?;
            records.artifacts.push(analysis_artifact.clone());
            records.files.push(SourceFileRecord {
                path: file.relative_path.clone(),
                language: file.language.clone(),
                content_hash: file.content_hash.clone(),
                artifact: artifact.clone(),
                byte_count: file.size_bytes,
                line_count: file.line_count,
                analysis_artifact_digest: Some(analysis_artifact.digest),
            });
            if parsed.parsed {
                parsed_files += 1;
            }
            source_units += parsed.units.len();
            parsed_by_path.insert(file.relative_path.clone(), parsed);
        }

        for file in &scanned.files {
            let parsed = parsed_by_path
                .get(&file.relative_path)
                .ok_or_else(|| CceError::Configuration("parsed file disappeared".to_owned()))?;
            let file_id = file_entities
                .get(&file.relative_path)
                .ok_or_else(|| CceError::Configuration("file entity disappeared".to_owned()))?;
            let file_region_id = file_regions
                .get(&file.relative_path)
                .ok_or_else(|| CceError::Configuration("file region disappeared".to_owned()))?;
            let file_artifact = records
                .files
                .iter()
                .find(|record| record.path == file.relative_path)
                .map(|record| record.artifact.digest.clone())
                .ok_or_else(|| CceError::Configuration("file artifact disappeared".to_owned()))?;
            let file_text = texts_by_path
                .get(&file.relative_path)
                .ok_or_else(|| CceError::Configuration("file text disappeared".to_owned()))?;
            let mut unit_ids = Vec::with_capacity(parsed.units.len());
            let mut unit_region_ids = Vec::with_capacity(parsed.units.len());
            for unit in &parsed.units {
                unit_ids.push(entity_id(
                    &scanned.identity.id,
                    &file.relative_path,
                    unit.start_byte,
                    unit.end_byte,
                    &format!("{:?}:{}", unit.kind, unit.name),
                ));
                unit_region_ids.push(region_id(
                    &file.relative_path,
                    unit.start_byte,
                    unit.end_byte,
                    RegionKind::Symbol,
                    &unit.name,
                ));
            }

            // L1 file descriptor: bounded routing evidence (path, language,
            // imports, top-level signatures) instead of the whole file body.
            let descriptor = file_descriptor(file, file_text, parsed);
            records.documents.push(IndexedDocument {
                document: RetrievalDocument {
                    id: document_id(file_id, "file_descriptor", 0, descriptor.len()),
                    entity_id: file_id.clone(),
                    region_id: Some(file_region_id.clone()),
                    snapshot_id: scanned.snapshot.id.clone(),
                    representation: RetrievalRepresentation::FileDescriptor,
                    body_artifact_digest: file_artifact.clone(),
                    address: None,
                    embedding_profile: None,
                    generated_by: Some("cce-file-descriptor-v1".to_owned()),
                    evidence: Vec::new(),
                    terms: lexical_terms(&descriptor),
                },
                path: file.relative_path.clone(),
                name: file.relative_path.clone(),
                body: descriptor,
            });

            if parsed.units.is_empty() && !file_text.is_empty() {
                // No symbol coverage (unparsed language, data files): fall back
                // to bounded fixed windows so retrieval stays granular.
                for (start, end) in chunk_ranges(
                    file_text,
                    0,
                    file_text.len(),
                    self.config.index.max_unit_bytes,
                ) {
                    let sub_region_id =
                        region_id(&file.relative_path, start, end, RegionKind::Subregion, "");
                    records.regions.push(CodeRegion {
                        id: sub_region_id.clone(),
                        snapshot_id: scanned.snapshot.id.clone(),
                        path: file.relative_path.clone(),
                        kind: RegionKind::Subregion,
                        language: file.language.clone(),
                        symbol_name: None,
                        symbol_kind: None,
                        qualified_name: None,
                        parent_region_id: Some(file_region_id.clone()),
                        start_byte: start as u64,
                        end_byte: end as u64,
                        start_line: line_for_offset(file_text, start),
                        end_line: line_for_offset(file_text, end),
                    });
                    let chunk_address = address(
                        &scanned.identity.id,
                        &scanned.snapshot.id,
                        &file.relative_path,
                        file_text,
                        start,
                        end,
                        None,
                    )?;
                    records.documents.push(IndexedDocument {
                        document: RetrievalDocument {
                            id: document_id(file_id, "raw_code", start, end),
                            entity_id: file_id.clone(),
                            region_id: Some(sub_region_id),
                            snapshot_id: scanned.snapshot.id.clone(),
                            representation: RetrievalRepresentation::RawCode,
                            body_artifact_digest: file_artifact.clone(),
                            address: Some(chunk_address),
                            embedding_profile: None,
                            generated_by: None,
                            evidence: Vec::new(),
                            terms: lexical_terms(&file.relative_path),
                        },
                        path: file.relative_path.clone(),
                        name: file.relative_path.clone(),
                        body: file_text
                            .get(start..end)
                            .ok_or_else(|| CceError::InvalidSourceRange {
                                path: file.relative_path.clone(),
                                start_byte: start as u64,
                                end_byte: end as u64,
                            })?
                            .to_owned(),
                    });
                }
            }

            for (index, ((unit, unit_id), unit_region_id)) in parsed
                .units
                .iter()
                .zip(&unit_ids)
                .zip(&unit_region_ids)
                .enumerate()
            {
                let unit_id = unit_id.clone();
                let unit_region_id = unit_region_id.clone();
                let parent_region = unit
                    .parent_unit
                    .and_then(|parent| unit_region_ids.get(parent))
                    .unwrap_or(file_region_id);
                records.regions.push(CodeRegion {
                    id: unit_region_id.clone(),
                    snapshot_id: scanned.snapshot.id.clone(),
                    path: file.relative_path.clone(),
                    kind: RegionKind::Symbol,
                    language: file.language.clone(),
                    symbol_name: Some(unit.name.clone()),
                    symbol_kind: Some(unit.kind.clone()),
                    qualified_name: Some(qualified_name(&file.relative_path, parsed, index)),
                    parent_region_id: Some(parent_region.clone()),
                    start_byte: unit.start_byte as u64,
                    end_byte: unit.end_byte as u64,
                    start_line: unit.start_line,
                    end_line: unit.end_line,
                });
                let unit_address = address(
                    &scanned.identity.id,
                    &scanned.snapshot.id,
                    &file.relative_path,
                    file_text,
                    unit.start_byte,
                    unit.end_byte,
                    Some(&unit_id),
                )?;
                let parent_id = unit
                    .parent_unit
                    .and_then(|parent| unit_ids.get(parent))
                    .unwrap_or(file_id);
                let qualified_name = qualified_name(&file.relative_path, parsed, index);
                let mut attributes = serde_json::Map::new();
                attributes.insert("syntaxKind".to_owned(), unit.syntax_kind.clone().into());
                attributes.insert(
                    "hasSyntaxErrors".to_owned(),
                    parsed.has_syntax_errors.into(),
                );
                records.entities.push(CodeEntity {
                    id: unit_id.clone(),
                    kind: unit.kind.clone(),
                    name: unit.name.clone(),
                    qualified_name: Some(qualified_name.clone()),
                    signature: unit.signature.clone(),
                    language: file.language.clone(),
                    region_id: Some(unit_region_id.clone()),
                    address: Some(unit_address.clone()),
                    capabilities: vec!["syntax_fact".to_owned()],
                    attributes,
                });
                records.relations.push(Relation {
                    id: relation_id(parent_id, &unit_id, "contains"),
                    source_entity_id: parent_id.clone(),
                    target_entity_id: unit_id.clone(),
                    kind: RelationKind::Contains,
                    origin: RelationOrigin::TreeSitter,
                    confidence: 1.0,
                    snapshot_id: scanned.snapshot.id.clone(),
                    extractor: parsed
                        .parser
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    evidence: vec![unit_address.clone()],
                    attributes: serde_json::Map::new(),
                });
                let chunks = chunk_ranges(
                    file_text,
                    unit.start_byte,
                    unit.end_byte,
                    self.config.index.max_unit_bytes,
                );
                for (start, end) in chunks {
                    // A single-chunk symbol reuses the symbol region; split
                    // symbols get one subregion per chunk.
                    let doc_region_id = if start == unit.start_byte && end == unit.end_byte {
                        unit_region_id.clone()
                    } else {
                        let sub_region_id = region_id(
                            &file.relative_path,
                            start,
                            end,
                            RegionKind::Subregion,
                            &unit.name,
                        );
                        records.regions.push(CodeRegion {
                            id: sub_region_id.clone(),
                            snapshot_id: scanned.snapshot.id.clone(),
                            path: file.relative_path.clone(),
                            kind: RegionKind::Subregion,
                            language: file.language.clone(),
                            symbol_name: Some(unit.name.clone()),
                            symbol_kind: Some(unit.kind.clone()),
                            qualified_name: Some(qualified_name.clone()),
                            parent_region_id: Some(unit_region_id.clone()),
                            start_byte: start as u64,
                            end_byte: end as u64,
                            start_line: line_for_offset(file_text, start),
                            end_line: line_for_offset(file_text, end),
                        });
                        sub_region_id
                    };
                    let chunk_address = address(
                        &scanned.identity.id,
                        &scanned.snapshot.id,
                        &file.relative_path,
                        file_text,
                        start,
                        end,
                        Some(&unit_id),
                    )?;
                    let body = file_text
                        .get(start..end)
                        .ok_or_else(|| CceError::InvalidSourceRange {
                            path: file.relative_path.clone(),
                            start_byte: start as u64,
                            end_byte: end as u64,
                        })?
                        .to_owned();
                    records.documents.push(IndexedDocument {
                        document: RetrievalDocument {
                            id: document_id(&unit_id, "raw_code", start, end),
                            entity_id: unit_id.clone(),
                            region_id: Some(doc_region_id),
                            snapshot_id: scanned.snapshot.id.clone(),
                            representation: if is_test(&file.relative_path, &unit.name) {
                                RetrievalRepresentation::TestBehavior
                            } else {
                                RetrievalRepresentation::RawCode
                            },
                            body_artifact_digest: file_artifact.clone(),
                            address: Some(chunk_address),
                            embedding_profile: None,
                            generated_by: None,
                            evidence: Vec::new(),
                            terms: lexical_terms(&format!(
                                "{} {}",
                                unit.name,
                                unit.signature.as_deref().unwrap_or("")
                            )),
                        },
                        path: file.relative_path.clone(),
                        name: unit.name.clone(),
                        body,
                    });
                }

                // Symbol summary: a deterministic natural-language-shaped
                // descriptor per symbol. This is the dense/lexical bridge
                // between "which code marks views stale" phrasing and
                // identifier-shaped implementation names — semantic material,
                // not generated knowledge.
                let summary_body = symbol_descriptor(&file.relative_path, file_text, parsed, index);
                records.documents.push(IndexedDocument {
                    document: RetrievalDocument {
                        id: document_id(&unit_id, "symbol_summary", 0, summary_body.len()),
                        entity_id: unit_id.clone(),
                        region_id: Some(unit_region_id.clone()),
                        snapshot_id: scanned.snapshot.id.clone(),
                        representation: RetrievalRepresentation::SymbolSummary,
                        body_artifact_digest: file_artifact.clone(),
                        address: Some(unit_address.clone()),
                        embedding_profile: None,
                        generated_by: Some("cce-symbol-descriptor-v1".to_owned()),
                        evidence: Vec::new(),
                        terms: lexical_terms(&summary_body),
                    },
                    path: file.relative_path.clone(),
                    name: unit.name.clone(),
                    body: summary_body,
                });
            }

            for (unit, unit_id) in parsed.units.iter().zip(&unit_ids) {
                name_index.entry(unit.name.clone()).or_default().push(
                    crate::relations::SymbolCandidate {
                        entity_id: unit_id.clone(),
                        path: file.relative_path.clone(),
                        kind: unit.kind.clone(),
                    },
                );
            }
            unit_ids_by_path.insert(file.relative_path.clone(), unit_ids);
            let summary = deterministic_role_summary(file, parsed);
            let summary_evidence = vec![address(
                &scanned.identity.id,
                &scanned.snapshot.id,
                &file.relative_path,
                file_text,
                0,
                file_text.len(),
                Some(file_id),
            )?];
            let summary_artifact = self
                .store
                .artifacts()
                .put_bytes(ArtifactKind::Knowledge, summary.as_bytes())?;
            records.artifacts.push(summary_artifact.clone());
            records.documents.push(IndexedDocument {
                document: RetrievalDocument {
                    id: document_id(file_id, "role_summary", 0, summary.len()),
                    entity_id: file_id.clone(),
                    region_id: Some(file_region_id.clone()),
                    snapshot_id: scanned.snapshot.id.clone(),
                    representation: RetrievalRepresentation::RoleSummary,
                    body_artifact_digest: summary_artifact.digest,
                    address: None,
                    embedding_profile: None,
                    generated_by: Some("cce-deterministic-role-v1".to_owned()),
                    evidence: summary_evidence,
                    terms: lexical_terms(&summary),
                },
                path: file.relative_path.clone(),
                name: file.relative_path.clone(),
                body: summary,
            });
        }

        crate::relations::add_derived_relations(
            &crate::relations::RelationContext {
                repository_id: &scanned.identity.id,
                snapshot_id: &scanned.snapshot.id,
                files: &scanned.files,
                texts: &texts_by_path,
                file_entities: &file_entities,
                parsed: &parsed_by_path,
                unit_ids: &unit_ids_by_path,
                name_index: &name_index,
                touched: &touched_commits,
            },
            &mut records.relations,
        );

        // L2 package architecture from build manifests (deterministic
        // boundaries — BuildSystem provenance at full confidence).
        let package_infos = crate::packages::extract(&scanned.files, &texts_by_path);
        let emission = crate::packages::emit(
            &package_infos,
            &scanned.identity.id,
            &scanned.snapshot.id,
            &scanned.files,
            &file_entities,
            &file_regions,
        );
        records.entities.extend(emission.entities);
        records.relations.extend(emission.relations);

        // Framework landmarks: axum route bindings emit Route entities and
        // RouteHandledBy edges (FrameworkRule provenance, 0.85 confidence).
        let route_bindings = crate::landmarks::axum_routes(&scanned.files, &texts_by_path);
        let emission = crate::landmarks::emit(
            &route_bindings,
            &scanned.identity.id,
            &scanned.snapshot.id,
            &file_regions,
            &name_index,
        );
        records.entities.extend(emission.entities);
        records.relations.extend(emission.relations);

        add_hierarchical_knowledge(
            &self.store,
            &scanned.snapshot,
            &scanned.files,
            &texts_by_path,
            &file_entities,
            &repository_entity_id,
            &mut records,
        )?;

        let history_view_status =
            match crate::history::summarize(&self.config.repository_root, HISTORY_COMMIT_LIMIT) {
                Ok(Some(summary)) => {
                    let artifact = self
                        .store
                        .artifacts()
                        .put_bytes(ArtifactKind::Knowledge, summary.body.as_bytes())?;
                    let diff_documents = self.index_commit_diffs(&scanned, &mut records)?;
                    let mut history_status = status(
                        &scanned.snapshot,
                        ViewState::Ready,
                        vec![
                            Capability {
                                name: "git_commit_messages".to_owned(),
                                level: "historical_evidence".to_owned(),
                                reason: Some(format!(
                                    "{} reachable commits indexed with gitoxide",
                                    summary.commit_count
                                )),
                            },
                            Capability {
                                name: "git_diff_hunks".to_owned(),
                                level: "historical_evidence".to_owned(),
                                reason: Some(format!(
                                    "{diff_documents} commits contributed extracted diff hunks"
                                )),
                            },
                        ],
                        None,
                    );
                    history_status.artifact_digest = Some(artifact.digest.clone());
                    records.artifacts.push(artifact.clone());
                    records.documents.push(IndexedDocument {
                        document: RetrievalDocument {
                            id: document_id(
                                &repository_entity_id,
                                "commit_summary",
                                0,
                                summary.body.len(),
                            ),
                            entity_id: repository_entity_id.clone(),
                            region_id: None,
                            snapshot_id: scanned.snapshot.id.clone(),
                            representation: RetrievalRepresentation::CommitSummary,
                            body_artifact_digest: artifact.digest,
                            address: None,
                            embedding_profile: None,
                            generated_by: Some("cce-gitoxide-history-v1".to_owned()),
                            evidence: Vec::new(),
                            terms: lexical_terms(&summary.body),
                        },
                        path: ".git".to_owned(),
                        name: "commit history".to_owned(),
                        body: summary.body,
                    });
                    history_status
                }
                Ok(None) => status(
                    &scanned.snapshot,
                    ViewState::Unavailable,
                    Vec::new(),
                    Some("Repository has no local .git object database".to_owned()),
                ),
                Err(error) => status(
                    &scanned.snapshot,
                    ViewState::Failed,
                    Vec::new(),
                    Some(format!("gitoxide could not read commit history: {error}")),
                ),
            };

        // External code-intelligence providers (SCIP indexers). Subprocess
        // runs happen on the blocking pool; ingestion into `records` is a
        // pure in-memory merge — a failed provider degrades its report and
        // view status, never the snapshot.
        let provider_reports = self
            .ingest_providers(
                &scanned,
                &texts_by_path,
                &parsed_by_path,
                &unit_ids_by_path,
                &file_entities,
                &mut records,
            )
            .await?;

        self.store.commit_snapshot(&scanned.snapshot, &records)?;
        let syntax_coverage = if parse_candidates == 0 {
            1.0
        } else {
            parsed_files as f64 / parse_candidates as f64
        };
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Source,
            &source_view_status(&scanned.snapshot),
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Lexical,
            &lexical_view_status(&scanned.snapshot),
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Symbols,
            &symbols_view_status(&scanned.snapshot, syntax_coverage),
        )?;
        let (graph_status, scip_edges) = graph_view_status(&scanned.snapshot, &provider_reports);
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Graph,
            &graph_status,
        )?;
        self.build_dense_view(&scanned.identity.id, &scanned.snapshot)
            .await?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Knowledge,
            &knowledge_view_status(&scanned.snapshot),
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::History,
            &history_view_status,
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Dataflow,
            &dataflow_view_status(&scanned.snapshot, scip_edges),
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Zoekt,
            &zoekt_view_status(&scanned.snapshot, &provider_reports),
        )?;
        if let Err(error) = self
            .store
            .prune_snapshots(&scanned.identity.id, self.config.index.snapshot_retention)
        {
            tracing::warn!(%error, "snapshot pruning failed; run `cce gc` to retry");
        }
        if let Err(error) = self.store.gc_artifacts() {
            tracing::warn!(%error, "artifact garbage collection failed; run `cce gc` to retry");
        }
        let manifest = self
            .store
            .view_manifest(&scanned.identity.id, &scanned.snapshot.id)?;
        Ok(IndexReport {
            repository_id: scanned.identity.id,
            snapshot: scanned.snapshot,
            reused_snapshot: false,
            indexed_files: scanned.files.len(),
            parsed_files,
            reused_file_analyses,
            source_units,
            relations: records.relations.len(),
            retrieval_documents: records.documents.len(),
            skipped_large_files: scanned.skipped_large_files,
            skipped_binary_files: scanned.skipped_binary_files,
            skipped_sensitive_files: scanned.skipped_sensitive_files,
            skipped_builtin_files: scanned
                .skipped_builtin_files
                .into_iter()
                .map(|(path, reason)| (path, reason.to_owned()))
                .collect(),
            providers: provider_reports,
            manifest,
        })
    }

    /// Run every applicable provider, store the produced artifacts, and
    /// merge ingested relations into the pending snapshot records. SCIP
    /// edges replace same-id `TreeSitter` rows (higher trust wins).
    async fn ingest_providers(
        &self,
        scanned: &ScannedRepository,
        texts: &HashMap<String, String>,
        parsed: &HashMap<String, crate::ParsedFile>,
        unit_ids: &HashMap<String, Vec<String>>,
        file_entities: &HashMap<String, String>,
        records: &mut SnapshotRecords,
    ) -> Result<Vec<crate::providers::ProviderReport>> {
        if !self.config.providers.enabled {
            return Ok(Vec::new());
        }
        let ranges = scip_ranges(texts, parsed, unit_ids, file_entities);
        let context = crate::scip::ScipContext {
            repository_id: &scanned.identity.id,
            snapshot_id: &scanned.snapshot.id,
            ranges: &ranges,
            texts,
        };
        let repo_root = self.config.repository_root.clone();
        let work_root = self.config.data_root.join("providers");
        let data_root = self.config.data_root.clone();
        let snapshot_id = scanned.snapshot.id.clone();
        let timeout = std::time::Duration::from_secs(self.config.providers.timeout_secs);
        let produced = tokio::task::spawn_blocking(move || {
            let produced = crate::providers::produce(&repo_root, &work_root, timeout);
            // Zoekt is a provider in lifecycle only: its shard directory
            // stays on disk under the data root and serves query-time
            // candidates, so there is no artifact to ingest.
            let zoekt_report =
                crate::zoekt::ensure_report(&repo_root, &data_root, &snapshot_id, timeout);
            (produced, zoekt_report)
        })
        .await
        .map_err(|error| CceError::Configuration(format!("provider runner failed: {error}")))?;
        let (produced, zoekt_report) = produced;
        let mut reports = Vec::with_capacity(produced.len());
        for (mut report, artifact) in produced {
            let Some(crate::providers::ProviderArtifact::ScipIndex { bytes }) = artifact else {
                reports.push(report);
                continue;
            };
            let artifact_record = self
                .store
                .artifacts()
                .put_bytes(ArtifactKind::ScipIndex, &bytes)?;
            report.artifact_digest = Some(artifact_record.digest.clone());
            records.artifacts.push(artifact_record);
            match crate::scip::ingest(&bytes, &context) {
                Ok(outcome) => {
                    report.scip_documents = outcome.documents;
                    report.scip_definitions = outcome.definitions;
                    report.scip_reference_edges = outcome.relations.len();
                    merge_relations(&mut records.relations, outcome.relations);
                    if outcome.foreign_documents > 0 {
                        let skipped = format!(
                            "{} documents skipped — not part of this snapshot",
                            outcome.foreign_documents
                        );
                        // Keep the detection note (e.g. chosen project root).
                        report.message = Some(match report.message.take() {
                            Some(note) => format!("{note}; {skipped}"),
                            None => skipped,
                        });
                    }
                }
                Err(error) => {
                    report.state = crate::providers::ProviderState::Failed;
                    report.message = Some(format!("artifact rejected: {error}"));
                }
            }
            reports.push(report);
        }
        reports.push(zoekt_report);
        Ok(reports)
    }

    /// Extract per-commit diff hunks into `EntityKind::Commit` nodes,
    /// `CommitPatch` artifacts, and `CommitDiff` retrieval documents. All
    /// ids derive from the commit id, so reindexing identical history
    /// reproduces the same rows — never duplicates.
    ///
    /// # Errors
    /// Storage error when an artifact cannot be persisted.
    fn index_commit_diffs(
        &self,
        scanned: &ScannedRepository,
        records: &mut SnapshotRecords,
    ) -> Result<usize> {
        let diffs = crate::history::commit_diffs(
            &self.config.repository_root,
            HISTORY_COMMIT_LIMIT,
            &scanned.identity.id,
            &scanned.snapshot.id,
            self.config.index.include_sensitive,
            self.config.index.respect_gitignore,
        );
        let mut indexed = 0_usize;
        for commit in diffs {
            let artifact = self
                .store
                .artifacts()
                .put_bytes(ArtifactKind::CommitPatch, commit.patch.as_bytes())?;
            let commit_entity_id = entity_id(
                &scanned.identity.id,
                "",
                0,
                0,
                &format!("commit:{}", commit.id),
            );
            let mut attributes = serde_json::Map::new();
            attributes.insert("committerTime".to_owned(), commit.timestamp.into());
            attributes.insert(
                "patchArtifactDigest".to_owned(),
                artifact.digest.clone().into(),
            );
            records.entities.push(CodeEntity {
                id: commit_entity_id.clone(),
                kind: EntityKind::Commit,
                name: commit.id.clone(),
                qualified_name: Some(format!("commit:{}", commit.id)),
                signature: None,
                language: None,
                region_id: None,
                address: None,
                capabilities: vec!["historical_evidence".to_owned()],
                attributes,
            });
            records.documents.push(IndexedDocument {
                document: RetrievalDocument {
                    id: digest_id("doc", &[&commit_entity_id, "commit_diff", &commit.id]),
                    entity_id: commit_entity_id,
                    region_id: None,
                    snapshot_id: scanned.snapshot.id.clone(),
                    representation: RetrievalRepresentation::CommitDiff,
                    body_artifact_digest: artifact.digest.clone(),
                    address: None,
                    embedding_profile: None,
                    generated_by: Some("cce-git-history-diff-v1".to_owned()),
                    evidence: commit.evidence,
                    terms: lexical_terms(&commit.body),
                },
                path: ".git".to_owned(),
                name: format!(
                    "commit {} {}",
                    commit.id.get(..12).unwrap_or(&commit.id),
                    commit.subject
                ),
                body: commit.body,
            });
            records.artifacts.push(artifact);
            indexed += 1;
        }
        Ok(indexed)
    }

    /// Detect-state report for every known provider (no execution).
    #[must_use]
    pub fn providers(&self) -> Vec<crate::providers::ProviderReport> {
        crate::providers::detect_all(&self.config.repository_root)
    }

    /// Worktree regex search honoring ignore/sensitive policy; always
    /// fresh, independent of the snapshot.
    ///
    /// # Errors
    /// `Configuration` on an invalid pattern; scan/I/O errors.
    pub fn grep(&self, request: &crate::GrepRequest) -> Result<crate::GrepReport> {
        let scanner = RepositoryScanner::new(self.config.clone());
        crate::grep::grep(&scanner, &self.store, request)
    }

    fn model_cache_dir(&self) -> std::path::PathBuf {
        self.config.data_root.join("models")
    }

    fn parse_with_cache(
        &self,
        repository_id: &str,
        file: &ScannedFile,
        source: &[u8],
        reused: &mut usize,
    ) -> Result<(crate::ParsedFile, ArtifactRecord)> {
        const CACHE_VERSION: u32 = 1;
        if let Some(digest) = self.store.cached_analysis_digest(
            repository_id,
            &file.relative_path,
            &file.content_hash,
        )? {
            match self.store.artifacts().read(&digest).and_then(|bytes| {
                serde_json::from_slice::<CachedFileAnalysis>(&bytes)
                    .map_err(|error| CceError::Storage(format!("invalid parse cache: {error}")))
            }) {
                Ok(cache)
                    if cache.version == CACHE_VERSION
                        && cache.content_hash == file.content_hash
                        && cache.language == file.language =>
                {
                    *reused += 1;
                    let artifact = self
                        .store
                        .artifacts()
                        .put_bytes(ArtifactKind::Other, &serde_json::to_vec(&cache)?)?;
                    return Ok((cache.parsed, artifact));
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    path = %file.relative_path,
                    %error,
                    "discarding unreadable file-analysis cache"
                ),
            }
        }

        let parsed = self.parser.parse(file, source);
        let cache = CachedFileAnalysis {
            version: CACHE_VERSION,
            language: file.language.clone(),
            content_hash: file.content_hash.clone(),
            parsed: parsed.clone(),
        };
        let artifact = self
            .store
            .artifacts()
            .put_bytes(ArtifactKind::Other, &serde_json::to_vec(&cache)?)?;
        Ok((parsed, artifact))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedFileAnalysis {
    version: u32,
    language: Option<String>,
    content_hash: String,
    parsed: crate::ParsedFile,
}

/// path → entity byte ranges ascending by span (innermost symbol first,
/// whole-file entity last) for SCIP occurrence → enclosing-entity mapping.
fn scip_ranges(
    texts: &HashMap<String, String>,
    parsed: &HashMap<String, crate::ParsedFile>,
    unit_ids: &HashMap<String, Vec<String>>,
    file_entities: &HashMap<String, String>,
) -> HashMap<String, Vec<(usize, usize, String)>> {
    let mut map = HashMap::new();
    for (path, text) in texts {
        let Some(file_id) = file_entities.get(path) else {
            continue;
        };
        let mut ranges: Vec<(usize, usize, String)> = Vec::new();
        if let (Some(file), Some(ids)) = (parsed.get(path), unit_ids.get(path)) {
            for (index, unit) in file.units.iter().enumerate() {
                if let Some(id) = ids.get(index) {
                    ranges.push((unit.start_byte, unit.end_byte, id.clone()));
                }
            }
        }
        ranges.push((0, text.len().max(1), file_id.clone()));
        ranges.sort_by_key(|(start, end, _)| end.saturating_sub(*start));
        map.insert(path.clone(), ranges);
    }
    map
}

/// Merge provider-derived relations into pending records. Relation ids are
/// pure functions of (source, target, kind), so a same-id provider edge
/// replaces the lower-trust row in place; new edges append. One artifact can
/// also carry the same id twice — distinct tool symbols may resolve to the
/// same entity pair — so `incoming` is deduped first (evidence merged).
fn merge_relations(existing: &mut Vec<Relation>, incoming: Vec<Relation>) -> usize {
    let positions: HashMap<&str, usize> = existing
        .iter()
        .enumerate()
        .map(|(index, relation)| (relation.id.as_str(), index))
        .collect();
    let mut replaces = Vec::new();
    let mut appends: Vec<Relation> = Vec::new();
    let mut appended: HashMap<String, usize> = HashMap::new();
    for relation in incoming {
        if let Some(&index) = positions.get(relation.id.as_str()) {
            replaces.push((index, relation));
        } else if let Some(&index) = appended.get(relation.id.as_str()) {
            if let Some(slot) = appends.get_mut(index) {
                slot.evidence.extend(relation.evidence);
            }
        } else {
            appended.insert(relation.id.clone(), appends.len());
            appends.push(relation);
        }
    }
    let replaced = replaces.len();
    for (index, relation) in replaces {
        if let Some(slot) = existing.get_mut(index) {
            *slot = relation;
        }
    }
    existing.extend(appends);
    replaced
}

/// Graph view status driven by provider outcomes: `Ready` when every
/// applicable provider produced an index, `Partial` when coverage is
/// incomplete or no provider applies.
fn graph_view_status(
    snapshot: &SnapshotIdentity,
    reports: &[crate::providers::ProviderReport],
) -> (ViewStatus, usize) {
    use crate::providers::ProviderState;
    let mut capabilities = base_graph_capabilities();
    let mut uncovered = Vec::new();
    let mut scip_edges = 0_usize;
    let mut digest = None;
    for report in reports {
        // Only SCIP providers feed graph coverage — non-artifact
        // providers (zoekt:index) report on their own view instead.
        if !report.provider_id.starts_with("scip:") {
            continue;
        }
        match report.state {
            ProviderState::NotApplicable => {}
            ProviderState::Ready => {
                scip_edges += report.scip_reference_edges;
                digest = digest.or_else(|| report.artifact_digest.clone());
                capabilities.push(Capability {
                    name: report.provider_id.clone(),
                    level: "compiler_derived".to_owned(),
                    reason: Some(format!(
                        "{} definitions, {} reference edges",
                        report.scip_definitions, report.scip_reference_edges
                    )),
                });
            }
            ProviderState::Missing | ProviderState::Failed => {
                uncovered.push(format!(
                    "{}: {}",
                    report.provider_id,
                    report.message.as_deref().unwrap_or("unavailable")
                ));
            }
        }
    }
    let any_applicable = reports.iter().any(|report| {
        report.provider_id.starts_with("scip:") && report.state != ProviderState::NotApplicable
    });
    let (state, message) = graph_state(any_applicable, &uncovered);
    let mut status = status(snapshot, state, capabilities, message);
    status.artifact_digest = digest;
    (status, scip_edges)
}

/// Graph status recomputed during post-commit repair: provider runs are
/// not persisted, so detect-state is re-probed and the compiler-derived
/// edge count comes from committed relations rather than run reports.
fn repaired_graph_status(
    snapshot: &SnapshotIdentity,
    reports: &[crate::providers::ProviderReport],
    scip_edges: usize,
) -> ViewStatus {
    use crate::providers::ProviderState;
    let mut capabilities = base_graph_capabilities();
    let mut uncovered = Vec::new();
    for report in reports {
        if !report.provider_id.starts_with("scip:") {
            continue;
        }
        match report.state {
            ProviderState::NotApplicable => {}
            ProviderState::Ready => capabilities.push(Capability {
                name: report.provider_id.clone(),
                level: "compiler_derived".to_owned(),
                reason: Some("provider output ingested".to_owned()),
            }),
            ProviderState::Missing | ProviderState::Failed => {
                uncovered.push(format!(
                    "{}: {}",
                    report.provider_id,
                    report.message.as_deref().unwrap_or("unavailable")
                ));
            }
        }
    }
    if scip_edges > 0 {
        capabilities.push(Capability {
            name: "reference_edges".to_owned(),
            level: "compiler_derived".to_owned(),
            reason: Some(format!(
                "{scip_edges} committed SCIP reference edges (recomputed post-repair)"
            )),
        });
    }
    let any_applicable = reports.iter().any(|report| {
        report.provider_id.starts_with("scip:") && report.state != ProviderState::NotApplicable
    });
    let (state, message) = graph_state(any_applicable, &uncovered);
    status(snapshot, state, capabilities, message)
}

fn base_graph_capabilities() -> Vec<Capability> {
    vec![
        Capability {
            name: "contains".to_owned(),
            level: "syntax_fact".to_owned(),
            reason: None,
        },
        Capability {
            name: "relative_imports".to_owned(),
            level: "framework_derived".to_owned(),
            reason: Some("relative imports only; no compiler resolution".to_owned()),
        },
    ]
}

fn graph_state(any_applicable: bool, uncovered: &[String]) -> (ViewState, Option<String>) {
    if !any_applicable {
        (
            ViewState::Partial,
            Some("Compiler/SCIP resolved references are not built".to_owned()),
        )
    } else if uncovered.is_empty() {
        (ViewState::Ready, None)
    } else {
        (
            ViewState::Partial,
            Some(format!(
                "SCIP coverage incomplete — {}",
                uncovered.join("; ")
            )),
        )
    }
}

fn source_view_status(snapshot: &SnapshotIdentity) -> ViewStatus {
    status(
        snapshot,
        ViewState::Ready,
        vec![Capability {
            name: "content_addressed_source".to_owned(),
            level: "authoritative".to_owned(),
            reason: None,
        }],
        None,
    )
}

fn lexical_view_status(snapshot: &SnapshotIdentity) -> ViewStatus {
    status(
        snapshot,
        ViewState::Ready,
        vec![Capability {
            name: "sqlite_fts5".to_owned(),
            level: "ready".to_owned(),
            reason: None,
        }],
        None,
    )
}

fn symbols_view_status(snapshot: &SnapshotIdentity, syntax_coverage: f64) -> ViewStatus {
    status(
        snapshot,
        if syntax_coverage >= 0.99 {
            ViewState::Ready
        } else {
            ViewState::Partial
        },
        vec![Capability {
            name: "syntax_symbols".to_owned(),
            level: "syntax_only".to_owned(),
            reason: Some(format!(
                "tree-sitter coverage {:.1}%",
                syntax_coverage * 100.0
            )),
        }],
        None,
    )
}

fn knowledge_view_status(snapshot: &SnapshotIdentity) -> ViewStatus {
    status(
        snapshot,
        ViewState::Partial,
        vec![
            Capability {
                name: "deterministic_role_summary".to_owned(),
                level: "derived".to_owned(),
                reason: Some("hierarchical model synthesis not configured".to_owned()),
            },
            Capability {
                name: "deterministic_hierarchy".to_owned(),
                level: "derived".to_owned(),
                reason: None,
            },
        ],
        None,
    )
}

fn dataflow_view_status(snapshot: &SnapshotIdentity, scip_edges: usize) -> ViewStatus {
    if scip_edges > 0 {
        status(
            snapshot,
            ViewState::Partial,
            vec![Capability {
                name: "scip_def_ref_substrate".to_owned(),
                level: "compiler_derived".to_owned(),
                reason: Some(format!("{scip_edges} definition/reference edges ingested")),
            }],
            Some(
                "SCIP definition/reference graph available; no source→sink \
                 taint analysis yet"
                    .to_owned(),
            ),
        )
    } else {
        status(
            snapshot,
            ViewState::Unavailable,
            Vec::new(),
            Some(
                "No evidence-backed SCIP or static-analysis dataflow artifact was supplied"
                    .to_owned(),
            ),
        )
    }
}

/// Zoekt view status derives straight from its provider report: Ready
/// when the shard set was (re)built for this snapshot, Unavailable with
/// remediation when the toolchain is absent, Failed with diagnostics
/// otherwise. The capability level marks it `external_index` — candidate
/// evidence, not ingested truth.
fn zoekt_view_status(
    snapshot: &SnapshotIdentity,
    reports: &[crate::providers::ProviderReport],
) -> ViewStatus {
    use crate::providers::ProviderState;
    let report = reports
        .iter()
        .find(|report| report.provider_id == "zoekt:index");
    match report.map(|report| &report.state) {
        Some(ProviderState::Ready) => status(
            snapshot,
            ViewState::Ready,
            vec![Capability {
                name: "zoekt_trigram".to_owned(),
                level: "external_index".to_owned(),
                reason: report
                    .and_then(|report| report.tool.clone())
                    .map(|tool| format!("shards via {tool}")),
            }],
            None,
        ),
        Some(ProviderState::Missing | ProviderState::NotApplicable) => status(
            snapshot,
            ViewState::Unavailable,
            Vec::new(),
            report.and_then(|report| report.message.clone()),
        ),
        Some(ProviderState::Failed) | None => status(
            snapshot,
            ViewState::Failed,
            Vec::new(),
            report
                .and_then(|report| report.message.clone())
                .or_else(|| Some("zoekt index build did not report".to_owned())),
        ),
    }
}

/// Commits walked for history extraction (message summary and diff
/// content), newest first; also the coverage bound reported by diff search.
pub(crate) const HISTORY_COMMIT_LIMIT: usize = 512;

fn status(
    snapshot: &SnapshotIdentity,
    state: ViewState,
    capabilities: Vec<Capability>,
    message: Option<String>,
) -> ViewStatus {
    ViewStatus {
        state,
        snapshot_id: snapshot.id.clone(),
        profile_hash: snapshot.index_profile_hash.clone(),
        updated_at: Utc::now(),
        capabilities,
        artifact_digest: None,
        message,
    }
}

pub(crate) fn entity_id(
    repository_id: &str,
    path: &str,
    start: usize,
    end: usize,
    discriminator: &str,
) -> String {
    digest_id(
        "entity",
        &[
            repository_id,
            path,
            &start.to_string(),
            &end.to_string(),
            discriminator,
        ],
    )
}

fn document_id(entity_id: &str, representation: &str, start: usize, end: usize) -> String {
    digest_id(
        "doc",
        &[
            entity_id,
            representation,
            &start.to_string(),
            &end.to_string(),
        ],
    )
}

pub(crate) fn relation_id(source: &str, target: &str, kind: &str) -> String {
    digest_id("rel", &[source, target, kind])
}

fn region_id(path: &str, start: usize, end: usize, kind: RegionKind, symbol: &str) -> String {
    digest_id(
        "region",
        &[
            path,
            &start.to_string(),
            &end.to_string(),
            &format!("{kind:?}"),
            symbol,
        ],
    )
}

/// Compact file-level retrieval document: path, language, leading
/// comment/import block, and top-level symbol signatures. This is the L1
/// routing unit — whole-file bodies are never indexed as documents.
fn file_descriptor(file: &ScannedFile, text: &str, parsed: &crate::ParsedFile) -> String {
    const MAX_DESCRIPTOR_BYTES: usize = 2_048;
    let mut parts = vec![format!(
        "{} ({})",
        file.relative_path,
        file.language.as_deref().unwrap_or("text")
    )];
    // Leading comment/import lines carry module intent and dependency facts.
    let head: String = text
        .lines()
        .take(24)
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("//")
                || trimmed.starts_with('#')
                || trimmed.starts_with("use ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("from ")
                || trimmed.starts_with("mod ")
                || trimmed.starts_with("//!")
        })
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    if !head.is_empty() {
        parts.push(head);
    }
    let signatures = parsed
        .units
        .iter()
        .filter(|unit| unit.parent_unit.is_none())
        .filter_map(|unit| {
            unit.signature
                .as_deref()
                .map(str::trim)
                .filter(|signature| !signature.is_empty())
                .map(str::to_owned)
                .or_else(|| Some(format!("{:?} {}", unit.kind, unit.name)))
        })
        .take(24)
        .collect::<Vec<_>>()
        .join("\n");
    if !signatures.is_empty() {
        parts.push(signatures);
    }
    let mut body = parts.join("\n\n");
    body.truncate(MAX_DESCRIPTOR_BYTES);
    body
}

fn digest_id(prefix: &str, components: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for component in components {
        hasher.update(component.as_bytes());
        hasher.update(&[0]);
    }
    format!("{prefix}_{}", &hasher.finalize().to_hex()[..32])
}

fn address(
    repository_id: &str,
    snapshot_id: &str,
    path: &str,
    text: &str,
    start: usize,
    end: usize,
    symbol_id: Option<&str>,
) -> Result<SourceAddress> {
    if start > end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return Err(CceError::InvalidSourceRange {
            path: path.to_owned(),
            start_byte: start as u64,
            end_byte: end as u64,
        });
    }
    let start_line = line_for_offset(text, start);
    let end_line = line_for_offset(text, end);
    let value = SourceAddress::new(
        repository_id,
        snapshot_id,
        path,
        start as u64..end as u64,
        start_line..=end_line,
    )?;
    Ok(match symbol_id {
        Some(id) => value.with_symbol(id),
        None => value,
    })
}

fn line_for_offset(text: &str, offset: usize) -> u32 {
    text.as_bytes()
        .get(..offset.min(text.len()))
        .unwrap_or(text.as_bytes())
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1
}

fn chunk_ranges(text: &str, start: usize, end: usize, maximum: usize) -> Vec<(usize, usize)> {
    if end.saturating_sub(start) <= maximum || maximum == 0 {
        return vec![(start, end)];
    }
    let mut ranges = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let mut chunk_end = cursor.saturating_add(maximum).min(end);
        while chunk_end > cursor && !text.is_char_boundary(chunk_end) {
            chunk_end -= 1;
        }
        if chunk_end < end {
            if let Some(newline) = text
                .get(cursor..chunk_end)
                .and_then(|slice| slice.rfind('\n'))
            {
                if newline > maximum / 2 {
                    chunk_end = cursor + newline + 1;
                }
            }
        }
        if chunk_end == cursor {
            break;
        }
        ranges.push((cursor, chunk_end));
        cursor = chunk_end;
    }
    ranges
}

fn qualified_name(path: &str, parsed: &crate::ParsedFile, index: usize) -> String {
    let Some(root) = parsed.units.get(index) else {
        return path.to_owned();
    };
    let mut names = vec![root.name.as_str()];
    let mut parent = root.parent_unit;
    while let Some(parent_index) = parent {
        let Some(unit) = parsed.units.get(parent_index) else {
            break;
        };
        names.push(unit.name.as_str());
        parent = unit.parent_unit;
    }
    names.reverse();
    format!("{}::{}", path, names.join("::"))
}

/// Deterministic descriptor for one symbol: kind + qualified name + file +
/// signature, phrased so both FTS and dense models can bridge
/// natural-language queries onto identifier-shaped implementations.
/// Deterministic contextual descriptor for a symbol — the index-time half
/// of contextual retrieval, built without a model. Natural-language queries
/// ("how are hits merged into one ranking") share almost no vocabulary with
/// identifier spellings (`add_candidate`), so the descriptor carries every
/// free semantic surface the parse already produced: split identifier
/// words, the doc comment, callee spellings, and type references.
fn symbol_descriptor(
    path: &str,
    file_text: &str,
    parsed: &crate::ParsedFile,
    index: usize,
) -> String {
    let Some(unit) = parsed.units.get(index) else {
        return path.to_owned();
    };
    let kind = format!("{:?}", unit.kind).to_ascii_lowercase();
    let qualified = qualified_name(path, parsed, index);
    let mut descriptor = format!("{kind} {qualified} in {path}");
    let split_words = split_identifier_words(&unit.name);
    if !split_words.is_empty() && split_words != unit.name.to_ascii_lowercase() {
        let _ = write!(descriptor, " ({split_words})");
    }
    if let Some(signature) = &unit.signature {
        descriptor.push_str(" — ");
        descriptor.push_str(signature);
    }
    if let Some(doc) = leading_doc_comment(file_text, unit.start_line) {
        descriptor.push('\n');
        descriptor.push_str(&doc);
    }
    let mut callees = parsed
        .calls
        .iter()
        .filter(|call| call.caller == Some(index))
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>();
    callees.sort_unstable();
    callees.dedup();
    callees.truncate(12);
    if !callees.is_empty() {
        let _ = write!(descriptor, "\ncalls: {}", callees.join(", "));
    }
    if !unit.type_references.is_empty() {
        let _ = write!(
            descriptor,
            "\nuses types: {}",
            unit.type_references.join(", ")
        );
    }
    descriptor
}

/// `add_candidate`/`addCandidate`/`HTTPServer` -> "add candidate" etc. —
/// the natural-language words hidden inside an identifier spelling.
fn split_identifier_words(name: &str) -> String {
    lexical_terms(name).join(" ")
}

/// Leading doc/comment lines above a unit, language-agnostic: contiguous
/// `///`, `//!`, `//`, `#`, `--`, or `#[doc = "..."]` lines, or a `*/`
/// block walked back to its `/*`. Capped at a few short lines — the goal
/// is topical vocabulary, not a doc extract.
fn leading_doc_comment(file_text: &str, start_line: u32) -> Option<String> {
    let lines: Vec<&str> = file_text.lines().collect();
    let mut cursor = start_line as usize;
    let mut collected: Vec<String> = Vec::new();
    while cursor > 0 && collected.len() < 4 {
        cursor -= 1;
        let Some(trimmed) = lines.get(cursor).map(|line| line.trim()) else {
            break;
        };
        if trimmed.is_empty() {
            if collected.is_empty() {
                continue;
            }
            break;
        }
        if trimmed.starts_with("#[") {
            if let Some(quoted) = trimmed.strip_prefix("#[doc").and_then(|rest| {
                rest.trim_start_matches([' ', '='])
                    .strip_prefix('"')
                    .and_then(|rest| rest.rsplit('"').nth(1))
            }) {
                collected.push(quoted.trim().to_owned());
            }
            // Other attributes belong to the unit, not the comment.
            continue;
        }
        if let Some(body) = trimmed
            .strip_prefix("///")
            .or_else(|| trimmed.strip_prefix("//!"))
            .or_else(|| trimmed.strip_prefix("//"))
            .or_else(|| trimmed.strip_prefix("--"))
            .or_else(|| trimmed.strip_prefix('#'))
        {
            collected.push(body.trim().to_owned());
            continue;
        }
        if trimmed.ends_with("*/") {
            // Walk back to the block comment opener, collecting content.
            while cursor > 0 {
                let Some(inner) = lines.get(cursor).map(|line| line.trim()) else {
                    break;
                };
                let is_open = inner.contains("/*");
                let body = inner
                    .trim_end_matches("*/")
                    .trim_start_matches("/*")
                    .trim_start_matches('*')
                    .trim();
                if !body.is_empty() {
                    collected.push(body.to_owned());
                }
                cursor -= 1;
                if is_open {
                    break;
                }
            }
            continue;
        }
        // Decorators belong to the unit, not the comment.
        if trimmed.starts_with('@') {
            continue;
        }
        break;
    }
    collected.reverse();
    let doc = collected.join(" ").chars().take(400).collect::<String>();
    (!doc.is_empty()).then_some(doc)
}

pub(crate) fn is_test(path: &str, name: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    path.contains("test")
        || path.contains("spec")
        || name.starts_with("test_")
        || name.ends_with("_test")
        || matches!(name.as_str(), "test" | "tests" | "testing")
}

fn deterministic_role_summary(file: &ScannedFile, parsed: &crate::ParsedFile) -> String {
    let mut kinds = HashMap::<String, Vec<&str>>::new();
    for unit in &parsed.units {
        kinds
            .entry(format!("{:?}", unit.kind).to_ascii_lowercase())
            .or_default()
            .push(&unit.name);
    }
    let mut groups = kinds.into_iter().collect::<Vec<_>>();
    groups.sort_by(|left, right| left.0.cmp(&right.0));
    let definitions = groups
        .into_iter()
        .map(|(kind, names)| format!("{kind}: {}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "File `{}` is a {} source artifact. Parser status: {}{}. Defined entities: {}. This summary is deterministic navigation metadata and must be verified against the cited source.",
        file.relative_path,
        file.language.as_deref().unwrap_or("unknown-language"),
        if parsed.parsed {
            "parsed"
        } else {
            "file-level only"
        },
        if parsed.has_syntax_errors {
            " with syntax errors"
        } else {
            ""
        },
        if definitions.is_empty() {
            "none extracted"
        } else {
            &definitions
        },
    )
}

fn lexical_terms(value: &str) -> Vec<String> {
    let mut terms = cce_core::split_identifier_terms(value);
    terms.truncate(128);
    terms
}

fn add_hierarchical_knowledge(
    store: &MetadataStore,
    snapshot: &SnapshotIdentity,
    files: &[ScannedFile],
    texts: &HashMap<String, String>,
    file_entities: &HashMap<String, String>,
    repository_entity_id: &str,
    records: &mut SnapshotRecords,
) -> Result<()> {
    let mut groups = BTreeMap::<String, Vec<&ScannedFile>>::new();
    for file in files {
        let group = knowledge_group(&file.relative_path);
        groups.entry(group).or_default().push(file);
    }

    let mut overview = vec![
        "# Repository orientation".to_owned(),
        String::new(),
        format!(
            "Snapshot `{}` contains {} indexed source artifacts.",
            snapshot.id,
            files.len()
        ),
        String::new(),
        "## Top-level areas".to_owned(),
    ];
    for (group, members) in &groups {
        overview.push(format!("- `{group}`: {} files", members.len()));
        let directory_id = entity_id(&snapshot.repository_id, group, 0, 0, "directory");
        records.entities.push(CodeEntity {
            id: directory_id.clone(),
            kind: EntityKind::Directory,
            name: group.clone(),
            qualified_name: Some(group.clone()),
            signature: None,
            language: None,
            region_id: None,
            address: None,
            capabilities: vec!["deterministic_hierarchy".to_owned()],
            attributes: serde_json::Map::new(),
        });
        records.relations.push(Relation {
            id: relation_id(repository_entity_id, &directory_id, "contains"),
            source_entity_id: repository_entity_id.to_owned(),
            target_entity_id: directory_id.clone(),
            kind: RelationKind::Contains,
            origin: RelationOrigin::FrameworkRule,
            confidence: 1.0,
            snapshot_id: snapshot.id.clone(),
            extractor: "cce-path-hierarchy-v1".to_owned(),
            evidence: Vec::new(),
            attributes: serde_json::Map::new(),
        });

        let mut page = vec![format!("# Area: {group}"), String::new()];
        let mut evidence = Vec::new();
        for file in members {
            page.push(format!(
                "- `{}` ({})",
                file.relative_path,
                file.language.as_deref().unwrap_or("text")
            ));
            if let Some(file_id) = file_entities.get(&file.relative_path) {
                records.relations.push(Relation {
                    id: relation_id(&directory_id, file_id, "contains"),
                    source_entity_id: directory_id.clone(),
                    target_entity_id: file_id.clone(),
                    kind: RelationKind::Contains,
                    origin: RelationOrigin::FrameworkRule,
                    confidence: 1.0,
                    snapshot_id: snapshot.id.clone(),
                    extractor: "cce-path-hierarchy-v1".to_owned(),
                    evidence: Vec::new(),
                    attributes: serde_json::Map::new(),
                });
                let text = texts
                    .get(&file.relative_path)
                    .ok_or_else(|| CceError::Configuration("file text disappeared".to_owned()))?;
                let source = address(
                    &snapshot.repository_id,
                    &snapshot.id,
                    &file.relative_path,
                    text,
                    0,
                    text.len(),
                    Some(file_id),
                )?;
                if evidence.len() < 16 {
                    evidence.push(source);
                }
            }
        }
        page.push(String::new());
        page.push(
            "This page is deterministic navigation metadata; use its evidence addresses to verify current source."
                .to_owned(),
        );
        let body = page.join("\n");
        let artifact = store
            .artifacts()
            .put_bytes(ArtifactKind::Knowledge, body.as_bytes())?;
        records.artifacts.push(artifact.clone());
        records.documents.push(IndexedDocument {
            document: RetrievalDocument {
                id: document_id(&directory_id, "knowledge_page", 0, body.len()),
                entity_id: directory_id,
                region_id: None,
                snapshot_id: snapshot.id.clone(),
                representation: RetrievalRepresentation::KnowledgePage,
                body_artifact_digest: artifact.digest,
                address: None,
                embedding_profile: None,
                generated_by: Some("cce-deterministic-hierarchy-v1".to_owned()),
                evidence,
                terms: lexical_terms(&body),
            },
            path: group.clone(),
            name: format!("area {group}"),
            body,
        });
    }

    overview.push(String::new());
    overview.push(
        "This orientation is derived from the current path hierarchy and remains subordinate to source truth."
            .to_owned(),
    );
    let body = overview.join("\n");
    let artifact = store
        .artifacts()
        .put_bytes(ArtifactKind::Knowledge, body.as_bytes())?;
    records.artifacts.push(artifact.clone());
    records.documents.push(IndexedDocument {
        document: RetrievalDocument {
            id: document_id(repository_entity_id, "knowledge_page", 0, body.len()),
            entity_id: repository_entity_id.to_owned(),
            region_id: None,
            snapshot_id: snapshot.id.clone(),
            representation: RetrievalRepresentation::KnowledgePage,
            body_artifact_digest: artifact.digest,
            address: None,
            embedding_profile: None,
            generated_by: Some("cce-deterministic-hierarchy-v1".to_owned()),
            evidence: Vec::new(),
            terms: lexical_terms(&body),
        },
        path: String::new(),
        name: "repository orientation".to_owned(),
        body,
    });
    Ok(())
}

fn knowledge_group(path: &str) -> String {
    let mut components = path.split('/');
    let first = components.next().unwrap_or("(root)");
    let second = components.next();
    match (first, second) {
        ("crates" | "apps", Some(second)) => format!("{first}/{second}"),
        (_, Some(_)) => first.to_owned(),
        _ => "(root)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_on_utf8_and_newline_boundaries() {
        let text = "fn one() {}\nfn 二() {}\nfn three() {}\n";
        let ranges = chunk_ranges(text, 0, text.len(), 18);
        assert!(ranges.len() > 1);
        assert!(
            ranges
                .iter()
                .all(|(start, end)| text.is_char_boundary(*start) && text.is_char_boundary(*end))
        );
    }

    #[test]
    fn extracts_camel_case_terms() {
        let terms = lexical_terms("resumeAttempt workspace_overlay");
        assert!(terms.contains(&"resume".to_owned()));
        assert!(terms.contains(&"attempt".to_owned()));
    }
}
