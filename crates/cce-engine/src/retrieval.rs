use std::{collections::HashMap, time::Instant};

use cce_core::{
    Result, RetrievalRepresentation, SearchHit, SearchRequest, SearchRoute, ViewKind, ViewManifest,
    ViewState,
};
use cce_store::RelationDirection;
use serde::{Deserialize, Serialize};

use crate::{
    CceEngine, DenseIndex, Embedder, EmbeddingBackend, GraphPolicy, QueryPlan, QueryPlanner,
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

impl CceEngine {
    pub async fn search(&self, mut request: SearchRequest) -> Result<SearchResult> {
        let started = Instant::now();
        let indexed = self.index().await?;
        request.repository_id.clone_from(&indexed.repository_id);
        request.snapshot_id.clone_from(&indexed.snapshot.id);
        let mut plan = QueryPlanner::new().plan(&request.query, request.intent);
        if !request.routes.is_empty() {
            plan.routes.clone_from(&request.routes);
            plan.required_views = required_views_for_routes(&request.routes);
            plan.reasons
                .push("caller supplied an explicit retrieval route override".to_owned());
        }
        let manifest = indexed.manifest;
        let mut missing_capabilities = missing_views(&manifest, &plan);
        if plan.rerank {
            missing_capabilities.push(
                "learned reranker is not configured; results use deterministic reciprocal-rank fusion"
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
                            verified_current: true,
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
                        symbol_name: Some(hit.symbol_name),
                        representation: hit.representation,
                        route: SearchRoute::Lexical,
                        rank,
                        score: hit.score,
                        contributing_routes: vec![SearchRoute::Lexical],
                        address: hit.address,
                        evidence: hit.evidence,
                        snippet: hit.snippet,
                        verified_current: true,
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
                        symbol_name: Some(hit.symbol_name),
                        representation: hit.representation,
                        route,
                        rank,
                        score: hit.score,
                        contributing_routes: vec![route],
                        address: hit.address,
                        evidence: hit.evidence,
                        snippet: hit.snippet,
                        verified_current: true,
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
                        EmbeddingBackend::from_config(&self.config().dense)?,
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
                                    symbol_name,
                                    representation: document.representation.clone(),
                                    route,
                                    rank,
                                    score: f64::from(hit.score),
                                    contributing_routes: vec![route],
                                    address: document.address.clone(),
                                    evidence: document.evidence.clone(),
                                    snippet: truncate_chars(&document.text, 2_000),
                                    verified_current: true,
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

        if !matches!(
            plan.graph_policy,
            GraphPolicy::None | GraphPolicy::ArchitectureBoundary
        ) {
            let direction = match plan.graph_policy {
                GraphPolicy::OutgoingTrace => RelationDirection::Outgoing,
                GraphPolicy::IncomingImpact => RelationDirection::Incoming,
                GraphPolicy::DataflowRequired
                | GraphPolicy::None
                | GraphPolicy::ArchitectureBoundary => RelationDirection::Both,
            };
            let seeds = ranked_candidates(&candidates)
                .into_iter()
                .take(5)
                .map(|candidate| candidate.hit.entity_id.clone())
                .collect::<Vec<_>>();
            let mut rank = 1_usize;
            for seed in seeds {
                for relation in
                    self.store()
                        .relations_for_entity(&request.snapshot_id, &seed, direction, 12)?
                {
                    let neighbor = if relation.source_entity_id == seed {
                        &relation.target_entity_id
                    } else {
                        &relation.source_entity_id
                    };
                    let Some(entity) = self.store().entity_by_id(&request.snapshot_id, neighbor)?
                    else {
                        continue;
                    };
                    let snippet = entity
                        .signature
                        .clone()
                        .or_else(|| entity.qualified_name.clone())
                        .unwrap_or_else(|| entity.name.clone());
                    add_candidate(
                        &mut candidates,
                        SearchHit {
                            document_id: format!("entity: {}", entity.id),
                            symbol_name: Some(entity.name.clone()),
                            entity_id: entity.id,
                            representation: RetrievalRepresentation::Signature,
                            route: SearchRoute::Structural,
                            rank,
                            score: f64::from(relation.confidence),
                            contributing_routes: vec![SearchRoute::Structural],
                            address: entity.address,
                            evidence: relation.evidence.clone(),
                            snippet,
                            verified_current: true,
                            explanation: vec![format!(
                                "selective {:?} graph expansion over {:?} ({:?}, confidence {:.2})",
                                plan.graph_policy,
                                relation.kind,
                                relation.origin,
                                relation.confidence
                            )],
                        },
                        f64::from(relation.confidence) / (RRF_K + rank as f64),
                    );
                    rank += 1;
                }
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

        let mut hits = ranked_candidates(&candidates)
            .into_iter()
            .map(|candidate| {
                let mut hit = candidate.hit.clone();
                hit.score = candidate.fused_score;
                hit
            })
            .collect::<Vec<_>>();
        hits.truncate(request.limit);
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
    tokens.sort_by_key(|token| std::cmp::Reverse(token.len()));
    tokens.truncate(8);
    tokens
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}
