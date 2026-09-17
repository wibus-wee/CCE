use std::{collections::HashMap, time::Instant};

use cce_core::{
    Result, RetrievalRepresentation, SearchHit, SearchRequest, SearchRoute, ViewKind, ViewManifest,
    ViewState,
};
use cce_store::RelationDirection;
use serde::{Deserialize, Serialize};

use crate::{
    CceEngine, DenseIndex, Embedder, GraphPolicy, QueryPlan, QueryPlanner, RepositoryScanner,
};

const RRF_K: f64 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub request: SearchRequest,
    pub plan: QueryPlan,
    pub manifest: ViewManifest,
    pub hits: Vec<SearchHit>,
    pub missing_capabilities: Vec<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    hit: SearchHit,
    fused_score: f64,
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

    pub async fn search(&self, mut request: SearchRequest) -> Result<SearchResult> {
        let started = Instant::now();
        let resolved = self.resolve_index(request.require_fresh).await?;
        request.repository_id.clone_from(&resolved.repository_id);
        request.snapshot_id.clone_from(&resolved.snapshot_id);
        let verified_fresh = resolved.verified_fresh;
        let mut plan = QueryPlanner::new().plan(&request.query, request.intent);
        if !request.routes.is_empty() {
            plan.routes.clone_from(&request.routes);
            plan.required_views = required_views_for_routes(&request.routes);
            plan.reasons
                .push("caller supplied an explicit retrieval route override".to_owned());
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
                )?
                .into_iter()
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
                &[RetrievalRepresentation::CommitSummary][..],
            ),
        ] {
            if !plan.routes.contains(&route) {
                continue;
            }
            let hits = self.store().lexical_search(
                &request.snapshot_id,
                &request.query,
                request.limit.saturating_mul(5),
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

        if plan.routes.iter().any(|route| {
            matches!(
                route,
                SearchRoute::DenseRaw | SearchRoute::DenseSummary | SearchRoute::Hybrid
            )
        }) {
            if let Some(dense_status) = manifest.views.get(&ViewKind::Dense) {
                if matches!(dense_status.state, ViewState::Ready | ViewState::Partial) {
                    if let (Some(digest), Some(embedder)) = (
                        dense_status.artifact_digest.as_deref(),
                        self.embedder().await?,
                    ) {
                        let index = DenseIndex::decode(&self.store().artifacts().read(digest)?)?;
                        let dense_hits = index
                            .search(&request.query, &embedder, request.limit.saturating_mul(3))
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

        if plan.routes.contains(&SearchRoute::Structural)
            && !matches!(
                plan.graph_policy,
                GraphPolicy::None | GraphPolicy::ArchitectureBoundary
            )
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

        apply_structural_features(&request.query, &mut candidates);

        // Multiple retrieval documents can describe one source region (raw
        // chunk + symbol summary); the hit list presents regions, so the
        // first — best-scored — document per region wins and later ones only
        // contribute their routes. Per-file cap keeps cross-file coverage;
        // hits beyond it stay in `candidates` for expansion seeds.
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut per_file = HashMap::<String, usize>::new();
        let mut seen_regions = HashMap::<String, usize>::new();
        for candidate in ranked_candidates(&candidates) {
            let mut hit = candidate.hit.clone();
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
                for route in &hit.contributing_routes {
                    if !hits[kept].contributing_routes.contains(route) {
                        hits[kept].contributing_routes.push(*route);
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
}

/// Which edges count as evidence for each graph policy. The intent chooses
/// the vocabulary of the expansion, not just its direction.
fn expansion_policy(policy: GraphPolicy) -> (RelationDirection, &'static [cce_core::RelationKind]) {
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

/// Structural priors layered on the fused ranking:
/// - exact symbol/word agreement between the query and a hit's symbol name;
/// - same-file evidence aggregation (a file holding a top-3 hit makes its
///   other hits more likely to be task evidence);
/// - definition prior (Signature/TestBehavior representations beat stray
///   raw-code mentions for symbol-shaped queries).
fn apply_structural_features(query: &str, candidates: &mut HashMap<String, Candidate>) {
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
    for candidate in candidates.values_mut() {
        let hit = &candidate.hit;
        let mut bonus = 0.0;
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
        }
        if matches!(
            hit.representation,
            RetrievalRepresentation::Signature | RetrievalRepresentation::TestBehavior
        ) {
            bonus += 0.1;
        }
        candidate.fused_score += bonus / RRF_K;
    }
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

fn representation_weight(representation: &RetrievalRepresentation) -> f64 {
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
            SearchRoute::History => &[ViewKind::History],
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
        .filter(|token| token.chars().count() >= 3)
        .filter(|token| !stop.contains(&token.to_ascii_lowercase().as_str()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // Exact-symbol is an identity lookup: inside a multi-token natural
    // language query a plain lowercase word ("from", "merged") colliding
    // with an entity of the same name is coincidence, not intent — such
    // terms are lexical evidence instead. Identifier-shaped tokens keep
    // the route; a single-token query keeps its only token.
    if tokens.len() > 1 {
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

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}
