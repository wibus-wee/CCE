use std::{collections::HashMap, time::Instant};

use cce_core::{
    QueryIntent, Relation, RelationKind, RelationOrigin, Result, RetrievalRepresentation,
    SearchHit, SearchRequest, SearchRoute, SourceAddress, ViewKind, ViewManifest, ViewState,
};
use cce_store::RelationDirection;
use serde::{Deserialize, Serialize};

use crate::{CceEngine, Embedder, GraphPolicy, QueryPlan, QueryPlanner};

const RRF_K: f64 = 60.0;
const RERANK_RRF_WEIGHT: f64 = 2.75;

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
        let (repository_id, snapshot, manifest, verified_current) =
            self.snapshot_for_query(request.require_fresh).await?;
        request.repository_id = repository_id;
        request.snapshot_id = snapshot.id;
        let mut plan = QueryPlanner::new().plan(&request.query, request.intent);
        if !request.routes.is_empty() {
            plan.routes.clone_from(&request.routes);
            plan.required_views = required_views_for_routes(&request.routes);
            plan.reasons
                .push("caller supplied an explicit retrieval route override".to_owned());
        }
        let mut missing_capabilities = missing_views(&manifest, &plan);
        if !verified_current {
            missing_capabilities.push(
                "results come from the last complete snapshot and are not verified against the current worktree"
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
                let contribution = lexical_route_weight(plan.intent, &hit.representation)
                    * lexical_source_weight(plan.intent, hit.address.as_ref())
                    / (RRF_K + rank as f64);
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
                    contribution,
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
                let contribution =
                    specialized_route_weight(route, &hit.representation) / (RRF_K + rank as f64);
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
                    contribution,
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
                        self.embedding_backend().await?,
                    ) {
                        let index = self.dense_index(digest)?;
                        let dense_hits = index
                            .search(&request.query, embedder, request.limit.saturating_mul(3))
                            .await?;
                        let dense_document_ids = dense_hits
                            .iter()
                            .map(|hit| hit.document_id.clone())
                            .collect::<Vec<_>>();
                        let documents = self
                            .store()
                            .documents_by_ids(&request.snapshot_id, &dense_document_ids)?
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
                            add_candidate(
                                &mut candidates,
                                SearchHit {
                                    document_id: document.document_id.clone(),
                                    entity_id: document.entity_id.clone(),
                                    symbol_name: document.symbol_name.clone(),
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

        if plan.graph_policy != GraphPolicy::None {
            let seeds = graph_seeds(&candidates, 8);
            for (offset, discovery) in self
                .graph_discoveries(&request.snapshot_id, &seeds, plan.graph_policy)?
                .into_iter()
                .enumerate()
            {
                let rank = offset + 1;
                let Some(entity) = self
                    .store()
                    .entity_by_id(&request.snapshot_id, &discovery.entity_id)?
                else {
                    continue;
                };
                if plan.graph_policy == GraphPolicy::ArchitectureBoundary
                    && entity
                        .capabilities
                        .iter()
                        .any(|capability| capability == "scip_external_symbol")
                {
                    continue;
                }
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
                        score: discovery.score,
                        contributing_routes: vec![SearchRoute::Structural],
                        address: entity.address,
                        evidence: discovery.evidence,
                        snippet,
                        verified_current: true,
                        explanation: vec![format!(
                            "bounded direction-preserving {:?} graph path (depth {}, score {:.3}): {}",
                            plan.graph_policy,
                            discovery.depth,
                            discovery.score,
                            discovery.steps.join(" -> ")
                        )],
                    },
                    graph_route_weight(plan.graph_policy) * discovery.score / (RRF_K + rank as f64),
                );
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

        if plan.rerank {
            if let Some(reranker) = self.reranker_backend().await? {
                let rerank_candidates = ranked_candidates(&candidates)
                    .into_iter()
                    .take(request.limit.saturating_mul(4).clamp(20, 100))
                    .map(|candidate| {
                        let path = candidate
                            .hit
                            .address
                            .as_ref()
                            .map(|address| address.path.as_str())
                            .unwrap_or("<generated>");
                        (
                            candidate.hit.document_id.clone(),
                            format!(
                                "path: {path}\nsymbol: {}\nrepresentation: {:?}\nroutes: {:?}\n{}",
                                candidate.hit.symbol_name.as_deref().unwrap_or("<unknown>"),
                                candidate.hit.representation,
                                candidate.hit.contributing_routes,
                                candidate.hit.snippet
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                let documents = rerank_candidates
                    .iter()
                    .map(|(_, document)| document.clone())
                    .collect::<Vec<_>>();
                for (rank, (index, raw_score)) in reranker
                    .rerank(&request.query, &documents)
                    .await?
                    .into_iter()
                    .enumerate()
                {
                    let Some((document_id, _)) = rerank_candidates.get(index) else {
                        return Err(cce_core::CceError::Provider(
                            "local reranker returned an invalid candidate index".to_owned(),
                        ));
                    };
                    if let Some(candidate) = candidates.get_mut(document_id) {
                        let rerank = rank + 1;
                        candidate.fused_score += RERANK_RRF_WEIGHT / (RRF_K + rerank as f64);
                        if !candidate
                            .hit
                            .contributing_routes
                            .contains(&SearchRoute::Reranked)
                        {
                            candidate
                                .hit
                                .contributing_routes
                                .push(SearchRoute::Reranked);
                        }
                        candidate.hit.explanation.push(format!(
                            "local cross-encoder rerank via {} (rank {rerank}, logit {raw_score:.4})",
                            reranker.profile()
                        ));
                    }
                }
            } else {
                missing_capabilities.push(
                    "learned reranker is not configured; results use deterministic reciprocal-rank fusion"
                        .to_owned(),
                );
            }
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
            hit.verified_current = verified_current;
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

    fn graph_discoveries(
        &self,
        snapshot_id: &str,
        seeds: &[GraphSeed],
        policy: GraphPolicy,
    ) -> Result<Vec<GraphDiscovery>> {
        let (direction, max_depth, per_node, maximum) = match policy {
            GraphPolicy::OutgoingTrace => (RelationDirection::Outgoing, 3, 24, 160),
            GraphPolicy::IncomingImpact => (RelationDirection::Incoming, 3, 24, 160),
            GraphPolicy::ArchitectureBoundary => (RelationDirection::Both, 2, 32, 160),
            GraphPolicy::DataflowRequired => (RelationDirection::Outgoing, 4, 24, 200),
            GraphPolicy::None => return Ok(Vec::new()),
        };
        let mut visited = seeds
            .iter()
            .map(|seed| (seed.entity_id.clone(), seed.score))
            .collect::<HashMap<_, _>>();
        let mut frontier = seeds
            .iter()
            .map(|seed| GraphDiscovery {
                entity_id: seed.entity_id.clone(),
                score: seed.score,
                depth: 0,
                steps: Vec::new(),
                evidence: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut output = HashMap::<String, GraphDiscovery>::new();
        for depth in 1..=max_depth {
            let mut next = HashMap::<String, GraphDiscovery>::new();
            let frontier_ids = frontier
                .iter()
                .map(|discovery| discovery.entity_id.clone())
                .collect::<Vec<_>>();
            let relations = self.store().relations_for_entities(
                snapshot_id,
                &frontier_ids,
                direction,
                per_node,
            )?;
            for current in &frontier {
                for relation in relations.get(&current.entity_id).into_iter().flatten() {
                    if !relation_allowed(policy, relation) {
                        continue;
                    }
                    let neighbor = if relation.source_entity_id == current.entity_id {
                        &relation.target_entity_id
                    } else {
                        &relation.source_entity_id
                    };
                    let score = current.score
                        * f64::from(relation.confidence)
                        * relation_weight(policy, relation)
                        * 0.86;
                    if score <= 0.02
                        || visited
                            .get(neighbor)
                            .is_some_and(|previous| *previous >= score)
                    {
                        continue;
                    }
                    let mut steps = current.steps.clone();
                    steps.push(format!(
                        "{:?}/{:?} {:.2}",
                        relation.kind, relation.origin, relation.confidence
                    ));
                    let mut evidence = current.evidence.clone();
                    merge_evidence(&mut evidence, &relation.evidence, 16);
                    let discovery = GraphDiscovery {
                        entity_id: neighbor.clone(),
                        score,
                        depth,
                        steps,
                        evidence,
                    };
                    next.entry(neighbor.clone())
                        .and_modify(|existing| {
                            if discovery.score > existing.score {
                                existing.clone_from(&discovery);
                            }
                        })
                        .or_insert(discovery);
                }
            }
            frontier = next.into_values().collect();
            frontier.sort_by(|left, right| right.score.total_cmp(&left.score));
            for discovery in &frontier {
                visited.insert(discovery.entity_id.clone(), discovery.score);
            }
            for discovery in &frontier {
                output
                    .entry(discovery.entity_id.clone())
                    .and_modify(|existing| {
                        if discovery.score > existing.score {
                            existing.clone_from(discovery);
                        }
                    })
                    .or_insert_with(|| discovery.clone());
            }
            if output.len() >= maximum || frontier.is_empty() {
                break;
            }
        }
        let mut output = output.into_values().collect::<Vec<_>>();
        output.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.depth.cmp(&right.depth))
                .then_with(|| left.entity_id.cmp(&right.entity_id))
        });
        output.truncate(maximum);
        Ok(output)
    }
}

#[derive(Debug, Clone)]
struct GraphDiscovery {
    entity_id: String,
    score: f64,
    depth: usize,
    steps: Vec<String>,
    evidence: Vec<SourceAddress>,
}

#[derive(Debug, Clone)]
struct GraphSeed {
    entity_id: String,
    score: f64,
}

fn graph_seeds(candidates: &HashMap<String, Candidate>, maximum: usize) -> Vec<GraphSeed> {
    let mut exact = candidates
        .values()
        .filter(|candidate| {
            candidate
                .hit
                .contributing_routes
                .contains(&SearchRoute::ExactSymbol)
        })
        .collect::<Vec<_>>();
    exact.sort_by_key(|candidate| candidate.hit.rank);
    let mut ordered = exact;
    ordered.extend(ranked_candidates(candidates));
    let mut seen = std::collections::HashSet::new();
    let mut seeds = Vec::new();
    for candidate in ordered {
        if !seen.insert(candidate.hit.entity_id.clone()) {
            continue;
        }
        let score = if candidate
            .hit
            .contributing_routes
            .contains(&SearchRoute::ExactSymbol)
        {
            (1.0 / (1.0 + 0.28 * candidate.hit.rank.saturating_sub(1) as f64)).max(0.55)
        } else {
            match candidate.hit.representation {
                RetrievalRepresentation::RawCode | RetrievalRepresentation::TestBehavior => 0.78,
                RetrievalRepresentation::Signature => 0.72,
                RetrievalRepresentation::KnowledgePage
                | RetrievalRepresentation::ModuleSummary
                | RetrievalRepresentation::RoleSummary => 0.52,
                _ => 0.62,
            }
        };
        seeds.push(GraphSeed {
            entity_id: candidate.hit.entity_id.clone(),
            score,
        });
        if seeds.len() == maximum {
            break;
        }
    }
    seeds
}

const fn graph_route_weight(policy: GraphPolicy) -> f64 {
    match policy {
        GraphPolicy::ArchitectureBoundary | GraphPolicy::DataflowRequired => 2.0,
        GraphPolicy::IncomingImpact => 1.7,
        GraphPolicy::OutgoingTrace => 1.55,
        GraphPolicy::None => 0.0,
    }
}

fn lexical_route_weight(intent: QueryIntent, representation: &RetrievalRepresentation) -> f64 {
    match (intent, representation) {
        (
            QueryIntent::Architecture,
            RetrievalRepresentation::KnowledgePage
            | RetrievalRepresentation::ModuleSummary
            | RetrievalRepresentation::RoleSummary,
        ) => 0.12,
        (QueryIntent::Architecture, RetrievalRepresentation::RawCode) => 1.2,
        (QueryIntent::History, RetrievalRepresentation::CommitSummary) => 0.1,
        (QueryIntent::History, _) => 0.75,
        _ => 1.0,
    }
}

fn lexical_source_weight(intent: QueryIntent, address: Option<&SourceAddress>) -> f64 {
    if intent != QueryIntent::Architecture {
        return 1.0;
    }
    let Some(path) = address.map(|address| address.path.to_ascii_lowercase()) else {
        return 0.8;
    };
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        Some(
            "rs" | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "go" | "java" | "kt" | "kts"
            | "py" | "pyi" | "js" | "jsx" | "ts" | "tsx" | "cs" | "fs" | "fsx" | "rb" | "php"
            | "swift" | "scala" | "sh" | "bash" | "zsh" | "fish",
        ) => 1.35,
        Some("md" | "mdx" | "rst" | "txt" | "adoc") => 0.45,
        _ => 0.82,
    }
}

fn specialized_route_weight(route: SearchRoute, representation: &RetrievalRepresentation) -> f64 {
    match (route, representation) {
        (SearchRoute::Knowledge, RetrievalRepresentation::KnowledgePage) => 1.05,
        (SearchRoute::Knowledge, RetrievalRepresentation::ModuleSummary) => 0.82,
        (SearchRoute::Knowledge, RetrievalRepresentation::RoleSummary) => 0.58,
        (SearchRoute::History, RetrievalRepresentation::CommitSummary) => 1.6,
        _ => 1.0,
    }
}

fn relation_allowed(policy: GraphPolicy, relation: &Relation) -> bool {
    if policy == GraphPolicy::DataflowRequired {
        return matches!(
            relation.kind,
            RelationKind::DataFlowsTo | RelationKind::TaintFlowsTo | RelationKind::ControlFlowsTo
        );
    }
    if policy == GraphPolicy::ArchitectureBoundary {
        return matches!(
            relation.kind,
            RelationKind::Contains
                | RelationKind::Defines
                | RelationKind::Imports
                | RelationKind::Exports
                | RelationKind::Implements
                | RelationKind::Extends
                | RelationKind::BuildDependsOn
                | RelationKind::RouteHandledBy
                | RelationKind::PublishesEvent
                | RelationKind::SubscribesEvent
                | RelationKind::PersistsTo
        );
    }
    !matches!(relation.kind, RelationKind::Semantic)
        || relation.origin != RelationOrigin::ModelInference
}

fn relation_weight(policy: GraphPolicy, relation: &Relation) -> f64 {
    let origin = match relation.origin {
        RelationOrigin::Compiler | RelationOrigin::Scip => 1.0,
        RelationOrigin::Lsp | RelationOrigin::BuildSystem => 0.9,
        RelationOrigin::StaticAnalysis => 1.0,
        RelationOrigin::FrameworkRule => 0.78,
        RelationOrigin::TreeSitter => 0.72,
        RelationOrigin::ModelInference => 0.35,
    };
    let kind = match relation.kind {
        RelationKind::Calls
        | RelationKind::References
        | RelationKind::Implements
        | RelationKind::Extends
        | RelationKind::DataFlowsTo
        | RelationKind::TaintFlowsTo => 1.0,
        RelationKind::Imports
        | RelationKind::BuildDependsOn
        | RelationKind::RouteHandledBy
        | RelationKind::PublishesEvent
        | RelationKind::SubscribesEvent => 0.9,
        RelationKind::Contains if policy == GraphPolicy::ArchitectureBoundary => 0.94,
        RelationKind::Contains => 0.48,
        RelationKind::ControlFlowsTo => 0.85,
        _ => 0.76,
    };
    origin * kind
}

fn merge_evidence(target: &mut Vec<SourceAddress>, source: &[SourceAddress], maximum: usize) {
    for address in source {
        if target.len() >= maximum {
            break;
        }
        if !target.contains(address) {
            target.push(address.clone());
        }
    }
}

fn add_candidate(candidates: &mut HashMap<String, Candidate>, hit: SearchHit, contribution: f64) {
    candidates
        .entry(hit.document_id.clone())
        .and_modify(|candidate| {
            let replace_snippet = should_replace_snippet(&candidate.hit, &hit);
            candidate.fused_score += contribution;
            for route in &hit.contributing_routes {
                if !candidate.hit.contributing_routes.contains(route) {
                    candidate.hit.contributing_routes.push(*route);
                }
            }
            candidate.hit.explanation.extend(hit.explanation.clone());
            if replace_snippet {
                candidate.hit.snippet.clone_from(&hit.snippet);
            }
        })
        .or_insert(Candidate {
            hit,
            fused_score: contribution,
        });
}

fn should_replace_snippet(existing: &SearchHit, incoming: &SearchHit) -> bool {
    let existing_is_lexical = existing.contributing_routes.contains(&SearchRoute::Lexical);
    let incoming_is_lexical = incoming.contributing_routes.contains(&SearchRoute::Lexical);
    (incoming_is_lexical && !existing_is_lexical)
        || (!existing_is_lexical
            && !incoming_is_lexical
            && incoming.snippet.len() > existing.snippet.len())
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
        "from",
        "to",
        "trace",
        "flow",
        "在哪",
        "定义",
        "哪里",
        "谁",
        "引用",
        "怎么",
        "如何",
    ];
    let compound = query
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
    let mut tokens = compound.clone();
    tokens.extend(compound.iter().flat_map(|token| {
        token
            .split([':', '.', '#'])
            .filter(|part| part.chars().count() >= 3)
            .map(str::to_owned)
    }));
    tokens.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    tokens.dedup();
    tokens.truncate(8);
    tokens
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_queries_also_seed_the_precise_member() {
        let tokens = entity_tokens("What calls CceEngine::embedding_backend?");
        assert!(tokens.contains(&"CceEngine::embedding_backend".to_owned()));
        assert!(tokens.contains(&"embedding_backend".to_owned()));
        assert!(tokens.contains(&"CceEngine".to_owned()));
    }

    #[test]
    fn architecture_does_not_double_reward_role_summaries() {
        assert!(
            lexical_route_weight(
                QueryIntent::Architecture,
                &RetrievalRepresentation::RoleSummary
            ) < specialized_route_weight(
                SearchRoute::Knowledge,
                &RetrievalRepresentation::RoleSummary
            )
        );
    }

    #[test]
    fn architecture_prefers_implementation_source_over_documentation_mentions() {
        let source = SourceAddress {
            repository_id: "repo".to_owned(),
            snapshot_id: "snapshot".to_owned(),
            path: "src/retrieval.rs".to_owned(),
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            symbol_id: None,
        };
        let mut documentation = source.clone();
        documentation.path = "docs/architecture.md".to_owned();
        assert!(
            lexical_source_weight(QueryIntent::Architecture, Some(&source))
                > lexical_source_weight(QueryIntent::Architecture, Some(&documentation))
        );
        assert_eq!(
            lexical_source_weight(QueryIntent::Impact, Some(&documentation)),
            1.0
        );
    }

    #[test]
    fn lexical_query_window_is_not_replaced_by_a_long_dense_prefix() {
        let hit = |route, snippet: &str| SearchHit {
            document_id: "document".to_owned(),
            symbol_name: Some("search".to_owned()),
            entity_id: "entity".to_owned(),
            representation: RetrievalRepresentation::RawCode,
            route,
            rank: 1,
            score: 1.0,
            contributing_routes: vec![route],
            address: None,
            evidence: Vec::new(),
            snippet: snippet.to_owned(),
            verified_current: true,
            explanation: Vec::new(),
        };
        let lexical = hit(SearchRoute::Lexical, "query-centered lexical window");
        let dense = hit(SearchRoute::DenseRaw, &"function prefix ".repeat(100));
        assert!(!should_replace_snippet(&lexical, &dense));
        assert!(should_replace_snippet(&dense, &lexical));
    }
}
