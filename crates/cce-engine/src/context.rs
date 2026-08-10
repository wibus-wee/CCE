use std::collections::{HashMap, HashSet};

use cce_core::{
    ContextItem, ContextItemKind, ContextPack, ContextProvenance, QueryIntent, Result,
    SearchRequest, SearchRoute, Uncertainty,
};
use serde::{Deserialize, Serialize};

use crate::{CceEngine, SearchResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRequest {
    pub query: String,
    pub intent: Option<QueryIntent>,
    pub budget_tokens: usize,
    pub max_candidates: usize,
    pub require_fresh: bool,
}

impl ContextRequest {
    #[must_use]
    pub fn new(query: impl Into<String>, budget_tokens: usize) -> Self {
        Self {
            query: query.into(),
            intent: None,
            budget_tokens,
            max_candidates: 50,
            require_fresh: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextPacker;

impl ContextPacker {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn pack(&self, search: &SearchResult, budget_tokens: usize) -> ContextPack {
        let orientation = orientation(search);
        let orientation_tokens = estimate_tokens(&orientation);
        let mut items = Vec::new();
        let mut used_tokens = 0_usize;
        if orientation_tokens <= budget_tokens {
            items.push(ContextItem {
                id: "orientation".to_owned(),
                kind: ContextItemKind::Orientation,
                title: "Repository context orientation".to_owned(),
                body: orientation,
                estimated_tokens: orientation_tokens,
                provenance: ContextProvenance {
                    why_retrieved: "planner and view-manifest orientation".to_owned(),
                    route: SearchRoute::Hybrid,
                    rank: 0,
                    score: 1.0,
                    snapshot_id: search.request.snapshot_id.clone(),
                    verified_current: true,
                    symbol_name: None,
                    source_address: None,
                    evidence_addresses: Vec::new(),
                },
            });
            used_tokens += orientation_tokens;
        }

        let mut seen_entities = HashSet::new();
        let mut seen_ranges = HashSet::new();
        let mut ranges_per_file = HashMap::<String, usize>::new();
        let maximum_ranges_per_file = if search.plan.intent == QueryIntent::PreciseDataflow {
            12
        } else {
            4
        };
        for hit in &search.hits {
            let range_key = hit.address.as_ref().map(|address| {
                (
                    address.path.clone(),
                    address.start_byte,
                    address.end_byte,
                    hit.representation.clone(),
                )
            });
            if range_key
                .as_ref()
                .is_some_and(|key| !seen_ranges.insert(key.clone()))
            {
                continue;
            }
            if let Some(path) = hit.address.as_ref().map(|address| &address.path) {
                let count = ranges_per_file.entry(path.clone()).or_default();
                if *count >= maximum_ranges_per_file {
                    continue;
                }
                *count += 1;
            }
            let first_entity_occurrence = seen_entities.insert(hit.entity_id.clone());
            if !first_entity_occurrence && hit.address.is_none() {
                continue;
            }
            let body = render_hit(hit);
            let tokens = estimate_tokens(&body);
            if tokens > budget_tokens.saturating_sub(used_tokens) {
                continue;
            }
            let kind = match hit.representation {
                cce_core::RetrievalRepresentation::TestBehavior => ContextItemKind::Test,
                cce_core::RetrievalRepresentation::CommitSummary => ContextItemKind::History,
                cce_core::RetrievalRepresentation::RoleSummary
                | cce_core::RetrievalRepresentation::ModuleSummary
                | cce_core::RetrievalRepresentation::FlowSummary
                | cce_core::RetrievalRepresentation::KnowledgePage => ContextItemKind::Knowledge,
                _ if hit.route == SearchRoute::Structural => ContextItemKind::RelationPath,
                _ if hit.route == SearchRoute::ExactSymbol => ContextItemKind::EntryPoint,
                _ => ContextItemKind::Source,
            };
            items.push(ContextItem {
                id: hit.document_id.clone(),
                kind,
                title: hit.address.as_ref().map_or_else(
                    || hit.entity_id.clone(),
                    |address| {
                        format!(
                            "{}:{}-{}",
                            address.path, address.start_line, address.end_line
                        )
                    },
                ),
                body,
                estimated_tokens: tokens,
                provenance: ContextProvenance {
                    why_retrieved: hit.explanation.join("; "),
                    route: hit.route,
                    rank: hit.rank,
                    score: hit.score,
                    snapshot_id: search.request.snapshot_id.clone(),
                    verified_current: hit.verified_current,
                    symbol_name: hit.symbol_name.clone(),
                    source_address: hit.address.clone(),
                    evidence_addresses: hit.evidence.clone(),
                },
            });
            used_tokens += tokens;
        }
        let uncertainties = search
            .missing_capabilities
            .iter()
            .map(|message| Uncertainty {
                capability: capability_from_message(message),
                message: message.clone(),
                recommended_action: Some(
                    "verify against source or build the missing view".to_owned(),
                ),
            })
            .collect();
        ContextPack {
            repository_id: search.request.repository_id.clone(),
            snapshot_id: search.request.snapshot_id.clone(),
            query: search.request.query.clone(),
            intent: search.plan.intent,
            budget_tokens,
            used_tokens,
            items,
            uncertainties,
            missing_capabilities: search.missing_capabilities.clone(),
        }
    }
}

impl CceEngine {
    pub async fn context(&self, request: ContextRequest) -> Result<ContextPack> {
        let search = self
            .search(SearchRequest {
                repository_id: String::new(),
                snapshot_id: String::new(),
                query: request.query,
                intent: request.intent,
                limit: request.max_candidates,
                require_fresh: request.require_fresh,
                routes: Vec::new(),
            })
            .await?;
        Ok(ContextPacker::new().pack(&search, request.budget_tokens))
    }
}

fn orientation(search: &SearchResult) -> String {
    let routes = search
        .plan
        .routes
        .iter()
        .map(|route| format!("{route:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let views = search
        .manifest
        .views
        .iter()
        .map(|(kind, status)| format!("{kind}={:?}", status.state))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Snapshot: {}\nIntent: {:?}\nRoutes: {}\nViews: {}\nPlanner rationale: {}\nAll summaries and derived relations are navigation aids; cited source is authoritative.",
        search.request.snapshot_id,
        search.plan.intent,
        routes,
        views,
        search.plan.reasons.join("; ")
    )
}

fn render_hit(hit: &cce_core::SearchHit) -> String {
    let source = hit.address.as_ref().map_or_else(
        || "no direct source range".to_owned(),
        |address| {
            format!(
                "{}:{}-{} bytes {}..{}",
                address.path,
                address.start_line,
                address.end_line,
                address.start_byte,
                address.end_byte
            )
        },
    );
    format!(
        "Source: {source}\nRoute: {:?}; routes: {:?}\nEvidence:\n{}",
        hit.route, hit.contributing_routes, hit.snippet
    )
}

fn estimate_tokens(value: &str) -> usize {
    let characters = value.chars().count();
    let code_punctuation = value
        .chars()
        .filter(|character| "{}[]();:,.<>/\\".contains(*character))
        .count();
    characters.saturating_add(code_punctuation) / 4 + 1
}

fn capability_from_message(message: &str) -> String {
    message
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_penalizes_code_punctuation() {
        assert!(estimate_tokens("fn x() { y(); }") > estimate_tokens("plain prose text"));
    }
}
