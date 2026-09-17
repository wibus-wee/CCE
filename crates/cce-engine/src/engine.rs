use std::collections::{BTreeMap, HashMap};

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
    ScannedFile, SourceParser,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReport {
    pub repository_id: String,
    pub snapshot: SnapshotIdentity,
    pub reused_snapshot: bool,
    pub indexed_files: usize,
    pub parsed_files: usize,
    pub reused_file_analyses: usize,
    pub source_units: usize,
    pub relations: usize,
    pub retrieval_documents: usize,
    pub skipped_large_files: Vec<String>,
    pub skipped_binary_files: Vec<String>,
    pub skipped_sensitive_files: Vec<String>,
    /// Files dropped by the unconditional built-in policy (lockfiles,
    /// minified assets), each paired with its skip reason.
    pub skipped_builtin_files: Vec<(String, String)>,
    pub manifest: ViewManifest,
}

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
}

impl CceEngine {
    pub fn open(config: EngineConfig) -> Result<Self> {
        let store = MetadataStore::open(&config.data_root)?;
        Ok(Self {
            config,
            store,
            parser: SourceParser::new(),
            embedder: tokio::sync::OnceCell::new(),
        })
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

    #[must_use]
    pub const fn store(&self) -> &MetadataStore {
        &self.store
    }

    #[must_use]
    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

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

    pub async fn index(&self) -> Result<IndexReport> {
        // Scan before taking the write lease: an unchanged repository returns
        // without ever contending with concurrent readers or writers.
        let scanned = RepositoryScanner::new(self.config.clone()).scan(Some(&self.store))?;
        self.store.register_repository(&scanned.identity)?;
        if self.store.snapshot_is_complete(&scanned.snapshot.id)? {
            let manifest = self
                .store
                .view_manifest(&scanned.identity.id, &scanned.snapshot.id)?;
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
                manifest,
            });
        }

        // Only the write path needs the lease. The scanned hashes pin the
        // snapshot; if the tree changes mid-build the byte-level hash check
        // in `ScannedFile::bytes` fails the index instead of committing a
        // snapshot that misidentifies content.
        let _lease = crate::lock::IndexLease::acquire(&self.config.data_root)?;
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
                attributes: serde_json::Map::new(),
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

