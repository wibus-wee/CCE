use std::{collections::HashMap, time::Instant};

use cce_core::{
    CodeEntity, EntityKind, QueryIntent, Relation, RelationKind, RelationOrigin, Result,
    RetrievalRepresentation, SearchHit, SearchRequest, SearchRoute, SourceAddress, ViewKind,
    ViewManifest, ViewState,
};
use cce_store::RelationDirection;
use serde::{Deserialize, Serialize};

use crate::{CceEngine, Embedder, GraphPolicy, QueryPlan, QueryPlanner};

const RRF_K: f64 = 60.0;
const RERANK_RRF_WEIGHT: f64 = 2.75;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<String>,
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
                let mut entities = self.store().entity_by_name(
                    &request.snapshot_id,
                    &token,
                    request.limit.saturating_mul(8).clamp(32, 512),
                )?;
                entities.sort_by(|left, right| {
                    exact_entity_priority(right, plan.intent)
                        .cmp(&exact_entity_priority(left, plan.intent))
                        .then_with(|| exact_entity_span(right).cmp(&exact_entity_span(left)))
                        .then_with(|| left.id.cmp(&right.id))
                });
                for entity in entities.into_iter().take(request.limit) {
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
            let lexical_limit = request.limit.saturating_mul(3);
            let lexical_pool = self.store().lexical_search(
                &request.snapshot_id,
                &request.query,
                lexical_limit.saturating_mul(4),
            )?;
            let mut lexical_pool = lexical_pool.into_iter().enumerate().collect::<Vec<_>>();
            lexical_pool.sort_by(|(left_rank, left), (right_rank, right)| {
                architecture_lexical_priority(plan.intent, right, &request.query, *right_rank)
                    .total_cmp(&architecture_lexical_priority(
                        plan.intent,
                        left,
                        &request.query,
                        *left_rank,
                    ))
                    .then_with(|| left_rank.cmp(right_rank))
            });
            let mut hits_per_path = HashMap::<String, usize>::new();
            let maximum_hits_per_path = if plan.intent == QueryIntent::Architecture {
                2
            } else {
                4
            };
            let lexical_hits = lexical_pool
                .into_iter()
                .map(|(_, hit)| hit)
                .filter(|hit| {
                    let Some(path) = hit.address.as_ref().map(|address| &address.path) else {
                        return true;
                    };
                    let count = hits_per_path.entry(path.clone()).or_default();
                    if *count >= maximum_hits_per_path {
                        return false;
                    }
                    *count += 1;
                    true
                })
                .take(lexical_limit);
            for (offset, hit) in lexical_hits.enumerate() {
                let rank = offset + 1;
                let contribution = lexical_route_weight(plan.intent, &hit.representation)
                    * lexical_source_weight(plan.intent, hit.address.as_ref(), &request.query)
                    * lexical_path_affinity(plan.intent, hit.address.as_ref(), &request.query)
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

            if plan.intent == QueryIntent::Architecture {
                let mut seen_paths = std::collections::HashSet::new();
                let path_hits = self.store().lexical_path_search(
                    &request.snapshot_id,
                    &request.query,
                    request.limit.saturating_mul(32).clamp(128, 4_096),
                )?;
                for (offset, hit) in path_hits
                    .into_iter()
                    .filter(|hit| {
                        lexical_path_affinity(plan.intent, hit.address.as_ref(), &request.query)
                            > 1.0
                    })
                    .filter(|hit| {
                        hit.address
                            .as_ref()
                            .is_none_or(|address| seen_paths.insert(address.path.clone()))
                    })
                    .take(request.limit)
                    .enumerate()
                {
                    let rank = offset + 1;
                    let contribution = 1.15
                        * lexical_source_weight(plan.intent, hit.address.as_ref(), &request.query)
                        * lexical_path_affinity(plan.intent, hit.address.as_ref(), &request.query)
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
                            explanation: vec![
                                "SQLite FTS5 query-to-path morphology match".to_owned(),
                            ],
                        },
                        contribution,
                    );
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
                &[RetrievalRepresentation::CommitSummary][..],
            ),
        ] {
            if !plan.routes.contains(&route) {
                continue;
            }
            let hits = self.store().lexical_search_representations(
                &request.snapshot_id,
                &request.query,
                accepted,
                request.limit.saturating_mul(3),
            )?;
            for (offset, hit) in hits.into_iter().enumerate() {
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
                                dense_route_weight(plan.intent)
                                    * representation_weight(&document.representation)
                                    * lexical_source_weight(
                                        plan.intent,
                                        document.address.as_ref(),
                                        &request.query,
                                    )
                                    * lexical_path_affinity(
                                        plan.intent,
                                        document.address.as_ref(),
                                        &request.query,
                                    )
                                    / (RRF_K + rank as f64),
                            );
                        }
                    }
                }
            }
        }

        let pregraph_hits = ranked_candidates(&candidates)
            .into_iter()
            .map(|candidate| candidate.hit.clone())
            .collect::<Vec<_>>();
        let selectively_abstained =
            should_selectively_abstain(plan.intent, &request.query, &pregraph_hits);

        if plan.graph_policy != GraphPolicy::None && !selectively_abstained {
            let seed_limit = match plan.graph_policy {
                GraphPolicy::ArchitectureBoundary => 4,
                GraphPolicy::IncomingImpact => 6,
                GraphPolicy::OutgoingTrace | GraphPolicy::DataflowRequired => 8,
                GraphPolicy::None => 0,
            };
            let seeds = graph_seeds(&candidates, seed_limit);
            let discoveries =
                self.graph_discoveries(&request.snapshot_id, &seeds, plan.graph_policy)?;
            let entity_ids = discoveries
                .iter()
                .map(|discovery| discovery.entity_id.clone())
                .collect::<Vec<_>>();
            let mut entities = self
                .store()
                .entities_by_ids(&request.snapshot_id, &entity_ids)?
                .into_iter()
                .map(|entity| (entity.id.clone(), entity))
                .collect::<HashMap<_, _>>();
            let addressed_entities = entities
                .values()
                .filter(|entity| {
                    !(plan.graph_policy == GraphPolicy::ArchitectureBoundary
                        && entity
                            .capabilities
                            .iter()
                            .any(|capability| capability == "scip_external_symbol"))
                })
                .filter_map(|entity| {
                    entity
                        .address
                        .clone()
                        .map(|address| (entity.id.clone(), address))
                })
                .collect::<Vec<_>>();
            let source_addresses = addressed_entities
                .iter()
                .map(|(_, address)| address.clone())
                .collect::<Vec<_>>();
            let source_by_entity = addressed_entities
                .into_iter()
                .zip(self.store().source_texts(&source_addresses)?)
                .filter_map(|((entity_id, _), source)| source.map(|source| (entity_id, source)))
                .collect::<HashMap<_, _>>();
            for (offset, discovery) in discoveries.into_iter().enumerate() {
                let rank = offset + 1;
                let Some(entity) = entities.remove(&discovery.entity_id) else {
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
                let snippet = source_by_entity
                    .get(&entity.id)
                    .cloned()
                    .filter(|source| !source.trim().is_empty())
                    .or_else(|| entity.signature.clone())
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

        if plan.graph_policy == GraphPolicy::DataflowRequired {
            match manifest.views.get(&ViewKind::Dataflow).map(|view| view.state) {
                Some(ViewState::Ready) => {}
                Some(ViewState::Partial) => missing_capabilities.push(
                    "precise dataflow is partially source-aligned; returned static-analysis paths are evidence-backed, but unmatched endpoints may omit valid paths"
                        .to_owned(),
                ),
                _ => missing_capabilities.push(
                    "precise dataflow is unavailable; CCE refuses to infer a source-to-sink path from embeddings"
                        .to_owned(),
                ),
            }
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
        if selectively_abstained || should_selectively_abstain(plan.intent, &request.query, &hits) {
            hits.clear();
            plan.reasons.push(
                "selective retrieval abstained because no distinctive query concept had source-backed support in the candidate set"
                    .to_owned(),
            );
        }
        hits.truncate(request.limit);
        for (offset, hit) in hits.iter_mut().enumerate() {
            hit.rank = offset + 1;
            hit.verified_current = verified_current;
        }
        let mut result = SearchResult {
            trajectory_id: None,
            request,
            plan,
            manifest,
            hits,
            missing_capabilities,
            latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        };
        if self.config().capture_learning_data
            && let Err(error) = self.capture_search_result(&mut result)
        {
            tracing::warn!(%error, "failed to capture local CCE learning trajectory");
            result.trajectory_id = None;
        }
        Ok(result)
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
            GraphPolicy::ArchitectureBoundary => (RelationDirection::Both, 2, 12, 80),
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

fn exact_entity_priority(entity: &CodeEntity, intent: QueryIntent) -> i32 {
    let mut priority = 0_i32;
    if entity
        .capabilities
        .iter()
        .any(|capability| capability == "scip_definition")
    {
        priority += 100;
    }
    if entity
        .capabilities
        .iter()
        .any(|capability| capability == "syntax_fact")
    {
        priority += 80;
    }
    if entity
        .capabilities
        .iter()
        .any(|capability| capability == "precise_dataflow_node")
    {
        priority += if intent == QueryIntent::PreciseDataflow {
            60
        } else {
            5
        };
    }
    if matches!(
        entity.kind,
        EntityKind::Function
            | EntityKind::Method
            | EntityKind::Class
            | EntityKind::Interface
            | EntityKind::Trait
            | EntityKind::Struct
            | EntityKind::Enum
            | EntityKind::Module
            | EntityKind::Namespace
    ) {
        priority += 30;
    }
    priority
}

fn exact_entity_span(entity: &CodeEntity) -> u64 {
    entity.address.as_ref().map_or(0, |address| {
        address.end_byte.saturating_sub(address.start_byte)
    })
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

fn lexical_source_weight(intent: QueryIntent, address: Option<&SourceAddress>, query: &str) -> f64 {
    if intent != QueryIntent::Architecture {
        return 1.0;
    }
    let Some(path) = address.map(|address| address.path.to_ascii_lowercase()) else {
        return 0.8;
    };
    let query = query.to_ascii_lowercase();
    let explicitly_requests_tests = ["test", "tests", "spec", "benchmark", "测试", "基准"]
        .iter()
        .any(|term| query.contains(term));
    let explicitly_requests_examples = ["example", "examples", "demo", "sample", "示例"]
        .iter()
        .any(|term| query.contains(term));
    let path_role = if !explicitly_requests_examples
        && (path.starts_with("examples/") || path.contains("/examples/"))
    {
        0.32
    } else if explicitly_requests_tests {
        1.0
    } else if path.contains("/__tests__/")
        || path.contains("/tests/")
        || path.contains("/test/")
        || path.contains("/__benchmarks__/")
        || path.contains("/benchmarks/")
        || path.contains(".spec.")
        || path.contains(".test.")
        || path.contains(".bench.")
    {
        0.38
    } else if path.contains("/src/") {
        1.18
    } else {
        0.88
    };
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    path_role
        * match extension {
            Some(
                "rs" | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "go" | "java" | "kt"
                | "kts" | "py" | "pyi" | "js" | "jsx" | "ts" | "tsx" | "cs" | "fs" | "fsx" | "rb"
                | "php" | "swift" | "scala" | "sh" | "bash" | "zsh" | "fish",
            ) => 1.35,
            Some("md" | "mdx" | "rst" | "txt" | "adoc") => 0.45,
            _ => 0.82,
        }
}

fn lexical_path_affinity(intent: QueryIntent, address: Option<&SourceAddress>, query: &str) -> f64 {
    if intent != QueryIntent::Architecture {
        return 1.0;
    }
    let Some(path) = address.map(|address| address.path.to_ascii_lowercase()) else {
        return 1.0;
    };
    let Some(file_stem) = std::path::Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
    else {
        return 1.0;
    };
    let tokens = query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.chars().count() >= 4)
        .collect::<Vec<_>>();
    if tokens.iter().any(|token| {
        let common = token
            .chars()
            .zip(file_stem.chars())
            .take_while(|(left, right)| left == right)
            .count();
        let shorter = token.chars().count().min(file_stem.chars().count());
        common >= 4 && common.saturating_add(2) >= shorter
    }) {
        1.9
    } else if path
        .split(['/', '.', '-', '_'])
        .any(|part| tokens.iter().any(|token| token == part))
    {
        1.35
    } else {
        1.0
    }
}

fn architecture_lexical_priority(
    intent: QueryIntent,
    hit: &cce_store::LexicalHit,
    query: &str,
    original_rank: usize,
) -> f64 {
    lexical_source_weight(intent, hit.address.as_ref(), query)
        * lexical_path_affinity(intent, hit.address.as_ref(), query)
        / (RRF_K + original_rank.saturating_add(1) as f64)
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
            for address in &hit.evidence {
                if !candidate.hit.evidence.contains(address) {
                    candidate.hit.evidence.push(address.clone());
                }
            }
            if candidate.hit.address.is_none() {
                candidate.hit.address.clone_from(&hit.address);
            }
            if canonical_representation_route(&hit).is_some() {
                candidate.hit.route = hit.route;
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

const fn canonical_representation_route(hit: &SearchHit) -> Option<SearchRoute> {
    match (hit.route, &hit.representation) {
        (SearchRoute::History, RetrievalRepresentation::CommitSummary) => {
            Some(SearchRoute::History)
        }
        (
            SearchRoute::Knowledge,
            RetrievalRepresentation::KnowledgePage
            | RetrievalRepresentation::ModuleSummary
            | RetrievalRepresentation::RoleSummary,
        ) => Some(SearchRoute::Knowledge),
        _ => None,
    }
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

const fn dense_route_weight(intent: QueryIntent) -> f64 {
    match intent {
        QueryIntent::Architecture => 0.65,
        QueryIntent::Impact | QueryIntent::History => 0.82,
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

fn should_selectively_abstain(intent: QueryIntent, query: &str, hits: &[SearchHit]) -> bool {
    if !matches!(
        intent,
        QueryIntent::Architecture
            | QueryIntent::NaturalLanguageBehavior
            | QueryIntent::IssueLocalization
            | QueryIntent::Unknown
    ) || !query.is_ascii()
        || hits.iter().any(|hit| {
            hit.contributing_routes
                .iter()
                .any(|route| matches!(route, SearchRoute::ExactSymbol | SearchRoute::History))
        })
    {
        return false;
    }
    let terms = distinctive_query_terms(query);
    if terms.len() < 2 || hits.is_empty() {
        return false;
    }
    let sample = hits.iter().take(24).collect::<Vec<_>>();
    let common_threshold = (sample.len() / 3).max(3);
    let supported = terms.iter().filter(|term| {
        let occurrences = sample
            .iter()
            .filter(|hit| hit_supports_term(hit, term))
            .count();
        occurrences > 0 && occurrences < common_threshold
    });
    supported.count() == 0
}

fn distinctive_query_terms(query: &str) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "about",
        "change",
        "changed",
        "code",
        "configure",
        "configured",
        "configuration",
        "does",
        "explain",
        "from",
        "handling",
        "implemented",
        "implementation",
        "into",
        "logic",
        "must",
        "over",
        "project",
        "repository",
        "source",
        "through",
        "toolkit",
        "turn",
        "used",
        "using",
        "what",
        "where",
        "which",
    ];
    let mut terms = query
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .map(str::to_ascii_lowercase)
        .filter(|term| term.len() >= 4 && !GENERIC.contains(&term.as_str()))
        .map(|term| query_term_stem(&term))
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

fn query_term_stem(term: &str) -> String {
    for suffix in ["ation", "ing", "ied", "ed", "es", "s"] {
        if term.len() > suffix.len() + 3 && term.ends_with(suffix) {
            return term[..term.len() - suffix.len()].to_owned();
        }
    }
    term.to_owned()
}

fn hit_supports_term(hit: &SearchHit, term: &str) -> bool {
    let mut haystack = hit.snippet.to_ascii_lowercase();
    if let Some(symbol) = &hit.symbol_name {
        haystack.push(' ');
        haystack.push_str(&symbol.to_ascii_lowercase());
    }
    if let Some(address) = &hit.address {
        haystack.push(' ');
        haystack.push_str(&address.path.to_ascii_lowercase());
    }
    haystack
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|token| {
            if term.len() <= 4 {
                token == term
            } else {
                token.starts_with(term)
            }
        })
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
            lexical_source_weight(QueryIntent::Architecture, Some(&source), "architecture")
                > lexical_source_weight(
                    QueryIntent::Architecture,
                    Some(&documentation),
                    "architecture"
                )
        );
        assert_eq!(
            lexical_source_weight(QueryIntent::Impact, Some(&documentation), "impact"),
            1.0
        );

        let mut example = source.clone();
        example.path = "examples/demo/src/retrieval.ts".to_owned();
        assert!(
            lexical_source_weight(QueryIntent::Architecture, Some(&source), "architecture")
                > lexical_source_weight(QueryIntent::Architecture, Some(&example), "architecture")
        );
        assert!(
            lexical_source_weight(
                QueryIntent::Architecture,
                Some(&example),
                "example architecture"
            ) > lexical_source_weight(QueryIntent::Architecture, Some(&example), "architecture")
        );

        let mut test = source.clone();
        test.path = "src/__tests__/retrieval.spec.ts".to_owned();
        assert!(
            lexical_source_weight(QueryIntent::Architecture, Some(&source), "architecture")
                > lexical_source_weight(QueryIntent::Architecture, Some(&test), "architecture")
        );
        assert_eq!(
            lexical_source_weight(
                QueryIntent::Architecture,
                Some(&source),
                "test architecture"
            ),
            lexical_source_weight(QueryIntent::Architecture, Some(&test), "test architecture")
        );

        let mut parser = source.clone();
        parser.path = "packages/compiler-core/src/parser.ts".to_owned();
        assert!(
            lexical_path_affinity(QueryIntent::Architecture, Some(&parser), "parse templates")
                > lexical_path_affinity(
                    QueryIntent::Architecture,
                    Some(&source),
                    "parse templates"
                )
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

    #[test]
    fn fused_routes_retain_static_analysis_evidence() {
        let evidence = SourceAddress {
            repository_id: "repo".to_owned(),
            snapshot_id: "snapshot".to_owned(),
            path: "src/flow.ts".to_owned(),
            start_byte: 10,
            end_byte: 20,
            start_line: 2,
            end_line: 2,
            symbol_id: None,
        };
        let hit = |route, evidence: Vec<SourceAddress>| SearchHit {
            document_id: "entity: node".to_owned(),
            symbol_name: Some("node".to_owned()),
            entity_id: "node".to_owned(),
            representation: RetrievalRepresentation::Signature,
            route,
            rank: 1,
            score: 1.0,
            contributing_routes: vec![route],
            address: None,
            evidence,
            snippet: "node".to_owned(),
            verified_current: true,
            explanation: Vec::new(),
        };
        let mut candidates = HashMap::new();

        add_candidate(
            &mut candidates,
            hit(SearchRoute::ExactSymbol, Vec::new()),
            1.0,
        );
        add_candidate(
            &mut candidates,
            hit(SearchRoute::Structural, vec![evidence.clone()]),
            1.0,
        );

        assert_eq!(candidates["entity: node"].hit.evidence, vec![evidence]);
    }

    #[test]
    fn specialized_representation_route_survives_lexical_fusion() {
        let hit = |route| SearchHit {
            document_id: "commit".to_owned(),
            symbol_name: Some("fix history".to_owned()),
            entity_id: "repository".to_owned(),
            representation: RetrievalRepresentation::CommitSummary,
            route,
            rank: 1,
            score: 1.0,
            contributing_routes: vec![route],
            address: None,
            evidence: Vec::new(),
            snippet: "Commit: deadbeef".to_owned(),
            verified_current: true,
            explanation: Vec::new(),
        };
        let mut candidates = HashMap::new();

        add_candidate(&mut candidates, hit(SearchRoute::Lexical), 1.0);
        add_candidate(&mut candidates, hit(SearchRoute::History), 1.0);

        let fused = &candidates["commit"].hit;
        assert_eq!(fused.route, SearchRoute::History);
        assert_eq!(
            fused.contributing_routes,
            vec![SearchRoute::Lexical, SearchRoute::History]
        );
    }

    #[test]
    fn selective_abstention_rejects_unsupported_distinctive_concepts() {
        let hit = |index: usize| SearchHit {
            document_id: format!("document-{index}"),
            symbol_name: Some("configureStore".to_owned()),
            entity_id: format!("entity-{index}"),
            representation: RetrievalRepresentation::RawCode,
            route: SearchRoute::Lexical,
            rank: index + 1,
            score: 0.05,
            contributing_routes: vec![SearchRoute::Lexical],
            address: None,
            evidence: Vec::new(),
            snippet: "Redux quick store configuration".to_owned(),
            verified_current: true,
            explanation: Vec::new(),
        };
        let hits = (0..12).map(hit).collect::<Vec<_>>();

        assert!(!hit_supports_term(&hits[0], "quic"));

        assert!(should_selectively_abstain(
            QueryIntent::Architecture,
            "Where does Redux Toolkit configure lunar cheese replication over QUIC?",
            &hits
        ));

        let mut supported = hits;
        supported[0].snippet = "cache invalidation after a fulfilled endpoint".to_owned();
        assert!(!should_selectively_abstain(
            QueryIntent::Architecture,
            "How are fulfilled endpoint tags turned into cache invalidation?",
            &supported
        ));
    }

    #[test]
    fn selective_abstention_does_not_override_exact_or_non_ascii_queries() {
        let hit = SearchHit {
            document_id: "document".to_owned(),
            symbol_name: Some("symbol".to_owned()),
            entity_id: "entity".to_owned(),
            representation: RetrievalRepresentation::RawCode,
            route: SearchRoute::ExactSymbol,
            rank: 1,
            score: 1.0,
            contributing_routes: vec![SearchRoute::ExactSymbol],
            address: None,
            evidence: Vec::new(),
            snippet: "symbol".to_owned(),
            verified_current: true,
            explanation: Vec::new(),
        };

        assert!(!should_selectively_abstain(
            QueryIntent::Architecture,
            "unknown extraterrestrial protocol",
            std::slice::from_ref(&hit)
        ));
        assert!(!should_selectively_abstain(
            QueryIntent::Architecture,
            "月球奶酪复制在哪里？",
            &[SearchHit {
                contributing_routes: vec![SearchRoute::Lexical],
                route: SearchRoute::Lexical,
                ..hit
            }]
        ));
    }
}
