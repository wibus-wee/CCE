use std::{collections::HashMap, time::Instant};

use cce_core::{
    BindingScope, BoundArtifact, ClaimFrame, ClaimPredicate, CodeEntity, DefinedWitness,
    DocumentClass, DrillDown, EntityKind, EvidenceTiers, QueryIntent, RelationKind, RelationOrigin,
    RelationWitness, Result, RetrievalRepresentation, SearchHit, SearchRequest, SearchRoute,
    SearchVerdict, TermWitness, VerdictState, ViewKind, ViewManifest, ViewState, WitnessReport,
    WitnessRequirement, document_class, folded_identifier, has_cjk,
};
use cce_store::{MetadataStore, RelationDirection};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    CceEngine, Embedder, GraphPolicy, QueryPlan, QueryPlanner, RepositoryScanner, RetrievalCounters,
};

const RRF_K: f64 = 60.0;

/// How many head hits the cross-encoder re-scores. Reranker cost is linear
/// in pairs; beyond ~50 the fused prior is already thin.
const RERANK_POOL: usize = 50;

/// Only topical hits are scored by the reranker: graph-expansion routes
/// (structural/knowledge/history) answer "what is connected", not "what
/// matches the query text" — judging them on topical relevance punishes
/// blast-radius evidence.
const TOPICAL: [SearchRoute; 6] = [
    SearchRoute::Lexical,
    SearchRoute::Zoekt,
    SearchRoute::DenseRaw,
    SearchRoute::DenseSummary,
    SearchRoute::ExactSymbol,
    SearchRoute::Hybrid,
];

/// Zoekt route bounds: files per query (candidate pool depth), regions
/// resolved per file, and the subprocess deadline. The trigram CLI
/// answers in ~75 ms at django scale, so the timeout is generous slack,
/// not an expected cost.
const ZOEKT_FILE_LIMIT_FACTOR: usize = 2;
const ZOEKT_REGIONS_PER_FILE: usize = 4;
const ZOEKT_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How many head topical hits the feedback pass mines for expansion terms.
/// Beyond ~5 the fused ranking is already thin and noise starts to dominate
/// the term vocabulary.
const PRF_FEEDBACK_DOCS: usize = 5;

/// Terms appended to the second-pass query: enough to bridge a vocabulary
/// mismatch without diluting the original intent terms.
const PRF_EXPANSION_TERMS: usize = 8;

/// Second-pass RRF numerator relative to the first-pass lexical weight.
/// Expansion-only evidence should land in the tail where structural
/// features and rerank can still promote it; documents confirmed by both
/// passes gain additive fused score, which is where promotion happens.
const PRF_ATTENUATION: f64 = 0.6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// The full result of a search: the executed plan, view manifest, ranked
/// hits, and explicit missing capabilities.
pub struct SearchResult {
    /// The effective request (repository/snapshot resolved).
    pub request: SearchRequest,
    /// The plan that was executed.
    pub plan: QueryPlan,
    /// View manifest at query time (staleness surfaced to caller).
    pub manifest: ViewManifest,
    /// Ranked hits.
    pub hits: Vec<SearchHit>,
    /// Capabilities the plan needed but could not satisfy.
    pub missing_capabilities: Vec<String>,
    /// The kernel's evidence verdict — what the evidence is and where it
    /// fails witness typing, exported so an external verifier can judge
    /// instead of re-deriving.
    pub verdict: SearchVerdict,
    /// Engine-side wall time in milliseconds.
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    hit: SearchHit,
    fused_score: f64,
    /// Admitted by at least one strict-tier pass — a literal match on the
    /// query's own terms (exact symbol, first-pass lexical, knowledge /
    /// history / diff FTS matches). Dense similarity, structural and flow
    /// expansion, and PRF-expanded vocabulary are inferred vicinity: they
    /// rerank real evidence but never constitute it on their own — the
    /// distinction the abstention gate reads.
    strict: bool,
}

/// Where graph expansion went, recorded beside the candidate map on
/// purpose: `add_candidate` keys hits by `document_id`, so the
/// `entity:`-keyed hits expansion emits can never merge into — and thus
/// never corroborate — the doc-keyed candidates describing the same
/// code. This side channel lets the feature pass join the evidence back.
#[derive(Debug, Default)]
struct ExpansionEvidence {
    /// entity id → (best propagated score, expansion edges surfacing it)
    entities: HashMap<String, (f64, usize)>,
    /// region id → same aggregation, for region-level document matches
    regions: HashMap<String, (f64, usize)>,
    /// file path → same aggregation; a file holding any surfaced entity
    /// is graph-adjacent to a seed at either file- or symbol-level
    /// granularity (a surfaced `ChangedWith` file entity and a surfaced
    /// `Calls` symbol both map here through `address.path`).
    paths: HashMap<String, (f64, usize)>,
    /// Best propagated score overall — normalizes strength to (0, 1].
    max_score: f64,
}

impl ExpansionEvidence {
    fn record(&mut self, entity: &CodeEntity, propagated: f64) {
        fn accumulate(map: &mut HashMap<String, (f64, usize)>, key: &str, score: f64) {
            let entry = map.entry(key.to_owned()).or_default();
            entry.0 = entry.0.max(score);
            entry.1 += 1;
        }
        accumulate(&mut self.entities, &entity.id, propagated);
        if let Some(region_id) = &entity.region_id {
            accumulate(&mut self.regions, region_id, propagated);
        }
        if let Some(address) = &entity.address {
            accumulate(&mut self.paths, &address.path, propagated);
        }
        self.max_score = self.max_score.max(propagated);
    }
}

/// The snapshot a search will run against, with a record of whether the
/// working tree was actually verified to match it.
#[derive(Debug)]
struct ResolvedIndex {
    repository_id: String,
    snapshot_id: String,
    manifest: ViewManifest,
    verified_fresh: bool,
}

impl CceEngine {
    /// Resolve the snapshot for this request. `require_fresh` runs the full
    /// scan-and-index path so results always match the working tree (or the
    /// request fails). Without it, the last committed snapshot is used as-is —
    /// no working-tree scan happens — and hits are marked unverified.
    async fn resolve_index(&self, require_fresh: bool) -> Result<ResolvedIndex> {
        if !require_fresh {
            let anchor = RepositoryScanner::new(self.config().clone()).identify()?;
            if let Some(snapshot_id) = self.store().current_snapshot(&anchor.identity.id)? {
                if self.store().snapshot_is_complete(&snapshot_id)? {
                    return Ok(ResolvedIndex {
                        repository_id: anchor.identity.id.clone(),
                        manifest: self
                            .store()
                            .view_manifest(&anchor.identity.id, &snapshot_id)?,
                        snapshot_id,
                        verified_fresh: false,
                    });
                }
            }
        }
        let report = self.index().await?;
        Ok(ResolvedIndex {
            repository_id: report.repository_id,
            snapshot_id: report.snapshot.id,
            manifest: report.manifest,
            verified_fresh: true,
        })
    }