            for (index, unit) in parsed.units.iter().enumerate() {
                let unit_id = unit_ids[index].clone();
                let unit_region_id = unit_region_ids[index].clone();
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
                            representation: if is_test(file, &unit.name) {
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
                let summary_body = symbol_descriptor(&file.relative_path, parsed, index);
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

            for (index, unit) in parsed.units.iter().enumerate() {
                name_index.entry(unit.name.clone()).or_default().push(
                    crate::relations::SymbolCandidate {
                        entity_id: unit_ids[index].clone(),
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

        let history_view_status = match crate::history::summarize(&self.config.repository_root, 512)
        {
            Ok(Some(summary)) => {
                let artifact = self
                    .store
                    .artifacts()
                    .put_bytes(ArtifactKind::Knowledge, summary.body.as_bytes())?;
                let mut history_status = status(
                    &scanned.snapshot,
                    ViewState::Ready,
                    vec![Capability {
                        name: "git_commit_messages".to_owned(),
                        level: "historical_evidence".to_owned(),
                        reason: Some(format!(
                            "{} reachable commits indexed with gitoxide",
                            summary.commit_count
                        )),
                    }],
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
            &status(
                &scanned.snapshot,
                ViewState::Ready,
                vec![Capability {
                    name: "content_addressed_source".to_owned(),
                    level: "authoritative".to_owned(),
                    reason: None,
                }],
                None,
            ),
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Lexical,
            &status(
                &scanned.snapshot,
                ViewState::Ready,
                vec![Capability {
                    name: "sqlite_fts5".to_owned(),
                    level: "ready".to_owned(),
                    reason: None,
                }],
                None,
            ),
        )?;
        let structural_state = if syntax_coverage >= 0.99 {
            ViewState::Ready
        } else {
            ViewState::Partial
        };
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Symbols,
            &status(
                &scanned.snapshot,
                structural_state,
                vec![Capability {
                    name: "syntax_symbols".to_owned(),
                    level: "syntax_only".to_owned(),
                    reason: Some(format!(
                        "tree-sitter coverage {:.1}%",
                        syntax_coverage * 100.0
                    )),
                }],
                None,
            ),
        )?;
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Graph,
            &status(
                &scanned.snapshot,
                ViewState::Partial,
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
                ],
                Some("Compiler/SCIP resolved references are not built".to_owned()),
            ),
        )?;
        if let Some(embedder) = self.embedder().await? {
            self.store.set_view_status(
                &scanned.identity.id,
                &scanned.snapshot.id,
                ViewKind::Dense,
                &status(
                    &scanned.snapshot,
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
                DenseBackendConfig::DeterministicBaseline { .. } | DenseBackendConfig::Disabled => {
                    64
                }
            };
            let dense_result: Result<ArtifactRecord> = async {
                let documents = self.store.documents_for_snapshot(&scanned.snapshot.id)?;
                let index = DenseIndex::build(&documents, &embedder, batch_size).await?;
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
                        &scanned.snapshot,
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
                        &scanned.identity.id,
                        &scanned.snapshot.id,
                        ViewKind::Dense,
                        &dense_status,
                    )?;
                }
                Err(error) => {
                    self.store.set_view_status(
                        &scanned.identity.id,
                        &scanned.snapshot.id,
                        ViewKind::Dense,
                        &status(
                            &scanned.snapshot,
                            ViewState::Failed,
                            vec![],
                            Some(error.to_string()),
                        ),
                    )?;
                    return Err(error);
                }
            }
        } else {
            self.store.set_view_status(
                &scanned.identity.id,
                &scanned.snapshot.id,
                ViewKind::Dense,
                &status(
                    &scanned.snapshot,
                    ViewState::Unavailable,
                    vec![],
                    Some(
                        "No embedding backend selected; lexical and structural views remain available"
                            .to_owned(),
                    ),
                ),
            )?;
        }
        self.store.set_view_status(
            &scanned.identity.id,
            &scanned.snapshot.id,
            ViewKind::Knowledge,
            &status(
                &scanned.snapshot,
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
            ),
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
            &status(
                &scanned.snapshot,
                ViewState::Unavailable,
                Vec::new(),
                Some(
                    "No evidence-backed SCIP or static-analysis dataflow artifact was supplied"
                        .to_owned(),
                ),
            ),
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
            manifest,
        })
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
    Ok(symbol_id.map_or(value.clone(), |id| value.with_symbol(id)))
}

fn line_for_offset(text: &str, offset: usize) -> u32 {
    text.as_bytes()[..offset.min(text.len())]
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
            if let Some(newline) = text[cursor..chunk_end].rfind('\n') {
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
    let mut names = vec![parsed.units[index].name.as_str()];
    let mut parent = parsed.units[index].parent_unit;
    while let Some(parent_index) = parent {
        names.push(parsed.units[parent_index].name.as_str());
        parent = parsed.units[parent_index].parent_unit;
    }
    names.reverse();
    format!("{}::{}", path, names.join("::"))
}

/// Deterministic descriptor for one symbol: kind + qualified name + file +
/// signature, phrased so both FTS and dense models can bridge
/// natural-language queries onto identifier-shaped implementations.
fn symbol_descriptor(path: &str, parsed: &crate::ParsedFile, index: usize) -> String {
    let unit = &parsed.units[index];
    let kind = format!("{:?}", unit.kind).to_ascii_lowercase();
    let qualified = qualified_name(path, parsed, index);
    let mut descriptor = format!("{kind} {qualified} in {path}");
    if let Some(signature) = &unit.signature {
        descriptor.push_str(" — ");
        descriptor.push_str(signature);
    }
    descriptor
}

fn is_test(file: &ScannedFile, name: &str) -> bool {
    let path = file.relative_path.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    path.contains("test")
        || path.contains("spec")
        || name.starts_with("test_")
        || name.ends_with("_test")
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
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if previous_lower && character.is_uppercase() && !current.is_empty() {
                terms.push(current.to_ascii_lowercase());
                current.clear();
            }
            previous_lower = character.is_lowercase();
            current.push(character);
        } else if !current.is_empty() {
            terms.push(current.to_ascii_lowercase());
            current.clear();
            previous_lower = false;
        }
    }
    if !current.is_empty() {
        terms.push(current.to_ascii_lowercase());
    }
    terms.sort();
    terms.dedup();
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
