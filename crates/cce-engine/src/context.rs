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
        let mut navigation_summary_tokens = 0_usize;
        let mut navigation_summaries = 0_usize;
        let mut architecture_test_items = 0_usize;
        let mut architecture_example_items = 0_usize;
        let query = search.request.query.to_ascii_lowercase();
        let explicitly_requests_tests = ["test", "tests", "spec", "benchmark", "测试", "基准"]
            .iter()
            .any(|term| query.contains(term));
        let explicitly_requests_examples = ["example", "examples", "demo", "sample", "示例"]
            .iter()
            .any(|term| query.contains(term));
        // Generated module/role/knowledge pages are useful orientation, but they must
        // never crowd source evidence out of a context pack. Twenty percent leaves a
        // deterministic source budget even when summaries rank highly.
        let navigation_summary_budget = budget_tokens.saturating_div(5).min(2_048);
        let maximum_ranges_per_file = if search.plan.intent == QueryIntent::PreciseDataflow {
            12
        } else {
            4
        };
        for hit in &search.hits {
            let address_path = hit.address.as_ref().map(|address| address.path.as_str());
            let counts_as_architecture_test = search.plan.intent == QueryIntent::Architecture
                && !explicitly_requests_tests
                && address_path.is_some_and(is_test_path);
            let counts_as_architecture_example = search.plan.intent == QueryIntent::Architecture
                && !explicitly_requests_examples
                && address_path.is_some_and(is_example_path);
            if counts_as_architecture_test && architecture_test_items >= 3 {
                continue;
            }
            if counts_as_architecture_example && architecture_example_items >= 2 {
                continue;
            }
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
            if is_navigation_summary(hit) {
                if navigation_summaries >= 3
                    || navigation_summary_tokens.saturating_add(tokens) > navigation_summary_budget
                {
                    continue;
                }
            }
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
            if counts_as_architecture_test {
                architecture_test_items += 1;
            }
            if counts_as_architecture_example {
                architecture_example_items += 1;
            }
            if is_navigation_summary(hit) {
                navigation_summaries += 1;
                navigation_summary_tokens += tokens;
            }
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
            trajectory_id: search.trajectory_id.clone(),
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

fn is_test_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.contains("/__tests__/")
        || path.contains("/tests/")
        || path.contains("/test/")
        || path.contains(".spec.")
        || path.contains(".test.")
}

fn is_example_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.starts_with("examples/") || path.contains("/examples/")
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
        let pack = ContextPacker::new().pack(&search, request.budget_tokens);
        if self.config().capture_learning_data
            && let Some(trajectory_id) = &pack.trajectory_id
        {
            let shown = pack
                .items
                .iter()
                .filter(|item| item.id != "orientation")
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            if let Err(error) = self.store().record_shown_documents(trajectory_id, &shown) {
                tracing::warn!(%error, "failed to capture context-pack exposure events");
            }
        }
        Ok(pack)
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