    /// Execute a search: resolve the snapshot, plan routes, gather hits,
    /// fuse, and attach manifest/missing-capability reporting.
    ///
    /// # Errors
    /// `ViewUnavailable` when no committed snapshot exists and
    /// `require_fresh`/`require_current` demand one.
    pub async fn search(&self, mut request: SearchRequest) -> Result<SearchResult> {
        let started = Instant::now();
        let resolved = self.resolve_index(request.require_fresh).await?;
        request.repository_id.clone_from(&resolved.repository_id);
        request.snapshot_id.clone_from(&resolved.snapshot_id);
        // `lang:`/`path:` tokens become structured filters and leave the
        // query text; an explicitly-set `filters` field wins and the query
        // is used verbatim.
        if request.filters.is_empty() {
            let (query, filters) = cce_core::parse_query_filters(&request.query);
            request.query = query;
            request.filters = filters;
        }
        // A pure/mixed-CJK query has no Latin anchor into English source
        // vocabulary, so the lexical route expands it through a curated
        // glossary. The expansion stays local to the lexical passes: dense
        // embeddings handle CJK natively, and exact-symbol mines
        // `entity_tokens`, which glossary words would only pollute.
        let glossary_terms = cjk_glossary_terms(&request.query);
        let lexical_query = if glossary_terms.is_empty() {
            request.query.clone()
        } else {
            format!("{} {}", request.query, glossary_terms.join(" "))
        };
        let verified_fresh = resolved.verified_fresh;
        let mut plan = QueryPlanner::new().plan(&request.query, request.intent);
        if !request.routes.is_empty() {
            plan.routes.clone_from(&request.routes);
            plan.required_views = required_views_for_routes(&request.routes);
            plan.reasons
                .push("caller supplied an explicit retrieval route override".to_owned());
        }
        // `type:` changes what the query means rather than which hits rank:
        // `diff` is a query-time regex over stored commit patches, `commit`
        // restricts retrieval to history documents, `file` is the default.
        match request.filters.hit_type.as_deref() {
            Some("diff") => {
                plan.routes = vec![SearchRoute::Diff];
                plan.required_views = required_views_for_routes(&plan.routes);
                plan.reasons
                    .push("type:diff pinned the diff route".to_owned());
            }
            Some("commit") => {
                plan.routes = vec![SearchRoute::History];
                plan.required_views = required_views_for_routes(&plan.routes);
                plan.reasons
                    .push("type:commit restricted retrieval to the history route".to_owned());
            }
            _ => {}
        }
        let manifest = resolved.manifest;
        let mut missing_capabilities = missing_views(&manifest, &plan);
        if !verified_fresh {
            missing_capabilities.push(
                "requireFresh=false: served from the last committed snapshot; working-tree changes may not be indexed"
                    .to_owned(),
            );
        }
        // Evidence gate, literal side: every anchor the query states
        // verbatim — a backticked spelling, or an identifier-shaped or
        // sole-anchor entity token — is checked for corpus presence. When
        // ALL of them are absent the query names things that provably do
        // not exist in this snapshot: no route can legitimately answer,
        // and inferred-vicinity machinery (dense, expansion, flow) must
        // not be given the chance to fabricate plausible support.
        let anchors = literal_anchors(&request.query);
        // The claim the evidence will be judged against: subjects are
        // the verbatim anchors, predicate/required-witness follow intent.
        // `distinguishing_terms` resolves lazily — it needs corpus dfs.
        let mut frame = claim_frame(plan.intent, anchors.clone());
        // Temporal routes read evidence the literal check can misjudge:
        // `type:diff` greps commit *patch payloads* in the artifact store —
        // the authoritative corpus for diff claims. Indexed diff documents
        // are a bounded projection of those payloads (diff limits, line
        // windows), so a term absent from FTS can still live in the raw
        // patch. The veto must not preempt the authoritative scan.
        let patch_grep_planned = plan.routes.contains(&SearchRoute::Diff);
        if !anchors.is_empty() && !patch_grep_planned {
            let mut absent = Vec::new();
            for anchor in &anchors {
                // `== 0` only needs to know one row exists — cap the count.
                if self
                    .store()
                    .term_document_frequency_capped(&request.snapshot_id, anchor, 1)?
                    == 0
                {
                    absent.push(anchor.as_str());
                }
            }
            if absent.len() == anchors.len() {
                let reason = format!(
                    "abstained: literal query term{} {} absent from the corpus — the named entity does not exist in this snapshot",
                    if absent.len() == 1 { "" } else { "s" },
                    absent
                        .iter()
                        .map(|anchor| format!("`{anchor}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return abstain_result(
                    self.store(),
                    AbstainContext {
                        request,
                        plan,
                        manifest,
                        missing_capabilities,
                        frame,
                        started,
                    },
                    reason,
                );
            }
        }
        // Witness gate, definition side: an exact-entity claim asks where
        // its subject is *defined*. When every subject is mention-only —
        // present in prose or identifiers that never resolve to a
        // definition — the claim is provably unanswerable in this
        // snapshot, so no route spends work. File entities count as
        // definitions: a module bearing the name is a witness.
        //
        // Exclusively-temporal plans (`type:diff`, `type:commit`, or an
        // explicit history/diff route override) ask whether the subject
        // *ever* existed — "never defined now" cannot refute "defined
        // then", so the gate must not preempt the temporal routes.
        let temporal_only = plan
            .routes
            .iter()
            .all(|route| matches!(route, SearchRoute::Diff | SearchRoute::History));
        if frame.required_witness == WitnessRequirement::Definition
            && !frame.subjects.is_empty()
            && !temporal_only
        {
            let mut any_defined = false;
            for subject in &frame.subjects {
                if !defined_witnesses(self.store(), &request.snapshot_id, subject)?.is_empty() {
                    any_defined = true;
                    break;
                }
            }
            if !any_defined {
                let reason = format!(
                    "abstained: claim subject{} {} ha{} no definition-tier witness — mentioned at most, never defined in this snapshot",
                    if frame.subjects.len() == 1 { "" } else { "s" },
                    frame
                        .subjects
                        .iter()
                        .map(|subject| format!("`{subject}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if frame.subjects.len() == 1 { "s" } else { "ve" },
                );
                return abstain_result(
                    self.store(),
                    AbstainContext {
                        request,
                        plan,
                        manifest,
                        missing_capabilities,
                        frame,
                        started,
                    },
                    reason,
                );
            }
        }
        // Witness gate, relation side: a trace/impact/architecture claim
        // asks what the typed graph says about its subject. When every
        // subject — defined or merely named — participates in zero
        // edges, the graph provably says nothing and no route can
        // legitimately answer. Temporal pins ask about historical
        // structure the current graph cannot refute.
        if frame.required_witness == WitnessRequirement::Relation
            && !frame.subjects.is_empty()
            && !temporal_only
        {
            let mut edgeful = false;
            for subject in &frame.subjects {
                let defined = defined_witnesses(self.store(), &request.snapshot_id, subject)?;
                if relation_witness(self.store(), &request.snapshot_id, &defined)?.edges > 0 {
                    edgeful = true;
                    break;
                }
            }
            if !edgeful {
                let reason = format!(
                    "abstained: claim subject{} {} participate{} in no typed relations — the graph records nothing about {} in this snapshot",
                    if frame.subjects.len() == 1 { "" } else { "s" },
                    frame
                        .subjects
                        .iter()
                        .map(|subject| format!("`{subject}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if frame.subjects.len() == 1 { "s" } else { "" },
                    if frame.subjects.len() == 1 {
                        "it"
                    } else {
                        "them"
                    },
                );
                return abstain_result(
                    self.store(),
                    AbstainContext {
                        request,
                        plan,
                        manifest,
                        missing_capabilities,
                        frame,
                        started,
                    },
                    reason,
                );
            }
        }
        // Witness gate, history side: a history claim asks what the
        // recorded change history says. When no subject appears in any
        // commit-class document, recorded history provably does not
        // mention it. `type:diff` is exempt — its authoritative surface
        // is raw patch payloads, and indexed commit-diff documents are
        // only a bounded projection of them.
        if frame.required_witness == WitnessRequirement::History
            && !frame.subjects.is_empty()
            && !patch_grep_planned
        {
            let mut witnessed = false;
            for subject in &frame.subjects {
                if !self
                    .store()
                    .history_documents_matching_term(&request.snapshot_id, subject, 1)?
                    .is_empty()
                {
                    witnessed = true;
                    break;
                }
            }
            if !witnessed {
                let reason = format!(
                    "abstained: claim subject{} {} appear{} in no commit-class document — recorded history does not mention {} in this snapshot",
                    if frame.subjects.len() == 1 { "" } else { "s" },
                    frame
                        .subjects
                        .iter()
                        .map(|subject| format!("`{subject}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if frame.subjects.len() == 1 { "s" } else { "" },
                    if frame.subjects.len() == 1 {
                        "it"
                    } else {
                        "them"
                    },
                );
                return abstain_result(
                    self.store(),
                    AbstainContext {
                        request,
                        plan,
                        manifest,
                        missing_capabilities,
                        frame,
                        started,
                    },
                    reason,
                );
            }
        }
        let mut candidates = HashMap::<String, Candidate>::new();

        if plan.routes.contains(&SearchRoute::ExactSymbol) {
            let mut rank = 1_usize;
            for token in entity_tokens(&request.query) {
                for entity in
                    self.store()
                        .entity_by_name(&request.snapshot_id, &token, request.limit)?
                {
                    let snippet = if let Some(address) = &entity.address {
                        self.store().source_text(address).unwrap_or_else(|_| {
                            entity
                                .signature
                                .clone()
                                .unwrap_or_else(|| entity.name.clone())
                        })
                    } else {
                        entity
                            .signature
                            .clone()
                            .unwrap_or_else(|| entity.name.clone())
                    };
                    add_candidate(
                        &mut candidates,
                        SearchHit {
                            document_id: format!("entity: {}", entity.id),
                            region_id: entity.region_id.clone(),
                            symbol_name: Some(entity.name.clone()),
                            entity_id: entity.id,
                            representation: RetrievalRepresentation::Signature,
                            route: SearchRoute::ExactSymbol,
                            rank,
                            score: 1.0,
                            contributing_routes: vec![SearchRoute::ExactSymbol],
                            address: entity.address,
                            evidence: Vec::new(),
                            snippet: truncate_chars(&snippet, 2_000),
                            verified_current: verified_fresh,
                            explanation: vec!["exact symbol or qualified-name match".to_owned()],
                        },
                        2.0 / (RRF_K + rank as f64),
                        true,
                    );
                    rank += 1;
                }
            }
        }

        if plan.routes.contains(&SearchRoute::Lexical) {
            for (offset, hit) in self
                .store()
                .lexical_search(
                    &request.snapshot_id,
                    &lexical_query,
                    request.limit.saturating_mul(3),
                    &request.filters,
                )?
                .into_iter()
                // Commit documents are historical evidence owned by the
                // history route; a commit must never crowd the lexical
                // ranking of current-source results.
                .filter(|hit| {
                    !matches!(
                        hit.representation,
                        RetrievalRepresentation::CommitSummary
                            | RetrievalRepresentation::CommitDiff
                    )
                })
                .enumerate()
            {
                let rank = offset + 1;
                add_candidate(
                    &mut candidates,
                    SearchHit {
                        document_id: hit.document_id,
                        entity_id: hit.entity_id,
                        region_id: hit.region_id,
                        symbol_name: Some(hit.symbol_name),
                        representation: hit.representation,
                        route: SearchRoute::Lexical,
                        rank,
                        score: hit.score,
                        contributing_routes: vec![SearchRoute::Lexical],
                        address: hit.address,
                        evidence: hit.evidence,
                        snippet: hit.snippet,
                        verified_current: verified_fresh,
                        explanation: {
                            let mut notes =
                                vec!["SQLite FTS5 identifier/path/source match".to_owned()];
                            if !glossary_terms.is_empty() {
                                notes.push(format!("CJK glossary: +{}", glossary_terms.join(" ")));
                            }
                            notes
                        },
                    },
                    1.0 / (RRF_K + rank as f64),
                    true,
                );
            }
        }

        // Zoekt: file-ranked trigram candidates resolve back to snapshot
        // documents — matched line numbers pick the containing regions
        // (entity precision), and each file's `FileDescriptor` joins as
        // file-level evidence. A hit that resolves to nothing simply
        // contributes no candidate (the path is not in this snapshot).
        if plan.routes.contains(&SearchRoute::Zoekt) {
            match self
                .zoekt_file_hits(&request.snapshot_id, &manifest, &request)
                .await
            {
                Ok(Some(file_hits)) => {
                    let mut path_rank = HashMap::new();
                    for (offset, file_hit) in file_hits.iter().enumerate() {
                        let rank = offset + 1;
                        path_rank.insert(file_hit.path.clone(), rank);
                        let regions = self
                            .store()
                            .regions_for_path(&request.snapshot_id, &file_hit.path)?;
                        let region_ids = crate::zoekt::smallest_regions_for_lines(
                            &regions,
                            &file_hit.lines,
                            ZOEKT_REGIONS_PER_FILE,
                        );
                        for hit in self
                            .store()
                            .documents_for_regions(&request.snapshot_id, &region_ids)?
                        {
                            let mut hit = hit;
                            hit.score = 1.0 / (1.0 + rank as f64);
                            if hit.snippet.is_empty() {
                                hit.snippet.clone_from(&file_hit.matched_text);
                            }
                            add_candidate(
                                &mut candidates,
                                SearchHit {
                                    document_id: hit.document_id,
                                    entity_id: hit.entity_id,
                                    region_id: hit.region_id,
                                    symbol_name: Some(hit.symbol_name),
                                    representation: hit.representation,
                                    route: SearchRoute::Zoekt,
                                    rank,
                                    score: hit.score,
                                    contributing_routes: vec![SearchRoute::Zoekt],
                                    address: hit.address,
                                    evidence: hit.evidence,
                                    snippet: hit.snippet,
                                    verified_current: verified_fresh,
                                    explanation: vec![format!(
                                        "zoekt trigram candidate in {} ({} line match(es))",
                                        file_hit.path,
                                        file_hit.lines.len()
                                    )],
                                },
                                1.0 / (RRF_K + rank as f64),
                                true,
                            );
                        }
                    }
                    let paths: Vec<String> = file_hits.iter().map(|hit| hit.path.clone()).collect();
                    for hit in self
                        .store()
                        .file_descriptors_for_paths(&request.snapshot_id, &paths)?
                    {
                        let path = hit
                            .address
                            .as_ref()
                            .map(|address| address.path.clone())
                            .unwrap_or_default();
                        let rank = path_rank.get(&path).copied().unwrap_or(paths.len());
                        let mut hit = hit;
                        hit.score = 1.0 / (1.0 + rank as f64);
                        add_candidate(
                            &mut candidates,
                            SearchHit {
                                document_id: hit.document_id,
                                entity_id: hit.entity_id,
                                region_id: hit.region_id,
                                symbol_name: Some(hit.symbol_name),
                                representation: hit.representation,
                                route: SearchRoute::Zoekt,
                                rank,
                                score: hit.score,
                                contributing_routes: vec![SearchRoute::Zoekt],
                                address: hit.address,
                                evidence: hit.evidence,
                                snippet: hit.snippet,
                                verified_current: verified_fresh,
                                explanation: vec![format!(
                                    "zoekt trigram file-level candidate: {path}"
                                )],
                            },
                            1.0 / (RRF_K + rank as f64),
                            true,
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "zoekt route failed; continuing without its candidates");
                }
            }
        }

        for (route, accepted) in [
            (
                SearchRoute::Knowledge,
                &[
                    RetrievalRepresentation::KnowledgePage,
                    RetrievalRepresentation::ModuleSummary,
                    RetrievalRepresentation::RoleSummary,
                ][..],
            ),
            (
                SearchRoute::History,
                &[
                    RetrievalRepresentation::CommitSummary,
                    RetrievalRepresentation::CommitDiff,
                ][..],
            ),
        ] {
            if !plan.routes.contains(&route) {
                continue;
            }
            let hits = self.store().lexical_search(
                &request.snapshot_id,
                &request.query,
                request.limit.saturating_mul(5),
                &request.filters,
            )?;
            for (offset, hit) in hits
                .into_iter()
                .filter(|hit| accepted.contains(&hit.representation))
                .enumerate()
            {
                let rank = offset + 1;
                add_candidate(
                    &mut candidates,
                    SearchHit {
                        document_id: hit.document_id,
                        entity_id: hit.entity_id,
                        region_id: hit.region_id,
                        symbol_name: Some(hit.symbol_name),
                        representation: hit.representation,
                        route,
                        rank,
                        score: hit.score,
                        contributing_routes: vec![route],
                        address: hit.address,
                        evidence: hit.evidence,
                        snippet: hit.snippet,
                        verified_current: verified_fresh,
                        explanation: vec![format!(
                            "snapshot-aligned {route:?} artifact retrieved through SQLite FTS5"
                        )],
                    },
                    1.25 / (RRF_K + rank as f64),
                    true,
                );
            }
        }

        if plan.routes.contains(&SearchRoute::Diff) {
            missing_capabilities.push(format!(
                "diff search scans stored commit patches; coverage is bounded by history indexing ({} most recent commits)",
                crate::engine::HISTORY_COMMIT_LIMIT
            ));
            for (offset, hit) in crate::diff::diff_grep(
                self.store(),
                &request.snapshot_id,
                &request.repository_id,
                &request.query,
                request.limit.saturating_mul(2),
            )?
            .into_iter()
            .enumerate()
            {
                let rank = offset + 1;
                add_candidate(
                    &mut candidates,
                    SearchHit { rank, ..hit },
                    1.0 / (RRF_K + rank as f64),
                    true,
                );
            }
        }

        if plan.routes.iter().any(|route| {
            matches!(
                route,
                SearchRoute::DenseRaw | SearchRoute::DenseSummary | SearchRoute::Hybrid
            )
        }) {
            if let Some(dense_status) = manifest.views.get(&ViewKind::Dense) {
                if matches!(dense_status.state, ViewState::Ready | ViewState::Partial) {
                    // A Ready view with a backend that cannot initialize now
                    // (model cache moved, offline first query) degrades to the
                    // fused ranking plus a missing-capability note.
                    let embedder = match self.embedder().await {
                        Ok(embedder) => embedder,
                        Err(error) => {
                            missing_capabilities.push(format!(
                                "dense view is Ready but the embedding backend failed to initialize: {error}"
                            ));
                            None
                        }
                    };
                    if let (Some(digest), Some(embedder)) =
                        (dense_status.artifact_digest.as_deref(), embedder.as_ref())
                    {
                        let counters = self.retrieval_counters();
                        // Index load+decode — a cache hit is a map lookup;
                        // a miss reads the vector artifact once.
                        let index_started = Instant::now();
                        let index = self
                            .dense_index(&request.snapshot_id, &dense_status.profile_hash, digest)
                            .await?;
                        RetrievalCounters::record(
                            &counters.dense_index_load_ns,
                            index_started.elapsed(),
                        );
                        // Query embedding.
                        let embed_started = Instant::now();
                        let query = index.embed_query(&request.query, embedder).await?;
                        RetrievalCounters::record(
                            &counters.dense_embed_ns,
                            embed_started.elapsed(),
                        );
                        // Flat dot product over the corpus — the one stage
                        // that is O(Nd) by design.
                        let score_started = Instant::now();
                        let dense_hits = index.score(&query, request.limit.saturating_mul(3));
                        RetrievalCounters::record(
                            &counters.dense_score_ns,
                            score_started.elapsed(),
                        );
                        // Materialize only the hit set: bodies and entity
                        // names batch-read, never the whole snapshot.
                        let docs_started = Instant::now();
                        let body_reads_before = self.store().artifacts().read_count();
                        let dense_ids: Vec<String> = dense_hits
                            .iter()
                            .map(|hit| hit.document_id.clone())
                            .collect();
                        let documents = self
                            .store()
                            .documents_by_ids(&request.snapshot_id, &dense_ids)?
                            .into_iter()
                            .map(|document| (document.document_id.clone(), document))
                            .collect::<HashMap<_, _>>();
                        counters
                            .dense_documents_restored
                            .fetch_add(documents.len(), std::sync::atomic::Ordering::Relaxed);
                        let mut seen_entities = std::collections::HashSet::new();
                        let entity_ids: Vec<String> = documents
                            .values()
                            .filter(|document| seen_entities.insert(document.entity_id.clone()))
                            .map(|document| document.entity_id.clone())
                            .collect();
                        let entities: HashMap<String, CodeEntity> = self
                            .store()
                            .entities_by_ids(&request.snapshot_id, &entity_ids)?
                            .into_iter()
                            .map(|entity| (entity.id.clone(), entity))
                            .collect();
                        counters.dense_body_artifact_reads.fetch_add(
                            self.store().artifacts().read_count() - body_reads_before,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        RetrievalCounters::record(
                            &counters.dense_document_load_ns,
                            docs_started.elapsed(),
                        );
                        for (offset, hit) in dense_hits.into_iter().enumerate() {
                            let Some(document) = documents.get(&hit.document_id) else {
                                continue;
                            };
                            let rank = offset + 1;
                            let route = match document.representation {
                                RetrievalRepresentation::RawCode
                                | RetrievalRepresentation::TestBehavior => SearchRoute::DenseRaw,
                                _ => SearchRoute::DenseSummary,
                            };
                            let symbol_name = entities
                                .get(&document.entity_id)
                                .map(|entity| entity.name.clone());
                            add_candidate(
                                &mut candidates,
                                SearchHit {
                                    document_id: document.document_id.clone(),
                                    entity_id: document.entity_id.clone(),
                                    region_id: document.region_id.clone(),
                                    symbol_name,
                                    representation: document.representation.clone(),
                                    route,
                                    rank,
                                    score: f64::from(hit.score),
                                    contributing_routes: vec![route],
                                    address: document.address.clone(),
                                    evidence: document.evidence.clone(),
                                    snippet: truncate_chars(&document.text, 2_000),
                                    verified_current: verified_fresh,
                                    explanation: vec![format!(
                                        "dense retrieval via {}",
                                        embedder.profile()
                                    )],
                                },
                                representation_weight(&document.representation)
                                    / (RRF_K + rank as f64),
                                false,
                            );
                        }
                    }
                }
            }
        }

        // Pseudo-relevance feedback: the fused head's discriminative terms
        // expand the query for one more lexical pass, before graph
        // expansion picks its seeds so both stages see the enriched head.
        self.prf_expansion_pass(
            &request,
            &plan,
            &lexical_query,
            &glossary_terms,
            &mut candidates,
            verified_fresh,
        )?;

        // Opportunistic expansion is not a required view (union plans must
        // not demand it); check the graph view at run time and report the
        // skip explicitly instead of silently expanding nothing.
        let opportunistic_blocked = plan.graph_policy == GraphPolicy::Opportunistic
            && !manifest
                .views
                .get(&ViewKind::Graph)
                .is_some_and(|view| matches!(view.state, ViewState::Ready | ViewState::Partial));
        if opportunistic_blocked {
            missing_capabilities
                .push("opportunistic graph expansion skipped: graph view is not ready".to_owned());
        }
        // Surfaced neighbors land in `candidates` as `entity:`-keyed hits;
        // the evidence map records the same visits keyed by entity,
        // region, and file so document candidates can be corroborated.
        let mut expansion_evidence = ExpansionEvidence::default();
        if plan.routes.contains(&SearchRoute::Structural)
            && !matches!(
                plan.graph_policy,
                GraphPolicy::None | GraphPolicy::ArchitectureBoundary
            )
            && !opportunistic_blocked
        {
            // Typed expansion: the intent selects which edge kinds and
            // direction are evidence. Seed score decays per hop and hub
            // nodes are discounted so barrel/utility modules don't flood
            // the frontier.
            let (direction, edge_kinds) = expansion_policy(plan.graph_policy);
            let mut visited = std::collections::HashSet::new();
            let mut frontier = ranked_candidates(&candidates, &HashMap::new())
                .into_iter()
                .take(5)
                .map(|candidate| (candidate.hit.entity_id.clone(), candidate.fused_score))
                .collect::<Vec<_>>();
            let mut rank = 1_usize;
            for hop in 0..2_usize {
                let mut next = Vec::new();
                for (seed, seed_score) in frontier {
                    if !visited.insert(seed.clone()) {
                        continue;
                    }
                    for relation in self.store().relations_for_entity(
                        &request.snapshot_id,
                        &seed,
                        direction,
                        24,
                    )? {
                        if !edge_kinds.contains(&relation.kind) {
                            continue;
                        }
                        let neighbor = if relation.source_entity_id == seed {
                            &relation.target_entity_id
                        } else {
                            &relation.source_entity_id
                        };
                        let Some(entity) =
                            self.store().entity_by_id(&request.snapshot_id, neighbor)?
                        else {
                            continue;
                        };
                        let degree = self
                            .store()
                            .entity_relation_degree(&request.snapshot_id, &entity.id)
                            .unwrap_or(1)
                            .max(1);
                        let hop_decay = 0.5_f64.powi(i32::try_from(hop).unwrap_or(0));
                        let propagated = seed_score * f64::from(relation.confidence) * hop_decay
                            / (1.0 + (degree as f64).ln());
                        expansion_evidence.record(&entity, propagated);
                        let snippet = entity
                            .signature
                            .clone()
                            .or_else(|| entity.qualified_name.clone())
                            .unwrap_or_else(|| entity.name.clone());
                        add_candidate(
                            &mut candidates,
                            SearchHit {
                                document_id: format!("entity: {}", entity.id),
                                region_id: entity.region_id.clone(),
                                symbol_name: Some(entity.name.clone()),
                                entity_id: entity.id.clone(),
                                representation: RetrievalRepresentation::Signature,
                                route: SearchRoute::Structural,
                                rank,
                                score: f64::from(relation.confidence),
                                contributing_routes: vec![SearchRoute::Structural],
                                address: entity.address,
                                evidence: relation.evidence.clone(),
                                snippet,
                                verified_current: verified_fresh,
                                explanation: vec![format!(
                                    "{:?} expansion over {:?} ({:?}, confidence {:.2}, hop {})",
                                    plan.graph_policy,
                                    relation.kind,
                                    relation.origin,
                                    relation.confidence,
                                    hop + 1
                                )],
                            },
                            propagated / (RRF_K + rank as f64),
                            false,
                        );
                        next.push((entity.id, propagated));
                        rank += 1;
                    }
                }
                // Second hop only when it can still matter: cap the frontier
                // and require meaningful propagated score.
                next.sort_by(|left, right| right.1.total_cmp(&left.1));
                next.truncate(4);
                next.retain(|(_, score)| *score > 0.001);
                if next.is_empty() {
                    break;
                }
                frontier = next;
            }
        }

        if plan.graph_policy == GraphPolicy::DataflowRequired
            && !manifest
                .views
                .get(&ViewKind::Dataflow)
                .is_some_and(|view| view.state == ViewState::Ready)
        {
            missing_capabilities.push(
                "precise dataflow is unavailable; CCE refuses to infer a source-to-sink path from embeddings"
                    .to_owned(),
            );
        }

        // One clock read anchors the recency prior for the whole pass so a
        // query's ordering is internally consistent.
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| {
                i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
            });
        apply_structural_features(
            self.store(),
            &request.snapshot_id,
            plan.intent,
            now_unix,
            &request.query,
            &mut candidates,
        )?;
        // Structural evidence: the unified query-seeded flow pass. The
        // v7 gate (`docs/benchmarking.md`) showed it computes everything
        // the legacy corroboration/cluster/file-vote/symbol-evidence
        // priors computed — they are deleted, not shadowed.
        apply_graph_flow(
            self.store(),
            &request.snapshot_id,
            &request.query,
            &expansion_evidence,
            &mut candidates,
            verified_fresh,
            request.limit,
        )?;

        // Evidence gate, inferred side: a candidate whose only support is
        // inferred vicinity — dense similarity, structural expansion,
        // PRF-borrowed vocabulary, flow mass — may rerank real evidence
        // but can never constitute it. A list with no strict-tier
        // candidate is not an answer, however confident the propagation.
        if !candidates.is_empty() && !candidates.values().any(|candidate| candidate.strict) {
            candidates.clear();
            missing_capabilities.push(
                "abstained: every candidate rests on inferred vicinity (dense/expansion/PRF/flow) with no literal query-term evidence"
                    .to_owned(),
            );
        }

        // The claim's distinguishing vocabulary must exist before
        // selection: coverage-aware selection surfaces documents
        // carrying it that pure fusion rank would leave out.
        frame.distinguishing_terms = resolve_distinguishing(
            self.store(),
            &request.snapshot_id,
            &content_terms(&request.query),
        )?;
        let mut hits = self.select_hits(&request, &candidates, &frame)?;

        // Cross-encoder rerank: the fused order is a coarse prior from
        // route-level rank fusion. A pairwise reranker re-scores the head
        // of the list against the raw query text and reorders it.
        //
        // Only topical hits are scored: graph-expansion routes
        // (structural/knowledge/history) answer "what is connected", not
        // "what matches the query text" — judging them on topical
        // relevance punishes blast-radius evidence and collapsed impact
        // and trace recall in benchmark v5.1. Non-topical hits keep their
        // fused slots; topical hits are permuted among their own slots.
        //
        // Configured-but-failed rerankers degrade to fused order with an
        // explicit missing-capability note.
        let topical: Vec<usize> = hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| {
                hit.contributing_routes
                    .iter()
                    .any(|route| TOPICAL.contains(route))
            })
            .map(|(index, _)| index)
            .take(RERANK_POOL)
            .collect();
        if topical.len() > 1 && self.config().reranker_model.is_some() {
            match self.reranker().await {
                Ok(Some(reranker)) => {
                    let documents = topical
                        .iter()
                        .filter_map(|&index| hits.get(index))
                        .map(|hit| {
                            let location = hit.address.as_ref().map_or_else(
                                || {
                                    hit.symbol_name
                                        .clone()
                                        .unwrap_or_else(|| hit.document_id.clone())
                                },
                                |address| {
                                    format!(
                                        "{}:{}",
                                        address.path,
                                        hit.symbol_name.as_deref().unwrap_or("")
                                    )
                                },
                            );
                            format!("{location}\n{}", hit.snippet)
                        })
                        .collect::<Vec<_>>();
                    match reranker.rerank(&request.query, &documents).await {
                        Ok(order) => {
                            let mut reranked = Vec::with_capacity(order.len());
                            for (index, score) in order {
                                let Some(&slot) = topical.get(index) else {
                                    continue;
                                };
                                let Some(mut hit) = hits.get(slot).cloned() else {
                                    continue;
                                };
                                hit.explanation.push(format!(
                                    "cross-encoder rerank by {}: fused {:.4} -> rerank {:.4}",
                                    reranker.model_code(),
                                    hit.score,
                                    score
                                ));
                                hit.score = f64::from(score);
                                if !hit.contributing_routes.contains(&SearchRoute::Reranked) {
                                    hit.contributing_routes.push(SearchRoute::Reranked);
                                }
                                reranked.push(hit);
                            }
                            for (slot, hit) in topical.iter().zip(reranked) {
                                if let Some(target) = hits.get_mut(*slot) {
                                    *target = hit;
                                }
                            }
                        }
                        Err(error) => missing_capabilities.push(format!(
                            "reranker configured but scoring failed: {error}; fused ranking order used"
                        )),
                    }
                }
                Ok(None) => {}
                Err(error) => missing_capabilities.push(format!(
                    "reranker configured but failed to initialize: {error}; fused ranking order used"
                )),
            }
        }

        for (offset, hit) in hits.iter_mut().enumerate() {
            hit.rank = offset + 1;
        }

        // The verdict is the kernel's honest report to whoever consumes
        // this result: what the evidence tiers look like, what the claim
        // required, and where the evidence fails witness typing. The
        // kernel reports; an external verifier judges.
        let witness = build_witness_report(self.store(), &request.snapshot_id, &frame, &hits)?;
        let evidence_tiers = EvidenceTiers {
            strict: candidates.values().filter(|c| c.strict).count(),
            weak: candidates.values().filter(|c| !c.strict).count(),
        };
        let mut reasons = Vec::new();
        let mut state = if hits.is_empty() {
            VerdictState::Abstained
        } else {
            VerdictState::Answered
        };
        if state == VerdictState::Answered {
            reasons = weak_witness_reasons(&frame, &witness);
            if !reasons.is_empty() {
                state = VerdictState::WeakWitness;
            }
        } else {
            reasons.extend(
                missing_capabilities
                    .iter()
                    .filter(|note| note.starts_with("abstained"))
                    .cloned(),
            );
            if reasons.is_empty() {
                reasons.push("no evidence survived the retrieval and filter gates".to_owned());
            }
        }
        let drill_downs = build_drill_downs(&frame, &witness);
        let verdict = SearchVerdict {
            state,
            reasons,
            claim: frame,
            witness,
            evidence_tiers,
            drill_downs,
        };
        Ok(SearchResult {
            request,
            plan,
            manifest,
            hits,
            missing_capabilities,
            verdict,
            latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        })
    }

    /// Fold fused candidates into the presented hit list.
    ///
    /// Multiple retrieval documents can describe one source region (raw
    /// chunk + symbol summary); the hit list presents regions, so the
    /// first — best-scored — document per region wins and later ones only
    /// contribute their routes. The per-file cap is windowed by list
    /// position (`per_file_cap`): the head users actually read enforces
    /// cross-file diversity, the tail relaxes to the historical flat cap.
    /// A hit skipped on the cap is dropped, not deferred — the file's
    /// other documents can still fill later slots on their own merits once
    /// the window relaxes — and everything skipped stays in `candidates`
    /// for expansion seeds.
    fn select_hits(
        &self,
        request: &SearchRequest,
        candidates: &HashMap<String, Candidate>,
        frame: &ClaimFrame,
    ) -> Result<Vec<SearchHit>> {
        // Claim coverage: documents carrying distinguishing terms get a
        // selection bonus — the claim vocabulary a hit carries is
        // evidence, not an afterthought. Prose corroborates but does not
        // witness (document class contract), so only code-class
        // candidates earn the bonus; coverage is capped since carrying a
        // fourth term adds nothing a third did not already attest. One
        // batch query per term keeps the probe bounded; `entity:`
        // pseudo-documents are not in FTS and cannot carry terms.
        let candidate_docs: Vec<String> = candidates
            .values()
            .filter(|candidate| {
                candidate
                    .hit
                    .address
                    .as_ref()
                    .is_some_and(|address| document_class(&address.path) == DocumentClass::Code)
            })
            .map(|candidate| candidate.hit.document_id.clone())
            .filter(|id| !id.starts_with("entity:"))
            .collect();
        let mut claim_terms = HashMap::<String, u32>::new();
        if !candidate_docs.is_empty() {
            for term in &frame.distinguishing_terms {
                for chunk in candidate_docs.chunks(500) {
                    for document_id in
                        self.store()
                            .documents_matching_term(&request.snapshot_id, term, chunk)?
                    {
                        *claim_terms.entry(document_id).or_default() += 1;
                    }
                }
            }
        }
        let claim_bonus: HashMap<String, f64> = claim_terms
            .iter()
            .map(|(document_id, terms)| {
                (
                    document_id.clone(),
                    f64::from((*terms).min(CLAIM_TERM_CAP)) * CLAIM_TERM_BONUS / RRF_K,
                )
            })
            .collect();
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut per_file = HashMap::<String, usize>::new();
        let mut seen_regions = HashMap::<String, usize>::new();
        let mut language_cache = HashMap::<String, Option<String>>::new();
        for candidate in ranked_candidates(candidates, &claim_bonus) {
            let mut hit = candidate.hit.clone();
            if !self.hit_matches_filters(
                &hit,
                &request.filters,
                &request.snapshot_id,
                &mut language_cache,
            )? {
                continue;
            }
            hit.score = candidate.fused_score
                + claim_bonus
                    .get(&candidate.hit.document_id)
                    .copied()
                    .unwrap_or(0.0);
            if let Some(terms) = claim_terms.get(&candidate.hit.document_id) {
                if *terms > 0 {
                    hit.explanation.push(format!(
                        "claim-term coverage: {} distinguishing term(s)",
                        *terms
                    ));
                }
            }
            let region_key = hit.region_id.clone().unwrap_or_else(|| {
                hit.address.as_ref().map_or_else(
                    || hit.document_id.clone(),
                    |address| {
                        format!(
                            "{}:{}:{}",
                            address.path, address.start_byte, address.end_byte
                        )
                    },
                )
            });
            if let Some(&kept) = seen_regions.get(&region_key) {
                if let Some(kept_hit) = hits.get_mut(kept) {
                    for route in &hit.contributing_routes {
                        if !kept_hit.contributing_routes.contains(route) {
                            kept_hit.contributing_routes.push(*route);
                        }
                    }
                }
                continue;
            }
            if let Some(path) = hit.address.as_ref().map(|address| &address.path) {
                let count = per_file.entry(path.clone()).or_default();
                // An exact-symbol hit answers the query's named subject
                // directly; the per-file cap exists to bound *expansion*
                // noise, so inferred-vicinity hits must never crowd it
                // out of the same file's slot.
                let exact_answer = hit.contributing_routes.contains(&SearchRoute::ExactSymbol);
                if !exact_answer && *count >= per_file_cap(hits.len()) {
                    continue;
                }
                *count += 1;
            }
            seen_regions.insert(region_key, hits.len());
            hits.push(hit);
            if hits.len() >= request.limit {
                break;
            }
        }
        Ok(hits)
    }

    /// Pack-time evidence backlink: the hits a query surfaced are pointers
    /// too — a test asserting freshness semantics calls the function that
    /// decides it, and "where is X decided" asks for exactly that callee.
    /// Each top callable hit contributes its strongest mechanism-pointing
    /// edge (`Calls`/`References`/`Tests`) to a non-test function. The
    /// returned pairs carry the source hit's index so the caller can
    /// insert each subject right after its pointer — the packer's caller
    /// quota then delivers the code the evidence *points at* beside the
    /// evidence, and bounded emission means backlinks corroborate the
    /// pack rather than flood it.
    pub(crate) fn subject_backlinks(
        &self,
        search: &SearchResult,
    ) -> Result<Vec<(usize, SearchHit)>> {
        const SOURCES: usize = 8;
        const EDGES: usize = 16;
        const TARGETS_PER_SOURCE: usize = 1;
        const EMIT_MAX: usize = 4;
        // Route contract: backlinks ride the structural channel; plans
        // that exclude it (e.g. History) get none.
        if !search.plan.routes.contains(&SearchRoute::Structural) {
            return Ok(Vec::new());
        }

        let query_terms = prf_terms_in(&search.request.query);
        let hit_entities: std::collections::HashSet<&str> = search
            .hits
            .iter()
            .map(|hit| hit.entity_id.as_str())
            .collect();
        let sources: Vec<(usize, &SearchHit)> = search
            .hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| hit.address.is_some())
            .take(SOURCES)
            .collect();
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        // Mechanism edges only lead out of callable entities — file/doc
        // nodes carry containment and imports, not decision pointers.
        let source_entities: HashMap<String, CodeEntity> = self
            .store()
            .entities_by_ids(
                &search.request.snapshot_id,
                &sources
                    .iter()
                    .map(|(_, hit)| hit.entity_id.clone())
                    .collect::<Vec<_>>(),
            )?
            .into_iter()
            .map(|entity| (entity.id.clone(), entity))
            .collect();
        let tail_score = sources.last().map_or(0.0, |pair| pair.1.score);

        let mut emitted: Vec<(usize, SearchHit)> = Vec::new();
        let mut emitted_ids = std::collections::HashSet::<String>::new();
        'sources: for (source_index, source) in sources {
            let Some(entity) = source_entities.get(&source.entity_id) else {
                continue;
            };
            if !matches!(entity.kind, EntityKind::Function | EntityKind::Method) {
                continue;
            }
            // Best edge per target: `Tests` outranks `Calls` outranks
            // `References` at equal confidence — a name-paired subject or
            // an exercised callee is a stronger pointer than a mention.
            let relations = self.store().relations_for_entity(
                &search.request.snapshot_id,
                &source.entity_id,
                RelationDirection::Outgoing,
                EDGES,
            )?;
            let mut best_edge = HashMap::<String, &cce_core::Relation>::new();
            for relation in &relations {
                if !matches!(
                    relation.kind,
                    RelationKind::Calls | RelationKind::References | RelationKind::Tests
                ) || hit_entities.contains(relation.target_entity_id.as_str())
                {
                    continue;
                }
                let rank = match relation.kind {
                    RelationKind::Tests => 2_u8,
                    RelationKind::Calls => 1,
                    _ => 0,
                };
                best_edge
                    .entry(relation.target_entity_id.clone())
                    .and_modify(|best| {
                        let best_rank = match best.kind {
                            RelationKind::Tests => 2_u8,
                            RelationKind::Calls => 1,
                            _ => 0,
                        };
                        if (relation.confidence, rank) > (best.confidence, best_rank) {
                            *best = relation;
                        }
                    })
                    .or_insert(relation);
            }
            if best_edge.is_empty() {
                continue;
            }
            let target_ids = best_edge.keys().cloned().collect::<Vec<_>>();
            let mut scored = Vec::new();
            for target in self
                .store()
                .entities_by_ids(&search.request.snapshot_id, &target_ids)?
            {
                if !matches!(target.kind, EntityKind::Function | EntityKind::Method)
                    || emitted_ids.contains(&target.id)
                {
                    continue;
                }
                let target_path = target
                    .address
                    .as_ref()
                    .map_or("", |address| address.path.as_str());
                // A test's subject is the answer; another test's subject
                // is sibling evidence, not the decision point.
                if crate::engine::is_test(target_path, &target.name) {
                    continue;
                }
                let overlap = prf_terms_in(&target.name)
                    .iter()
                    .filter(|term| query_terms.contains(term.as_str()))
                    .count();
                // `Calls` marks the subject the assertion runs through,
                // but a spelling-resolved call can land on a same-named
                // wrong entity — it breaks ties after extractor
                // confidence, never above it.
                let exercised = relations.iter().any(|relation| {
                    relation.target_entity_id == target.id
                        && matches!(relation.kind, RelationKind::Calls)
                });
                let degree = self
                    .store()
                    .entity_relation_degree(&search.request.snapshot_id, &target.id)
                    .unwrap_or(1);
                let confidence = best_edge
                    .get(&target.id)
                    .map_or(0.0, |relation| f64::from(relation.confidence));
                scored.push((overlap, confidence, exercised, degree, target));
            }
            scored.sort_by(|left, right| {
                right
                    .0
                    .cmp(&left.0)
                    .then_with(|| right.1.total_cmp(&left.1))
                    .then_with(|| right.2.cmp(&left.2))
                    .then_with(|| right.3.cmp(&left.3))
                    .then_with(|| left.4.name.cmp(&right.4.name))
            });
            let source_name = source
                .symbol_name
                .clone()
                .unwrap_or_else(|| source.entity_id.clone());
            for (_, _, _, _, target) in scored.into_iter().take(TARGETS_PER_SOURCE) {
                let Some(relation) = best_edge.get(&target.id) else {
                    continue;
                };
                let snippet = target
                    .address
                    .as_ref()
                    .and_then(|address| self.store().source_text(address).ok())
                    .filter(|text| !text.is_empty())
                    .map_or_else(
                        || {
                            target
                                .signature
                                .clone()
                                .or_else(|| target.qualified_name.clone())
                                .unwrap_or_else(|| target.name.clone())
                        },
                        |text| truncate_chars(&text, 2_000),
                    );
                emitted_ids.insert(target.id.clone());
                emitted.push((
                    source_index,
                    SearchHit {
                        document_id: format!("subject:{}", target.id),
                        entity_id: target.id.clone(),
                        region_id: target.region_id.clone(),
                        symbol_name: Some(target.name.clone()),
                        representation: RetrievalRepresentation::RawCode,
                        route: SearchRoute::Structural,
                        rank: 0,
                        score: tail_score,
                        contributing_routes: vec![SearchRoute::Structural],
                        address: target.address.clone(),
                        evidence: relation.evidence.clone(),
                        snippet,
                        verified_current: source.verified_current,
                        explanation: vec![format!(
                            "evidence subject: {source_name} reaches it via {:?} ({:?}, confidence {:.2})",
                            relation.kind, relation.origin, relation.confidence
                        )],
                    },
                ));
                if emitted.len() >= EMIT_MAX {
                    break 'sources;
                }
            }
        }
        Ok(emitted)
    }

    /// Pseudo-relevance feedback (RM3-style): treat the fused topical head
    /// as relevant, mine its discriminative terms, and run ONE extra FTS5
    /// query with the expanded text so vocabulary-mismatched documents in
    /// the tail re-enter through the same RRF fusion. Fully deterministic —
    /// the only added work is a single `lexical_search` call.
    ///
    /// Skipped when the pass cannot help or was opted out of:
    /// - `ExactEntity` intent — identifier queries gain nothing and
    ///   expansion risks precision;
    /// - plans without the lexical route — explicit route overrides and
    ///   `type:` pins (`diff`/`commit` repoint the plan to other routes);
    /// - zero topical seeds — there is no relevance signal to feed back.
    fn prf_expansion_pass(
        &self,
        request: &SearchRequest,
        plan: &QueryPlan,
        lexical_query: &str,
        glossary_terms: &[String],
        candidates: &mut HashMap<String, Candidate>,
        verified_fresh: bool,
    ) -> Result<()> {
        if plan.intent == QueryIntent::ExactEntity || !plan.routes.contains(&SearchRoute::Lexical) {
            return Ok(());
        }
        let seeds: Vec<&Candidate> = ranked_candidates(candidates, &HashMap::new())
            .into_iter()
            .filter(|candidate| {
                candidate
                    .hit
                    .contributing_routes
                    .iter()
                    .any(|route| TOPICAL.contains(route))
            })
            .take(PRF_FEEDBACK_DOCS)
            .collect();
        // The glossary-expanded text is the term vocabulary too: anchors
        // already in the lexical query are not re-mined as feedback terms.
        let terms = prf_expansion_terms(lexical_query, &seeds);
        if terms.is_empty() {
            return Ok(());
        }
        let expanded = format!("{} {}", lexical_query, terms.join(" "));
        for (offset, hit) in self
            .store()
            .lexical_search(
                &request.snapshot_id,
                &expanded,
                request.limit.saturating_mul(2),
                &request.filters,
            )?
            .into_iter()
            // Same exclusion as the first lexical pass: commit documents are
            // history-route evidence, not current-source answers.
            .filter(|hit| {
                !matches!(
                    hit.representation,
                    RetrievalRepresentation::CommitSummary
                        | RetrievalRepresentation::CommitDiff
                )
            })
            .enumerate()
        {
            let rank = offset + 1;
            add_candidate(
                candidates,
                SearchHit {
                    document_id: hit.document_id,
                    entity_id: hit.entity_id,
                    region_id: hit.region_id,
                    symbol_name: Some(hit.symbol_name),
                    representation: hit.representation,
                    route: SearchRoute::Lexical,
                    rank,
                    score: hit.score,
                    contributing_routes: vec![SearchRoute::Lexical],
                    address: hit.address,
                    evidence: hit.evidence,
                    snippet: hit.snippet,
                    verified_current: verified_fresh,
                    explanation: {
                        let mut notes = vec![format!("PRF expansion: +{}", terms.join(" "))];
                        if !glossary_terms.is_empty() {
                            notes.push(format!("CJK glossary: +{}", glossary_terms.join(" ")));
                        }
                        notes
                    },
                },
                PRF_ATTENUATION / (RRF_K + rank as f64),
                false,
            );
        }
        Ok(())
    }

    /// Post-fusion check for `path:`/`lang:` filters on routes that cannot
    /// push them into SQL (exact-symbol, dense, graph expansion). A hit
    /// without a source path fails a `path:` filter; language resolves
    /// through the entity, cached per entity id.
    fn hit_matches_filters(
        &self,
        hit: &SearchHit,
        filters: &cce_core::QueryFilters,
        snapshot_id: &str,
        language_cache: &mut HashMap<String, Option<String>>,
    ) -> Result<bool> {
        if filters.is_empty() {
            return Ok(true);
        }
        if let Some(prefix) = &filters.path_prefix {
            let matches_path = hit
                .address
                .as_ref()
                .is_some_and(|address| address.path.starts_with(prefix.as_str()))
                || hit
                    .evidence
                    .iter()
                    .any(|address| address.path.starts_with(prefix.as_str()));
            if !matches_path {
                return Ok(false);
            }
        }
        if let Some(language) = &filters.language {
            let entity_language = if let Some(cached) = language_cache.get(&hit.entity_id) {
                cached.clone()
            } else {
                let resolved = self
                    .store()
                    .entity_by_id(snapshot_id, &hit.entity_id)?
                    .and_then(|entity| entity.language)
                    .map(|value| value.to_ascii_lowercase());
                language_cache.insert(hit.entity_id.clone(), resolved.clone());
                resolved
            };
            if entity_language.as_deref() != Some(language.as_str()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Opportunistic zoekt candidates: only run when the view reports
    /// Ready for this snapshot AND the on-disk marker agrees (a stale or
    /// absent shard must never serve — freshness is checked, not assumed).
    /// `None` means "silently skip the route"; errors mean the index was
    /// present but the query failed, which the caller logs and degrades.
    async fn zoekt_file_hits(
        &self,
        snapshot_id: &str,
        manifest: &ViewManifest,
        request: &SearchRequest,
    ) -> Result<Option<Vec<crate::zoekt::ZoektFileHit>>> {
        let usable = manifest
            .views
            .get(&ViewKind::Zoekt)
            .is_some_and(|status| matches!(status.state, ViewState::Ready | ViewState::Partial));
        if !usable {
            return Ok(None);
        }
        let Some(tools) = crate::zoekt::detect(&self.config().data_root) else {
            return Ok(None);
        };
        let index_dir = crate::zoekt::index_dir(&self.config().data_root);
        if crate::zoekt::indexed_snapshot(&index_dir).as_deref() != Some(snapshot_id) {
            return Ok(None);
        }
        let Some(query) = crate::zoekt::build_query(&request.query) else {
            return Ok(None);
        };
        let limit = request
            .limit
            .saturating_mul(ZOEKT_FILE_LIMIT_FACTOR)
            .max(20);
        let hits = tokio::task::spawn_blocking(move || {
            crate::zoekt::search(&tools, &index_dir, &query, limit, ZOEKT_QUERY_TIMEOUT)
        })
        .await
        .map_err(|error| {
            cce_core::CceError::Configuration(format!("zoekt query task failed: {error}"))
        })??;
        Ok(Some(hits))
    }
}

/// Which edges count as evidence for each graph policy. The intent chooses
/// the vocabulary of the expansion, not just its direction.
///
/// `ChangedWith` appears only in the `Opportunistic` arm: a co-change edge
/// is stored once per pair with the smaller entity id as source, so a
/// one-directional arm would only find it from the larger-id side — but
/// `Opportunistic` walks `Both`, which sees the symmetric edge from either
/// endpoint. Directional arms keep co-change evidence in the partner bonus
/// of `apply_structural_features`, which also looks it up in both
/// directions.
const fn expansion_policy(policy: GraphPolicy) -> (RelationDirection, &'static [RelationKind]) {
    match policy {
        GraphPolicy::OutgoingTrace => (
            RelationDirection::Outgoing,
            &[
                RelationKind::Calls,
                RelationKind::Imports,
                RelationKind::RouteHandledBy,
            ],
        ),
        GraphPolicy::IncomingImpact => (
            RelationDirection::Incoming,
            &[
                RelationKind::Calls,
                RelationKind::References,
                RelationKind::Tests,
                RelationKind::Implements,
            ],
        ),
        GraphPolicy::Opportunistic => (
            RelationDirection::Both,
            &[
                RelationKind::Calls,
                RelationKind::References,
                RelationKind::Imports,
                RelationKind::ChangedWith,
            ],
        ),
        GraphPolicy::DataflowRequired | GraphPolicy::None | GraphPolicy::ArchitectureBoundary => (
            RelationDirection::Both,
            &[
                RelationKind::Calls,
                RelationKind::References,
                RelationKind::Imports,
            ],
        ),
    }
}

/// Same-package bonus: a top-3 hit's package is likely the task's package,
/// so sibling files get a small lift — below the same-file 0.25.
const PACKAGE_BONUS: f64 = 0.15;

/// Structural priors layered on the fused ranking:
/// - exact symbol/word agreement between the query and a hit's symbol name;
/// - same-file evidence aggregation (a file holding a top-3 hit makes its
///   other hits more likely to be task evidence);
/// - same-package membership (a file in a top-3 hit's package is likelier
///   task evidence than a cross-package one);
/// - git recency prior for issue/history intents (a recently touched file
///   is the likelier culprit — BugCache-style version-history evidence);
/// - definition prior (Signature/TestBehavior representations beat stray
///   raw-code mentions for symbol-shaped queries).
fn apply_structural_features(
    store: &MetadataStore,
    snapshot_id: &str,
    intent: QueryIntent,
    now_unix: i64,
    query: &str,
    candidates: &mut HashMap<String, Candidate>,
) -> Result<()> {
    let query_words: std::collections::HashSet<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| word.len() >= 3)
        .map(str::to_ascii_lowercase)
        .collect();
    let top_paths: std::collections::HashSet<String> =
        ranked_candidates(candidates, &HashMap::new())
            .into_iter()
            .take(3)
            .filter_map(|candidate| {
                candidate
                    .hit
                    .address
                    .as_ref()
                    .map(|address| address.path.clone())
            })
            .collect();
    // Workspace packages as (rootDir, name); the task package set is the
    // owners of the top-3 files, mirroring the top_paths aggregation.
    let packages = store
        .entities_by_kind(snapshot_id, &EntityKind::Package)?
        .into_iter()
        .map(|entity| {
            let root_dir = entity
                .attributes
                .get("rootDir")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            (root_dir, entity.name)
        })
        .collect::<Vec<_>>();
    let task_packages: std::collections::HashSet<&str> = top_paths
        .iter()
        .filter_map(|path| package_of(path, &packages))
        .collect();
    let mut candidate_packages = HashMap::<String, Option<&str>>::new();
    // BugCache-style recency prior for fault-localization intents: a file
    // touched recently is the likelier culprit. A 90-day half-life keeps a
    // same-day touch near the 0.2 ceiling and a year-old touch near
    // nothing; staleness decays evidence rather than disqualifying it.
    let recency_intent = matches!(
        intent,
        QueryIntent::IssueLocalization | QueryIntent::History
    );
    let mut last_touched = HashMap::<String, Option<i64>>::new();
    for candidate in candidates.values_mut() {
        let hit = &candidate.hit;
        let mut bonus = 0.0;
        let mut notes = Vec::new();
        if let Some(name) = &hit.symbol_name {
            let lowered = name.to_ascii_lowercase();
            if query_words.contains(&lowered)
                || lowered
                    .split('_')
                    .any(|part| part.len() >= 3 && query_words.contains(part))
            {
                bonus += 0.5;
            }
        }
        if let Some(path) = hit.address.as_ref().map(|address| &address.path) {
            if top_paths.contains(path) {
                bonus += 0.25;
            }
            let package = *candidate_packages
                .entry(path.clone())
                .or_insert_with(|| package_of(path, &packages));
            if package.is_some_and(|name| task_packages.contains(name)) {
                bonus += PACKAGE_BONUS;
                notes.push(format!(
                    "same package as a top hit ({})",
                    package.unwrap_or("")
                ));
            }
            if recency_intent {
                let touched = if let Some(cached) = last_touched.get(path) {
                    *cached
                } else {
                    let touched = file_entity(store, snapshot_id, path)?.and_then(|entity| {
                        entity
                            .attributes
                            .get("lastTouched")
                            .and_then(serde_json::Value::as_i64)
                    });
                    last_touched.insert(path.clone(), touched);
                    touched
                };
                if let Some(touched) = touched {
                    // Future-dated commits (clock skew) clamp to a same-day
                    // touch rather than earning a negative-age bonus.
                    let days = (now_unix - touched).max(0) as f64 / 86_400.0;
                    let recency = (0.2 * (-days / 90.0).exp2()).min(0.2);
                    bonus += recency;
                    notes.push(format!("recently touched file ({days:.0}d ago)"));
                }
            }
        }
        if matches!(
            hit.representation,
            RetrievalRepresentation::Signature | RetrievalRepresentation::TestBehavior
        ) {
            bonus += 0.1;
        }
        candidate.hit.explanation.extend(notes);
        candidate.fused_score += bonus / RRF_K;
    }
    Ok(())
}

/// Query-conditioned structural flow: the principled mechanism the
/// graph-adjacent priors each approximate. Candidate entities are seeded
/// with their fused topical score; mass then propagates along typed
/// edges — `calls`/`references`/`imports` carry topicality from a
/// retrieved entity to the mechanism it uses, `tests` flows test
/// topicality to its target, `contains` aggregates member evidence back
/// into its file at reduced weight, `changed_with` diffuses
/// symmetrically. One pass subsumes corroboration joins (mass flows from
/// expansion neighbors into candidates), cluster support (shared
/// neighbors relay mass between seeds), file-vote (`contains` backward
/// flow IS file aggregation), and symbol-evidence emission (high-mass
/// symbol nodes emit as `entity:` hits).
///
/// Direction and type are the signal lexical similarity lacks: a test
/// that calls the mechanism is a flow *source*, the mechanism is a flow
/// *sink* — the implementation-vs-adjacent distinction falls out of the
/// topology instead of a hand-tuned bonus.
const FLOW_SEEDS: usize = 32;
const FLOW_SEED_DEGREE: usize = 64;
const FLOW_HOPS: usize = 2;
const FLOW_HOP_DECAY: f64 = 0.5;
const FLOW_BONUS: f64 = 0.25;
/// Selection bonus per distinguishing term a document carries — same
/// magnitude as `FLOW_BONUS`/`PACKAGE_BONUS`, scaled by `/ RRF_K` at the
/// application site.
const CLAIM_TERM_BONUS: f64 = 0.2;
/// Coverage beyond this many distinguishing terms earns no further
/// bonus — the third co-occurring term already attests the document is
/// on-claim; extra matches are diminishing returns, not new evidence.
const CLAIM_TERM_CAP: u32 = 3;
const FLOW_EMIT_MAX: usize = 12;
const FLOW_FRONTIER: usize = 16;
/// `Contains` forward conductance for FILE-KIND SEED nodes only.
/// Generic file→member flow dilutes across dozens of members (0.3),
/// but a retrieved file is topical evidence for its own members —
/// the granularity hop the champion file-vote approximated. Moderated
/// between the generic forward rate and the member→file aggregation
/// rate: strong enough that a strong file's member symbols become
/// flow-reachable in hop 1, bounded so a file cannot flood its whole
/// member list with near-seed mass.
const FLOW_SEED_CONTAINS: f64 = 0.6;

/// Per-kind conductance `(forward, backward)`: the fraction of a
/// node's mass crossing an edge in each direction per hop. The table
/// follows the per-label parameterization of typed random walks
/// (PCRW/metapath): mechanism-pointing edges conduct strongly
/// downstream, callers/importers carry weaker backward impact
/// evidence, `contains` aggregates member evidence upward, and
/// symmetric couplings (`changed_with`, `tests`) conduct both ways.
const fn flow_edge_weights(kind: &RelationKind) -> (f64, f64) {
    match kind {
        // A relevant route almost certainly means its handler is the
        // mechanism; the route entity itself is a lookup anchor.
        RelationKind::RouteHandledBy => (0.9, 0.3),
        // The callee is part of the caller's mechanism (Portfolio);
        // callers are usage/impact evidence — real but secondary.
        RelationKind::Calls => (0.8, 0.5),
        RelationKind::Imports => (0.7, 0.35),
        // Tests encode expected behavior; either endpoint being on-topic
        // lifts the other. Convention-inferred edges stay mid-band.
        RelationKind::Tests => (0.6, 0.6),
        // Signature/type use is the weakest semantic link and the
        // hubbiest kind — always in-degree gated downstream.
        RelationKind::References => (0.5, 0.4),
        // Member→file aggregates evidence (the file-vote readout);
        // file→member dilutes across dozens of members.
        RelationKind::Contains => (0.3, 0.9),
        // Co-change is coupling, not mechanism — symmetric, moderate.
        RelationKind::ChangedWith => (0.4, 0.4),
        // Declared package coupling is architecture-layer evidence.
        RelationKind::BuildDependsOn => (0.25, 0.15),
        _ => (0.0, 0.0),
    }
}

/// A stable ordering tag for edge dedup and deterministic iteration —
/// `RelationKind` carries data variants so it cannot cast to an int.
const fn flow_kind_tag(kind: &RelationKind) -> u8 {
    match kind {
        RelationKind::Calls => 0,
        RelationKind::References => 1,
        RelationKind::Imports => 2,
        RelationKind::Contains => 3,
        RelationKind::Tests => 4,
        RelationKind::ChangedWith => 5,
        RelationKind::RouteHandledBy => 6,
        RelationKind::BuildDependsOn => 7,
        _ => 255,
    }
}

/// Fetch `entity_id`'s relations and append unseen `(source, target,
/// kind)` edges with extractor confidence. Relation rows repeat per
/// extractor merge, so `seen` dedups or flow mass multiplies. Each
/// direction gets its own `FLOW_SEED_DEGREE` budget: a single shared
/// cap lets a hub seed's incoming edges crowd its outgoing evidence
/// pointers out of the universe — the emitted-consensus residual the
/// symbol-evidence join's outgoing-only pointer fetch never had.
fn flow_collect_edges(
    store: &MetadataStore,
    snapshot_id: &str,
    entity_id: &str,
    seen: &mut std::collections::BTreeSet<(String, String, u8)>,
    edges: &mut Vec<(String, String, RelationKind, f64)>,
) -> Result<()> {
    let degree = store
        .entity_relation_degree(snapshot_id, entity_id)?
        .min(FLOW_SEED_DEGREE);
    for direction in [RelationDirection::Outgoing, RelationDirection::Incoming] {
        for relation in store.relations_for_entity(snapshot_id, entity_id, direction, degree)? {
            let key = (
                relation.source_entity_id.clone(),
                relation.target_entity_id.clone(),
                flow_kind_tag(&relation.kind),
            );
            if seen.insert(key) {
                edges.push((
                    relation.source_entity_id,
                    relation.target_entity_id,
                    relation.kind,
                    f64::from(relation.confidence),
                ));
            }
        }
    }
    Ok(())
}

/// Per-(node, kind) confidence sums: `(outgoing, incoming)` totals the
/// propagation loop normalizes edge shares against.
type FlowConfMaps = (
    HashMap<(String, RelationKind), f64>,
    HashMap<(String, RelationKind), f64>,
);

/// Rebuild the per-(node, kind) confidence sums the propagation loop
/// normalizes against. Called once for the seed universe and again
/// after frontier edges merge.
fn flow_degree_maps(edges: &[(String, String, RelationKind, f64)]) -> FlowConfMaps {
    let mut out_conf = HashMap::<(String, RelationKind), f64>::new();
    let mut in_conf = HashMap::<(String, RelationKind), f64>::new();
    for (source, target, kind, confidence) in edges {
        *out_conf.entry((source.clone(), kind.clone())).or_default() += confidence;
        *in_conf.entry((target.clone(), kind.clone())).or_default() += confidence;
    }
    (out_conf, in_conf)
}

fn apply_graph_flow(
    store: &MetadataStore,
    snapshot_id: &str,
    query: &str,
    expansion: &ExpansionEvidence,
    candidates: &mut HashMap<String, Candidate>,
    verified_fresh: bool,
    limit: usize,
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    // Seeds: strongest unique candidate entities — the head carries the
    // topical signal and the cap bounds relation fetches.
    let mut unique = HashMap::<String, f64>::new();
    for candidate in candidates.values() {
        let entry = unique.entry(candidate.hit.entity_id.clone()).or_default();
        *entry = entry.max(candidate.fused_score);
    }
    let mut seeds: Vec<(String, f64)> = unique.into_iter().collect();
    seeds.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    seeds.truncate(FLOW_SEEDS);
    // Expansion-surfaced evidence seeds too — the corroboration-join
    // subsumption: candidate → expansion-entity → candidate is the
    // two-hop path that returns mass to docs expansion confirmed but
    // `add_candidate` could never merge. Expansion seeds carry their
    // recorded propagated score and append after candidate seeds
    // inside the same FLOW_SEEDS budget. `regions` keys are not
    // entity-graph nodes (no relations attach to them); their evidence
    // already enters through the member entities that recorded them.
    let mut seed_set: std::collections::HashSet<String> =
        seeds.iter().map(|(id, _)| id.clone()).collect();
    let mut expansion_seeds: Vec<(String, f64)> = Vec::new();
    for (entity_id, (score, _)) in &expansion.entities {
        if seed_set.insert(entity_id.clone()) {
            expansion_seeds.push((entity_id.clone(), *score));
        }
    }
    // File-path keys resolve to the file entity holding the surfaced
    // symbols — the file-level corroboration granularity. Resolution
    // is a name lookup per path, so it runs only against the seed
    // budget candidate entities did not already fill.
    if seeds.len() + expansion_seeds.len() < FLOW_SEEDS {
        let mut paths: Vec<(&String, f64)> = expansion
            .paths
            .iter()
            .map(|(path, (score, _))| (path, *score))
            .collect();
        paths.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        let remaining = FLOW_SEEDS - seeds.len() - expansion_seeds.len();
        // A few extra lookups cover paths with no file entity row.
        let mut added = 0_usize;
        for (path, score) in paths.into_iter().take(remaining + 4) {
            if let Some(entity) = file_entity(store, snapshot_id, path)? {
                if seed_set.insert(entity.id.clone()) {
                    expansion_seeds.push((entity.id, score));
                    added += 1;
                }
            }
            if added >= remaining {
                break;
            }
        }
    }
    expansion_seeds.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    seeds.extend(expansion_seeds);
    seeds.truncate(FLOW_SEEDS);
    let Some(max_seed) = seeds.first().map(|(_, score)| *score) else {
        return Ok(());
    };
    if max_seed <= 0.0 {
        return Ok(());
    }
    // File-kind seeds inject into their own members at a moderated
    // rate (FLOW_SEED_CONTAINS) instead of the generic diluting
    // contains-forward: a retrieved file is topical evidence for the
    // symbols it contains — the granularity hop the champion
    // file-vote approximated by fiat.
    let file_seeds: std::collections::HashSet<String> = store
        .entities_by_ids(
            snapshot_id,
            &seeds.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        )?
        .into_iter()
        .filter(|entity| entity.kind == EntityKind::File)
        .map(|entity| entity.id)
        .collect();

    // Edge universe: deduplicated relations touching any seed. Relation
    // rows repeat per extractor merge, so dedup by (source, target,
    // kind) or the flow multiplies. Mass flows ALONG edge direction:
    // only nodes with mass feed, so incoming edges matter exactly when
    // their source is also a seed — seed→shared-neighbor→seed is how
    // cluster support emerges in hop 2.
    let mut edges: Vec<(String, String, RelationKind, f64)> = Vec::new();
    let mut seen = std::collections::BTreeSet::<(String, String, u8)>::new();
    for (seed_id, _) in &seeds {
        flow_collect_edges(store, snapshot_id, seed_id, &mut seen, &mut edges)?;
    }
    if edges.is_empty() {
        return Ok(());
    }
    edges.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| flow_kind_tag(&left.2).cmp(&flow_kind_tag(&right.2)))
    });

    // Multiplex normalization: each kind is its own stochastic layer —
    // a source's mass splits across its same-kind edges proportional to
    // extractor confidence, and the kind's conductance weight then
    // decides how much of the source's total mass that layer carries.
    // Receiver-side degree is deliberately NOT gated: within this
    // bounded universe (≤ FLOW_SEEDS senders) high in-degree is
    // corroborating consensus, not global hubness — gating it here
    // punished exactly the multi-sender evidence the mechanism exists
    // to reward (type hubs lost to low-degree utilities in v6.0).
    let (mut out_conf, mut in_conf) = flow_degree_maps(&edges);

    let mut mass: HashMap<String, f64> = seeds.iter().cloned().collect();
    let seed_ids: std::collections::HashSet<&String> = seeds.iter().map(|(id, _)| id).collect();
    // Mass RECEIVED from other nodes — the evidence signal. Seed mass
    // itself is excluded: a candidate's own topicality is not evidence
    // corroborating it.
    let mut received: HashMap<String, f64> = HashMap::new();
    // Distinct senders per receiver — the flow analogue of the
    // symbol-evidence join's referrer count: mass arriving over
    // independent paths is consensus, one edge's worth of mass is not.
    let mut senders: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    // Non-backtracking: an edge traversed at hop t cannot reverse at
    // t+1 — test↔mechanism ping-pong would inflate both endpoints
    // without adding corroboration. Re-firing the same direction is
    // legitimate accumulation; only immediate reversal is blocked. A
    // node may still be reached via OTHER seeds — multi-path arrival
    // is the consensus signal itself.
    let mut traversed = std::collections::HashSet::<(usize, u8)>::new();
    for hop in 1..=FLOW_HOPS {
        if hop > 1 {
            // Frontier expansion: hop-1's strongest receivers get their
            // own edges fetched so mass can relay past the seed star —
            // file→member→mechanism is the two-hop path lexical
            // evidence cannot name. Without this, hop 2 could only echo
            // inside edges incident to seeds.
            let mut frontier: Vec<(f64, &String)> = received
                .iter()
                .filter(|(id, _)| !seed_ids.contains(id))
                .map(|(id, flow)| (*flow, id))
                .collect();
            frontier.sort_by(|left, right| {
                right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1))
            });
            for (_, entity_id) in frontier.into_iter().take(FLOW_FRONTIER) {
                flow_collect_edges(store, snapshot_id, entity_id, &mut seen, &mut edges)?;
            }
            (out_conf, in_conf) = flow_degree_maps(&edges);
        }
        let prev = mass.clone();
        let used = std::mem::take(&mut traversed);
        let mut delta = std::collections::BTreeMap::<String, f64>::new();
        for (index, (source, target, kind, confidence)) in edges.iter().enumerate() {
            let (mut forward, backward) = flow_edge_weights(kind);
            // Seed-file member injection: contains edges out of a
            // file-kind seed conduct at the moderated rate, not the
            // generic diluting one.
            if matches!(kind, RelationKind::Contains) && file_seeds.contains(source) {
                forward = FLOW_SEED_CONTAINS;
            }
            let source_mass = prev.get(source).copied().unwrap_or_default();
            if forward > 0.0 && source_mass > 0.0 && !used.contains(&(index, 1)) {
                let share = confidence
                    / out_conf
                        .get(&(source.clone(), kind.clone()))
                        .copied()
                        .unwrap_or(1.0)
                        .max(f64::EPSILON);
                let flow = forward * source_mass * share;
                *delta.entry(target.clone()).or_default() += flow;
                senders
                    .entry(target.clone())
                    .or_default()
                    .insert(source.clone());
                traversed.insert((index, 0));
            }
            let target_mass = prev.get(target).copied().unwrap_or_default();
            if backward > 0.0 && target_mass > 0.0 && !used.contains(&(index, 0)) {
                let share = confidence
                    / in_conf
                        .get(&(target.clone(), kind.clone()))
                        .copied()
                        .unwrap_or(1.0)
                        .max(f64::EPSILON);
                let flow = backward * target_mass * share;
                *delta.entry(source.clone()).or_default() += flow;
                senders
                    .entry(source.clone())
                    .or_default()
                    .insert(target.clone());
                traversed.insert((index, 1));
            }
        }
        let decay = FLOW_HOP_DECAY.powi(i32::try_from(hop).unwrap_or(0));
        for (node, gain) in delta {
            *mass.entry(node.clone()).or_default() += decay * gain;
            *received.entry(node).or_default() += decay * gain;
        }
    }

    // Readout 1 — candidate bonus: received mass, normalized by the
    // best-corroborated node, bounded at FLOW_BONUS. A candidate joins
    // on every granularity the flow graph carries: its own entity AND
    // the file entity its address lives in — member evidence
    // aggregates into the file node (`contains` backward), so the
    // file's received mass is the pooled vote backing every document
    // the file owns. This is the corroboration/file-vote readout: the
    // legacy priors keyed the same join on `address.path`.
    //
    // Expansion membership is the second channel — the corroboration
    // join's direct readout. A document whose own entity, region, or
    // path structural expansion surfaced is externally corroborated
    // even when no flow returns to it (seed mass is not self-evidence),
    // so membership strength joins the same bounded bonus. Candidates
    // carrying the structural route ARE that evidence; bonusing them
    // would double-count.
    let max_received = received.values().copied().fold(0.0_f64, f64::max);
    if max_received > 0.0 || expansion.max_score > 0.0 {
        let mut file_entities = HashMap::<String, Option<String>>::new();
        for candidate in candidates.values_mut() {
            if candidate
                .hit
                .contributing_routes
                .contains(&SearchRoute::Structural)
            {
                continue;
            }
            let mut flow = received
                .get(&candidate.hit.entity_id)
                .copied()
                .unwrap_or_default();
            if let Some(path) = candidate.hit.address.as_ref().map(|a| a.path.clone()) {
                let file_id = match file_entities.entry(path) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let resolved =
                            file_entity(store, snapshot_id, entry.key())?.map(|entity| entity.id);
                        entry.insert(resolved).clone()
                    }
                };
                if let Some(file_id) = file_id {
                    if file_id != candidate.hit.entity_id {
                        flow += received.get(&file_id).copied().unwrap_or_default();
                    }
                }
            }
            let mut strength = if max_received > 0.0 {
                (flow / max_received).min(1.0)
            } else {
                0.0
            };
            let mut notes = Vec::new();
            if flow > 0.0 {
                notes.push(format!("graph flow corroboration {strength:.2}"));
            }
            if expansion.max_score > 0.0 {
                let membership = [
                    expansion.entities.get(&candidate.hit.entity_id),
                    candidate
                        .hit
                        .region_id
                        .as_ref()
                        .and_then(|region_id| expansion.regions.get(region_id)),
                    candidate
                        .hit
                        .address
                        .as_ref()
                        .and_then(|address| expansion.paths.get(&address.path)),
                ]
                .into_iter()
                .flatten()
                .max_by(|left, right| left.0.total_cmp(&right.0));
                if let Some(&(score, edges)) = membership {
                    strength = strength.max((score / expansion.max_score).min(1.0));
                    notes.push(format!(
                        "corroborated by graph expansion ({edges} edge{})",
                        if edges == 1 { "" } else { "s" }
                    ));
                }
            }
            if strength <= 0.0 {
                continue;
            }
            candidate.fused_score += FLOW_BONUS * strength / RRF_K;
            candidate.hit.explanation.extend(notes);
        }
    }

    // Readout 2 — symbol emission: high-flow non-candidate symbol nodes
    // emit as `entity:` hits at boundary mass, same slotting contract
    // as the symbol-evidence join. Callable kinds get their own lane
    // because claim facts name functions/methods and type hubs would
    // otherwise monopolize emission on pointer mass.
    let candidate_ids: std::collections::HashSet<&String> = candidates
        .values()
        .map(|candidate| &candidate.hit.entity_id)
        .collect();
    let emitted_ids: Vec<String> = received
        .keys()
        .filter(|id| !candidate_ids.contains(id))
        .cloned()
        .collect();
    let entities: HashMap<String, CodeEntity> = store
        .entities_by_ids(snapshot_id, &emitted_ids)?
        .into_iter()
        .map(|entity| (entity.id.clone(), entity))
        .collect();
    let query_terms = prf_terms_in(query);
    // Plural-tolerant term set, same rule as the symbol-evidence join:
    // "views" in prose should match the "view" inside `set_view_status`.
    // Both spellings are admitted so singular/plural collapse onto the
    // symbol token — without it the named lane silently narrows versus
    // the join it replaces (v7: set_view_status lost on "views").
    let mut tolerant_terms = query_terms.clone();
    // Literal spellings are exempt: a backticked or identifier-shaped
    // anchor states an exact name, and singularizing a pluralized
    // near-miss back into the real identifier is exactly how near-miss
    // lookups resurrect the real entity. Exemption is judged in
    // prf-term space, so anchors are split before comparison.
    let literal_terms: std::collections::HashSet<String> = literal_anchors(query)
        .iter()
        .flat_map(|token| cce_core::split_identifier_terms(token))
        .collect();
    for term in &query_terms {
        if term.len() > 3 && term.ends_with('s') && !literal_terms.contains(term) {
            tolerant_terms.insert(term[..term.len() - 1].to_owned());
        }
    }
    let mass_norm = max_received.max(f64::EPSILON);
    let mut scored: Vec<(f64, f64, usize, &CodeEntity)> = Vec::new();
    for (id, entity) in &entities {
        if !matches!(
            entity.kind,
            EntityKind::Function
                | EntityKind::Method
                | EntityKind::Struct
                | EntityKind::Enum
                | EntityKind::Trait
                | EntityKind::Class
                | EntityKind::Interface
                | EntityKind::Constant
        ) {
            continue;
        }
        let flow = received.get(id).copied().unwrap_or_default();
        // Emission score is the pointer rule stated in flow terms:
        // distinct SEED senders are consensus votes (the join's
        // referrer count), name overlap is topical evidence, and
        // normalized received mass is the tie-breaking nudge the
        // referrer score played. Raw mass ordering alone dilutes
        // multi-sender evidence: seven thin seed→struct edges lose
        // to one fat call edge even though the struct is what the
        // retrieved head actually points at.
        let seed_consensus = senders.get(id).map_or(0, |set| {
            set.iter()
                .filter(|sender| seed_ids.contains(sender))
                .count()
        });
        let overlap = prf_terms_in(&entity.name)
            .iter()
            .filter(|term| {
                tolerant_terms.contains(term.as_str())
                    || (term.len() > 3
                        && term.ends_with('s')
                        && tolerant_terms.contains(&term[..term.len() - 1]))
            })
            .count();
        let sender_count = senders.get(id).map_or(0, std::collections::HashSet::len);
        // Admission floor, the join's `pointers < 2 && overlap == 0`
        // gate stated over flow senders: a node needs consensus —
        // mass arriving over at least two independent paths — or a
        // topical name. Single-sender incidental mass must not spend
        // an emission slot that displaces a lexical incumbent.
        if sender_count < 2 && overlap == 0 {
            continue;
        }
        let score = 1.5f64.mul_add(overlap as f64, seed_consensus as f64) + flow / mass_norm;
        scored.push((score, flow, overlap, entity));
    }
    scored.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.3.id.cmp(&right.3.id))
    });
    let (named, plain): (Vec<_>, Vec<_>) =
        scored.into_iter().partition(|(_, _, overlap, entity)| {
            *overlap > 0 && matches!(entity.kind, EntityKind::Function | EntityKind::Method)
        });
    let mut named = named.into_iter();
    let mut plain = plain.into_iter();
    let mut block: Vec<(f64, f64, usize, &CodeEntity)> = Vec::new();
    while block.len() < FLOW_EMIT_MAX {
        let mut progressed = false;
        for lane in [&mut named, &mut plain] {
            if let Some(entry) = lane.next() {
                block.push(entry);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    if block.is_empty() {
        return Ok(());
    }
    let mut pool_scores: Vec<f64> = candidates.values().map(|c| c.fused_score).collect();
    pool_scores.sort_by(|a, b| b.total_cmp(a));
    let boundary = pool_scores
        .get(limit.saturating_sub(FLOW_EMIT_MAX))
        .or_else(|| pool_scores.last())
        .copied()
        .unwrap_or_default();
    if boundary <= 0.0 {
        return Ok(());
    }
    for (rank, (_, flow, _, entity)) in block.into_iter().enumerate() {
        let mass_hit = boundary * 0.005f64.mul_add(-(rank as f64), 1.0);
        let snippet = entity
            .signature
            .clone()
            .or_else(|| entity.qualified_name.clone())
            .unwrap_or_else(|| entity.name.clone());
        add_candidate(
            candidates,
            SearchHit {
                document_id: format!("entity:{}", entity.id),
                region_id: entity.region_id.clone(),
                symbol_name: Some(entity.name.clone()),
                entity_id: entity.id.clone(),
                representation: RetrievalRepresentation::Signature,
                route: SearchRoute::Structural,
                rank: rank + 1,
                score: mass_hit,
                contributing_routes: vec![SearchRoute::Structural],
                address: entity.address.clone(),
                evidence: Vec::new(),
                snippet,
                verified_current: verified_fresh,
                explanation: vec![format!(
                    "graph flow evidence: received mass {flow:.3} from retrieved candidates"
                )],
            },
            mass_hit,
            false,
        );
    }
    Ok(())
}

/// The package owning `path`: the longest matching `rootDir` prefix.
/// A trailing-slash requirement keeps `crates/alpha` from claiming
/// `crates/alpha2/…`, and the root package's empty rootDir matches every
/// path but loses to any member root — so it claims only files no member
/// package owns, mirroring `packages::emit`'s `Contains` edges.
fn package_of<'a>(path: &str, packages: &'a [(String, String)]) -> Option<&'a str> {
    packages
        .iter()
        .filter(|(root_dir, _)| {
            root_dir.is_empty()
                || path
                    .strip_prefix(root_dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|(root_dir, _)| root_dir.len())
        .map(|(_, name)| name.as_str())
}

/// The `File` entity whose name is exactly `path`, when one is indexed.
/// File entities are named by repository-relative path; a symbol sharing
/// the spelling must not be mistaken for the file.
fn file_entity(store: &MetadataStore, snapshot_id: &str, path: &str) -> Result<Option<CodeEntity>> {
    Ok(store
        .entity_by_name(snapshot_id, path, 4)?
        .into_iter()
        .find(|entity| entity.kind == EntityKind::File))
}

fn add_candidate(
    candidates: &mut HashMap<String, Candidate>,
    hit: SearchHit,
    contribution: f64,
    strict: bool,
) {
    candidates
        .entry(hit.document_id.clone())
        .and_modify(|candidate| {
            candidate.fused_score += contribution;
            candidate.strict |= strict;
            for route in &hit.contributing_routes {
                if !candidate.hit.contributing_routes.contains(route) {
                    candidate.hit.contributing_routes.push(*route);
                }
            }
            candidate.hit.explanation.extend(hit.explanation.clone());
            if hit.snippet.len() > candidate.hit.snippet.len() {
                candidate.hit.snippet.clone_from(&hit.snippet);
            }
        })
        .or_insert(Candidate {
            hit,
            fused_score: contribution,
            strict,
        });
}

fn ranked_candidates<'a>(
    candidates: &'a HashMap<String, Candidate>,
    claim_bonus: &HashMap<String, f64>,
) -> Vec<&'a Candidate> {
    let effective = |candidate: &Candidate| {
        candidate.fused_score
            + claim_bonus
                .get(&candidate.hit.document_id)
                .copied()
                .unwrap_or(0.0)
    };
    let mut ranked = candidates.values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        effective(right)
            .total_cmp(&effective(left))
            .then_with(|| left.hit.document_id.cmp(&right.hit.document_id))
    });
    ranked
}

