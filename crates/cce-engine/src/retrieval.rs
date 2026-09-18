use std::{collections::HashMap, time::Instant};

use cce_core::{
    CodeEntity, EntityKind, QueryIntent, RelationKind, Result, RetrievalRepresentation, SearchHit,
    SearchRequest, SearchRoute, ViewKind, ViewManifest, ViewState, has_cjk,
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
        // Structural passes. `CCE_FLOW` selects the graph evidence
        // mechanism: `off` keeps only the legacy priors, `replace` runs
        // the unified flow pass INSTEAD of the priors it subsumes, and
        // anything else unions them (flow is additive on top).
        let flow_mode = std::env::var("CCE_FLOW").unwrap_or_default();
        if !matches!(flow_mode.as_str(), "replace") {
            apply_corroboration(
                self.store(),
                &request.snapshot_id,
                &expansion_evidence,
                &mut candidates,
            )?;
            // Symbol evidence runs last among scoring passes: its
            // seeds are the candidates the other passes already ranked.
            symbol_evidence_join(
                self.store(),
                &request.snapshot_id,
                &request.query,
                &mut candidates,
                verified_fresh,
                request.limit,
            )?;
        }
        if flow_mode != "off" {
            apply_graph_flow(
                self.store(),
                &request.snapshot_id,
                &request.query,
                &mut candidates,
                verified_fresh,
                request.limit,
            )?;
        }

        let mut hits = self.select_hits(&request, &candidates)?;

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
    ) -> Result<Vec<SearchHit>> {
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut per_file = HashMap::<String, usize>::new();
        let mut seen_regions = HashMap::<String, usize>::new();
        let mut language_cache = HashMap::<String, Option<String>>::new();
        for candidate in ranked_candidates(candidates) {
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
                if *count >= per_file_cap(hits.len()) {
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
/// catch a task's file set when it genuinely clusters — gold evidence for
/// one query routinely spans 6-11 files parked across ranks 6-30 — while
/// bounded so the relations fetch stays at ~one query per head file.
const CLUSTER_HEAD: usize = 30;

/// Intra-candidate edge weight. ln-damped so the first supporting edges
/// matter most — 0.12·ln(2) ≈ 0.08 for one edge, ≈0.17 for three — and
/// capped beside the co-change ceiling: a file whose neighbors also rank
/// is corroborated, never self-evident.
const CLUSTER_SCALE: f64 = 0.12;
const CLUSTER_CAP: f64 = 0.2;

/// File-vote prior ceiling. Mechanism files recur across the candidate
/// pool — the regions answering a query cluster in a handful of files —
/// while an isolated vocabulary match occupies exactly one rank. The
/// vote scales a file's best-rank mass by ln(1+occurrences): a single
/// hit earns nothing (ln 2 damped by the cap check below would still
/// score, so the prior only pays out past one occurrence), a file
/// holding three ranks earns ≈1.4× its best-rank mass. Occurrences are
/// capped so a large file's raw chunk count cannot dominate on size
/// alone; the ceiling sits at the co-change/cluster tier.
const FILE_VOTE_BONUS: f64 = 0.2;
const FILE_VOTE_MAX_OCCURRENCES: usize = 8;

/// Symbol-evidence join bounds. Claims are supported by atomic symbols,
/// but topical retrieval scores documents — the entities the head
/// already surfaced point at the symbols the mechanism actually uses.
/// The pass walks outgoing `References`/`Calls` edges from candidate
/// entities, counts how many retrieved entities point at each neighbor,
/// and emits bounded-mass `entity:` hits into the mid-tail where
/// `claim_support` and `symbol_recall` read the list — never scoring
/// high enough to reorder the head. Seeds are degree-capped so a hub
/// entity cannot buy rank for its whole neighborhood.
const SYMBOL_EVIDENCE_MAX: usize = 12;
const SYMBOL_EVIDENCE_SEEDS: usize = 24;
const SYMBOL_EVIDENCE_SEED_DEGREE: usize = 48;
/// Mass floor for an emitted block entry: it must beat the score at
/// `limit - MAX` so the whole block lands inside the emitted window
/// while displacing only the weakest incumbents. A hair above the
/// boundary keeps ordering inside the block deterministic.
const SYMBOL_EVIDENCE_BOUNDARY_FRACTION: f64 = 1.02;

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
            if relation.kind != RelationKind::ChangedWith {
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
/// Three priors share one per-candidate loop:
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
///
/// File vote: the files answering one query recur across the candidate
/// pool — a mechanism's regions fill many ranks — while an isolated
/// vocabulary match occupies exactly one. Each file is keyed by its
/// best rank (a lucky tail hit cannot outvote a real head presence) and
/// earns `FILE_VOTE_BONUS` × best-rank mass × ln(1+occurrences) once it
/// holds more than one pooled rank. No store access: the pool itself is
/// the evidence.
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
    // File vote: occurrences per path across the whole pool, each file
    // keyed by its best rank so a lucky tail hit cannot outvote a real
    // head presence. Only the file's champion — the document holding
    // that best rank — collects the bonus: it is the slot competing for
    // the head; boosting the file's tail docs would just crowd the
    // dedup-relaxed middle ranks.
    let mut votes = HashMap::<String, (String, usize, usize)>::new(); // path → (champion doc, best rank, occurrences)
    for (rank, candidate) in ranked_candidates(candidates).iter().enumerate() {
        let Some(path) = candidate.hit.address.as_ref().map(|a| a.path.clone()) else {
            continue;
        };
        let entry = votes
            .entry(path)
            .or_insert_with(|| (candidate.hit.document_id.clone(), rank, 0));
        entry.2 += 1;
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
        if let Some((_, best_rank, occurrences)) = hit
            .address
            .as_ref()
            .and_then(|address| votes.get(&address.path))
            .filter(|(champion, _, occurrences)| *occurrences > 1 && *champion == hit.document_id)
        {
            let strength = RRF_K / (RRF_K + *best_rank as f64 + 1.0)
                * ((*occurrences).min(FILE_VOTE_MAX_OCCURRENCES) as f64).ln_1p()
                / (FILE_VOTE_MAX_OCCURRENCES as f64).ln_1p();
            bonus = FILE_VOTE_BONUS.mul_add(strength, bonus);
            notes.push(format!(
                "file vote: {occurrences} pooled occurrences (champion)"
            ));
        }
        candidate.hit.explanation.extend(notes);
        candidate.fused_score += bonus / RRF_K;
    }
    Ok(())
}

/// Symbol-evidence join: the documents answering a query name the
/// files, but claims are supported by the atomic symbols those files'
/// mechanism functions actually use. Every candidate entity's outgoing
/// `References`/`Calls` edges are evidence pointers — a neighbor
/// pointed at by several retrieved entities is consensus evidence, and
/// a neighbor whose name overlaps the query is on-topic. Both signals
/// are combined per neighbor; the best few emit as `entity:`-keyed hits
/// at bounded mid-tail mass.
///
/// Placement is deliberate: `claim_support` and `symbol_recall` read
/// the whole hit list, so evidence enriches the tail without touching
/// the head ordering the other passes just established. Neighbors
/// already in the candidate map merge score through `add_candidate`,
/// corroborating rather than duplicating.
fn symbol_evidence_join(
    store: &MetadataStore,
    snapshot_id: &str,
    query: &str,
    candidates: &mut HashMap<String, Candidate>,
    verified_fresh: bool,
    limit: usize,
) -> Result<()> {
    let query_terms = prf_terms_in(query);
    if query_terms.is_empty() || candidates.is_empty() {
        return Ok(());
    }
    // Plural-tolerant term set: "views" in prose should match the "view"
    // inside `set_view_status`. Both forms are admitted so a query's
    // singular and plural spellings collapse onto the symbol token.
    let mut tolerant_terms = query_terms.clone();
    for term in &query_terms {
        if term.len() > 3 && term.ends_with('s') {
            tolerant_terms.insert(term[..term.len() - 1].to_owned());
        }
    }

    // Consensus count: for every neighbor, how many retrieved entities
    // point at it, and the strongest single referrer's score. Seeds are
    // the strongest unique candidate entities — the head carries the
    // signal and the cap bounds the per-seed relation fetches. Seed
    // degree is capped so hub entities cannot flood the neighbor space.
    let mut neighbors = HashMap::<String, (usize, f64)>::new(); // id → (pointers, best referrer)
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
    seeds.truncate(SYMBOL_EVIDENCE_SEEDS);
    // The block's mass is read off the incumbent pool: beating the
    // score at `limit - MAX` lands every entry inside the emitted
    // window at the cost of only the weakest incumbents. Shallow pools
    // anchor at their own tail instead.
    let mut pool_scores: Vec<f64> = candidates.values().map(|c| c.fused_score).collect();
    pool_scores.sort_by(|a, b| b.total_cmp(a));
    let boundary = pool_scores
        .get(limit.saturating_sub(SYMBOL_EVIDENCE_MAX))
        .or_else(|| pool_scores.last())
        .copied()
        .unwrap_or_default();
    for (seed_id, seed_score) in &seeds {
        let degree = store
            .entity_relation_degree(snapshot_id, seed_id)?
            .min(SYMBOL_EVIDENCE_SEED_DEGREE);
        for relation in
            store.relations_for_entity(snapshot_id, seed_id, RelationDirection::Outgoing, degree)?
        {
            if !matches!(
                relation.kind,
                RelationKind::References | RelationKind::Calls
            ) {
                continue;
            }
            let entry = neighbors
                .entry(relation.target_entity_id.clone())
                .or_default();
            entry.0 += 1;
            entry.1 = entry.1.max(*seed_score);
        }
    }
    if neighbors.is_empty() {
        return Ok(());
    }

    // Score each neighbor: consensus pointers plus identifier overlap.
    // Either signal alone can admit — a symbol named like the query is
    // topical, one referenced by the head is structural — so a neighbor
    // needs at least one of them, and raw score orders the emission cap.
    // Neighbor resolution is one batched fetch: hundreds of entities
    // fan out per query, and point lookups dominate the join's cost.
    let neighbor_ids: Vec<String> = neighbors.keys().cloned().collect();
    let entities: HashMap<String, CodeEntity> = store
        .entities_by_ids(snapshot_id, &neighbor_ids)?
        .into_iter()
        .map(|entity| (entity.id.clone(), entity))
        .collect();
    let mut scored: Vec<(f64, usize, CodeEntity)> = Vec::new();
    for (entity_id, (pointers, best_referrer)) in neighbors {
        let Some(entity) = entities.get(&entity_id) else {
            continue;
        };
        if !matches!(
            entity.kind,
            EntityKind::Function
                | EntityKind::Struct
                | EntityKind::Enum
                | EntityKind::Trait
                | EntityKind::Class
                | EntityKind::Interface
                | EntityKind::Constant
        ) {
            continue;
        }
        let overlap = prf_terms_in(&entity.name)
            .iter()
            .filter(|term| {
                tolerant_terms.contains(term.as_str())
                    || (term.len() > 3
                        && term.ends_with('s')
                        && tolerant_terms.contains(&term[..term.len() - 1]))
            })
            .count();
        if pointers < 2 && overlap == 0 {
            continue;
        }
        // Raw score: consensus pointers dominate, identifier overlap
        // weighs 1.5× a pointer, and the strongest referrer's fused
        // score (~0.01-0.05) is a mild nudge that breaks pointer ties
        // toward head-endorsed evidence.
        let raw = 1.5f64.mul_add(overlap as f64, pointers as f64) + best_referrer;
        scored.push((raw, overlap, entity.clone()));
    }
    scored.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.2.id.cmp(&right.2.id))
    });
    // Two emission lanes, interleaved. The named lane is restricted to
    // callable symbols: claim facts name functions and methods, while
    // name-matched types still reach the list through the consensus
    // lane on pointer count alone. Without the restriction, type hubs
    // pointed at by everything crowd the exact claim evidence out of
    // the emission cap.
    let (named, plain): (Vec<_>, Vec<_>) = scored.into_iter().partition(|(_, overlap, entity)| {
        *overlap > 0 && matches!(entity.kind, EntityKind::Function | EntityKind::Method)
    });
    let mut named = named.into_iter();
    let mut plain = plain.into_iter();
    let mut scored: Vec<(f64, usize, CodeEntity)> = Vec::new();
    while scored.len() < SYMBOL_EVIDENCE_MAX {
        let mut progressed = false;
        for lane in [&mut named, &mut plain] {
            if let Some(entry) = lane.next() {
                scored.push(entry);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    if scored.is_empty() {
        return Ok(());
    }
    if std::env::var_os("CCE_DEBUG_SYMEV").is_some() {
        for (i, (raw, ov, e)) in scored.iter().enumerate() {
            eprintln!("[symev] #{i} raw={raw:.3} ov={ov} {}", e.name);
        }
    }

    for (rank, (raw, overlap, entity)) in scored.into_iter().enumerate() {
        if boundary <= 0.0 {
            break;
        }
        // A hair above the boundary, decaying within the block so lane
        // order is preserved among the emitted entries.
        let mass =
            boundary * SYMBOL_EVIDENCE_BOUNDARY_FRACTION * 0.005f64.mul_add(-(rank as f64), 1.0);
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
                score: mass,
                contributing_routes: vec![SearchRoute::Structural],
                address: entity.address,
                evidence: Vec::new(),
                snippet,
                verified_current: verified_fresh,
                explanation: vec![format!(
                    "symbol evidence: referenced by retrieved candidates, consensus score {raw:.2}, term overlap {overlap}"
                )],
            },
            mass,
        );
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
const FLOW_EMIT_MAX: usize = 8;

/// Receiver-side hub gate `1/(1+ln degree)`: one inbound edge passes
/// mass at full strength, while a type referenced by hundreds of
/// functions absorbs proportionally little per sender. This is the
/// target-side counterpart of source-side fan-out normalization —
/// without it, signature types monopolize the flow exactly as pointer
/// hubs did in the symbol-evidence join.
fn flow_hub_gate(degree: f64) -> f64 {
    1.0 / (1.0 + degree.max(1.0).ln())
}

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

fn apply_graph_flow(
    store: &MetadataStore,
    snapshot_id: &str,
    query: &str,
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
    let Some(max_seed) = seeds.first().map(|(_, score)| *score) else {
        return Ok(());
    };
    if max_seed <= 0.0 {
        return Ok(());
    }

    // Edge universe: deduplicated relations touching any seed. Relation
    // rows repeat per extractor merge, so dedup by (source, target,
    // kind) or the flow multiplies. Mass flows ALONG edge direction:
    // only nodes with mass feed, so incoming edges matter exactly when
    // their source is also a seed — seed→shared-neighbor→seed is how
    // cluster support emerges in hop 2.
    let mut edges: Vec<(String, String, RelationKind, f64)> = Vec::new();
    {
        let mut seen = std::collections::BTreeSet::<(String, String, u8)>::new();
        for (seed_id, _) in &seeds {
            let degree = store
                .entity_relation_degree(snapshot_id, seed_id)?
                .min(FLOW_SEED_DEGREE);
            for relation in
                store.relations_for_entity(snapshot_id, seed_id, RelationDirection::Both, degree)?
            {
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
    // Per-kind in-degree powers the receiver-side hub gate: a type
    // referenced by hundreds of functions absorbs little per sender,
    // while a file `contains`-ing many members is a normal parent.
    let mut out_conf = HashMap::<(String, RelationKind), f64>::new();
    let mut in_conf = HashMap::<(String, RelationKind), f64>::new();
    let mut out_deg = HashMap::<(String, RelationKind), f64>::new();
    let mut in_deg = HashMap::<(String, RelationKind), f64>::new();
    for (source, target, kind, confidence) in &edges {
        *out_conf.entry((source.clone(), kind.clone())).or_default() += confidence;
        *in_conf.entry((target.clone(), kind.clone())).or_default() += confidence;
        *out_deg.entry((source.clone(), kind.clone())).or_default() += 1.0;
        *in_deg.entry((target.clone(), kind.clone())).or_default() += 1.0;
    }

    let mut mass: HashMap<String, f64> = seeds.iter().cloned().collect();
    // Mass RECEIVED from other nodes — the evidence signal. Seed mass
    // itself is excluded: a candidate's own topicality is not evidence
    // corroborating it.
    let mut received: HashMap<String, f64> = HashMap::new();
    // Non-backtracking: an edge traversed at hop t cannot reverse at
    // t+1 — test↔mechanism ping-pong would inflate both endpoints
    // without adding corroboration. Re-firing the same direction is
    // legitimate accumulation; only immediate reversal is blocked. A
    // node may still be reached via OTHER seeds — multi-path arrival
    // is the consensus signal itself.
    let mut traversed = std::collections::HashSet::<(usize, u8)>::new();
    for hop in 1..=FLOW_HOPS {
        let prev = mass.clone();
        let used = std::mem::take(&mut traversed);
        let mut delta = std::collections::BTreeMap::<String, f64>::new();
        for (index, (source, target, kind, confidence)) in edges.iter().enumerate() {
            let (forward, backward) = flow_edge_weights(kind);
            let source_mass = prev.get(source).copied().unwrap_or_default();
            if forward > 0.0 && source_mass > 0.0 && !used.contains(&(index, 1)) {
                let share = confidence
                    / out_conf
                        .get(&(source.clone(), kind.clone()))
                        .copied()
                        .unwrap_or(1.0)
                        .max(f64::EPSILON);
                let hub_gate = flow_hub_gate(
                    in_deg
                        .get(&(target.clone(), kind.clone()))
                        .copied()
                        .unwrap_or(1.0),
                );
                *delta.entry(target.clone()).or_default() +=
                    (forward * source_mass * share).mul_add(hub_gate, 0.0);
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
                let hub_gate = flow_hub_gate(
                    out_deg
                        .get(&(source.clone(), kind.clone()))
                        .copied()
                        .unwrap_or(1.0),
                );
                *delta.entry(source.clone()).or_default() +=
                    (backward * target_mass * share).mul_add(hub_gate, 0.0);
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
    // best-corroborated node, bounded at FLOW_BONUS.
    let max_received = received.values().copied().fold(0.0_f64, f64::max);
    if max_received > 0.0 {
        for candidate in candidates.values_mut() {
            let Some(flow) = received.get(&candidate.hit.entity_id) else {
                continue;
            };
            let bonus = FLOW_BONUS * (flow / max_received) / RRF_K;
            candidate.fused_score += bonus;
            candidate.hit.explanation.push(format!(
                "graph flow corroboration {:.2}",
                flow / max_received
            ));
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
    let mut scored: Vec<(f64, usize, &CodeEntity)> = Vec::new();
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
        let overlap = prf_terms_in(&entity.name)
            .iter()
            .filter(|term| query_terms.contains(term.as_str()))
            .count();
        scored.push((flow, overlap, entity));
    }
    scored.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.2.id.cmp(&right.2.id))
    });
    let (named, plain): (Vec<_>, Vec<_>) = scored.into_iter().partition(|(_, overlap, entity)| {
        *overlap > 0 && matches!(entity.kind, EntityKind::Function | EntityKind::Method)
    });
    let mut named = named.into_iter();
    let mut plain = plain.into_iter();
    let mut block: Vec<(f64, usize, &CodeEntity)> = Vec::new();
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
    for (rank, (flow, _, entity)) in block.into_iter().enumerate() {
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
        relation(source, target, RelationKind::References)
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
            .select_hits(&request("query", 5), &candidates)
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
            .select_hits(&request("query", 5), &candidates)
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
            .select_hits(&request("query", 6), &candidates)
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
        let mut structural = candidate("s", "src/e.rs", 0.1);
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
    fn symbol_evidence_surfaces_pointed_symbols() {
        // Seeds are the retrieved file entities; their outgoing
        // references edges name the atomic symbols claim evidence lives
        // in. `set_view_status` is admitted on query overlap alone
        // ("views" tolerates the singular "view"), `consensus_target`
        // on two pointers without any name match, while a single
        // pointer with no overlap is noise and a `changed_with` edge
        // is the wrong kind entirely.
        let records = SnapshotRecords {
            entities: vec![
                file("src/a.rs"),
                file("src/b.rs"),
                symbol("set_view_status", "src/m.rs"),
                symbol("consensus_target", "src/m.rs"),
                symbol("unrelated_helper", "src/m.rs"),
                symbol("decoy_only_changed", "src/m.rs"),
            ],
            relations: vec![
                references("file:src/a.rs", "symbol:set_view_status"),
                references("file:src/a.rs", "symbol:consensus_target"),
                references("file:src/b.rs", "symbol:consensus_target"),
                references("file:src/a.rs", "symbol:unrelated_helper"),
                changed_with("file:src/a.rs", "symbol:decoy_only_changed", 0.9),
            ],
            ..SnapshotRecords::default()
        };
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a".to_owned(), candidate("a", "src/a.rs", 0.1));
        candidates.insert("b".to_owned(), candidate("b", "src/b.rs", 0.05));

        symbol_evidence_join(
            &store,
            &snapshot_id,
            "mark views stale",
            &mut candidates,
            true,
            50,
        )
        .expect("symbol evidence");

        let emitted = &candidates["entity:symbol:set_view_status"].hit;
        assert_eq!(emitted.route, SearchRoute::Structural);
        assert_eq!(emitted.symbol_name.as_deref(), Some("set_view_status"));
        assert!(emitted.verified_current);
        assert!(
            emitted
                .explanation
                .iter()
                .any(|line| line.contains("symbol evidence"))
        );
        assert!(candidates.contains_key("entity:symbol:consensus_target"));
        assert!(!candidates.contains_key("entity:symbol:unrelated_helper"));
        assert!(!candidates.contains_key("entity:symbol:decoy_only_changed"));
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
    fn graph_flow_empty_pool_is_noop() {
        // No-context guard: an empty candidate pool must not touch the
        // store or panic — abstention stays clean.
        let records = SnapshotRecords::default();
        let (_dir, store, snapshot_id) = store_with(&records);
        let mut candidates = HashMap::new();
        apply_graph_flow(&store, &snapshot_id, "anything", &mut candidates, true, 50)
            .expect("graph flow");
        assert!(candidates.is_empty());
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

    #[test]
    fn recurring_file_outranks_single_hit_file() {
        // File vote: a.rs holds three pooled ranks, c.rs holds one — the
        // recurring file's regions corroborate each other. Equal fused
        // scores in, a.rs must come out ahead; the single-occurrence
        // file earns nothing (occurrences > 1 gate).
        let records = SnapshotRecords::default();
        let (_dir, store, snapshot_id) = store_with(&records);

        let mut candidates = HashMap::new();
        candidates.insert("a1".to_owned(), candidate("a1", "src/a.rs", 0.1));
        candidates.insert("a2".to_owned(), candidate("a2", "src/a.rs", 0.09));
        candidates.insert("a3".to_owned(), candidate("a3", "src/a.rs", 0.08));
        candidates.insert("c".to_owned(), candidate("c", "src/c.rs", 0.1));
        apply_corroboration(
            &store,
            &snapshot_id,
            &ExpansionEvidence::default(),
            &mut candidates,
        )
        .expect("corroboration");

        // a1 is a.rs's champion — the document at the file's best rank —
        // and alone collects the vote; a2/a3 are the tail the vote is
        // measured on, not recipients. strength = best-rank mass ×
        // ln(occ)/ln(cap); c has one occurrence and earns nothing.
        let strength =
            RRF_K / (RRF_K + 1.0) * 3.0_f64.ln_1p() / (FILE_VOTE_MAX_OCCURRENCES as f64).ln_1p();
        let expected = FILE_VOTE_BONUS * strength / RRF_K;
        assert!((candidates["a1"].fused_score - 0.1 - expected).abs() < 1e-9);
        assert!(
            candidates["a1"]
                .hit
                .explanation
                .iter()
                .any(|line| line.contains("file vote: 3 pooled occurrences"))
        );
        assert!((candidates["a2"].fused_score - 0.09).abs() < f64::EPSILON);
        assert!((candidates["a3"].fused_score - 0.08).abs() < f64::EPSILON);
        assert!((candidates["c"].fused_score - 0.1).abs() < f64::EPSILON);
        assert!(candidates["a2"].hit.explanation.is_empty());
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
}