fn is_navigation_summary(hit: &cce_core::SearchHit) -> bool {
    matches!(
        hit.representation,
        cce_core::RetrievalRepresentation::RoleSummary
            | cce_core::RetrievalRepresentation::ModuleSummary
            | cce_core::RetrievalRepresentation::KnowledgePage
    )
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
    use cce_core::{
        RetrievalRepresentation, SearchHit, SearchRequest, SearchRoute, SourceAddress, ViewManifest,
    };

    #[test]
    fn token_estimate_penalizes_code_punctuation() {
        assert!(estimate_tokens("fn x() { y(); }") > estimate_tokens("plain prose text"));
    }

    #[test]
    fn navigation_summaries_cannot_consume_the_source_budget() {
        let request = SearchRequest {
            repository_id: "repo".to_owned(),
            snapshot_id: "snapshot".to_owned(),
            query: "architecture".to_owned(),
            intent: Some(QueryIntent::Architecture),
            limit: 20,
            require_fresh: true,
            routes: Vec::new(),
        };
        let mut hits = (0..8)
            .map(|index| SearchHit {
                document_id: format!("summary-{index}"),
                entity_id: format!("summary-entity-{index}"),
                symbol_name: None,
                representation: RetrievalRepresentation::ModuleSummary,
                route: SearchRoute::Knowledge,
                rank: index + 1,
                score: 1.0,
                contributing_routes: vec![SearchRoute::Knowledge],
                address: None,
                evidence: Vec::new(),
                snippet: "generated navigation summary ".repeat(80),
                verified_current: true,
                explanation: vec!["summary".to_owned()],
            })
            .collect::<Vec<_>>();
        hits.push(SearchHit {
            document_id: "source".to_owned(),
            entity_id: "source-entity".to_owned(),
            symbol_name: Some("implementation".to_owned()),
            representation: RetrievalRepresentation::RawCode,
            route: SearchRoute::Lexical,
            rank: 9,
            score: 0.5,
            contributing_routes: vec![SearchRoute::Lexical],
            address: None,
            evidence: Vec::new(),
            snippet: "fn implementation() {}".to_owned(),
            verified_current: true,
            explanation: vec!["source".to_owned()],
        });
        let result = SearchResult {
            trajectory_id: None,
            request,
            plan: crate::QueryPlanner::new().plan("architecture", Some(QueryIntent::Architecture)),
            manifest: ViewManifest {
                repository_id: "repo".to_owned(),
                snapshot_id: "snapshot".to_owned(),
                views: Default::default(),
            },
            hits,
            missing_capabilities: Vec::new(),
            latency_ms: 1,
        };
        let pack = ContextPacker::new().pack(&result, 2_000);

        assert!(pack.items.iter().any(|item| item.id == "source"));
        assert!(
            pack.items
                .iter()
                .filter(|item| item.id.starts_with("summary-"))
                .count()
                <= 3
        );
    }

    #[test]
    fn architecture_pack_bounds_unrequested_examples_and_tests() {
        let request = SearchRequest {
            repository_id: "repo".to_owned(),
            snapshot_id: "snapshot".to_owned(),
            query: "explain the cancellation pipeline".to_owned(),
            intent: Some(QueryIntent::Architecture),
            limit: 20,
            require_fresh: true,
            routes: Vec::new(),
        };
        let hit = |index: usize, path: String| SearchHit {
            document_id: format!("document-{index}"),
            entity_id: format!("entity-{index}"),
            symbol_name: Some(format!("symbol-{index}")),
            representation: RetrievalRepresentation::RawCode,
            route: SearchRoute::Lexical,
            rank: index + 1,
            score: 1.0,
            contributing_routes: vec![SearchRoute::Lexical],
            address: Some(SourceAddress {
                repository_id: "repo".to_owned(),
                snapshot_id: "snapshot".to_owned(),
                path,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 1,
                symbol_id: None,
            }),
            evidence: Vec::new(),
            snippet: "implementation".to_owned(),
            verified_current: true,
            explanation: vec!["source".to_owned()],
        };
        let mut hits = (0..6)
            .map(|index| hit(index, format!("examples/demo-{index}/index.ts")))
            .collect::<Vec<_>>();
        hits.extend((6..12).map(|index| hit(index, format!("src/tests/pipeline-{index}.test.ts"))));
        hits.push(hit(12, "src/pipeline.ts".to_owned()));
        let result = SearchResult {
            trajectory_id: None,
            request,
            plan: crate::QueryPlanner::new().plan(
                "explain the cancellation pipeline",
                Some(QueryIntent::Architecture),
            ),
            manifest: ViewManifest {
                repository_id: "repo".to_owned(),
                snapshot_id: "snapshot".to_owned(),
                views: Default::default(),
            },
            hits,
            missing_capabilities: Vec::new(),
            latency_ms: 1,
        };

        let pack = ContextPacker::new().pack(&result, 8_000);

        assert_eq!(
            pack.items
                .iter()
                .filter_map(|item| item.provenance.source_address.as_ref())
                .filter(|address| is_example_path(&address.path))
                .count(),
            2
        );
        assert_eq!(
            pack.items
                .iter()
                .filter_map(|item| item.provenance.source_address.as_ref())
                .filter(|address| is_test_path(&address.path))
                .count(),
            3
        );
        assert!(pack.items.iter().any(|item| {
            item.provenance
                .source_address
                .as_ref()
                .is_some_and(|address| address.path == "src/pipeline.ts")
        }));
    }
}