/// Per-file cap by filled list length: the head enforces file diversity —
/// 1 hit per file inside the top 5, 2 inside the top 10 — then relaxes to
/// the historical flat cap of 3 for the tail. Measured on the v5.3
/// bundle, this windowing is what lifts recall@5 without moving anything
/// else. `filled` is `hits.len()` at the moment a candidate is judged, so
/// a skipped hit is dropped, not deferred; when few files carry hits the
/// head starves and the list just fills with what remains.
const fn per_file_cap(filled: usize) -> usize {
    if filled < 5 {
        1
    } else if filled < 10 {
        2
    } else {
        3
    }
}

const fn representation_weight(representation: &RetrievalRepresentation) -> f64 {
    match representation {
        RetrievalRepresentation::RoleSummary
        | RetrievalRepresentation::SymbolSummary
        | RetrievalRepresentation::ModuleSummary
        | RetrievalRepresentation::FlowSummary => 1.15,
        RetrievalRepresentation::TestBehavior => 1.1,
        _ => 1.0,
    }
}

fn missing_views(manifest: &ViewManifest, plan: &QueryPlan) -> Vec<String> {
    plan.required_views
        .iter()
        .filter_map(|kind| match manifest.views.get(kind) {
            Some(status) if matches!(status.state, ViewState::Ready | ViewState::Partial) => None,
            Some(status) => Some(format!(
                "{kind} view is {:?}: {}",
                status.state,
                status.message.as_deref().unwrap_or("no detail")
            )),
            None => Some(format!("{kind} view has no manifest entry")),
        })
        .collect()
}

