use std::collections::{HashMap, HashSet};

use cce_core::{
    ContextItem, ContextItemKind, ContextPack, ContextProvenance, QueryIntent, Result,
    SearchRequest, SearchRoute, Uncertainty,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{CceEngine, SearchResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Parameters for `CceEngine::context` — a token-budgeted context pack.
pub struct ContextRequest {
    /// Query/task text to gather context for.
    pub query: String,
    /// Optional pre-classified intent.
    pub intent: Option<QueryIntent>,
    /// Token budget for the pack.
    pub budget_tokens: usize,
    /// Search candidates feeding the packer.
    pub max_candidates: usize,
    /// Refuse results from stale snapshots.
    pub require_fresh: bool,
    /// Route allowlist; empty = planner chooses.
    #[serde(default)]
    pub routes: Vec<SearchRoute>,
}

impl ContextRequest {
    /// A request with default candidate/freshness settings.
    #[must_use]
    pub fn new(query: impl Into<String>, budget_tokens: usize) -> Self {
        Self {
            query: query.into(),
            intent: None,
            budget_tokens,
            max_candidates: 50,
            require_fresh: true,
            routes: Vec::new(),
        }
    }
}

/// Packs search hits into a `ContextPack` within a token budget.
#[derive(Debug, Clone, Default)]
pub struct ContextPacker;

impl ContextPacker {
    /// Create a packer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Assemble the pack: orientation first, then hits until budget runs
    /// out, preserving per-item provenance.
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

        // Role-aware packing: every evidence role that produced a hit keeps a
        // quota slot, then the remaining budget is filled in rank order. This
        // prevents a dominant top-K of raw targets from starving callers,
        // tests, config, and history that the task actually needs.
        let classified = search
            .hits
            .iter()
            .map(|hit| (role_of(hit), hit))
            .collect::<Vec<_>>();
        let mut seen_entities = HashSet::new();
        let mut seen_ranges = HashSet::new();
        let mut ranges_per_file = HashMap::<String, usize>::new();
        let mut taken = HashSet::new();
        for role in [
            ContextRole::Test,
            ContextRole::Contract,
            ContextRole::Caller,
            ContextRole::Config,
            ContextRole::History,
            ContextRole::Knowledge,
        ] {
            if let Some((_, hit)) = classified.iter().find(|(hit_role, hit)| {
                *hit_role == role && !taken.contains(hit.document_id.as_str())
            }) {
                if let Some(item) = render_item(
                    hit,
                    role,
                    search,
                    budget_tokens,
                    used_tokens,
                    &mut seen_entities,
                    &mut seen_ranges,
                    &mut ranges_per_file,
                ) {
                    used_tokens += item.estimated_tokens;
                    taken.insert(hit.document_id.clone());
                    items.push(item);
                }
            }
        }
        for (role, hit) in &classified {
            if taken.contains(hit.document_id.as_str()) {
                continue;
            }
            if let Some(item) = render_item(
                hit,
                *role,
                search,
                budget_tokens,
                used_tokens,
                &mut seen_entities,
                &mut seen_ranges,
                &mut ranges_per_file,
            ) {
                used_tokens += item.estimated_tokens;
                taken.insert(hit.document_id.clone());
                items.push(item);
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
            repository_id: search.request.repository_id.clone(),
            snapshot_id: search.request.snapshot_id.clone(),
            query: search.request.query.clone(),
            intent: search.plan.intent,
            plan_routes: search.plan.routes.clone(),
            graph_policy: Some(search.plan.graph_policy),
            budget_tokens,
            used_tokens,
            latency_ms: search.latency_ms,
            items,
            uncertainties,
            missing_capabilities: search.missing_capabilities.clone(),
        }
    }
}

impl CceEngine {
    /// Search then pack into a token-budgeted `ContextPack` with explicit
    /// uncertainty reporting.
    ///
    /// # Errors
    /// Propagates search/storage errors.
    pub async fn context(&self, request: ContextRequest) -> Result<ContextPack> {
        let search = self
            .search(SearchRequest {
                repository_id: String::new(),
                snapshot_id: String::new(),
                query: request.query,
                intent: request.intent,
                limit: request.max_candidates,
                require_fresh: request.require_fresh,
                routes: request.routes,
                filters: cce_core::QueryFilters::default(),
            })
            .await?;
        let mut pack = ContextPacker::new().pack(&search, request.budget_tokens);
        pack.latency_ms = search.latency_ms;
        Ok(pack)
    }
}

/// The task-facing role a hit plays in the delivered pack. Distinct from
/// `ContextItemKind` (wire format): roles drive quota selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextRole {
    Target,
    Contract,
    Caller,
    Test,
    Config,
    History,
    Knowledge,
}

fn role_of(hit: &cce_core::SearchHit) -> ContextRole {
    use cce_core::RetrievalRepresentation as Repr;
    match hit.representation {
        Repr::TestBehavior => ContextRole::Test,
        Repr::CommitSummary => ContextRole::History,
        Repr::RoleSummary | Repr::ModuleSummary | Repr::FlowSummary | Repr::KnowledgePage => {
            ContextRole::Knowledge
        }
        Repr::Signature if hit.route == SearchRoute::Structural => ContextRole::Caller,
        Repr::Signature => ContextRole::Contract,
        _ => {
            if hit.route == SearchRoute::Structural {
                ContextRole::Caller
            } else if hit.address.as_ref().is_some_and(|address| {
                let path = std::path::Path::new(&address.path);
                let config_extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        ["toml", "yaml", "yml", "lock"]
                            .iter()
                            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
                    });
                config_extension || path.file_name().is_some_and(|name| name == "package.json")
            }) {
                ContextRole::Config
            } else {
                ContextRole::Target
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_item(
    hit: &cce_core::SearchHit,
    role: ContextRole,
    search: &SearchResult,
    budget_tokens: usize,
    used_tokens: usize,
    seen_entities: &mut HashSet<String>,
    seen_ranges: &mut HashSet<(String, u64, u64)>,
    ranges_per_file: &mut HashMap<String, usize>,
) -> Option<ContextItem> {
    // One item per source range: a symbol's summary, signature, and raw
    // chunks are the same evidence wearing different representations, so
    // whichever ranks highest wins the slot.
    let range_key = hit
        .address
        .as_ref()
        .map(|address| (address.path.clone(), address.start_byte, address.end_byte));
    if range_key
        .as_ref()
        .is_some_and(|key| !seen_ranges.insert(key.clone()))
    {
        return None;
    }
    if let Some(path) = hit.address.as_ref().map(|address| &address.path) {
        let count = ranges_per_file.entry(path.clone()).or_default();
        if *count >= 4 {
            return None;
        }
        *count += 1;
    }
    let first_entity_occurrence = seen_entities.insert(hit.entity_id.clone());
    if !first_entity_occurrence && hit.address.is_none() {
        return None;
    }
    let body = render_hit(hit);
    let tokens = estimate_tokens(&body);
    if tokens > budget_tokens.saturating_sub(used_tokens) {
        return None;
    }
    let kind = match role {
        ContextRole::Test => ContextItemKind::Test,
        ContextRole::History => ContextItemKind::History,
        ContextRole::Knowledge => ContextItemKind::Knowledge,
        ContextRole::Caller => ContextItemKind::RelationPath,
        ContextRole::Contract => ContextItemKind::Contract,
        ContextRole::Config => ContextItemKind::Config,
        ContextRole::Target if hit.route == SearchRoute::ExactSymbol => ContextItemKind::EntryPoint,
        ContextRole::Target => ContextItemKind::Source,
    };
    Some(ContextItem {
        id: hit.document_id.clone(),
        kind,
        title: hit.address.as_ref().map_or_else(
            || {
                hit.symbol_name
                    .clone()
                    .unwrap_or_else(|| hit.entity_id.clone())
            },
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
    })
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
