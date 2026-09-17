use std::{collections::HashMap, time::Instant};

use cce_core::{
    CodeEntity, EntityKind, QueryIntent, Result, RetrievalRepresentation, SearchHit, SearchRequest,
    SearchRoute, ViewKind, ViewManifest, ViewState, has_cjk,
};
use cce_store::{MetadataStore, RelationDirection};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    CceEngine, DenseIndex, Embedder, GraphPolicy, QueryPlan, QueryPlanner, RepositoryScanner,
};

const RRF_K: f64 = 60.0;

/// How many head hits the cross-encoder re-scores. Reranker cost is linear
/// in pairs; beyond ~50 the fused prior is already thin.
const RERANK_POOL: usize = 50;

/// Only topical hits are scored by the reranker: graph-expansion routes
/// (structural/knowledge/history) answer "what is connected", not "what
/// matches the query text" — judging them on topical relevance punishes
/// blast-radius evidence.
const TOPICAL: [SearchRoute; 5] = [
    SearchRoute::Lexical,
    SearchRoute::DenseRaw,
    SearchRoute::DenseSummary,
    SearchRoute::ExactSymbol,
    SearchRoute::Hybrid,
];

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
    /// Engine-side wall time in milliseconds.
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    hit: SearchHit,
    fused_score: f64,
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
                    &request.query,
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
                        explanation: vec!["SQLite FTS5 identifier/path/source match".to_owned()],
                    },
                    1.0 / (RRF_K + rank as f64),
                );
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
                        let index = DenseIndex::decode(&self.store().artifacts().read(digest)?)?;
                        let dense_hits = index
                            .search(&request.query, embedder, request.limit.saturating_mul(3))
                            .await?;
                        let documents = self
                            .store()
                            .documents_for_snapshot(&request.snapshot_id)?
                            .into_iter()
                            .map(|document| (document.document_id.clone(), document))
                            .collect::<HashMap<_, _>>();
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
                            let symbol_name = self
                                .store()
                                .entity_by_id(&request.snapshot_id, &document.entity_id)?
                                .map(|entity| entity.name);
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
                            );
                        }
                    }
                }
            }
        }

        // Pseudo-relevance feedback: the fused head's discriminative terms
        // expand the query for one more lexical pass, before graph
        // expansion picks its seeds so both stages see the enriched head.
        self.prf_expansion_pass(&request, &plan, &mut candidates, verified_fresh)?;

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
            let mut frontier = ranked_candidates(&candidates)
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
        apply_corroboration(
            self.store(),
            &request.snapshot_id,
            &expansion_evidence,
            &mut candidates,
        )?;

        // Multiple retrieval documents can describe one source region (raw
        // chunk + symbol summary); the hit list presents regions, so the
        // first — best-scored — document per region wins and later ones only
        // contribute their routes. Per-file cap keeps cross-file coverage;
        // hits beyond it stay in `candidates` for expansion seeds.
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut per_file = HashMap::<String, usize>::new();
        let mut seen_regions = HashMap::<String, usize>::new();
        let mut language_cache = HashMap::<String, Option<String>>::new();
        for candidate in ranked_candidates(&candidates) {
            let mut hit = candidate.hit.clone();
            if !self.hit_matches_filters(
                &hit,
                &request.filters,
                &request.snapshot_id,
                &mut language_cache,
            )? {
                continue;
            }
            hit.score = candidate.fused_score;
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
                if *count >= 3 {
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
        Ok(SearchResult {
            request,
            plan,
            manifest,
            hits,
            missing_capabilities,
            latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        })
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
        candidates: &mut HashMap<String, Candidate>,
        verified_fresh: bool,
    ) -> Result<()> {
        if plan.intent == QueryIntent::ExactEntity || !plan.routes.contains(&SearchRoute::Lexical) {
            return Ok(());
        }
        let seeds: Vec<&Candidate> = ranked_candidates(candidates)
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
        let terms = prf_expansion_terms(&request.query, &seeds);
        if terms.is_empty() {
            return Ok(());
        }
        let expanded = format!("{} {}", request.query, terms.join(" "));
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
                    explanation: vec![format!("PRF expansion: +{}", terms.join(" "))],
                },
                PRF_ATTENUATION / (RRF_K + rank as f64),
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
const fn expansion_policy(
    policy: GraphPolicy,
) -> (RelationDirection, &'static [cce_core::RelationKind]) {
    use cce_core::RelationKind;
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

/// Scale for the co-change bonus. `ChangedWith` confidence is capped at
/// 0.7, so a partner earns at most 0.3 × 0.7 = 0.21 — deliberately below
/// the flat 0.25 a same-file sibling gets: sharing commits is weaker
/// evidence than sharing the file a top hit already lives in.
const CO_CHANGE_SCALE: f64 = 0.3;

/// Same-package bonus: a top-3 hit's package is likely the task's package,
/// so sibling files get a small lift — below the same-file 0.25 and the
/// co-change ceiling.
const PACKAGE_BONUS: f64 = 0.15;

/// Ceiling for the corroboration bonus: the lift a document candidate
/// earns when graph expansion independently surfaced its entity, region,
/// or file. 0.2 sits beside the co-change ceiling (0.21) and under the
/// same-file 0.25 — corroboration confirms a vocabulary match, it does
/// not replace one. Its fused effect (0.2 / `RRF_K` ≈ 0.003) is an order
/// of magnitude under a rank-1 exact-symbol hit's RRF mass
/// (2.0 / (`RRF_K` + 1) ≈ 0.033), so it can reorder the head but never
/// leapfrog an identity match on its own.
const CORROBORATION_BONUS: f64 = 0.2;

/// Head candidates inspected for mechanism clustering: wide enough to
/// catch a task's file set when it genuinely clusters, narrow enough to
/// keep the relations fetch at ~one query per head file.
const CLUSTER_HEAD: usize = 15;

/// Intra-candidate edge weight. ln-damped so the first supporting edges
/// matter most — 0.12·ln(2) ≈ 0.08 for one edge, ≈0.17 for three — and
/// capped beside the co-change ceiling: a file whose neighbors also rank
/// is corroborated, never self-evident.
const CLUSTER_SCALE: f64 = 0.12;
const CLUSTER_CAP: f64 = 0.2;

/// Structural priors layered on the fused ranking:
/// - exact symbol/word agreement between the query and a hit's symbol name;
/// - same-file evidence aggregation (a file holding a top-3 hit makes its
///   other hits more likely to be task evidence);
/// - git co-change neighborhood (files that keep landing in the same
///   commits as a top-3 file);
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
    let top_paths: std::collections::HashSet<String> = ranked_candidates(candidates)
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
    // Co-change partners of the top-3 files. The edge is stored once per
    // pair (smaller entity id as source), so only a Both-direction lookup
    // sees it from either endpoint — and since `ChangedWith` caps at 0.7
    // while structural edges carry up to 1.0, the confidence-ordered fetch
    // limit must cover the file's whole degree or the co-change rows are
    // crowded out entirely.
    let mut co_changed = HashMap::<String, f32>::new();
    for path in &top_paths {
        let Some(file) = file_entity(store, snapshot_id, path)? else {
            continue;
        };
        let degree = store.entity_relation_degree(snapshot_id, &file.id)?;
        for relation in
            store.relations_for_entity(snapshot_id, &file.id, RelationDirection::Both, degree)?
        {
            if relation.kind != cce_core::RelationKind::ChangedWith {
                continue;
            }
            let partner_id = if relation.source_entity_id == file.id {
                &relation.target_entity_id
            } else {
                &relation.source_entity_id
            };
            let Some(partner) = store.entity_by_id(snapshot_id, partner_id)? else {
                continue;
            };
            // File entity names are repository-relative paths.
            let confidence = co_changed.entry(partner.name).or_default();
            *confidence = confidence.max(relation.confidence);
        }
    }
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
            if let Some(confidence) = co_changed.get(path) {
                bonus = CO_CHANGE_SCALE.mul_add(f64::from(*confidence), bonus);
                notes.push(format!(
                    "co-change partner of a top hit file (confidence {confidence:.2})"
                ));
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

/// Join graph evidence back onto document candidates — the pass that
/// fixes the `entity:`-vs-document key asymmetry of `add_candidate`.
/// Two priors share one per-candidate loop:
///
/// Corroboration: expansion hits are keyed `entity:{id}` while document
/// candidates carry document ids, so the merge can never connect them —
/// before this pass a gold file confirmed by three graph edges scored
/// identically to an isolated vocabulary match. A candidate is
/// corroborated when it IS a surfaced entity, shares a surfaced entity's
/// region, or lives in a file any surfaced entity points into — path
/// membership covers both file-entity-adjacent (`ChangedWith`/`Imports`)
/// and symbol-entity-adjacent (`Calls`/`References`) expansion, since
/// surfaced entities of either granularity carry `address.path`.
/// Strength is the best normalized propagated score reaching the
/// candidate, so weak tail evidence lifts less than a head-confirmed hit.
///
/// Cluster support: the files answering one query call, import, and
/// co-change each other; isolated vocabulary matches don't. The head's
/// candidates resolve to FILE entities (document hits on symbol regions
/// still map to a file through `address.path`), then edges whose other
/// endpoint is also in the head file set count as per-file support. Only
/// file-level adjacency is visible at this granularity: symbol-level
/// `Calls` never touch file entities as endpoints, so `Imports`/
/// `ChangedWith` carry the cluster signal.
fn apply_corroboration(
    store: &MetadataStore,
    snapshot_id: &str,
    expansion: &ExpansionEvidence,
    candidates: &mut HashMap<String, Candidate>,
) -> Result<()> {
    let mut head_files = HashMap::<String, String>::new(); // path → file entity id
    for candidate in ranked_candidates(candidates).into_iter().take(CLUSTER_HEAD) {
        let Some(path) = candidate.hit.address.as_ref().map(|a| a.path.clone()) else {
            continue;
        };
        if head_files.contains_key(&path) {
            continue;
        }
        if let Some(entity) = file_entity(store, snapshot_id, &path)? {
            head_files.insert(path, entity.id);
        }
    }
    let head_ids: std::collections::HashSet<&str> =
        head_files.values().map(String::as_str).collect();
    // path → count of edges whose other endpoint is another head file.
    let mut support = HashMap::<String, usize>::new();
    for (path, file_id) in &head_files {
        // Span the whole degree like the co-change fetch: confidence-
        // ordered heads crowd out the 0.7-capped ChangedWith edges that
        // carry much of the cluster signal.
        let degree = store.entity_relation_degree(snapshot_id, file_id)?;
        let count = store
            .relations_for_entity(snapshot_id, file_id, RelationDirection::Both, degree)?
            .iter()
            .filter(|relation| {
                let other = if relation.source_entity_id == *file_id {
                    relation.target_entity_id.as_str()
                } else {
                    relation.source_entity_id.as_str()
                };
                other != file_id.as_str() && head_ids.contains(other)
            })
            .count();
        support.insert(path.clone(), count);
    }
    for candidate in candidates.values_mut() {
        let hit = &candidate.hit;
        // Structural candidates ARE the expansion evidence; corroborating
        // them with it would double-count.
        if hit.contributing_routes.contains(&SearchRoute::Structural) {
            continue;
        }
        let mut bonus = 0.0_f64;
        let mut notes = Vec::new();
        if !expansion.entities.is_empty() {
            let evidence = [
                expansion.entities.get(&hit.entity_id),
                hit.region_id
                    .as_ref()
                    .and_then(|region_id| expansion.regions.get(region_id)),
                hit.address
                    .as_ref()
                    .and_then(|address| expansion.paths.get(&address.path)),
            ]
            .into_iter()
            .flatten()
            .max_by(|left, right| left.0.total_cmp(&right.0));
            if let Some(&(score, edges)) = evidence {
                let strength = (score / expansion.max_score).min(1.0);
                bonus = CORROBORATION_BONUS.mul_add(strength, bonus);
                notes.push(format!(
                    "corroborated by graph expansion ({edges} edge{})",
                    if edges == 1 { "" } else { "s" }
                ));
            }
        }
        if let Some(&count) = hit
            .address
            .as_ref()
            .and_then(|address| support.get(&address.path))
            .filter(|count| **count > 0)
        {
            bonus += (CLUSTER_SCALE * (count as f64).ln_1p()).min(CLUSTER_CAP);
            notes.push(format!("{count} intra-candidate edges (mechanism cluster)"));
        }
        candidate.hit.explanation.extend(notes);
        candidate.fused_score += bonus / RRF_K;
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

fn add_candidate(candidates: &mut HashMap<String, Candidate>, hit: SearchHit, contribution: f64) {
    candidates
        .entry(hit.document_id.clone())
        .and_modify(|candidate| {
            candidate.fused_score += contribution;
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
        });
}

fn ranked_candidates(candidates: &HashMap<String, Candidate>) -> Vec<&Candidate> {
    let mut ranked = candidates.values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .fused_score
            .total_cmp(&left.fused_score)
            .then_with(|| left.hit.document_id.cmp(&right.hit.document_id))
    });
    ranked
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
        tokens.retain(|token| is_identifier_like(token));
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
    use cce_core::{RelationKind, RelationOrigin, SnapshotIdentity, SourceAddress};
    use cce_store::SnapshotRecords;

    // Test fixtures panic freely: an unmet precondition is a test bug.
    fn store_with(records: &SnapshotRecords) -> (tempfile::TempDir, MetadataStore, String) {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = MetadataStore::open(directory.path()).expect("store");
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
        (directory, store, snapshot.id)
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

    fn references(source: &str, target: &str) -> cce_core::Relation {
        cce_core::Relation {
            id: format!("rel:{source}:{target}"),
            source_entity_id: source.to_owned(),
            target_entity_id: target.to_owned(),
            kind: RelationKind::References,
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
        }
    }

    #[test]
    fn co_change_partner_receives_bonus() {
        // The edge is stored once with the smaller id as source; the bonus
        // must still find it from the larger-id endpoint. Twenty 1.0-
        // confidence references crowd the co-change edge out of any naive
        // confidence-ordered head fetch — the fetch must span the degree.
        let mut relations = vec![changed_with("file:src/a.rs", "file:src/b.rs", 0.6)];
        for index in 0..20 {
            relations.push(references("file:src/a.rs", &format!("dummy:{index}")));
        }
        let records = SnapshotRecords {
            entities: vec![file("src/a.rs"), file("src/b.rs"), file("src/c.rs")],
            relations,
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.1));
        candidates.insert("c".to_owned(), candidate("c", "src/c.rs", 0.1));
        apply_structural_features(
            &store,
            &snapshot_id,
            QueryIntent::NaturalLanguageBehavior,
            0,
            "query",
            &mut candidates,
        )
        .expect("structural features");

        // All three files sit in top_paths and take the same-file 0.25, so
        // the fused-score gap between a partner and the unrelated file is
        // exactly the co-change bonus. Both endpoints are top hits and the
        // lookup is bidirectional, so a is itself b's partner.
        let expected = CO_CHANGE_SCALE * 0.6 / RRF_K;
        for endpoint in ["a", "b"] {
            let gap = candidates[endpoint].fused_score - candidates["c"].fused_score;
            assert!(
                (gap - expected).abs() < 1e-9,
                "{endpoint} fused {} vs unrelated {}",
                candidates[endpoint].fused_score,
                candidates["c"].fused_score
            );
            assert!(
                candidates[endpoint]
                    .hit
                    .explanation
                    .iter()
                    .any(|line| line.contains("co-change"))
            );
        }
        assert!(candidates["c"].hit.explanation.is_empty());
    }

    #[test]
    fn corroborated_candidate_outranks_isolated() {
        // The expansion set is the join key `add_candidate` cannot
        // provide: a file entity surfaced as a co-change neighbor
        // corroborates its document candidate through `entity_id`, and a
        // surfaced symbol corroborates every candidate in its file
        // through `address.path`.
        let mut expansion = ExpansionEvidence::default();
        expansion.record(&file("src/b.rs"), 0.05); // ChangedWith neighbor of a seed
        expansion.record(&symbol("helper", "src/d.rs"), 0.04); // Calls neighbor

        let mut candidates = HashMap::new();
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.1));
        candidates.insert("d".to_owned(), candidate("d", "src/d.rs", 0.1));
        candidates.insert("c".to_owned(), candidate("c", "src/c.rs", 0.1));
        let mut structural = candidate("s", "src/b.rs", 0.1);
        structural.hit.route = SearchRoute::Structural;
        structural.hit.contributing_routes = vec![SearchRoute::Structural];
        candidates.insert("s".to_owned(), structural);

        // No file entities are indexed, so no head file resolves and the
        // cluster pass contributes nothing — the gap is pure corroboration.
        let records = SnapshotRecords::default();
        let (_dir, store, snapshot_id) = store_with(&records);
        apply_corroboration(&store, &snapshot_id, &expansion, &mut candidates)
            .expect("corroboration");

        // b matches its entity id at full strength; d matches through the
        // symbol's file at 0.04/0.05 strength; c is isolated; the
        // structural hit IS the evidence and must not corroborate itself.
        let expected = |score: f64| CORROBORATION_BONUS * (score / 0.05) / RRF_K;
        assert!((candidates["b"].fused_score - 0.1 - expected(0.05)).abs() < 1e-9);
        assert!((candidates["d"].fused_score - 0.1 - expected(0.04)).abs() < 1e-9);
        assert!((candidates["c"].fused_score - 0.1).abs() < f64::EPSILON);
        assert!((candidates["s"].fused_score - 0.1).abs() < f64::EPSILON);
        for key in ["b", "d"] {
            assert!(
                candidates[key]
                    .hit
                    .explanation
                    .iter()
                    .any(|line| line.contains("corroborated by graph expansion")),
                "{key} must explain its corroboration"
            );
        }
        assert!(candidates["c"].hit.explanation.is_empty());
    }

    #[test]
    fn clustered_file_outranks_lone_file() {
        // Mechanism density: a and b co-change inside the candidate head;
        // c is isolated. Each endpoint counts the edge once, so both
        // clustered files earn the ln-damped bonus and the lone file
        // earns nothing — the sibling's presence in the ranking is the
        // evidence.
        let records = SnapshotRecords {
            entities: vec![file("src/a.rs"), file("src/b.rs"), file("src/c.rs")],
            relations: vec![changed_with("file:src/a.rs", "file:src/b.rs", 0.6)],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.1));
        candidates.insert("c".to_owned(), candidate("c", "src/c.rs", 0.1));
        apply_corroboration(
            &store,
            &snapshot_id,
            &ExpansionEvidence::default(),
            &mut candidates,
        )
        .expect("corroboration");

        let expected = CLUSTER_SCALE * 2.0_f64.ln() / RRF_K;
        for key in ["a", "b"] {
            assert!(
                (candidates[key].fused_score - 0.1 - expected).abs() < 1e-9,
                "{key} fused {}",
                candidates[key].fused_score
            );
            assert!(
                candidates[key]
                    .hit
                    .explanation
                    .iter()
                    .any(|line| line.contains("intra-candidate edges (mechanism cluster)"))
            );
        }
        assert!((candidates["c"].fused_score - 0.1).abs() < f64::EPSILON);
        assert!(candidates["c"].hit.explanation.is_empty());
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
}