fn required_views_for_routes(routes: &[SearchRoute]) -> Vec<ViewKind> {
    let mut views = Vec::new();
    for route in routes {
        let candidates: &[ViewKind] = match route {
            SearchRoute::NoRetrieval => &[],
            SearchRoute::ExactSymbol => &[ViewKind::Symbols],
            SearchRoute::Lexical => &[ViewKind::Lexical],
            SearchRoute::DenseRaw | SearchRoute::DenseSummary => &[ViewKind::Dense],
            SearchRoute::Hybrid => &[ViewKind::Lexical, ViewKind::Dense],
            SearchRoute::Structural => &[ViewKind::Graph],
            SearchRoute::Knowledge => &[ViewKind::Knowledge],
            SearchRoute::History | SearchRoute::Diff => &[ViewKind::History],
            // Zoekt is opportunistic: it contributes candidates when its
            // shard exists and is current, but no query should abstain
            // because an external index is absent — the FTS channel
            // already covers the same vocabulary.
            SearchRoute::Zoekt => &[],
            SearchRoute::Reranked => &[],
        };
        for view in candidates {
            if !views.contains(view) {
                views.push(*view);
            }
        }
    }
    views
}

fn entity_tokens(query: &str) -> Vec<String> {
    let stop = [
        "where",
        "what",
        "which",
        "defined",
        "definition",
        "references",
        "is",
        "the",
        "in",
        "在哪",
        "定义",
        "哪里",
        "谁",
        "引用",
        "怎么",
        "如何",
    ];
    let mut tokens = query
        .split(|character: char| {
            !(character.is_alphanumeric()
                || character == '_'
                || character == ':'
                || character == '.')
        })
        .map(|token| token.trim_matches('.'))
        // CJK interrogatives glue onto the phrase they lead ("在哪定义" is
        // "在哪" + "定义"): peel non-ASCII stopword prefixes so the remainder
        // is judged on its own. Latin stopwords stay whole-word — "there"
        // must not lose "the".
        .map(|token| {
            let mut rest = token;
            while let Some(prefix) = stop
                .iter()
                .filter(|stop| stop.chars().any(has_cjk))
                .find(|stop| rest.len() > stop.len() && rest.starts_with(*stop))
            {
                rest = &rest[prefix.len()..];
            }
            rest
        })
        .filter(|token| token.chars().count() >= 3)
        .filter(|token| !stop.contains(&token.to_ascii_lowercase().as_str()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // Exact-symbol is an identity lookup: inside a multi-token natural
    // language query a plain lowercase word ("from", "merged") colliding
    // with an entity of the same name is coincidence, not intent — such
    // terms are lexical evidence instead. Identifier-shaped tokens keep
    // the route; a single-token query keeps its only token.
    //
    // Code-switched queries are the exception: a Latin word embedded in a
    // CJK sentence ("…标记为 stale") is a deliberate anchor the writer chose
    // not to translate, and CJK spellings are themselves valid identifier
    // territory — so the identifier-shape gate stays off for them.
    let code_switched = tokens.iter().any(|token| token.chars().any(has_cjk));
    if tokens.len() > 1 && !code_switched {
        // Sentence-initial capitals are orthography, not naming intent:
        // "Trace how hits are packed" claims a trace of the phrase, not
        // an entity "Trace". Interior capitals stay name-shaped — the
        // writer chose to capitalize mid-sentence ("how does Tokio…").
        let mut leading = true;
        tokens.retain(|token| {
            let sentence_leading = std::mem::replace(&mut leading, false);
            is_identifier_like(token) && !(sentence_leading && is_sentence_capital(token))
        });
    }
    tokens.sort_by_key(|token| std::cmp::Reverse(token.len()));
    tokens.truncate(8);
    tokens
}

fn is_identifier_like(token: &str) -> bool {
    token.contains('_')
        || token.contains("::")
        || token.contains('.')
        || token.chars().any(|character| character.is_ascii_digit())
        || token
            .chars()
            .any(|character| character.is_ascii_uppercase())
}

/// Capitalized only because it opens the sentence — a plain English word
/// wearing orthography, not a name. `SourceAddress` fails the check (a
/// second capital marks real naming intent); `Trace`, `Describe`,
/// `Which` pass it and are stripped when leading.
fn is_sentence_capital(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_uppercase())
        && !token[1..]
            .chars()
            .any(|character| character.is_ascii_uppercase())
        && !token.contains('_')
        && !token.contains("::")
}

/// Entity-name shape: capitals, qualifiers, or underscores — the markers
/// that distinguish a named thing from a plain word. A bare dot or digit
/// run does not qualify (`18.04`, `config.toml` are values, not entity
/// names) unless the token anchors the query on its own.
fn is_name_shaped(token: &str) -> bool {
    token.contains('_')
        || token.contains("::")
        || token
            .chars()
            .any(|character| character.is_ascii_uppercase())
}

/// Tokens the writer spelled verbatim inside backticks — explicit
/// literals. A backticked spelling must never be normalized (no plural
/// tolerance, no case folding of intent): a backticked pluralized
/// near-miss asks about exactly that spelling, not its real stem.
fn backtick_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut rest = query;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else {
            break;
        };
        for token in rest[..close]
            .split(|character: char| {
                !(character.is_alphanumeric()
                    || character == '_'
                    || character == ':'
                    || character == '.')
            })
            .map(|token| token.trim_matches('.'))
        {
            if token.chars().count() >= 3 && token.chars().any(char::is_alphanumeric) {
                terms.push(token.to_owned());
            }
        }
        rest = &rest[close + 1..];
    }
    terms
}

/// True when a token could be an entity spelling rather than a value:
/// all alphabetic/underscore, or carrying identifier shape (capitals,
/// `::`, `_`). `18.04` and `config.toml` fail both — dotted values are
/// never provable-absence anchors.
fn is_entity_spelling(token: &str) -> bool {
    token
        .chars()
        .all(|character| character.is_alphabetic() || character == '_')
        || is_name_shaped(token)
}

/// Literal anchors the abstention gate may check for corpus presence:
/// backticked spellings that could name an entity, plus `entity_tokens`
/// output that is either the query's only anchor or name-shaped.
///
/// Exclusions keep the veto honest:
/// * CJK tokens — monolithic indexing makes absence unprovable (a phrase
///   fragment may index inside a longer token), so CJK never vetoes;
/// * `18.04`/`config.toml`-style values — dotted or numeric tokens are
///   not entity spellings and never veto, even as the query's sole
///   anchor; the retrieval cascade already returns empty for them,
///   which is the honest no-context signal;
/// * tokens without any alphanumeric, which would emit an empty FTS
///   phrase.
fn literal_anchors(query: &str) -> Vec<String> {
    let entity = entity_tokens(query)
        .into_iter()
        .filter(|token| !token.chars().any(has_cjk))
        .filter(|token| token.chars().any(char::is_alphanumeric))
        .collect::<Vec<_>>();
    let mut anchors: Vec<String> = backtick_terms(query)
        .into_iter()
        .filter(|token| !token.chars().any(has_cjk))
        .filter(|token| is_entity_spelling(token))
        .collect();
    let sole_anchor = entity.len() == 1;
    for token in entity {
        if is_entity_spelling(&token)
            && (sole_anchor || is_name_shaped(&token))
            && !anchors.contains(&token)
        {
            anchors.push(token);
        }
    }
    anchors
}

/// Query-shape and predicate words — the grammar of asking, not claim
/// content. "Where is X defined" claims a definition of X; "defined"
/// itself carries no distinguishing vocabulary.
fn is_claim_shape_word(term: &str) -> bool {
    matches!(
        term,
        "defined"
            | "definition"
            | "declared"
            | "declaration"
            | "references"
            | "referenced"
            | "implement"
            | "implemented"
            | "implementation"
            | "work"
            | "works"
            | "working"
            | "handle"
            | "handles"
            | "handled"
            | "handling"
            | "logic"
            | "support"
            | "supports"
            | "supported"
            | "exist"
            | "exists"
            | "existing"
            | "way"
            | "currently"
            | "code"
            | "file"
            | "files"
            | "flow"
            | "repository"
            | "repo"
            | "project"
            | "system"
            | "在哪"
            | "定义"
            | "哪里"
            | "谁"
            | "引用"
            | "怎么"
            | "如何"
            | "实现"
            | "支持"
    )
}

/// All claim content terms: alphanumeric tokens that are neither
/// function words nor query-shape vocabulary, with no CJK (monolithic
/// CJK tokens cannot bind reliably). Unlike `entity_tokens` this keeps
/// plain lowercase words — a claim's distinguishing vocabulary
/// ("distributed", "bearer") is rarely identifier-shaped.
fn content_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for token in query.split(|character: char| {
        !(character.is_alphanumeric() || character == '_' || character == ':' || character == '.')
    }) {
        let token = token.trim_matches('.');
        if token.chars().count() < 3
            || !token.chars().any(char::is_alphanumeric)
            || token.chars().any(has_cjk)
        {
            continue;
        }
        let lowered = token.to_ascii_lowercase();
        if is_prf_stopword(&lowered) || is_claim_shape_word(&lowered) {
            continue;
        }
        if !terms.contains(&lowered) {
            terms.push(lowered);
        }
    }
    terms
}

/// What the query claims in witness terms: subjects are the verbatim
/// anchors it names; predicate and required witness follow the
/// classified intent. `distinguishing_terms` is filled by
/// `resolve_distinguishing`, which needs corpus frequencies.
const fn claim_frame(intent: QueryIntent, subjects: Vec<String>) -> ClaimFrame {
    let (predicate, required_witness) = match intent {
        QueryIntent::ExactEntity => (ClaimPredicate::Definition, WitnessRequirement::Definition),
        QueryIntent::NaturalLanguageBehavior | QueryIntent::IssueLocalization => (
            ClaimPredicate::Implementation,
            WitnessRequirement::CodeBinding,
        ),
        // PreciseDataflow keeps `Any`: its dedicated capability refusal
        // (no dataflow view) is the honest reason, and a relation gate
        // would preempt it with the wrong one.
        QueryIntent::Trace | QueryIntent::Impact | QueryIntent::Architecture => {
            (ClaimPredicate::Relation, WitnessRequirement::Relation)
        }
        QueryIntent::PreciseDataflow => (ClaimPredicate::Relation, WitnessRequirement::Any),
        QueryIntent::History => (ClaimPredicate::History, WitnessRequirement::History),
        QueryIntent::Unknown => (ClaimPredicate::Lookup, WitnessRequirement::Any),
    };
    ClaimFrame {
        intent,
        predicate,
        required_witness,
        subjects,
        distinguishing_terms: Vec::new(),
    }
}

/// The rare content terms carrying the claim's specificity — df-capped
/// at the corpus's rarity band and rarest-first, capped at four so the
/// binding query stays satisfiable rather than becoming an impossible
/// conjunction.
fn resolve_distinguishing(
    store: &MetadataStore,
    snapshot_id: &str,
    terms: &[String],
) -> Result<Vec<String>> {
    let documents = store.document_count(snapshot_id)?.max(1);
    let cap = (documents / 200).max(32);
    let mut rare = Vec::new();
    for term in terms.iter().take(24) {
        // Only `df <= cap` is needed — cap the count so a ubiquitous
        // term stops at cap+1 rows instead of enumerating its posting.
        let df = store.term_document_frequency_capped(snapshot_id, term, cap + 1)?;
        if df <= cap {
            rare.push((df, term.clone()));
        }
    }
    rare.sort();
    rare.truncate(4);
    Ok(rare.into_iter().map(|(_, term)| term).collect())
}

/// Ordered identifier parts — camel/snake boundaries without
/// sort/dedup, so contiguous runs preserve the name's structure.
fn identifier_parts(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for character in name.chars() {
        if character.is_alphanumeric() {
            if previous_lower && character.is_uppercase() && !current.is_empty() {
                parts.push(current.to_ascii_lowercase());
                current.clear();
            }
            previous_lower = character.is_lowercase();
            current.push(character);
        } else if !current.is_empty() {
            parts.push(current.to_ascii_lowercase());
            current.clear();
            previous_lower = false;
        }
    }
    if !current.is_empty() {
        parts.push(current.to_ascii_lowercase());
    }
    parts
}

/// Whether `name` carries `term` as the fold of a contiguous part-run:
/// `lock` witnesses `LockManager` and `lock_manager.rs` but not
/// `blockchain`; `websocket` witnesses `WebSocket` and
/// `WebSocketHandler`. Substring matching without part boundaries
/// would let unrelated names masquerade as definitions.
fn name_witnesses(term: &str, name: &str) -> bool {
    let term = folded_identifier(term);
    if term.is_empty() {
        return false;
    }
    let parts = identifier_parts(name);
    for start in 0..parts.len() {
        let mut run = String::new();
        for part in parts.iter().skip(start) {
            run.push_str(part);
            if run == term {
                return true;
            }
            if run.len() >= term.len() {
                break;
            }
        }
    }
    false
}

/// Definition-tier witnesses for a term: entities and files whose name
/// carries it as a contiguous part-run, capped for report size.
/// Definition-tier witnesses for a claim term: entities or files whose
/// name carries it. A `::`-qualified subject decomposes into scope +
/// member — every segment must itself be defined, so
/// `NonexistentType::method` is not witnessed by a bare `method`, while
/// `ArtifactStore::open` is witnessed by `open` living where
/// `ArtifactStore` is defined. Reported witnesses are the tail segment's
/// definitions — the definable name — ordered to prefer entities
/// coherent with a scope segment's home file.
fn defined_witnesses(
    store: &MetadataStore,
    snapshot_id: &str,
    term: &str,
) -> Result<Vec<DefinedWitness>> {
    let segments: Vec<&str> = term
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect();
    let matches = |segment: &str, entity: &CodeEntity| {
        name_witnesses(segment, &entity.name)
            || entity
                .qualified_name
                .as_deref()
                .is_some_and(|qualified| name_witnesses(segment, qualified))
    };
    if segments.len() > 1 {
        let mut segment_witnesses = Vec::with_capacity(segments.len());
        for segment in &segments {
            let found: Vec<CodeEntity> = store
                .entity_name_candidates(snapshot_id, segment, 64)?
                .into_iter()
                .filter(|entity| matches(segment, entity))
                .collect();
            if found.is_empty() {
                return Ok(Vec::new());
            }
            segment_witnesses.push(found);
        }
        let Some(tail) = segment_witnesses.pop() else {
            return Ok(Vec::new());
        };
        let scope_files: std::collections::HashSet<&str> = segment_witnesses
            .iter()
            .flatten()
            .filter_map(|entity| entity.address.as_ref().map(|address| address.path.as_str()))
            .collect();
        let mut tail = tail;
        tail.sort_by_key(|entity| {
            !entity
                .address
                .as_ref()
                .is_some_and(|address| scope_files.contains(address.path.as_str()))
        });
        return Ok(tail
            .into_iter()
            .take(8)
            .map(|entity| DefinedWitness {
                entity_id: entity.id,
                name: entity.name,
                kind: entity.kind,
                path: entity.address.map(|address| address.path),
            })
            .collect());
    }
    let candidates = store.entity_name_candidates(snapshot_id, term, 64)?;
    Ok(candidates
        .into_iter()
        .filter(|entity| matches(term, entity))
        .take(8)
        .map(|entity| DefinedWitness {
            entity_id: entity.id,
            name: entity.name,
            kind: entity.kind,
            path: entity.address.map(|address| address.path),
        })
        .collect())
}

/// The typed-edge summary across a term's definition entities — the
/// relation claim's witness. Per-entity fetches are capped, so the
/// count is a bounded sample: enough to attest that relations exist
/// and of which kinds/origins, not to enumerate the neighborhood.
fn relation_witness(
    store: &MetadataStore,
    snapshot_id: &str,
    defined: &[DefinedWitness],
) -> Result<RelationWitness> {
    let mut witness = RelationWitness::default();
    for entity in defined {
        for edge in store.relations_for_entity(
            snapshot_id,
            &entity.entity_id,
            RelationDirection::Both,
            64,
        )? {
            witness.edges += 1;
            match edge.origin {
                RelationOrigin::Compiler | RelationOrigin::Scip | RelationOrigin::Lsp => {
                    witness.provenance.precise += 1;
                }
                RelationOrigin::TreeSitter => witness.provenance.syntactic += 1,
                RelationOrigin::BuildSystem | RelationOrigin::FrameworkRule => {
                    witness.provenance.derived += 1;
                }
                RelationOrigin::ModelInference => witness.provenance.inferred += 1,
            }
            if !witness.kinds.contains(&edge.kind) {
                witness.kinds.push(edge.kind);
            }
            if !witness.origins.contains(&edge.origin) {
                witness.origins.push(edge.origin);
            }
        }
    }
    Ok(witness)
}

/// One term's witness data: definitions vs mentions, plus whichever
/// claim-scoped surface the required witness actually reads. Relation
/// and history probes are computed only for claims that demand them —
/// an exact-entity lookup must not pay for graph/history queries it
/// never checks.
fn build_term_witness(
    store: &MetadataStore,
    snapshot_id: &str,
    term: &str,
    required_witness: WitnessRequirement,
) -> Result<TermWitness> {
    let defined = defined_witnesses(store, snapshot_id, term)?;
    let relations = if required_witness == WitnessRequirement::Relation {
        relation_witness(store, snapshot_id, &defined)?
    } else {
        RelationWitness::default()
    };
    let history_documents = if required_witness == WitnessRequirement::History {
        store.history_documents_matching_term(snapshot_id, term, 8)?
    } else {
        Vec::new()
    };
    Ok(TermWitness {
        term: term.to_owned(),
        relations,
        defined,
        mentions: store.term_document_frequency(snapshot_id, term)?,
        mention_paths: store.term_mention_paths(snapshot_id, term, 5)?,
        history_documents,
    })
}

/// What the evidence contains relative to the claim: per-term
/// definition/mention data, the coherent-scope binding set for the
/// distinguishing vocabulary, and the claim terms absent from every
/// returned hit.
fn build_witness_report(
    store: &MetadataStore,
    snapshot_id: &str,
    frame: &ClaimFrame,
    hits: &[SearchHit],
) -> Result<WitnessReport> {
    let mut checked = frame.subjects.clone();
    for term in &frame.distinguishing_terms {
        if !checked.contains(term) {
            checked.push(term.clone());
        }
    }
    checked.truncate(6);
    let mut terms = Vec::with_capacity(checked.len());
    for term in &checked {
        terms.push(build_term_witness(
            store,
            snapshot_id,
            term,
            frame.required_witness,
        )?);
    }
    let binding_artifacts = if frame.distinguishing_terms.len() >= 2 {
        let mut by_path: std::collections::BTreeMap<String, BoundArtifact> =
            std::collections::BTreeMap::new();
        for binding in store.terms_binding_scopes(snapshot_id, &frame.distinguishing_terms, 10)? {
            let scope = binding
                .entity_kind
                .as_ref()
                .map_or(BindingScope::File, |kind| {
                    if code_scope_entity(kind) {
                        BindingScope::Entity
                    } else {
                        BindingScope::File
                    }
                });
            let test = scope == BindingScope::Entity
                && is_test_entity(
                    &binding.path,
                    binding.entity_name.as_deref(),
                    binding.entity_qualified.as_deref(),
                );
            by_path
                .entry(binding.path.clone())
                .and_modify(|artifact| {
                    if scope == BindingScope::Entity
                        && (artifact.scope == BindingScope::File || (artifact.test && !test))
                    {
                        // An implementation entity outranks a test one
                        // for the same path — marking the artifact
                        // test-only on a weaker binding would be
                        // slander, not caution.
                        artifact.scope = BindingScope::Entity;
                        artifact.entity.clone_from(&binding.entity_name);
                        artifact.test = test;
                    }
                })
                .or_insert_with(|| BoundArtifact {
                    class: document_class(&binding.path),
                    path: binding.path,
                    scope,
                    entity: if scope == BindingScope::Entity {
                        binding.entity_name
                    } else {
                        None
                    },
                    test,
                });
        }
        by_path.into_values().collect()
    } else {
        Vec::new()
    };
    let document_ids: Vec<String> = hits
        .iter()
        .filter(|hit| !hit.document_id.starts_with("entity:"))
        .map(|hit| hit.document_id.clone())
        .collect();
    let mut scope_gaps = Vec::new();
    if !document_ids.is_empty() {
        for term in &frame.distinguishing_terms {
            if store
                .documents_matching_term(snapshot_id, term, &document_ids)?
                .is_empty()
            {
                scope_gaps.push(term.clone());
            }
        }
    }
    Ok(WitnessReport {
        terms,
        binding_artifacts,
        scope_gaps,
    })
}

/// Whether the binding entity is test code — the name or path rules
/// plus the qualified-name module path (`…::tests::helper`), which
/// marks `#[cfg(test)]` members whose bare name shows no test marker.
fn is_test_entity(path: &str, name: Option<&str>, qualified: Option<&str>) -> bool {
    crate::engine::is_test(path, name.unwrap_or_default())
        || qualified.is_some_and(|qualified| {
            qualified
                .split("::")
                .any(|segment| matches!(segment, "test" | "tests" | "testing"))
        })
}

/// Whether an entity kind marks one coherent code span — the
/// entity-level binding scope. Containers (file/directory/package),
/// historical entities (commit), documentation concepts, and
/// unclassified entities all attest file-level scope at best.
const fn code_scope_entity(kind: &EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Module
            | EntityKind::Namespace
            | EntityKind::Class
            | EntityKind::Interface
            | EntityKind::Trait
            | EntityKind::Struct
            | EntityKind::Enum
            | EntityKind::Function
            | EntityKind::Method
            | EntityKind::Field
            | EntityKind::Constant
            | EntityKind::Route
            | EntityKind::Schema
            | EntityKind::Test
            | EntityKind::Configuration
    )
}

/// Witness gaps worth flagging on an otherwise-answered verdict. These
/// are reports, not verdicts — a consuming verifier reads them to
/// decide whether the hits can constitute an answer.
fn weak_witness_reasons(frame: &ClaimFrame, report: &WitnessReport) -> Vec<String> {
    let quoted = |terms: &[String]| {
        terms
            .iter()
            .map(|term| format!("`{term}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut reasons = Vec::new();
    for witness in &report.terms {
        if frame.subjects.contains(&witness.term) && witness.defined.is_empty() {
            reasons.push(format!(
                "weak_witness: `{}` has no definition-tier witness — the named subject is mention-only vocabulary",
                witness.term
            ));
        }
    }
    if frame.distinguishing_terms.len() >= 2 {
        let implementation_bound = report.binding_artifacts.iter().any(|artifact| {
            artifact.class == DocumentClass::Code
                && artifact.scope == BindingScope::Entity
                && !artifact.test
        });
        if !implementation_bound {
            if report.binding_artifacts.is_empty() {
                reasons.push(format!(
                    "weak_witness: no artifact binds the claim's distinguishing terms {}",
                    quoted(&frame.distinguishing_terms)
                ));
            } else {
                let test_bound = report.binding_artifacts.iter().any(|artifact| {
                    artifact.class == DocumentClass::Code
                        && artifact.scope == BindingScope::Entity
                        && artifact.test
                });
                let code_bound = report
                    .binding_artifacts
                    .iter()
                    .any(|artifact| artifact.class == DocumentClass::Code);
                let paths = report
                    .binding_artifacts
                    .iter()
                    .take(3)
                    .map(|artifact| artifact.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if test_bound {
                    reasons.push(format!(
                        "weak_witness: distinguishing terms {} co-bind only in test code — assertions name the vocabulary, they do not implement it: {paths}",
                        quoted(&frame.distinguishing_terms)
                    ));
                } else if code_bound {
                    reasons.push(format!(
                        "weak_witness: distinguishing terms {} co-bind only at file scope — no single entity carries them: {paths}",
                        quoted(&frame.distinguishing_terms)
                    ));
                } else {
                    reasons.push(format!(
                        "weak_witness: distinguishing terms {} co-bind only in prose: {paths}",
                        quoted(&frame.distinguishing_terms)
                    ));
                }
            }
        }
    }
    if !frame.distinguishing_terms.is_empty()
        && report.scope_gaps.len() == frame.distinguishing_terms.len()
    {
        reasons.push(format!(
            "weak_witness: no returned hit carries the claim's distinguishing terms {}",
            quoted(&frame.distinguishing_terms)
        ));
    }
    match frame.required_witness {
        WitnessRequirement::Relation => {
            for witness in &report.terms {
                if !frame.subjects.contains(&witness.term) {
                    continue;
                }
                if witness.relations.edges == 0 && !witness.defined.is_empty() {
                    reasons.push(format!(
                        "weak_witness: `{}` is defined but participates in no typed relations — the relation claim rests on non-graph evidence",
                        witness.term
                    ));
                } else if !witness.relations.origins.is_empty()
                    && witness
                        .relations
                        .origins
                        .iter()
                        .all(|origin| *origin == RelationOrigin::ModelInference)
                {
                    reasons.push(format!(
                        "weak_witness: `{}`'s relation evidence is model-inferred only — no deterministic edge backs it",
                        witness.term
                    ));
                }
            }
        }
        WitnessRequirement::History => {
            for witness in &report.terms {
                if frame.subjects.contains(&witness.term) && witness.history_documents.is_empty() {
                    reasons.push(format!(
                        "weak_witness: `{}` appears in no commit-class document — answered from current-source evidence only",
                        witness.term
                    ));
                }
            }
        }
        _ => {}
    }
    reasons
}

/// Everything an early-abstain needs that a normal result carries —
/// bundled so the gate call sites stay readable.
struct AbstainContext {
    request: SearchRequest,
    plan: QueryPlan,
    manifest: ViewManifest,
    missing_capabilities: Vec<String>,
    frame: ClaimFrame,
    started: Instant,
}

/// Early-abstain assembly shared by the pre-retrieval witness gates:
/// resolve the claim's distinguishing vocabulary for the report, then
/// return the abstained verdict with empty hits.
/// Deterministic follow-up queries derived from witness gaps — every
/// suggestion runs in the query language's own filters, no model
/// judgment. Order is fixed (history, mention, scope, relation,
/// binding) and the list caps at six so the surface stays readable.
fn build_drill_downs(frame: &ClaimFrame, report: &WitnessReport) -> Vec<DrillDown> {
    let parent_dir = |path: &str| path.rsplit_once('/').map(|(dir, _)| dir.to_owned());
    let mut drills = Vec::new();
    // A history claim whose subjects never reach commit-class
    // documents: query the recorded history surfaces directly.
    if frame.required_witness == WitnessRequirement::History {
        for witness in &report.terms {
            if frame.subjects.contains(&witness.term) && witness.history_documents.is_empty() {
                drills.push(DrillDown {
                    query: format!("type:commit {}", witness.term),
                    reason: format!("`{}` has no commit-class witness", witness.term),
                });
                drills.push(DrillDown {
                    query: format!("type:diff {}", witness.term),
                    reason: format!("`{}` may live only in raw patches", witness.term),
                });
            }
        }
    }
    // A mention-only subject: drill into the directory that mentions
    // it — the subject may live under a name the claim did not use.
    for witness in &report.terms {
        if frame.subjects.contains(&witness.term) && witness.defined.is_empty() {
            if let Some(dir) = witness
                .mention_paths
                .first()
                .and_then(|path| parent_dir(path))
            {
                drills.push(DrillDown {
                    query: format!("path:{dir} {}", witness.term),
                    reason: format!(
                        "`{}` is mention-only; `{dir}` is where it is mentioned",
                        witness.term
                    ),
                });
            }
        }
    }
    // A distinguishing term absent from every returned hit: look it up
    // where it actually lives (mention path) or by itself.
    for term in &report.scope_gaps {
        let mention = report.terms.iter().find(|witness| &witness.term == term);
        let query = mention
            .and_then(|witness| witness.mention_paths.first())
            .and_then(|path| parent_dir(path))
            .map_or_else(|| term.clone(), |dir| format!("path:{dir} {term}"));
        drills.push(DrillDown {
            query,
            reason: format!("`{term}` is absent from every returned hit"),
        });
    }
    // A relation claim whose defined subject is edgeless: the graph
    // records nothing, so textual reference lookup is the fallback.
    if frame.required_witness == WitnessRequirement::Relation {
        for witness in &report.terms {
            if frame.subjects.contains(&witness.term)
                && witness.relations.edges == 0
                && !witness.defined.is_empty()
            {
                drills.push(DrillDown {
                    query: witness.term.clone(),
                    reason: format!(
                        "`{}` has no typed edges; find textual references",
                        witness.term
                    ),
                });
            }
        }
    }
    // Terms co-binding only at file scope: narrow into the bound file
    // to find which span — if any — carries them together.
    for artifact in &report.binding_artifacts {
        if artifact.class == DocumentClass::Code && artifact.scope == BindingScope::File {
            if let Some(term) = frame.distinguishing_terms.first() {
                drills.push(DrillDown {
                    query: format!("path:{} {term}", artifact.path),
                    reason: format!("terms co-bind only at file scope in `{}`", artifact.path),
                });
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    drills.retain(|drill| seen.insert(drill.query.clone()));
    drills.truncate(6);
    drills
}

fn abstain_result(
    store: &MetadataStore,
    mut context: AbstainContext,
    reason: String,
) -> Result<SearchResult> {
    context.missing_capabilities.push(reason.clone());
    context.frame.distinguishing_terms = resolve_distinguishing(
        store,
        &context.request.snapshot_id,
        &content_terms(&context.request.query),
    )?;
    let witness = build_witness_report(store, &context.request.snapshot_id, &context.frame, &[])?;
    let drill_downs = build_drill_downs(&context.frame, &witness);
    let verdict = SearchVerdict {
        state: VerdictState::Abstained,
        reasons: vec![reason],
        witness,
        claim: context.frame,
        evidence_tiers: EvidenceTiers::default(),
        drill_downs,
    };
    Ok(SearchResult {
        request: context.request,
        plan: context.plan,
        manifest: context.manifest,
        hits: Vec::new(),
        missing_capabilities: context.missing_capabilities,
        verdict,
        latency_ms: context
            .started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    })
}

/// Curated CJK→English glossary for the lexical route. Source vocabulary
/// is English, so a query phrased in CJK terms indexes as one monolithic
/// unicode61 token with no anchor into the documents it asks about — the
/// documented pure-CJK boundary. Translating the domain terms it does
/// contain ("置信度" → "confidence") hands the FTS cascade Latin anchors
/// without a model. Keys stay sorted by codepoint; each maps to
/// space-separated English synonyms emitted in order.
static CJK_GLOSSARY: &[(&str, &str)] = &[
    ("上下文", "context"),
    ("仓库", "repository"),
    ("令牌", "token"),
    ("依赖", "dependency"),
    ("修复", "fix repair"),
    ("关系", "relation relationship"),
    ("函数", "function"),
    ("包", "package"),
    ("历史", "history"),
    ("合并", "merge fuse"),
    ("向量", "vector embedding"),
    ("守护", "daemon"),
    ("实体", "entity"),
    ("实现", "implement"),
    ("导入", "import"),
    ("属性", "attribute"),
    ("嵌入", "embedding"),
    ("差异", "diff"),
    ("引用", "reference"),
    ("快照", "snapshot"),
    ("忽略", "ignore"),
    ("扫描", "scan"),
    ("排序", "rank score"),
    ("接口", "interface api"),
    ("提交", "commit"),
    ("文件", "file"),
    ("文档", "document"),
    ("服务", "service"),
    ("权限", "permission"),
    ("构建", "build"),
    ("架构", "architecture"),
    ("标记", "mark tag status"),
    ("检索", "retrieval search"),
    ("模块", "module"),
    ("模式", "schema"),
    ("测试", "test"),
    ("清单", "manifest"),
    ("目录", "directory"),
    ("符号", "symbol"),
    ("类型", "type"),
    ("索引", "index"),
    ("组件", "component"),
    ("缓存", "cache"),
    ("置信度", "confidence"),
    ("能力", "capability"),
    ("范围", "range"),
    ("行", "line"),
    ("视图", "view"),
    ("解析", "parse"),
    ("证据", "evidence"),
    ("调用", "call"),
    ("路径", "path"),
    ("路由", "route"),
    ("边界", "boundary"),
    ("迁移", "migration"),
    ("过期", "stale"),
    ("锁定", "lock"),
    ("陈旧", "stale"),
    ("预算", "budget"),
];

/// Cap on emitted glossary anchors: enough to bridge the vocabulary gap
/// without flooding the FTS term budget (32 terms) or drowning the
/// query's own vocabulary.
const CJK_GLOSSARY_LIMIT: usize = 10;

/// English anchors for the CJK domain terms actually present in `query`,
/// in sorted-table order, deduplicated, capped at `CJK_GLOSSARY_LIMIT`.
/// Empty when the query has no CJK or no covered terms — the lexical
/// query is then used verbatim.
fn cjk_glossary_terms(query: &str) -> Vec<String> {
    if !query.chars().any(has_cjk) {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut terms = Vec::new();
    for &(cjk, english) in CJK_GLOSSARY {
        if !query.contains(cjk) {
            continue;
        }
        for word in english.split_whitespace() {
            if terms.len() >= CJK_GLOSSARY_LIMIT {
                return terms;
            }
            if seen.insert(word) {
                terms.push(word.to_owned());
            }
        }
    }
    terms
}

/// Mine expansion terms from the feedback documents: identifier-split each
/// hit's snippet, symbol name, and path into word terms, then rank by
/// document frequency across the head. Terms already in the query are
/// dropped — re-adding them buys nothing — and stopwords/short/numeric
/// tokens are filtered inside `prf_terms_in`.
fn prf_expansion_terms(query: &str, seeds: &[&Candidate]) -> Vec<String> {
    let query_terms = prf_terms_in(query);
    let mut frequency = HashMap::<String, usize>::new();
    for seed in seeds {
        // Per-document dedup: the score is document frequency across the
        // head, so one snippet repeating a term cannot buy rank.
        let mut document_terms = prf_terms_in(&seed.hit.snippet);
        if let Some(name) = &seed.hit.symbol_name {
            document_terms.extend(prf_terms_in(name));
        }
        if let Some(address) = &seed.hit.address {
            document_terms.extend(prf_terms_in(&address.path));
        }
        for term in document_terms {
            if !query_terms.contains(&term) {
                *frequency.entry(term).or_default() += 1;
            }
        }
    }
    let mut ranked = frequency.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked.truncate(PRF_EXPANSION_TERMS);
    ranked.into_iter().map(|(term, _)| term).collect()
}

/// Tokenize text into candidate expansion terms: `split_identifier_terms`
/// breaks on non-alphanumeric plus `camelCase`/`snake_case`/digit boundaries
/// and lowercases, so source text, symbol names, and paths all yield
/// comparable words. Kept: ≥3 chars, at least one letter (pure digits are
/// version/line noise), not a boilerplate stopword.
fn prf_terms_in(text: &str) -> std::collections::HashSet<String> {
    cce_core::split_identifier_terms(text)
        .into_iter()
        .filter(|term| {
            term.chars().count() >= 3
                && term.chars().any(char::is_alphabetic)
                && !is_prf_stopword(term)
        })
        .collect()
}

/// Feedback stopwords: English function words, language keywords, and
/// file-layout boilerplate — tokens that are frequent yet carry no
/// discriminative signal as vocabulary bridges. Only words of ≥3 chars
/// appear here; shorter tokens are already dropped by the length floor.
fn is_prf_stopword(term: &str) -> bool {
    matches!(
        term,
        "and" | "are" | "was" | "were" | "with" | "this" | "that" | "from" | "into" | "not"
            | "but" | "all" | "any" | "can" | "has" | "have" | "had" | "its" | "our" | "out"
            | "use" | "used" | "using" | "when" | "where" | "which" | "who" | "whom" | "will"
            | "would" | "should" | "could" | "than" | "then" | "them" | "they" | "you" | "your"
            | "how" | "what" | "why" | "does" | "the" | "for"
            // Language keywords and identifier boilerplate.
            | "return" | "function" | "pub" | "let" | "const" | "var" | "new" | "get" | "set"
            | "impl" | "struct" | "enum" | "type" | "def" | "class" | "import" | "export"
            | "async" | "await" | "true" | "false" | "none" | "null" | "nil" | "self" | "void"
            | "int" | "str" | "string" | "bool" | "else" | "match" | "case" | "break" | "continue"
            | "while" | "loop" | "try" | "catch" | "throw" | "throws" | "static" | "final"
            | "public" | "private" | "protected" | "override" | "extends" | "implements"
            | "package" | "crate" | "super" | "mod"
            // File-layout boilerplate mined from path components.
            | "src" | "lib" | "test" | "tests" | "index" | "main" | "pkg" | "cmd" | "app"
    )
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cce_core::{
        RelationKind, RelationOrigin, RelationProvenance, SnapshotIdentity, SourceAddress,
    };
    use cce_store::SnapshotRecords;

    // Test fixtures panic freely: an unmet precondition is a test bug.
    fn commit_into(store: &MetadataStore, records: &SnapshotRecords) {
        store
            .register_repository(&cce_core::RepositoryIdentity {
                id: "repo_test".to_owned(),
                canonical_root: "/repo".to_owned(),
                remote: None,
            })
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snap_test".to_owned(),
            repository_id: "repo_test".to_owned(),
            base_revision: None,
            workspace_overlay_hash: String::new(),
            index_profile_hash: String::new(),
            created_at: chrono::Utc::now(),
            file_count: 0,
            source_bytes: 0,
        };
        store.begin_snapshot(&snapshot).expect("begin snapshot");
        store
            .commit_snapshot(&snapshot, records)
            .expect("commit records");
    }

    fn store_with(records: &SnapshotRecords) -> (tempfile::TempDir, MetadataStore, String) {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = MetadataStore::open(directory.path()).expect("store");
        commit_into(&store, records);
        (directory, store, "snap_test".to_owned())
    }

    /// An engine whose own store carries `records` — `subject_backlinks`
    /// reads entities and relations through `engine.store()`.
    fn engine_with(records: &SnapshotRecords) -> (tempfile::TempDir, CceEngine) {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        commit_into(engine.store(), records);
        (directory, engine)
    }

    fn file(path: &str) -> CodeEntity {
        CodeEntity {
            id: format!("file:{path}"),
            kind: EntityKind::File,
            name: path.to_owned(),
            qualified_name: Some(path.to_owned()),
            signature: None,
            language: None,
            region_id: None,
            address: None,
            capabilities: Vec::new(),
            attributes: serde_json::Map::new(),
        }
    }

    fn symbol(name: &str, path: &str) -> CodeEntity {
        CodeEntity {
            id: format!("symbol:{name}"),
            kind: EntityKind::Function,
            name: name.to_owned(),
            qualified_name: None,
            signature: None,
            language: None,
            region_id: Some(format!("region:{name}")),
            address: Some(
                SourceAddress::new("repo_test", "snap_test", path, 0..1, 1..=1).expect("address"),
            ),
            capabilities: Vec::new(),
            attributes: serde_json::Map::new(),
        }
    }

    fn changed_with(source: &str, target: &str, confidence: f32) -> cce_core::Relation {
        cce_core::Relation {
            id: format!("rel:{source}:{target}"),
            source_entity_id: source.to_owned(),
            target_entity_id: target.to_owned(),
            kind: RelationKind::ChangedWith,
            origin: RelationOrigin::FrameworkRule,
            confidence,
            snapshot_id: "snap_test".to_owned(),
            extractor: "test".to_owned(),
            evidence: Vec::new(),
            attributes: serde_json::Map::new(),
        }
    }

    fn calls(source: &str, target: &str) -> cce_core::Relation {
        relation(source, target, RelationKind::Calls)
    }

    fn other(source: &str, target: &str) -> cce_core::Relation {
        relation(source, target, RelationKind::Other("custom".to_owned()))
    }

    fn relation(source: &str, target: &str, kind: RelationKind) -> cce_core::Relation {
        cce_core::Relation {
            id: format!("rel:{source}:{target}:{kind:?}"),
            source_entity_id: source.to_owned(),
            target_entity_id: target.to_owned(),
            kind,
            origin: RelationOrigin::TreeSitter,
            confidence: 1.0,
            snapshot_id: "snap_test".to_owned(),
            extractor: "test".to_owned(),
            evidence: Vec::new(),
            attributes: serde_json::Map::new(),
        }
    }

    fn candidate(document_id: &str, path: &str, fused_score: f64) -> Candidate {
        Candidate {
            hit: SearchHit {
                document_id: document_id.to_owned(),
                entity_id: format!("file:{path}"),
                region_id: None,
                symbol_name: None,
                representation: RetrievalRepresentation::RawCode,
                route: SearchRoute::Lexical,
                rank: 1,
                score: 0.0,
                contributing_routes: vec![SearchRoute::Lexical],
                address: Some(
                    SourceAddress::new("repo_test", "snap_test", path, 0..1, 1..=1)
                        .expect("address"),
                ),
                evidence: Vec::new(),
                snippet: String::new(),
                verified_current: true,
                explanation: Vec::new(),
            },
            fused_score,
            strict: true,
        }
    }

    /// Same-file candidates need distinct byte ranges or region dedup
    /// collapses them before the per-file cap is even consulted.
    fn candidate_at(document_id: &str, path: &str, byte_start: u64, fused_score: f64) -> Candidate {
        let mut candidate = candidate(document_id, path, fused_score);
        candidate.hit.address = Some(
            SourceAddress::new(
                "repo_test",
                "snap_test",
                path,
                byte_start..byte_start + 1,
                1..=1,
            )
            .expect("address"),
        );
        candidate
    }

    /// A hit without a source path bypasses per-file accounting entirely.
    fn candidate_pathless(document_id: &str, fused_score: f64) -> Candidate {
        let mut candidate = candidate(document_id, "src/nowhere.rs", fused_score);
        candidate.hit.address = None;
        candidate
    }

    /// A surfaced hit pointing at `entity_id` — the input shape
    /// `subject_backlinks` reads.
    fn hit(entity_id: &str, path: &str) -> SearchHit {
        SearchHit {
            document_id: format!("doc:{entity_id}"),
            entity_id: entity_id.to_owned(),
            region_id: Some(format!("region:{entity_id}")),
            symbol_name: Some(entity_id.to_owned()),
            representation: RetrievalRepresentation::RawCode,
            route: SearchRoute::DenseRaw,
            rank: 1,
            score: 0.5,
            contributing_routes: vec![SearchRoute::DenseRaw],
            address: Some(
                SourceAddress::new("repo_test", "snap_test", path, 0..1, 1..=1).expect("address"),
            ),
            evidence: Vec::new(),
            snippet: String::new(),
            verified_current: true,
            explanation: Vec::new(),
        }
    }

    fn search_result(routes: &[SearchRoute], hits: Vec<SearchHit>) -> SearchResult {
        SearchResult {
            request: SearchRequest {
                repository_id: String::new(),
                snapshot_id: "snap_test".to_owned(),
                query: "where is snapshot freshness decided?".to_owned(),
                intent: None,
                limit: 10,
                require_fresh: false,
                routes: Vec::new(),
                filters: cce_core::QueryFilters::default(),
            },
            plan: QueryPlan {
                intent: QueryIntent::NaturalLanguageBehavior,
                routes: routes.to_vec(),
                graph_policy: GraphPolicy::Opportunistic,
                required_views: Vec::new(),
                reasons: Vec::new(),
            },
            manifest: ViewManifest {
                repository_id: "repo_test".to_owned(),
                snapshot_id: "snap_test".to_owned(),
                views: std::collections::BTreeMap::new(),
            },
            hits,
            missing_capabilities: Vec::new(),
            verdict: SearchVerdict {
                state: VerdictState::default(),
                reasons: Vec::new(),
                claim: ClaimFrame {
                    intent: QueryIntent::NaturalLanguageBehavior,
                    predicate: ClaimPredicate::Lookup,
                    required_witness: WitnessRequirement::Any,
                    subjects: Vec::new(),
                    distinguishing_terms: Vec::new(),
                },
                witness: WitnessReport::default(),
                evidence_tiers: EvidenceTiers::default(),
                drill_downs: Vec::new(),
            },
            latency_ms: 0,
        }
    }

    const SUBJECT_ROUTES: &[SearchRoute] = &[SearchRoute::Structural, SearchRoute::Lexical];

    #[test]
    fn backlink_pairs_test_evidence_with_the_function_it_calls() {
        let evidence = symbol("asserts_freshness_semantics", "tests/freshness.rs");
        let subject = symbol("decide_freshness", "src/freshness.rs");
        let records = SnapshotRecords {
            entities: vec![evidence.clone(), subject.clone()],
            relations: vec![relation(&evidence.id, &subject.id, RelationKind::Calls)],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let mut unverified_hit = hit(&evidence.id, "tests/freshness.rs");
        unverified_hit.verified_current = false;
        let search = search_result(SUBJECT_ROUTES, vec![unverified_hit]);
        let backlinks = engine.subject_backlinks(&search).expect("backlinks");
        assert_eq!(backlinks.len(), 1);
        let (index, backlink) = &backlinks[0];
        // The returned index is the source hit's own position — the
        // caller inserts the subject right after its pointer.
        assert_eq!(*index, 0);
        assert_eq!(backlink.entity_id, subject.id);
        assert_eq!(backlink.route, SearchRoute::Structural);
        assert!(!backlink.verified_current);
        assert!(
            backlink
                .explanation
                .iter()
                .any(|line| line.contains("asserts_freshness_semantics"))
        );
    }

    #[test]
    fn backlink_index_marks_each_subject_after_its_own_source() {
        let first = symbol("watches_committed", "tests/watch.rs");
        let second = symbol("asserts_reuse", "tests/reuse.rs");
        let first_subject = symbol("is_stale", "src/watch.rs");
        let second_subject = symbol("resolve_index", "src/retrieval.rs");
        let records = SnapshotRecords {
            entities: vec![
                first.clone(),
                second.clone(),
                first_subject.clone(),
                second_subject.clone(),
            ],
            relations: vec![
                relation(&first.id, &first_subject.id, RelationKind::Calls),
                relation(&second.id, &second_subject.id, RelationKind::Calls),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let search = search_result(
            SUBJECT_ROUTES,
            vec![
                hit(&first.id, "tests/watch.rs"),
                hit(&second.id, "tests/reuse.rs"),
            ],
        );
        let backlinks = engine.subject_backlinks(&search).expect("backlinks");
        let pairs: Vec<(usize, &str)> = backlinks
            .iter()
            .map(|(index, backlink)| (*index, backlink.entity_id.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (0, first_subject.id.as_str()),
                (1, second_subject.id.as_str())
            ]
        );
    }

    #[test]
    fn backlink_skips_plans_without_the_structural_route() {
        let evidence = symbol("asserts_freshness_semantics", "tests/freshness.rs");
        let subject = symbol("decide_freshness", "src/freshness.rs");
        let records = SnapshotRecords {
            entities: vec![evidence.clone(), subject.clone()],
            relations: vec![relation(&evidence.id, &subject.id, RelationKind::Calls)],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let search = search_result(
            &[SearchRoute::Lexical],
            vec![hit(&evidence.id, "tests/freshness.rs")],
        );
        assert!(
            engine
                .subject_backlinks(&search)
                .expect("backlinks")
                .is_empty()
        );
    }

    #[test]
    fn backlink_never_repeats_an_already_surfaced_entity() {
        let evidence = symbol("asserts_freshness_semantics", "tests/freshness.rs");
        let subject = symbol("decide_freshness", "src/freshness.rs");
        let records = SnapshotRecords {
            entities: vec![evidence.clone(), subject.clone()],
            relations: vec![relation(&evidence.id, &subject.id, RelationKind::Calls)],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let search = search_result(
            SUBJECT_ROUTES,
            vec![
                hit(&evidence.id, "tests/freshness.rs"),
                hit(&subject.id, "src/freshness.rs"),
            ],
        );
        assert!(
            engine
                .subject_backlinks(&search)
                .expect("backlinks")
                .is_empty()
        );
    }

    #[test]
    fn backlink_skips_subjects_that_are_themselves_tests() {
        let evidence = symbol("asserts_freshness_semantics", "tests/freshness.rs");
        let helper = symbol("freshness_helper", "tests/helpers.rs");
        let records = SnapshotRecords {
            entities: vec![evidence.clone(), helper.clone()],
            relations: vec![relation(&evidence.id, &helper.id, RelationKind::Calls)],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let search = search_result(
            SUBJECT_ROUTES,
            vec![hit(&evidence.id, "tests/freshness.rs")],
        );
        assert!(
            engine
                .subject_backlinks(&search)
                .expect("backlinks")
                .is_empty()
        );
    }

    #[test]
    fn backlink_prefers_confident_relation_over_a_weaker_call() {
        let evidence = symbol("asserts_freshness_semantics", "tests/freshness.rs");
        let called = symbol("incidental_helper", "src/util.rs");
        let referenced = symbol("freshness", "src/freshness.rs");
        let mut weak_call = relation(&evidence.id, &called.id, RelationKind::Calls);
        weak_call.confidence = 0.3;
        let records = SnapshotRecords {
            entities: vec![evidence.clone(), called, referenced.clone()],
            relations: vec![
                weak_call,
                relation(&evidence.id, &referenced.id, RelationKind::References),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let search = search_result(
            SUBJECT_ROUTES,
            vec![hit(&evidence.id, "tests/freshness.rs")],
        );
        let backlinks = engine.subject_backlinks(&search).expect("backlinks");
        assert_eq!(backlinks.len(), 1);
        // `referenced` wins twice over — the query names it and its edge
        // is extractor-confident — but confidence alone must already
        // outrank the ambiguous spelling-resolved call.
        assert_eq!(backlinks[0].1.entity_id, referenced.id);
    }

    #[test]
    fn backlink_ignores_hits_without_a_source_address() {
        let evidence = symbol("asserts_freshness_semantics", "tests/freshness.rs");
        let subject = symbol("decide_freshness", "src/freshness.rs");
        let records = SnapshotRecords {
            entities: vec![evidence.clone(), subject.clone()],
            relations: vec![relation(&evidence.id, &subject.id, RelationKind::Calls)],
            ..SnapshotRecords::default()
        };
        let (_dir, engine) = engine_with(&records);
        let mut pathless = hit(&evidence.id, "tests/freshness.rs");
        pathless.address = None;
        let search = search_result(SUBJECT_ROUTES, vec![pathless]);
        assert!(
            engine
                .subject_backlinks(&search)
                .expect("backlinks")
                .is_empty()
        );
    }

    #[test]
    fn windowed_per_file_cap_diversifies_top_five() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        let mut candidates = HashMap::new();
        // One file owns the three best-scored candidates; under the flat
        // 3-per-file cap it would sweep the head.
        for (suffix, score) in [("1", 0.9), ("2", 0.8), ("3", 0.7)] {
            let id = format!("a{suffix}");
            candidates.insert(
                id.clone(),
                candidate_at(
                    &id,
                    "src/a.rs",
                    suffix.parse::<u64>().expect("u64") * 10,
                    score,
                ),
            );
        }
        for (name, score) in [("b", 0.6), ("c", 0.5), ("d", 0.4), ("e", 0.3)] {
            candidates.insert(
                name.to_owned(),
                candidate_at(name, &format!("src/{name}.rs"), 0, score),
            );
        }

        let hits = engine
            .select_hits(
                &request("query", 5),
                &candidates,
                &claim_frame(QueryIntent::Unknown, Vec::new()),
            )
            .expect("select hits");
        assert_eq!(hits.len(), 5);
        let paths = hits
            .iter()
            .filter_map(|hit| hit.address.as_ref().map(|address| address.path.as_str()))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            paths.len(),
            5,
            "top-5 must show five distinct files: {paths:?}"
        );
        // The capped file keeps only its best hit, still ranked first.
        assert_eq!(
            hits.first()
                .and_then(|hit| hit.address.as_ref())
                .map(|a| a.path.as_str()),
            Some("src/a.rs")
        );
    }

    #[test]
    fn windowed_per_file_cap_fills_limit_with_what_remains() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        let mut candidates = HashMap::new();
        for (file, scores) in [
            ("src/a.rs", [0.95_f64, 0.8, 0.65, 0.5]),
            ("src/b.rs", [0.9_f64, 0.75, 0.6, 0.45]),
        ] {
            for (index, score) in scores.into_iter().enumerate() {
                let id = format!("{file}:{index}");
                candidates.insert(
                    id.clone(),
                    candidate_at(&id, file, index as u64 * 10, score),
                );
            }
        }

        // Only two files carry hits: the top-5 window caps each at one and
        // the starved head is accepted — the list fills with what remains.
        let starved = engine
            .select_hits(
                &request("query", 5),
                &candidates,
                &claim_frame(QueryIntent::Unknown, Vec::new()),
            )
            .expect("select hits");
        assert_eq!(starved.len(), 2);

        // Hits without a source path are uncapped, so the same two files
        // still fill up to the limit — and once the list crosses 5 the cap
        // relaxes, letting a file's next document in on its own merits.
        for (index, score) in [0.85_f64, 0.7, 0.55, 0.4].into_iter().enumerate() {
            let id = format!("pathless:{index}");
            candidates.insert(id.clone(), candidate_pathless(&id, score));
        }
        let filled = engine
            .select_hits(
                &request("query", 6),
                &candidates,
                &claim_frame(QueryIntent::Unknown, Vec::new()),
            )
            .expect("select hits");
        assert_eq!(filled.len(), 6);
        let count_of = |path: &str| {
            filled
                .iter()
                .filter(|hit| {
                    hit.address
                        .as_ref()
                        .is_some_and(|address| address.path == path)
                })
                .count()
        };
        assert_eq!(
            count_of("src/a.rs"),
            2,
            "cap relaxes to 2 past the top-5 window"
        );
        assert_eq!(count_of("src/b.rs"), 1);
    }

    #[test]
    fn graph_flow_emits_called_mechanism_and_corroborates() {
        // Seeds a and b both call `set_view_status` — consensus flow makes
        // the mechanism the dominant sink and it emits as an `entity:`
        // hit. `isolated_helper` hangs off a zero-weight `Other` edge and
        // receives nothing; the `changed_with` edge between the seeds
        // relays mass both ways so both corroborate while the isolated
        // candidate c earns none.
        let records = SnapshotRecords {
            entities: vec![
                file("src/a.rs"),
                file("src/b.rs"),
                symbol("set_view_status", "src/m.rs"),
                symbol("isolated_helper", "src/m.rs"),
            ],
            relations: vec![
                calls("file:src/a.rs", "symbol:set_view_status"),
                calls("file:src/b.rs", "symbol:set_view_status"),
                other("file:src/a.rs", "symbol:isolated_helper"),
                changed_with("file:src/a.rs", "file:src/b.rs", 0.9),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.05));
        candidates.insert("c".to_owned(), candidate("c", "src/c.rs", 0.05));
        apply_graph_flow(
            &store,
            &snapshot_id,
            "mark views stale",
            &ExpansionEvidence::default(),
            &mut candidates,
            true,
            50,
        )
        .expect("graph flow");

        let emitted = &candidates["entity:symbol:set_view_status"].hit;
        assert_eq!(emitted.route, SearchRoute::Structural);
        assert_eq!(emitted.symbol_name.as_deref(), Some("set_view_status"));
        assert!(emitted.verified_current);
        assert!(
            emitted
                .explanation
                .iter()
                .any(|line| line.contains("graph flow evidence"))
        );
        assert!(!candidates.contains_key("entity:symbol:isolated_helper"));

        for key in ["a", "b"] {
            assert!(
                candidates[key]
                    .hit
                    .explanation
                    .iter()
                    .any(|line| line.contains("graph flow corroboration")),
                "{key} must explain its received flow"
            );
        }
        assert!((candidates["a"].fused_score - 0.1).abs() > f64::EPSILON);
        assert!((candidates["b"].fused_score - 0.05).abs() > f64::EPSILON);
        assert!((candidates["c"].fused_score - 0.05).abs() < f64::EPSILON);
        assert!(candidates["c"].hit.explanation.is_empty());
    }

    #[test]
    fn graph_flow_seeds_expansion_evidence_entities() {
        // `relay_fn`/`relay_two` were surfaced by structural expansion —
        // they are not candidates, but their recorded propagated scores
        // seed flow, so the mechanism they both call becomes reachable
        // in hop 1 and emits on two-path consensus. Without expansion
        // seeding `mechanism_fn` is outside the seed star entirely:
        // candidate → expansion-entity → mechanism is the
        // corroboration-join path only this seeding opens.
        let records = SnapshotRecords {
            entities: vec![
                file("src/a.rs"),
                symbol("relay_fn", "src/x.rs"),
                symbol("relay_two", "src/y.rs"),
                symbol("mechanism_fn", "src/m.rs"),
            ],
            relations: vec![
                calls("symbol:relay_fn", "symbol:mechanism_fn"),
                calls("symbol:relay_two", "symbol:mechanism_fn"),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut expansion = ExpansionEvidence::default();
        expansion.record(&symbol("relay_fn", "src/x.rs"), 0.05);
        expansion.record(&symbol("relay_two", "src/y.rs"), 0.04);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        apply_graph_flow(
            &store,
            &snapshot_id,
            "query",
            &expansion,
            &mut candidates,
            true,
            50,
        )
        .expect("graph flow");

        let emitted = &candidates["entity:symbol:mechanism_fn"].hit;
        assert_eq!(emitted.route, SearchRoute::Structural);
        assert_eq!(emitted.symbol_name.as_deref(), Some("mechanism_fn"));
        // The expansion seeds are evidence, not results — they must not
        // emit themselves.
        assert!(!candidates.contains_key("entity:symbol:relay_fn"));
        assert!(!candidates.contains_key("entity:symbol:relay_two"));
    }

    #[test]
    fn graph_flow_bonuses_expansion_evidenced_candidate() {
        // The corroboration join's direct channel: a candidate whose
        // entity/path structural expansion surfaced is corroborated
        // even when the flow graph returns no mass to it — seed mass
        // is not self-evidence, but expansion membership is external
        // evidence. `b` matches its file entity at full strength; `c`
        // is unevidenced; the structural candidate IS the evidence and
        // must not corroborate itself. The lone calls edge keeps the
        // edge universe non-empty; its target stays unemitted under
        // the single-sender admission floor.
        let records = SnapshotRecords {
            entities: vec![
                file("src/b.rs"),
                file("src/c.rs"),
                symbol("helper_fn", "src/m.rs"),
            ],
            relations: vec![calls("file:src/b.rs", "symbol:helper_fn")],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut expansion = ExpansionEvidence::default();
        expansion.record(&file("src/b.rs"), 0.05);

        let mut candidates = HashMap::new();
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.1));
        candidates.insert("c".to_owned(), candidate("c", "src/c.rs", 0.1));
        let mut structural = candidate("s", "src/s.rs", 0.1);
        structural
            .hit
            .contributing_routes
            .push(SearchRoute::Structural);
        candidates.insert("s".to_owned(), structural);
        apply_graph_flow(
            &store,
            &snapshot_id,
            "query",
            &expansion,
            &mut candidates,
            true,
            50,
        )
        .expect("graph flow");

        let expected = FLOW_BONUS / RRF_K;
        assert!((candidates["b"].fused_score - 0.1 - expected).abs() < 1e-9);
        assert!((candidates["c"].fused_score - 0.1).abs() < f64::EPSILON);
        assert!((candidates["s"].fused_score - 0.1).abs() < f64::EPSILON);
        assert!(!candidates.contains_key("entity:symbol:helper_fn"));
        assert!(
            candidates["b"]
                .hit
                .explanation
                .iter()
                .any(|line| line.contains("corroborated by graph expansion"))
        );
    }

    #[test]
    fn graph_flow_emission_requires_consensus_or_overlap() {
        // The emission admission floor is the join's pointer gate in
        // flow terms: `noisy_neighbor` hangs off a single seed path
        // with no name overlap and must not spend an emission slot,
        // while `set_view_status` admits on the plural-tolerant name
        // lane alone ("views" collapses onto "view") and
        // `consensus_target` on two independent senders.
        let records = SnapshotRecords {
            entities: vec![
                file("src/a.rs"),
                file("src/b.rs"),
                symbol("set_view_status", "src/m.rs"),
                symbol("consensus_target", "src/m.rs"),
                symbol("noisy_neighbor", "src/m.rs"),
            ],
            relations: vec![
                calls("file:src/a.rs", "symbol:set_view_status"),
                calls("file:src/a.rs", "symbol:consensus_target"),
                calls("file:src/b.rs", "symbol:consensus_target"),
                calls("file:src/a.rs", "symbol:noisy_neighbor"),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.05));
        apply_graph_flow(
            &store,
            &snapshot_id,
            "mark views stale",
            &ExpansionEvidence::default(),
            &mut candidates,
            true,
            50,
        )
        .expect("graph flow");

        let named = &candidates["entity:symbol:set_view_status"].hit;
        assert_eq!(named.route, SearchRoute::Structural);
        assert_eq!(named.symbol_name.as_deref(), Some("set_view_status"));
        assert!(candidates.contains_key("entity:symbol:consensus_target"));
        assert!(!candidates.contains_key("entity:symbol:noisy_neighbor"));
    }

    #[test]
    fn graph_flow_file_seed_reaches_member_through_contains() {
        // A file-level candidate is topical evidence for its own
        // members: the file-kind seed injects into `member_fn` through
        // its contains edge, so the member becomes flow-reachable in
        // hop 1 and emits through the named lane — the granularity hop
        // the champion file-vote approximated by fiat. The contains
        // edge is the member's ONLY path to seed mass, so its emission
        // proves the hop conducted. An unrelated member of a non-seed
        // file receives nothing.
        let records = SnapshotRecords {
            entities: vec![
                file("src/a.rs"),
                file("src/c.rs"),
                symbol("member_fn", "src/a.rs"),
                symbol("foreign_fn", "src/c.rs"),
            ],
            relations: vec![
                relation("file:src/a.rs", "symbol:member_fn", RelationKind::Contains),
                relation("file:src/c.rs", "symbol:foreign_fn", RelationKind::Contains),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        apply_graph_flow(
            &store,
            &snapshot_id,
            "member function",
            &ExpansionEvidence::default(),
            &mut candidates,
            true,
            50,
        )
        .expect("graph flow");

        let emitted = &candidates["entity:symbol:member_fn"].hit;
        assert_eq!(emitted.route, SearchRoute::Structural);
        assert_eq!(emitted.symbol_name.as_deref(), Some("member_fn"));
        assert!(!candidates.contains_key("entity:symbol:foreign_fn"));
    }

    #[test]
    fn graph_flow_empty_pool_is_noop() {
        // No-context guard: an empty candidate pool must not touch the
        // store or panic — abstention stays clean.
        let records = SnapshotRecords::default();
        let (_dir, store, snapshot_id) = store_with(&records);
        let mut candidates = HashMap::new();
        apply_graph_flow(
            &store,
            &snapshot_id,
            "anything",
            &ExpansionEvidence::default(),
            &mut candidates,
            true,
            50,
        )
        .expect("graph flow");
        assert!(candidates.is_empty());
    }

    fn file_touched(path: &str, last_touched: i64) -> CodeEntity {
        let mut entity = file(path);
        entity.attributes =
            serde_json::Map::from_iter([("lastTouched".to_owned(), last_touched.into())]);
        entity
    }

    fn package(name: &str, root_dir: &str) -> CodeEntity {
        CodeEntity {
            id: format!("package:{name}"),
            kind: EntityKind::Package,
            name: name.to_owned(),
            qualified_name: None,
            signature: None,
            language: None,
            region_id: None,
            address: None,
            capabilities: Vec::new(),
            attributes: serde_json::Map::from_iter([("rootDir".to_owned(), root_dir.into())]),
        }
    }

    #[test]
    fn recency_prior_prefers_recently_touched_under_issue_intent() {
        let now = 1_700_000_000_i64;
        let day = 86_400_i64;
        let records = SnapshotRecords {
            entities: vec![
                file_touched("src/recent.rs", now - day),
                file_touched("src/stale.rs", now - 400 * day),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);
        let build = || {
            let mut candidates = HashMap::new();
            candidates.insert("r".to_owned(), candidate("r", "src/recent.rs", 0.1));
            candidates.insert("s".to_owned(), candidate("s", "src/stale.rs", 0.1));
            candidates
        };

        // Issue intent: the day-old file earns ~0.2, the 400-day file ~0.
        let mut issue = build();
        apply_structural_features(
            &store,
            &snapshot_id,
            QueryIntent::IssueLocalization,
            now,
            "query",
            &mut issue,
        )
        .expect("structural features");
        assert!(issue["r"].fused_score > issue["s"].fused_score);
        assert!(
            issue["r"]
                .hit
                .explanation
                .iter()
                .any(|line| line.contains("recently touched"))
        );

        // Behavior intent carries no recency prior at all.
        let mut behavior = build();
        apply_structural_features(
            &store,
            &snapshot_id,
            QueryIntent::NaturalLanguageBehavior,
            now,
            "query",
            &mut behavior,
        )
        .expect("structural features");
        assert!((behavior["r"].fused_score - behavior["s"].fused_score).abs() < f64::EPSILON);
    }

    #[test]
    fn same_package_candidate_outranks_cross_package() {
        let records = SnapshotRecords {
            entities: vec![
                package("alpha", "crates/alpha"),
                package("beta", "crates/beta"),
                package("root", ""),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        // All three top hits live in alpha, so the task package set is
        // {alpha}; the compared candidates sit outside the top-3 entirely.
        let mut candidates = HashMap::new();
        candidates.insert(
            "a1".to_owned(),
            candidate("a1", "crates/alpha/src/lib.rs", 0.5),
        );
        candidates.insert(
            "a2".to_owned(),
            candidate("a2", "crates/alpha/src/main.rs", 0.4),
        );
        candidates.insert(
            "a3".to_owned(),
            candidate("a3", "crates/alpha/src/mods.rs", 0.3),
        );
        candidates.insert(
            "same".to_owned(),
            candidate("same", "crates/alpha/src/helper.rs", 0.1),
        );
        candidates.insert(
            "diff".to_owned(),
            candidate("diff", "crates/beta/src/lib.rs", 0.1),
        );
        candidates.insert("root".to_owned(), candidate("root", "README.md", 0.1));
        apply_structural_features(
            &store,
            &snapshot_id,
            QueryIntent::NaturalLanguageBehavior,
            0,
            "query",
            &mut candidates,
        )
        .expect("structural features");

        // None of the three compared files is a top-3 hit, so the fused
        // gap is exactly the package bonus.
        let expected = PACKAGE_BONUS / RRF_K;
        assert!(
            (candidates["same"].fused_score - candidates["diff"].fused_score - expected).abs()
                < 1e-9,
            "same {} vs diff {}",
            candidates["same"].fused_score,
            candidates["diff"].fused_score
        );
        assert!(
            candidates["same"]
                .hit
                .explanation
                .iter()
                .any(|line| line.contains("same package"))
        );
        for outsider in ["diff", "root"] {
            assert!(
                !candidates[outsider]
                    .hit
                    .explanation
                    .iter()
                    .any(|line| line.contains("same package")),
                "{outsider} must not earn a package bonus"
            );
        }
    }

    /// Mutate a real spelling the way the perturbation trap families do.
    /// The near-miss literals must never appear in source: a verbatim
    /// trap spelling in committed code indexes into the corpus and
    /// falsifies the "provably absent" claim the veto relies on — the
    /// self-dogfood benchmark would then be testing a contaminated
    /// index.
    fn pluralized(name: &str) -> String {
        format!("{name}s")
    }
    fn swapped_last(name: &str) -> String {
        let mut chars: Vec<char> = name.chars().collect();
        let last = chars.len() - 1;
        chars.swap(last - 1, last);
        chars.into_iter().collect()
    }
    fn truncated(name: &str) -> String {
        name.chars().take(name.chars().count() - 1).collect()
    }

    #[test]
    fn backtick_terms_extract_verbatim_spans() {
        assert_eq!(
            backtick_terms("Where is `SourceAddress` defined?"),
            ["SourceAddress"]
        );
        // Multiple spans each yield their token; `:` is preserved
        // inside qualified names.
        assert_eq!(
            backtick_terms("how `SourceAddress` and `std::fmt` interact"),
            ["SourceAddress", "std::fmt"]
        );
        // Unterminated spans and short tokens are ignored.
        assert!(backtick_terms("a `dangling span").is_empty());
        assert!(backtick_terms("`ok`").is_empty());
    }

    #[test]
    fn literal_anchors_cover_verbatim_and_name_shaped() {
        // The trap families: backticked near-miss spellings are anchors
        // whether or not they look like identifiers.
        let misspelled = pluralized("SourceAddress");
        assert_eq!(
            literal_anchors(&format!("Where is `{misspelled}` defined?")),
            [misspelled]
        );
        let misspelled = pluralized("search");
        assert_eq!(
            literal_anchors(&format!("Where is `{misspelled}` defined?")),
            [misspelled]
        );
        // Name-shaped tokens anchor even without backticks.
        let misspelled = pluralized("IndexLease");
        assert_eq!(
            literal_anchors(&format!("Where is {misspelled} defined?")),
            [misspelled]
        );
        // A sole all-alpha anchor is still checkable.
        let misspelled = pluralized("search");
        assert_eq!(literal_anchors(&misspelled), [misspelled]);
    }

    #[test]
    fn literal_anchors_skip_values_and_cjk() {
        // Backticked values are not entity spellings: versions, paths,
        // and dotted names never veto.
        assert!(literal_anchors("the `18.04` image fails on pytest").is_empty());
        // A sole dotted value is a retrieval matter, not a veto.
        assert!(literal_anchors("Where is config.toml defined?").is_empty());
        // CJK tokens never anchor a veto — monolithic indexing cannot
        // prove their absence.
        assert!(literal_anchors("快照在哪里定义").is_empty());
        assert!(literal_anchors("`快照`在哪定义").is_empty());
        // ...but a Latin anchor in a code-switched query still vetoes.
        let misspelled = swapped_last("ViewStatus");
        assert_eq!(
            literal_anchors(&format!("{misspelled} 在哪定义")),
            [misspelled]
        );
        // Punctuation-only tokens cannot emit an FTS phrase.
        assert!(literal_anchors("where is ::: defined").is_empty());
    }

    #[test]
    fn content_terms_keep_claim_vocabulary() {
        // The claim's specificity lives in plain lowercase words —
        // "distributed" and "bearer" are not identifier-shaped but they
        // ARE the claim. Shape words ("how", "implemented", "logic") are
        // grammar, not content.
        assert_eq!(
            content_terms("how does the repository implement distributed locking"),
            ["distributed", "locking"]
        );
        assert_eq!(
            content_terms("how does the gateway handle bearer token validation"),
            ["gateway", "bearer", "token", "validation"]
        );
        // Identifier-shaped terms survive lowercasing for matching;
        // CJK cannot bind in the monolithic index and is excluded.
        assert_eq!(
            content_terms("Where is `ViewStatus` defined?"),
            ["viewstatus"]
        );
        assert!(content_terms("快照在哪里定义").is_empty());
    }

    #[test]
    fn name_witnesses_requires_contiguous_part_runs() {
        // Contiguous part-run folds witness a term; inner substrings of
        // a single part do not — `block` contains "lock" textually but
        // never names it.
        assert!(name_witnesses("lock", "LockManager"));
        assert!(name_witnesses("lock", "lock_manager.rs"));
        assert!(name_witnesses("lock", "src/lock/mod.rs"));
        assert!(name_witnesses("websocket", "WebSocket"));
        assert!(name_witnesses("websocket", "WebSocketHandler"));
        assert!(name_witnesses("socket", "WebSocket"));
        assert!(!name_witnesses("lock", "blockchain.rs"));
        // Plural-suffix drift is a different spelling, not a witness —
        // the same exactness the literal veto relies on.
        assert!(!name_witnesses("lock", "src/locks/mod.rs"));
        assert!(!name_witnesses("websocket", "ws_handler.rs"));
        // The query's own near-miss spellings witness nothing real.
        assert!(!name_witnesses(&pluralized("ViewStatus"), "ViewStatus"));
    }

    #[test]
    fn defined_witnesses_qualified_subject_needs_each_segment_defined() {
        // `ArtifactStore::open` decomposes into scope + member: `open` is
        // a method where `ArtifactStore` is defined — qualified names in
        // the index are file-path-qualified, so each `::` segment must
        // witness independently. `NonexistentType::open` must not be
        // witnessed by the bare `open` methods.
        let mut artifact_open = symbol("open", "crates/cce-store/src/artifact.rs");
        artifact_open.qualified_name = Some("crates/cce-store/src/artifact.rs::open".to_owned());
        let mut engine_open = symbol("open", "crates/cce-engine/src/engine.rs");
        engine_open.id = "symbol:open:engine".to_owned();
        engine_open.qualified_name = Some("crates/cce-engine/src/engine.rs::open".to_owned());
        let mut artifact_store = symbol("ArtifactStore", "crates/cce-store/src/artifact.rs");
        artifact_store.id = "symbol:artifactstore".to_owned();
        artifact_store.kind = EntityKind::Struct;
        let records = SnapshotRecords {
            entities: vec![
                artifact_open,
                engine_open,
                artifact_store,
                file("crates/cce-store/src/artifact.rs"),
                file("crates/cce-engine/src/engine.rs"),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let witnesses =
            defined_witnesses(&store, &snapshot_id, "ArtifactStore::open").expect("witnesses");
        assert!(
            !witnesses.is_empty(),
            "scope + member both defined: the qualified subject witnesses"
        );
        // The reported witnesses are the member definitions, scope-
        // coherent first: `open` living where `ArtifactStore` is defined
        // outranks the unrelated `open` in engine.rs.
        assert_eq!(
            witnesses[0].path.as_deref(),
            Some("crates/cce-store/src/artifact.rs"),
            "scope-coherent member must order first: {witnesses:?}"
        );

        let ghost =
            defined_witnesses(&store, &snapshot_id, "NonexistentType::open").expect("ghost");
        assert!(
            ghost.is_empty(),
            "an undefined scope segment must not be witnessed by the bare member"
        );

        let ghost_member =
            defined_witnesses(&store, &snapshot_id, "ArtifactStore::nonexistent_method")
                .expect("ghost member");
        assert!(
            ghost_member.is_empty(),
            "an undefined member segment must not be witnessed by the scope alone"
        );
    }

    #[test]
    fn claim_frame_maps_intent_to_witness_requirement() {
        let frame = claim_frame(QueryIntent::ExactEntity, vec!["Foo".to_owned()]);
        assert_eq!(frame.predicate, ClaimPredicate::Definition);
        assert_eq!(frame.required_witness, WitnessRequirement::Definition);
        let frame = claim_frame(QueryIntent::NaturalLanguageBehavior, Vec::new());
        assert_eq!(frame.predicate, ClaimPredicate::Implementation);
        assert_eq!(frame.required_witness, WitnessRequirement::CodeBinding);
        let frame = claim_frame(QueryIntent::Impact, Vec::new());
        assert_eq!(frame.predicate, ClaimPredicate::Relation);
        assert_eq!(frame.required_witness, WitnessRequirement::Relation);
        let frame = claim_frame(QueryIntent::History, Vec::new());
        assert_eq!(frame.predicate, ClaimPredicate::History);
        assert_eq!(frame.required_witness, WitnessRequirement::History);
        // PreciseDataflow keeps `Any`: the dedicated capability refusal
        // is its honest reason; a relation gate would preempt it.
        let frame = claim_frame(QueryIntent::PreciseDataflow, Vec::new());
        assert_eq!(frame.required_witness, WitnessRequirement::Any);
    }

    /// Every drill-down suggestion is a verbatim query in the query
    /// language itself, derived only from reported witness gaps.
    #[test]
    fn drill_downs_derive_from_witness_gaps() {
        // History claim, subject absent from commit-class docs →
        // type:commit and type:diff.
        let frame = ClaimFrame {
            intent: QueryIntent::History,
            predicate: ClaimPredicate::History,
            required_witness: WitnessRequirement::History,
            subjects: vec!["gateway".to_owned()],
            distinguishing_terms: vec!["gateway".to_owned()],
        };
        let report = WitnessReport {
            terms: vec![TermWitness {
                term: "gateway".to_owned(),
                defined: Vec::new(),
                mentions: 3,
                mention_paths: vec!["src/net/gateway.rs".to_owned()],
                relations: RelationWitness::default(),
                history_documents: Vec::new(),
            }],
            binding_artifacts: Vec::new(),
            scope_gaps: vec!["gateway".to_owned()],
        };
        let drills = build_drill_downs(&frame, &report);
        let queries: Vec<&str> = drills.iter().map(|drill| drill.query.as_str()).collect();
        assert!(queries.contains(&"type:commit gateway"), "{queries:?}");
        assert!(queries.contains(&"type:diff gateway"), "{queries:?}");
        // Mention-only + scope gap both point at the directory that
        // mentions the term — deduped to one suggestion.
        assert!(
            queries.contains(&"path:src/net gateway"),
            "mention path becomes a path-constrained query: {queries:?}"
        );

        // Relation claim, defined-but-edgeless subject → textual
        // reference lookup on the subject name.
        let frame = ClaimFrame {
            intent: QueryIntent::Impact,
            predicate: ClaimPredicate::Relation,
            required_witness: WitnessRequirement::Relation,
            subjects: vec!["connect".to_owned()],
            distinguishing_terms: Vec::new(),
        };
        let report = WitnessReport {
            terms: vec![TermWitness {
                term: "connect".to_owned(),
                defined: vec![DefinedWitness {
                    entity_id: "symbol:connect".to_owned(),
                    name: "connect".to_owned(),
                    path: Some("src/ws.rs".to_owned()),
                    kind: EntityKind::Function,
                }],
                mentions: 1,
                mention_paths: Vec::new(),
                relations: RelationWitness::default(),
                history_documents: Vec::new(),
            }],
            binding_artifacts: Vec::new(),
            scope_gaps: Vec::new(),
        };
        let drills = build_drill_downs(&frame, &report);
        assert_eq!(drills.len(), 1);
        assert_eq!(drills[0].query, "connect");

        // File-scope binding → path-narrowed follow-up on the bound
        // file.
        let frame = ClaimFrame {
            intent: QueryIntent::NaturalLanguageBehavior,
            predicate: ClaimPredicate::Implementation,
            required_witness: WitnessRequirement::CodeBinding,
            subjects: Vec::new(),
            distinguishing_terms: vec!["bearer".to_owned(), "validation".to_owned()],
        };
        let report = WitnessReport {
            terms: Vec::new(),
            binding_artifacts: vec![BoundArtifact {
                path: "src/mixed.rs".to_owned(),
                class: DocumentClass::Code,
                scope: BindingScope::File,
                entity: None,
                test: false,
            }],
            scope_gaps: Vec::new(),
        };
        let drills = build_drill_downs(&frame, &report);
        assert_eq!(drills.len(), 1);
        assert_eq!(drills[0].query, "path:src/mixed.rs bearer");
    }

    #[test]
    fn weak_witness_reasons_cover_each_gap_kind() {
        let frame = ClaimFrame {
            intent: QueryIntent::NaturalLanguageBehavior,
            predicate: ClaimPredicate::Implementation,
            required_witness: WitnessRequirement::CodeBinding,
            subjects: vec!["gateway".to_owned()],
            distinguishing_terms: vec!["bearer".to_owned(), "validation".to_owned()],
        };
        // Subject mention-only + binding prose-only + full scope gap.
        let report = WitnessReport {
            terms: vec![TermWitness {
                term: "gateway".to_owned(),
                defined: Vec::new(),
                mentions: 7,
                mention_paths: vec!["src/gateway.rs".to_owned()],
                relations: RelationWitness::default(),
                history_documents: Vec::new(),
            }],
            binding_artifacts: vec![BoundArtifact {
                path: "plans/009.md".to_owned(),
                class: DocumentClass::Prose,
                scope: BindingScope::File,
                entity: None,
                test: false,
            }],
            scope_gaps: vec!["bearer".to_owned(), "validation".to_owned()],
        };
        let reasons = weak_witness_reasons(&frame, &report);
        assert_eq!(reasons.len(), 3, "each gap kind reports once: {reasons:?}");
        assert!(reasons[0].contains("no definition-tier witness"));
        assert!(reasons[1].contains("only in prose"));
        assert!(reasons[2].contains("no returned hit carries"));
        // A file-scope code binding reports the looser tier — proximity
        // in one file is not one coherent entity.
        let report = WitnessReport {
            binding_artifacts: vec![BoundArtifact {
                path: "src/gateway.rs".to_owned(),
                class: DocumentClass::Code,
                scope: BindingScope::File,
                entity: None,
                test: false,
            }],
            scope_gaps: vec!["bearer".to_owned()],
            ..report
        };
        let reasons = weak_witness_reasons(&frame, &report);
        assert_eq!(reasons.len(), 2, "{reasons:?}");
        assert!(reasons[0].contains("no definition-tier witness"));
        assert!(reasons[1].contains("only at file scope"));
        // An entity-scope binding silences the binding reason entirely.
        let report = WitnessReport {
            binding_artifacts: vec![BoundArtifact {
                path: "src/gateway.rs".to_owned(),
                class: DocumentClass::Code,
                scope: BindingScope::Entity,
                entity: Some("validate".to_owned()),
                test: false,
            }],
            ..report
        };
        let reasons = weak_witness_reasons(&frame, &report);
        assert_eq!(
            reasons.len(),
            1,
            "only the subject gap remains: {reasons:?}"
        );
        // A test-only entity binding reports its own tier — assertions
        // name vocabulary, they do not implement it.
        let report = WitnessReport {
            binding_artifacts: vec![BoundArtifact {
                path: "src/gateway.rs".to_owned(),
                class: DocumentClass::Code,
                scope: BindingScope::Entity,
                entity: Some("tests".to_owned()),
                test: true,
            }],
            ..report
        };
        let reasons = weak_witness_reasons(&frame, &report);
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("only in test code")),
            "test-only binding flags its tier: {reasons:?}"
        );
    }

    #[test]
    fn add_candidate_strict_survives_merging() {
        // A weak-first admission must not lock a document into weakness:
        // once any strict pass claims it the flag flips, and later weak
        // contributions must not dilute it back — the gate reads "at
        // least one strict route ever admitted this document".
        let mut candidates = HashMap::new();
        add_candidate(
            &mut candidates,
            candidate("doc", "src/a.rs", 0.5).hit,
            0.5,
            false,
        );
        assert!(!candidates["doc"].strict);
        add_candidate(
            &mut candidates,
            candidate("doc", "src/a.rs", 0.3).hit,
            0.3,
            true,
        );
        assert!(candidates["doc"].strict);
        add_candidate(
            &mut candidates,
            candidate("doc", "src/a.rs", 0.1).hit,
            0.1,
            false,
        );
        assert!(
            candidates["doc"].strict,
            "weak contributions merge into a strict document without diluting it"
        );
    }

    #[test]
    fn root_package_claims_only_unowned_files() {
        let packages = vec![
            ("crates/alpha".to_owned(), "alpha".to_owned()),
            (String::new(), "root".to_owned()),
        ];
        // Longest-prefix ownership: a file under a member root belongs to
        // the member, and the root package claims only leftovers.
        assert_eq!(package_of("docs/a.md", &packages), Some("root"));
        assert_eq!(
            package_of("crates/alpha/src/lib.rs", &packages),
            Some("alpha")
        );
        assert_eq!(
            package_of("crates/alpha2/src/lib.rs", &packages),
            Some("root")
        );

        let records = SnapshotRecords {
            entities: vec![package("alpha", "crates/alpha"), package("root", "")],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        // The top-3 are all owned by the root package, so the task package
        // set is {root}; a member-package file must not inherit the bonus
        // through the root's match-everything prefix.
        let mut candidates = HashMap::new();
        candidates.insert("seed".to_owned(), candidate("seed", "README.md", 0.5));
        candidates.insert("aroot".to_owned(), candidate("aroot", "docs/a.md", 0.1));
        candidates.insert("broot".to_owned(), candidate("broot", "docs/b.md", 0.1));
        candidates.insert(
            "member".to_owned(),
            candidate("member", "crates/alpha/src/lib.rs", 0.1),
        );
        apply_structural_features(
            &store,
            &snapshot_id,
            QueryIntent::NaturalLanguageBehavior,
            0,
            "query",
            &mut candidates,
        )
        .expect("structural features");

        assert!(
            !candidates["member"]
                .hit
                .explanation
                .iter()
                .any(|line| line.contains("same package")),
            "member file must not be claimed by the root package"
        );
        assert!(
            candidates["seed"]
                .hit
                .explanation
                .iter()
                .any(|line| line.contains("same package"))
        );
    }

    #[test]
    fn entity_tokens_gate_drops_plain_words_in_homogeneous_query() {
        // Multi-token natural-language queries keep only identifier-shaped
        // tokens; plain lowercase words are lexical evidence, not identity.
        assert!(entity_tokens("where is the merged from handle").is_empty());
        assert_eq!(
            entity_tokens("where is SnapshotIdentity defined"),
            ["SnapshotIdentity"]
        );
    }

    #[test]
    fn entity_tokens_code_switched_query_keeps_latin_anchors() {
        // In a CJK query a Latin word is a deliberate technical anchor, and
        // CJK spellings are valid identifier territory — the shape gate is
        // off for the whole query.
        let tokens = entity_tokens("search 偶发返回陈旧结果，哪里把过期视图标记为 stale");
        assert!(tokens.contains(&"search".to_owned()));
        assert!(tokens.contains(&"stale".to_owned()));
        assert!(tokens.contains(&"偶发返回陈旧结果".to_owned()));
    }

    #[test]
    fn entity_tokens_cjk_stopwords_still_filtered() {
        // Code-switching does not resurrect interrogative filler.
        let tokens = entity_tokens("SnapshotIdentity 在哪定义");
        assert_eq!(tokens, ["SnapshotIdentity"]);
    }

    #[test]
    fn entity_tokens_strips_sentence_capital_but_keeps_interior_names() {
        // "Trace" leads the query — orthographic capitalization, not a
        // named thing. The wrongly-abstained `self-trace-context-pack`
        // family came from exactly this leak.
        assert!(entity_tokens("Trace how search hits are packed").is_empty());
        // A second capital marks real naming intent even at the lead.
        assert_eq!(
            entity_tokens("SourceAddress stops carrying snapshot identity"),
            ["SourceAddress"]
        );
        // Interior capitals are chosen, not orthographic — keep them.
        assert_eq!(entity_tokens("how does Tokio spawn tasks"), ["Tokio"]);
        // Identifier characters rescue a leading plain-capital too.
        assert_eq!(
            entity_tokens("Trace_context packing hits"),
            ["Trace_context"]
        );
    }

    fn prf_seed(symbol: &str, snippet: &str, path: &str, score: f64) -> Candidate {
        Candidate {
            hit: SearchHit {
                document_id: format!("doc:{symbol}"),
                entity_id: format!("entity:{symbol}"),
                region_id: None,
                symbol_name: Some(symbol.to_owned()),
                representation: RetrievalRepresentation::RawCode,
                route: SearchRoute::Lexical,
                rank: 1,
                score,
                contributing_routes: vec![SearchRoute::Lexical],
                address: Some(SourceAddress {
                    repository_id: "repo".to_owned(),
                    snapshot_id: "snap".to_owned(),
                    path: path.to_owned(),
                    start_byte: 0,
                    end_byte: 1,
                    start_line: 1,
                    end_line: 1,
                    symbol_id: None,
                }),
                evidence: Vec::new(),
                snippet: snippet.to_owned(),
                verified_current: true,
                explanation: Vec::new(),
            },
            fused_score: score,
            strict: true,
        }
    }

    #[test]
    fn prf_terms_identifier_split_and_drop_query_terms() {
        // camelCase, snake_case, and path components all break into word
        // terms; query terms and boilerplate never become expansion terms.
        let seeds = [prf_seed(
            "resumeSessionState",
            "pub fn resume_session_state(cursor) { restore(cursor) }",
            "src/session.rs",
            1.0,
        )];
        let terms = prf_expansion_terms("cursor restore", &seeds.iter().collect::<Vec<_>>());
        assert!(terms.contains(&"session".to_owned()));
        assert!(terms.contains(&"state".to_owned()));
        assert!(terms.contains(&"resume".to_owned()));
        assert!(!terms.contains(&"cursor".to_owned()));
        assert!(!terms.contains(&"restore".to_owned()));
        assert!(!terms.contains(&"pub".to_owned()));
        assert!(!terms.contains(&"src".to_owned()));
        // "rs" is below the length floor.
        assert!(!terms.contains(&"rs".to_owned()));
    }

    #[test]
    fn prf_terms_ranked_by_document_frequency() {
        // A term in two head documents outranks a term in one, however
        // distinctive the singleton looks.
        let seeds = [
            prf_seed(
                "snapshot_freshness",
                "freshness is computed per snapshot",
                "src/snapshot.rs",
                1.0,
            ),
            prf_seed(
                "snapshot_anchor",
                "every snapshot carries an anchor",
                "src/anchor.rs",
                0.9,
            ),
            prf_seed("quixotic", "quixotic marker", "src/quixotic.rs", 0.8),
        ];
        let terms = prf_expansion_terms("where is decided", &seeds.iter().collect::<Vec<_>>());
        assert_eq!(terms.first(), Some(&"snapshot".to_owned()));
    }

    #[test]
    fn prf_terms_cap_and_deterministic_order() {
        // More candidates than the term budget: the cap keeps the most
        // frequent, breaking ties alphabetically for determinism.
        let seeds = [prf_seed(
            "vocabulary",
            "zebra yarrow xenon walnut violet umber topaz silver quartz",
            "src/vocabulary.rs",
            1.0,
        )];
        let terms = prf_expansion_terms("query", &seeds.iter().collect::<Vec<_>>());
        assert_eq!(terms.len(), PRF_EXPANSION_TERMS);
        let mut sorted = terms.clone();
        sorted.sort();
        assert_eq!(terms, sorted);
    }

    #[test]
    fn prf_terms_empty_without_seeds() {
        assert!(prf_expansion_terms("anything", &[]).is_empty());
    }

    #[test]
    fn cjk_glossary_english_query_expands_nothing() {
        assert!(cjk_glossary_terms("where is snapshot freshness decided").is_empty());
        // CJK text without covered vocabulary expands nothing either.
        assert!(cjk_glossary_terms("今天天气怎么样呢").is_empty());
    }

    #[test]
    fn cjk_glossary_pure_cjk_query_gets_english_anchors() {
        // The benchmark's worst case: every domain term translates.
        let terms = cjk_glossary_terms("为什么函数调用关系的置信度低于导入关系");
        for anchor in ["confidence", "call", "import", "relation", "function"] {
            assert!(
                terms.contains(&anchor.to_owned()),
                "missing anchor {anchor} in {terms:?}"
            );
        }
    }

    #[test]
    fn cjk_glossary_mixed_query_dedups_translations() {
        // 陈旧 and 过期 both translate to "stale"; the anchor is emitted
        // once, and the query's own Latin anchor stays in the query text
        // (the glossary only appends translations, never rewrites).
        let terms = cjk_glossary_terms("search 偶发返回陈旧过期结果 stale");
        assert_eq!(terms.iter().filter(|term| *term == "stale").count(), 1);
        assert!(terms.contains(&"stale".to_owned()));
    }

    #[test]
    fn cjk_glossary_terms_capped_in_table_order() {
        // Emission order follows the sorted table, not the query's term
        // order, and the output is capped at CJK_GLOSSARY_LIMIT anchors.
        let query = "预算 锁定 过期 迁移 边界 路由 路径 调用 证据 解析 视图 行 范围 能力 \
                     置信度 缓存 组件 索引 类型 符号 目录 清单 测试 模式 模块 检索 标记 \
                     架构 构建 权限 服务 文档 文件 提交 接口 排序 扫描 忽略 快照 引用 差异 \
                     嵌入 属性 导入 实现 实体 守护 向量 合并 历史 包 函数 关系 修复 依赖 \
                     令牌 仓库 上下文";
        let terms = cjk_glossary_terms(query);
        assert_eq!(terms.len(), CJK_GLOSSARY_LIMIT);
        // 上下文 is the first sorted key present in the query.
        assert_eq!(terms.first(), Some(&"context".to_owned()));
        let mut unique = terms.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), terms.len(), "anchors must be deduplicated");
    }

    // `body_artifact_digest` has a foreign key into `artifacts`, so the
    // body is really stored and its record registered with the snapshot.
    fn indexed_document(
        store: &MetadataStore,
        document_id: &str,
        entity_id: &str,
        path: &str,
        name: &str,
        body: &str,
    ) -> (cce_store::IndexedDocument, cce_store::ArtifactRecord) {
        let artifact = store
            .artifacts()
            .put_bytes(cce_store::ArtifactKind::Source, body.as_bytes())
            .expect("store document body");
        (
            cce_store::IndexedDocument {
                document: cce_core::RetrievalDocument {
                    id: document_id.to_owned(),
                    entity_id: entity_id.to_owned(),
                    snapshot_id: "snap_test".to_owned(),
                    representation: RetrievalRepresentation::RawCode,
                    body_artifact_digest: artifact.digest.clone(),
                    region_id: None,
                    address: Some(
                        SourceAddress::new("repo_test", "snap_test", path, 0..1, 1..=1)
                            .expect("address"),
                    ),
                    embedding_profile: None,
                    generated_by: None,
                    evidence: Vec::new(),
                    terms: Vec::new(),
                },
                path: path.to_owned(),
                name: name.to_owned(),
                body: body.to_owned(),
            },
            artifact,
        )
    }

    fn engine_at(directory: &tempfile::TempDir) -> CceEngine {
        let mut config = crate::EngineConfig::for_repository(directory.path());
        config.data_root = directory.path().join("data");
        CceEngine::open(config).expect("engine")
    }

    fn request(query: &str, limit: usize) -> SearchRequest {
        SearchRequest {
            repository_id: String::new(),
            snapshot_id: String::new(),
            query: query.to_owned(),
            intent: None,
            limit,
            require_fresh: false,
            routes: vec![SearchRoute::Lexical],
            filters: cce_core::QueryFilters::default(),
        }
    }

    #[tokio::test]
    async fn search_expands_pure_cjk_query_through_glossary() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        let anchor = RepositoryScanner::new(engine.config().clone())
            .identify()
            .expect("anchor");
        engine
            .store()
            .register_repository(&anchor.identity)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snap_glossary".to_owned(),
            repository_id: anchor.identity.id.clone(),
            base_revision: None,
            workspace_overlay_hash: String::new(),
            index_profile_hash: String::new(),
            created_at: chrono::Utc::now(),
            file_count: 1,
            source_bytes: 0,
        };
        engine
            .store()
            .begin_snapshot(&snapshot)
            .expect("begin snapshot");
        let (relations_doc, relations_artifact) = indexed_document(
            engine.store(),
            "doc:relations",
            "file:src/relations.rs",
            "src/relations.rs",
            "relations",
            "call relation confidence is lower than import relation confidence",
        );
        let (unrelated_doc, unrelated_artifact) = indexed_document(
            engine.store(),
            "doc:unrelated",
            "file:src/unrelated.rs",
            "src/unrelated.rs",
            "unrelated",
            "banana hammock yogurt carousel",
        );
        let records = SnapshotRecords {
            artifacts: vec![relations_artifact, unrelated_artifact],
            entities: vec![file("src/relations.rs"), file("src/unrelated.rs")],
            documents: vec![relations_doc, unrelated_doc],
            ..SnapshotRecords::default()
        };
        engine
            .store()
            .commit_snapshot(&snapshot, &records)
            .expect("commit records");

        // Without the glossary the CJK monolith cannot reach English
        // source at all — the documented pure-CJK boundary.
        let raw = engine
            .store()
            .lexical_search(
                &snapshot.id,
                "为什么函数调用关系的置信度低于导入关系",
                10,
                &cce_core::QueryFilters::default(),
            )
            .expect("raw lexical");
        assert!(raw.is_empty(), "CJK monolith must not match: {raw:?}");

        let result = engine
            .search(request("为什么函数调用关系的置信度低于导入关系", 10))
            .await
            .expect("search");
        let paths = result
            .hits
            .iter()
            .filter_map(|hit| hit.address.as_ref().map(|address| address.path.as_str()))
            .collect::<Vec<_>>();
        assert!(
            paths.contains(&"src/relations.rs"),
            "glossary expansion must surface the gold file: {paths:?}"
        );
        assert!(
            result
                .hits
                .iter()
                .flat_map(|hit| hit.explanation.iter())
                .any(|line| line.contains("CJK glossary: +")),
            "hits must record the applied glossary anchors"
        );
        assert!(
            !paths.contains(&"src/unrelated.rs"),
            "unrelated vocabulary must stay unfound: {paths:?}"
        );
    }

    #[tokio::test]
    async fn search_abstains_on_provably_absent_literal() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        let anchor = RepositoryScanner::new(engine.config().clone())
            .identify()
            .expect("anchor");
        engine
            .store()
            .register_repository(&anchor.identity)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snap_veto".to_owned(),
            repository_id: anchor.identity.id.clone(),
            base_revision: None,
            workspace_overlay_hash: String::new(),
            index_profile_hash: String::new(),
            created_at: chrono::Utc::now(),
            file_count: 1,
            source_bytes: 0,
        };
        engine
            .store()
            .begin_snapshot(&snapshot)
            .expect("begin snapshot");
        let (view_doc, view_artifact) = indexed_document(
            engine.store(),
            "doc:viewstatus",
            "file:src/view_status.rs",
            "src/view_status.rs",
            "ViewStatus",
            "pub struct ViewStatus { stale: bool } // where staleness is defined",
        );
        let records = SnapshotRecords {
            artifacts: vec![view_artifact],
            entities: vec![file("src/view_status.rs")],
            documents: vec![view_doc],
            ..SnapshotRecords::default()
        };
        engine
            .store()
            .commit_snapshot(&snapshot, &records)
            .expect("commit records");

        // The perturbation trap families — pluralized, swapped,
        // doubled, and truncated spellings of a real entity — are all
        // provably absent and must abstain rather than answer with the
        // near-miss's neighbor.
        for trap in [
            swapped_last("ViewStatus"),
            pluralized("ViewStatus"),
            truncated("ViewStatus"),
            pluralized("search"),
        ] {
            let result = engine
                .search(request(&format!("Where is `{trap}` defined?"), 10))
                .await
                .expect("search");
            assert!(
                result.hits.is_empty(),
                "`{trap}` is provably absent — hits must be empty: {:?}",
                result.hits
            );
            assert!(
                result
                    .missing_capabilities
                    .iter()
                    .any(|note| note.contains("abstained")),
                "`{trap}` abstention must be explained in missing capabilities"
            );
        }

        // Control: the real entity answers normally — the veto must not
        // over-fire on a spelling that IS in the corpus, and the verdict
        // reports a clean answer.
        let result = engine
            .search(request("Where is `ViewStatus` defined?", 10))
            .await
            .expect("search");
        assert!(
            !result.hits.is_empty(),
            "the real entity must still answer: {:?}",
            result.missing_capabilities
        );
        assert_eq!(result.verdict.state, VerdictState::Answered);
        assert!(
            result.verdict.evidence_tiers.strict >= 1,
            "the lexical hit is strict evidence: {:?}",
            result.verdict
        );
        assert_eq!(result.verdict.claim.subjects, ["ViewStatus"]);
        let subject = result
            .verdict
            .witness
            .terms
            .iter()
            .find(|witness| witness.term == "ViewStatus")
            .expect("subject witness");
        assert!(
            !subject.defined.is_empty(),
            "`src/view_status.rs` is a definition-tier witness: {subject:?}"
        );
    }

    /// A fixture with a real implementation file plus a prose document
    /// that *mentions* claim vocabulary — the adversarial no-context
    /// shape the verdict exists to describe.
    fn verdict_fixture(
        engine: &CceEngine,
        snapshot_id: &str,
        documents: &[(&str, &str, &str, &str)],
    ) {
        let documents: Vec<(&str, &str, &str, &str, RetrievalRepresentation)> = documents
            .iter()
            .map(|(id, path, name, body)| {
                (*id, *path, *name, *body, RetrievalRepresentation::RawCode)
            })
            .collect();
        verdict_fixture_full(engine, snapshot_id, &documents, &[], &[]);
    }

    /// The verdict fixture with extra graph/history material: documents
    /// carry an explicit representation (commit-class docs seed the
    /// history surface), entities and relations seed the typed graph.
    fn verdict_fixture_full(
        engine: &CceEngine,
        snapshot_id: &str,
        documents: &[(&str, &str, &str, &str, RetrievalRepresentation)],
        entities: &[CodeEntity],
        relations: &[cce_core::Relation],
    ) {
        let anchor = RepositoryScanner::new(engine.config().clone())
            .identify()
            .expect("anchor");
        engine
            .store()
            .register_repository(&anchor.identity)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: snapshot_id.to_owned(),
            repository_id: anchor.identity.id,
            base_revision: None,
            workspace_overlay_hash: String::new(),
            index_profile_hash: String::new(),
            created_at: chrono::Utc::now(),
            file_count: documents.len() as u64,
            source_bytes: 0,
        };
        engine
            .store()
            .begin_snapshot(&snapshot)
            .expect("begin snapshot");
        let mut records = SnapshotRecords::default();
        for (document_id, path, name, body, representation) in documents {
            let (mut document, artifact) = indexed_document(
                engine.store(),
                document_id,
                &format!("file:{path}"),
                path,
                name,
                body,
            );
            document.document.representation = representation.clone();
            records.artifacts.push(artifact);
            records.entities.push(file(path));
            records.documents.push(document);
        }
        records.entities.extend(entities.iter().cloned());
        records.relations.extend(relations.iter().cloned());
        engine
            .store()
            .commit_snapshot(&snapshot, &records)
            .expect("commit records");
    }

    /// Documents bound to an explicit entity — symbol-level docs seed
    /// entity-scope bindings, file-level docs seed file scope.
    fn binding_fixture(
        engine: &CceEngine,
        snapshot_id: &str,
        documents: &[(&str, &str, &str, &str, &str)],
        entities: &[CodeEntity],
    ) {
        let anchor = RepositoryScanner::new(engine.config().clone())
            .identify()
            .expect("anchor");
        engine
            .store()
            .register_repository(&anchor.identity)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: snapshot_id.to_owned(),
            repository_id: anchor.identity.id,
            base_revision: None,
            workspace_overlay_hash: String::new(),
            index_profile_hash: String::new(),
            created_at: chrono::Utc::now(),
            file_count: documents.len() as u64,
            source_bytes: 0,
        };
        engine
            .store()
            .begin_snapshot(&snapshot)
            .expect("begin snapshot");
        let mut records = SnapshotRecords::default();
        for (document_id, entity_id, path, name, body) in documents {
            let (document, artifact) =
                indexed_document(engine.store(), document_id, entity_id, path, name, body);
            records.artifacts.push(artifact);
            records.documents.push(document);
        }
        records.entities.extend(entities.iter().cloned());
        engine
            .store()
            .commit_snapshot(&snapshot, &records)
            .expect("commit records");
    }

    fn edge(
        source: &str,
        target: &str,
        kind: RelationKind,
        origin: RelationOrigin,
    ) -> cce_core::Relation {
        cce_core::Relation {
            id: format!("rel:{source}:{target}"),
            source_entity_id: source.to_owned(),
            target_entity_id: target.to_owned(),
            kind,
            origin,
            confidence: 1.0,
            snapshot_id: "snap_test".to_owned(),
            extractor: "test".to_owned(),
            evidence: Vec::new(),
            attributes: serde_json::Map::new(),
        }
    }

    #[tokio::test]
    async fn search_abstains_when_claim_subject_never_defined() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        verdict_fixture(
            &engine,
            "snap_witness",
            &[(
                "doc:rfc",
                "docs/rfc.md",
                "rfc",
                "the websocket design was considered and rejected",
            )],
        );

        // `WebSocket` is *mentioned* in prose (df > 0, so the literal
        // veto passes) but never defined — no entity and no file bears
        // the name. For a definition claim that is a provable negative.
        let mut lookup = request("Where is `WebSocket` defined?", 10);
        lookup.intent = Some(QueryIntent::ExactEntity);
        let result = engine.search(lookup).await.expect("search");
        assert!(
            result.hits.is_empty(),
            "no definition exists: {:?}",
            result.hits
        );
        assert_eq!(result.verdict.state, VerdictState::Abstained);
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("never defined")),
            "abstention names the witness failure: {:?}",
            result.verdict.reasons
        );
        let witness = result
            .verdict
            .witness
            .terms
            .iter()
            .find(|witness| witness.term == "WebSocket")
            .expect("subject witness");
        assert!(witness.defined.is_empty());
        assert_eq!(witness.mentions, 1, "the mention is still reported");
        assert_eq!(witness.mention_paths, ["docs/rfc.md"]);

        // A file bearing the name IS a definition — `docs/websocket.md`
        // in the corpus turns the abstain into a weak-witness answer.
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        verdict_fixture(
            &engine,
            "snap_witness2",
            &[(
                "doc:ws",
                "docs/websocket.md",
                "ws",
                "the websocket approach defined in this sketch was rejected",
            )],
        );
        let mut lookup = request("Where is `WebSocket` defined?", 10);
        lookup.intent = Some(QueryIntent::ExactEntity);
        let result = engine.search(lookup).await.expect("search");
        assert_ne!(
            result.verdict.state,
            VerdictState::Abstained,
            "a file named after the subject is a definition: {:?}",
            result.verdict
        );
    }

    #[tokio::test]
    async fn search_flags_weak_witness_on_mention_only_subject() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        verdict_fixture(
            &engine,
            "snap_weak",
            &[
                (
                    "doc:rfc",
                    "docs/rfc.md",
                    "rfc",
                    "the websocket reconnection design sketch",
                ),
                (
                    "doc:gw",
                    "src/gateway.rs",
                    "gateway",
                    "the gateway validates every request token",
                ),
            ],
        );

        // A behavior claim whose backticked subject is mention-only:
        // lexical evidence exists (the prose doc matches), but the
        // subject has no definition-tier witness — answered hits with a
        // weak-witness flag, not a silent pass.
        let mut query = request("how does `WebSocket` handle reconnection", 10);
        query.intent = Some(QueryIntent::NaturalLanguageBehavior);
        let result = engine.search(query).await.expect("search");
        assert!(!result.hits.is_empty(), "the prose doc still surfaces");
        assert_eq!(result.verdict.state, VerdictState::WeakWitness);
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("no definition-tier witness")),
            "the witness gap is named: {:?}",
            result.verdict.reasons
        );
    }

    #[tokio::test]
    async fn search_flags_weak_witness_when_terms_bind_nowhere() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        verdict_fixture(
            &engine,
            "snap_bind",
            &[
                (
                    "doc:impl",
                    "src/lock_manager.rs",
                    "locks",
                    "the coordinator locking mechanism acquires",
                ),
                (
                    "doc:plan",
                    "plans/009.md",
                    "plan",
                    "a distributed locking design sketch",
                ),
            ],
        );

        // `distributed` and `coordinator` never co-bind with `locking`
        // in one artifact — each document carries part of the claim, no
        // document carries it whole. Hits exist (pair-floor matches),
        // but the witness report must say no artifact binds the claim.
        let mut query = request("how does the coordinator handle distributed locking", 10);
        query.intent = Some(QueryIntent::NaturalLanguageBehavior);
        let result = engine.search(query).await.expect("search");
        assert!(
            !result.hits.is_empty(),
            "partial coverage still surfaces: {:?}",
            result.missing_capabilities
        );
        assert_eq!(result.verdict.state, VerdictState::WeakWitness);
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("bind")),
            "the binding gap is named: {:?}",
            result.verdict.reasons
        );
        assert!(
            result
                .verdict
                .witness
                .binding_artifacts
                .iter()
                .all(|artifact| artifact.class == DocumentClass::Prose),
            "no code artifact binds the claim: {:?}",
            result.verdict.witness.binding_artifacts
        );
    }

    /// The relation fixture: `helper` calls `beta_fn` (tree-sitter
    /// fact), `inferred_fn` is the target of a model-inferred call, and
    /// `orphan_fn` is defined but participates in nothing.
    fn relation_fixture(engine: &CceEngine) {
        verdict_fixture_full(
            engine,
            "snap_rel",
            &[(
                "doc:lib",
                "src/lib.rs",
                "lib",
                "fn helper() { beta_fn() } fn orphan_fn() {} fn inferred_fn() {}",
                RetrievalRepresentation::RawCode,
            )],
            &[
                symbol("helper", "src/lib.rs"),
                symbol("beta_fn", "src/lib.rs"),
                symbol("orphan_fn", "src/lib.rs"),
                symbol("inferred_fn", "src/lib.rs"),
            ],
            &[
                edge(
                    "symbol:helper",
                    "symbol:beta_fn",
                    RelationKind::Calls,
                    RelationOrigin::TreeSitter,
                ),
                edge(
                    "symbol:helper",
                    "symbol:inferred_fn",
                    RelationKind::Calls,
                    RelationOrigin::ModelInference,
                ),
            ],
        );
    }

    #[tokio::test]
    async fn search_abstains_when_relation_subject_has_no_edges() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        relation_fixture(&engine);

        // `orphan_fn` is defined and corpus-present (df > 0), but zero
        // edges touch it: for a relation claim that is a provable
        // negative — the graph records nothing about the subject.
        let mut query = request("what breaks if `orphan_fn` changes", 10);
        query.intent = Some(QueryIntent::Impact);
        let result = engine.search(query).await.expect("search");
        assert!(result.hits.is_empty());
        assert_eq!(result.verdict.state, VerdictState::Abstained);
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("no typed relations")),
            "abstention names the relation witness failure: {:?}",
            result.verdict.reasons
        );
    }

    #[tokio::test]
    async fn relation_witness_reports_edge_kinds_and_origins() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        relation_fixture(&engine);

        // `helper` has a deterministic Calls edge: the gate must let the
        // search run, and the witness report must carry the edge's kind
        // and tree-sitter provenance.
        let mut query = request("what breaks if `helper` changes", 10);
        query.intent = Some(QueryIntent::Impact);
        let result = engine.search(query).await.expect("search");
        assert!(
            !result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("no typed relations")),
            "an edgeful subject must not hit the relation gate: {:?}",
            result.verdict.reasons
        );
        let witness = result
            .verdict
            .witness
            .terms
            .iter()
            .find(|term| term.term == "helper")
            .expect("helper witness");
        assert!(witness.relations.edges > 0);
        assert!(witness.relations.kinds.contains(&RelationKind::Calls));
        assert!(
            witness
                .relations
                .origins
                .contains(&RelationOrigin::TreeSitter)
        );
        assert_eq!(
            witness.relations.provenance.syntactic, 1,
            "the tree-sitter edge counts in the syntactic tier"
        );
        assert_eq!(
            witness.relations.provenance.inferred, 1,
            "the model-inferred edge counts separately"
        );
        assert_eq!(witness.relations.provenance.precise, 0);
    }

    #[tokio::test]
    async fn inference_only_relation_evidence_is_flagged_weak() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        relation_fixture(&engine);

        // `inferred_fn` has edges — the gate passes — but every one is
        // model-inferred. Inference is not source truth, so the verdict
        // must say so rather than letting the structure stand as fact.
        let mut query = request("`inferred_fn`", 10);
        query.intent = Some(QueryIntent::Impact);
        let result = engine.search(query).await.expect("search");
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("model-inferred only")),
            "inference-only structure is flagged, not presented as fact: {:?}",
            result.verdict.reasons
        );
    }

    #[tokio::test]
    async fn search_abstains_when_history_subject_absent_from_commits() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        verdict_fixture_full(
            &engine,
            "snap_hist",
            &[
                (
                    "doc:commit1",
                    "commit:aaa111",
                    "commit",
                    "added grpc transport for the control plane",
                    RetrievalRepresentation::CommitSummary,
                ),
                (
                    "doc:patch",
                    "commit:bbb222",
                    "commit",
                    "src/net.rs +grpc client channel",
                    RetrievalRepresentation::CommitDiff,
                ),
                (
                    "doc:net",
                    "src/net.rs",
                    "net",
                    "fn dial() { /* grpc endpoint on the tokio runtime */ }",
                    RetrievalRepresentation::RawCode,
                ),
            ],
            &[],
            &[],
        );

        // `tokio` lives only in current source — recorded history never
        // mentions it. For a history claim that is a provable negative.
        let mut query = request("when was `tokio` added", 10);
        query.intent = Some(QueryIntent::History);
        let result = engine.search(query).await.expect("search");
        assert!(result.hits.is_empty());
        assert_eq!(result.verdict.state, VerdictState::Abstained);
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("commit-class document")),
            "abstention names the history witness failure: {:?}",
            result.verdict.reasons
        );

        // `grpc` is attested in commit-class documents: the gate must
        // not fire, and the witness report carries the document ids.
        let mut query = request("when was `grpc` added", 10);
        query.intent = Some(QueryIntent::History);
        let result = engine.search(query).await.expect("search");
        assert!(
            !result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("commit-class document")),
            "a history-attested subject must not hit the history gate: {:?}",
            result.verdict.reasons
        );
        let witness = result
            .verdict
            .witness
            .terms
            .iter()
            .find(|term| term.term == "grpc")
            .expect("grpc witness");
        assert_eq!(witness.history_documents.len(), 2);
    }

    #[test]
    fn weak_witness_reasons_flag_relation_and_history_gaps() {
        let term = |relations: RelationWitness, history: Vec<String>| TermWitness {
            term: "subject".to_owned(),
            defined: vec![DefinedWitness {
                entity_id: "symbol:subject".to_owned(),
                name: "subject".to_owned(),
                kind: EntityKind::Function,
                path: Some("src/lib.rs".to_owned()),
            }],
            mentions: 3,
            mention_paths: vec!["src/lib.rs".to_owned()],
            relations,
            history_documents: history,
        };
        let report = |terms: Vec<TermWitness>| WitnessReport {
            terms,
            binding_artifacts: Vec::new(),
            scope_gaps: Vec::new(),
        };
        let mut frame = claim_frame(QueryIntent::Impact, vec!["subject".to_owned()]);

        // Defined but edgeless: the relation claim rests on non-graph
        // evidence.
        let reasons = weak_witness_reasons(
            &frame,
            &report(vec![term(RelationWitness::default(), Vec::new())]),
        );
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("no typed relations")),
            "{reasons:?}"
        );

        // Inference-only edges: structure exists but nothing
        // deterministic backs it.
        let inferred = RelationWitness {
            edges: 2,
            kinds: vec![RelationKind::Calls],
            origins: vec![RelationOrigin::ModelInference],
            provenance: RelationProvenance {
                inferred: 2,
                ..RelationProvenance::default()
            },
        };
        let reasons = weak_witness_reasons(&frame, &report(vec![term(inferred, Vec::new())]));
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("model-inferred only")),
            "{reasons:?}"
        );

        // History claim with no commit-class witness: answered from
        // current source only.
        frame = claim_frame(QueryIntent::History, vec!["subject".to_owned()]);
        let reasons = weak_witness_reasons(
            &frame,
            &report(vec![term(RelationWitness::default(), Vec::new())]),
        );
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("no commit-class document")),
            "{reasons:?}"
        );
    }

    #[tokio::test]
    async fn search_english_query_and_pinned_routes_skip_glossary() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        let anchor = RepositoryScanner::new(engine.config().clone())
            .identify()
            .expect("anchor");
        engine
            .store()
            .register_repository(&anchor.identity)
            .expect("register repository");
        let snapshot = SnapshotIdentity {
            id: "snap_glossary2".to_owned(),
            repository_id: anchor.identity.id.clone(),
            base_revision: None,
            workspace_overlay_hash: String::new(),
            index_profile_hash: String::new(),
            created_at: chrono::Utc::now(),
            file_count: 1,
            source_bytes: 0,
        };
        engine
            .store()
            .begin_snapshot(&snapshot)
            .expect("begin snapshot");
        let (relations_doc, relations_artifact) = indexed_document(
            engine.store(),
            "doc:relations",
            "file:src/relations.rs",
            "src/relations.rs",
            "relations",
            "call relation confidence is lower than import relation confidence",
        );
        let records = SnapshotRecords {
            artifacts: vec![relations_artifact],
            entities: vec![file("src/relations.rs")],
            documents: vec![relations_doc],
            ..SnapshotRecords::default()
        };
        engine
            .store()
            .commit_snapshot(&snapshot, &records)
            .expect("commit records");

        // English query: no glossary, but the same document still ranks.
        let english = engine
            .search(request(
                "why is call confidence lower than import confidence",
                10,
            ))
            .await
            .expect("english search");
        assert!(!english.hits.is_empty());
        assert!(
            !english
                .hits
                .iter()
                .flat_map(|hit| hit.explanation.iter())
                .any(|line| line.contains("CJK glossary")),
            "english query must not record glossary anchors"
        );

        // `type:commit` pins the plan to the history route, which owns no
        // documents here — the glossary never applies off the lexical route.
        let pinned = engine
            .search(request(
                "为什么函数调用关系的置信度低于导入关系 type:commit",
                10,
            ))
            .await
            .expect("pinned search");
        assert!(pinned.hits.is_empty());
    }
    /// Terms inside one symbol's span bind at entity scope; terms split
    /// across two functions of the same file bind only at file scope —
    /// the granularity difference the verdict must report.
    #[tokio::test]
    async fn binding_scope_distinguishes_entity_from_file_cooccurrence() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        binding_fixture(
            &engine,
            "snap_test",
            &[
                (
                    "doc:connect",
                    "symbol:connect",
                    "src/ws.rs",
                    "connect",
                    "fn connect() { reconnect with exponential backoff }",
                ),
                // Two functions in one file: neither symbol doc carries
                // both terms, the file doc does.
                (
                    "doc:check",
                    "symbol:bearer_check",
                    "src/mixed.rs",
                    "bearer_check",
                    "fn bearer_check() { bearer }",
                ),
                (
                    "doc:validate",
                    "symbol:run_validation",
                    "src/mixed.rs",
                    "run_validation",
                    "fn run_validation() { validation }",
                ),
                (
                    "doc:mixed",
                    "file:src/mixed.rs",
                    "src/mixed.rs",
                    "mixed",
                    "fn bearer_check() { bearer } fn run_validation() { validation }",
                ),
            ],
            &[
                file("src/ws.rs"),
                symbol("connect", "src/ws.rs"),
                file("src/mixed.rs"),
                symbol("bearer_check", "src/mixed.rs"),
                symbol("run_validation", "src/mixed.rs"),
            ],
        );

        // Entity scope: `connect` carries both terms in one span.
        let result = engine
            .search(request("reconnect backoff", 10))
            .await
            .expect("entity search");
        let artifact = result
            .verdict
            .witness
            .binding_artifacts
            .iter()
            .find(|artifact| artifact.path == "src/ws.rs")
            .expect("binding artifact");
        assert_eq!(artifact.scope, BindingScope::Entity);
        assert_eq!(artifact.entity.as_deref(), Some("connect"));
        assert!(
            !result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("file scope")),
            "entity-bound terms report no scope weakness: {:?}",
            result.verdict.reasons
        );

        // File scope: the conjunction holds only at file granularity —
        // two unrelated spans share a path, which is proximity, not
        // coherence.
        let result = engine
            .search(request("bearer validation", 10))
            .await
            .expect("file search");
        let artifact = result
            .verdict
            .witness
            .binding_artifacts
            .iter()
            .find(|artifact| artifact.path == "src/mixed.rs")
            .expect("binding artifact");
        assert_eq!(artifact.scope, BindingScope::File);
        assert_eq!(artifact.entity, None);
        assert_eq!(result.verdict.state, VerdictState::WeakWitness);
        assert!(
            result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("only at file scope")),
            "file-scope binding is flagged: {:?}",
            result.verdict.reasons
        );
    }

    /// A binding whose only coherent entity is test code does not
    /// witness — assertions name vocabulary, implementations implement
    /// it. The qualified-name module path (`…::tests::…`) is what marks
    /// a helper whose bare name shows no test marker.
    #[tokio::test]
    async fn binding_in_test_code_corroborates_but_does_not_witness() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        binding_fixture(
            &engine,
            "snap_test",
            &[
                (
                    "doc:helper",
                    "symbol:helper",
                    "src/auth.rs",
                    "helper",
                    "fn helper() { bearer token check }",
                ),
                (
                    "doc:real",
                    "symbol:issue",
                    "src/issue.rs",
                    "issue",
                    "fn issue() { bearer token check }",
                ),
            ],
            &[
                file("src/auth.rs"),
                CodeEntity {
                    qualified_name: Some("src/auth.rs::tests::helper".to_owned()),
                    ..symbol("helper", "src/auth.rs")
                },
                file("src/issue.rs"),
                CodeEntity {
                    qualified_name: Some("src/issue.rs::issue".to_owned()),
                    ..symbol("issue", "src/issue.rs")
                },
            ],
        );

        // Both terms bind inside one entity each — the `tests`-module
        // member marks its artifact test while the plain function's
        // binding stays implementation-tier.
        let result = engine
            .search(request("bearer token", 10))
            .await
            .expect("test binding search");
        let artifact = result
            .verdict
            .witness
            .binding_artifacts
            .iter()
            .find(|artifact| artifact.path == "src/auth.rs")
            .expect("test binding artifact");
        assert_eq!(artifact.scope, BindingScope::Entity);
        assert!(artifact.test, "test-module binding is marked: {artifact:?}");
        let artifact = result
            .verdict
            .witness
            .binding_artifacts
            .iter()
            .find(|artifact| artifact.path == "src/issue.rs")
            .expect("impl binding artifact");
        assert!(
            !artifact.test,
            "implementation binding stays unmarked: {artifact:?}"
        );
        assert!(
            !result
                .verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("only in test code")),
            "a real implementation binding silences the test flag: {:?}",
            result.verdict.reasons
        );
    }

    /// Selection is claim-aware: a document carrying the claim's
    /// distinguishing vocabulary outranks a slightly stronger candidate
    /// that carries none of it.
    #[tokio::test]
    async fn select_hits_prefers_claim_term_carriers() {
        let directory = tempfile::tempdir().expect("tempdir");
        let engine = engine_at(&directory);
        binding_fixture(
            &engine,
            "snap_test",
            &[
                (
                    "doc:carrier",
                    "file:src/carrier.rs",
                    "src/carrier.rs",
                    "carrier",
                    "fn carrier() { bearer validation }",
                ),
                (
                    "doc:plain",
                    "file:src/plain.rs",
                    "src/plain.rs",
                    "plain",
                    "fn plain() { filler words }",
                ),
                (
                    "doc:notes",
                    "file:docs/notes.md",
                    "docs/notes.md",
                    "notes",
                    "bearer validation notes",
                ),
            ],
            &[
                file("src/carrier.rs"),
                file("src/plain.rs"),
                file("docs/notes.md"),
            ],
        );

        let mut candidates = HashMap::new();
        candidates.insert(
            "doc:carrier".to_owned(),
            candidate_at("doc:carrier", "src/carrier.rs", 0, 0.594),
        );
        candidates.insert(
            "doc:plain".to_owned(),
            candidate_at("doc:plain", "src/plain.rs", 0, 0.6),
        );
        candidates.insert(
            "doc:notes".to_owned(),
            candidate_at("doc:notes", "docs/notes.md", 0, 0.6),
        );
        let mut request = request("bearer validation", 5);
        request.snapshot_id = "snap_test".to_owned();
        let frame = ClaimFrame {
            intent: QueryIntent::NaturalLanguageBehavior,
            predicate: ClaimPredicate::Implementation,
            required_witness: WitnessRequirement::CodeBinding,
            subjects: Vec::new(),
            distinguishing_terms: vec!["bearer".to_owned(), "validation".to_owned()],
        };

        let hits = engine
            .select_hits(&request, &candidates, &frame)
            .expect("select hits");
        assert_eq!(
            hits.first().map(|hit| hit.document_id.as_str()),
            Some("doc:carrier"),
            "the claim-term carrier outranks the bare candidate: {:?}",
            hits.iter()
                .map(|hit| (hit.document_id.as_str(), hit.score))
                .collect::<Vec<_>>()
        );
        let carrier = hits.first().expect("carrier hit");
        assert!(
            carrier
                .explanation
                .iter()
                .any(|line| line.contains("claim-term coverage")),
            "coverage is reported in the hit explanation: {:?}",
            carrier.explanation
        );
        assert!(
            carrier.score > 0.594,
            "the coverage bonus lands in the reported score"
        );
        let notes = hits
            .iter()
            .find(|hit| hit.document_id == "doc:notes")
            .expect("prose hit still returned");
        assert!(
            (notes.score - 0.6).abs() < f64::EPSILON,
            "prose corroborates but does not witness — no claim bonus: {:?}",
            notes.explanation
        );
        assert!(
            !notes
                .explanation
                .iter()
                .any(|line| line.contains("claim-term coverage")),
            "no coverage line on prose: {:?}",
            notes.explanation
        );
    }
}
