use std::collections::{HashMap, HashSet};

use cce_core::{
    ContextItem, ContextItemKind, ContextPack, ContextProvenance, QueryIntent, Result,
    SearchRequest, SearchRoute, Uncertainty,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{CceEngine, SearchResult};
use cce_core::{DeliveryReport, OmissionReason, OmittedHit, WitnessVerification};

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
        // The last rejection reason per hit — a hit rejected in role
        // preselection can still ship in rank-order fill, so omissions
        // are only final once both passes are done.
        let mut last_rejection = HashMap::<&str, OmissionReason>::new();
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
                match render_item(
                    hit,
                    role,
                    search,
                    budget_tokens,
                    used_tokens,
                    &mut seen_entities,
                    &mut seen_ranges,
                    &mut ranges_per_file,
                ) {
                    Ok(item) => {
                        used_tokens += item.estimated_tokens;
                        taken.insert(hit.document_id.clone());
                        items.push(item);
                    }
                    Err(reason) => {
                        last_rejection.insert(hit.document_id.as_str(), reason);
                    }
                }
            }
        }
        for (role, hit) in &classified {
            if taken.contains(hit.document_id.as_str()) {
                continue;
            }
            match render_item(
                hit,
                *role,
                search,
                budget_tokens,
                used_tokens,
                &mut seen_entities,
                &mut seen_ranges,
                &mut ranges_per_file,
            ) {
                Ok(item) => {
                    used_tokens += item.estimated_tokens;
                    taken.insert(hit.document_id.clone());
                    items.push(item);
                }
                Err(reason) => {
                    last_rejection.insert(hit.document_id.as_str(), reason);
                }
            }
        }
        let mut uncertainties: Vec<Uncertainty> = search
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
        // Weak-witness reasons are evidence gaps, not missing views —
        // surface them under a fixed evidence capability so consumers
        // cannot confuse "the index is unsure" with "a view failed".
        if search.verdict.state == cce_core::VerdictState::WeakWitness {
            for reason in &search.verdict.reasons {
                if uncertainties.iter().any(|gap| gap.message == *reason) {
                    continue;
                }
                uncertainties.push(Uncertainty {
                    capability: "evidence_witness".to_owned(),
                    message: reason.clone(),
                    recommended_action: Some(
                        "inspect the cited evidence or run a drill-down query".to_owned(),
                    ),
                });
            }
        }
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
            search_latency_ms: Some(search.latency_ms),
            search_verdict: Some(search.verdict.clone()),
            delivery_report: Some(delivery_report(
                &classified,
                &taken,
                &last_rejection,
                &items,
                &search.verdict.claim.distinguishing_terms,
            )),
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
        // `latency_ms` must cover the whole call — search, backlink
        // expansion, and packing. The search-only segment stays on
        // `search_latency_ms`.
        let started = std::time::Instant::now();
        let mut search = self
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
        // Delivery-granularity decision (not retrieval): the surfaced
        // evidence points at its subject — a top test calls the function
        // that decides the queried behavior. Each backlink lands right
        // after its pointer so the packer's caller quota delivers the
        // pointed-at code beside the evidence.
        let mut backlinks = self.subject_backlinks(&search)?;
        backlinks.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        for (index, hit) in backlinks {
            search.hits.insert((index + 1).min(search.hits.len()), hit);
        }
        for (rank, hit) in search.hits.iter_mut().enumerate() {
            hit.rank = rank + 1;
        }
        let mut pack = ContextPacker::new().pack(&search, request.budget_tokens);
        pack.latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
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
) -> std::result::Result<ContextItem, OmissionReason> {
    // Admission checks are read-only: a rejected candidate must not
    // consume range, entity, or per-file quota, or later evidence that
    // would fit gets starved by state from items that never shipped.
    // One item per source range: a symbol's summary, signature, and raw
    // chunks are the same evidence wearing different representations, so
    // whichever ranks highest wins the slot.
    let range_key = hit
        .address
        .as_ref()
        .map(|address| (address.path.clone(), address.start_byte, address.end_byte));
    if range_key
        .as_ref()
        .is_some_and(|key| seen_ranges.contains(key))
    {
        return Err(OmissionReason::DuplicateRange);
    }
    if let Some(path) = hit.address.as_ref().map(|address| &address.path) {
        if ranges_per_file.get(path).copied().unwrap_or(0) >= 4 {
            return Err(OmissionReason::FileCap);
        }
    }
    if hit.address.is_none() && seen_entities.contains(&hit.entity_id) {
        return Err(OmissionReason::DuplicateEntity);
    }
    let body = render_hit(hit);
    let tokens = estimate_tokens(&body);
    if tokens > budget_tokens.saturating_sub(used_tokens) {
        return Err(OmissionReason::Budget);
    }
    // Admitted — commit all three quotas now that the item ships.
    if let Some(key) = range_key {
        seen_ranges.insert(key);
    }
    if let Some(path) = hit.address.as_ref().map(|address| &address.path) {
        *ranges_per_file.entry(path.clone()).or_default() += 1;
    }
    seen_entities.insert(hit.entity_id.clone());
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
    Ok(ContextItem {
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

/// What the pack shipped versus what retrieval found. Term coverage is
/// computed only over shipped evidence snippets — the orientation text,
/// item titles, provenance strings, and source tails that never shipped
/// are not delivery evidence.
fn delivery_report(
    classified: &[(ContextRole, &cce_core::SearchHit)],
    taken: &HashSet<String>,
    last_rejection: &HashMap<&str, OmissionReason>,
    items: &[ContextItem],
    distinguishing_terms: &[String],
) -> DeliveryReport {
    let included_item_ids = items
        .iter()
        .filter(|item| item.kind != ContextItemKind::Orientation)
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let omitted_hits = classified
        .iter()
        .filter(|(_, hit)| !taken.contains(hit.document_id.as_str()))
        .map(|(_, hit)| OmittedHit {
            document_id: hit.document_id.clone(),
            // Every untaken hit passed through render_item in the fill
            // pass, so a reason always exists; Budget is the safe label
            // if a future pass ever skips an attempt.
            reason: last_rejection
                .get(hit.document_id.as_str())
                .copied()
                .unwrap_or(OmissionReason::Budget),
        })
        .collect();
    let delivered_snippets = classified
        .iter()
        .filter(|(_, hit)| taken.contains(hit.document_id.as_str()))
        .map(|(_, hit)| hit.snippet.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    let (delivered_terms, missing_terms): (Vec<String>, Vec<String>) = distinguishing_terms
        .iter()
        .cloned()
        .partition(|term| delivered_snippets.contains(term));
    // The packer observes shipped bodies only: it can state that no
    // source-backed evidence shipped, but it can never promote that to a
    // completeness claim — definitions/relations stay `not_verified`.
    let shipped_source_evidence = classified
        .iter()
        .any(|(_, hit)| taken.contains(hit.document_id.as_str()) && hit.address.is_some());
    let witness_verification = if shipped_source_evidence {
        WitnessVerification::NotVerified
    } else {
        WitnessVerification::NoSourceEvidence
    };
    DeliveryReport {
        included_item_ids,
        omitted_hits,
        delivered_terms,
        missing_terms,
        witness_verification,
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
    use cce_core::{
        ClaimFrame, ClaimPredicate, EvidenceTiers, QueryFilters, SearchHit, SourceAddress,
        VerdictState, WitnessReport, WitnessRequirement,
    };

    use crate::QueryPlan;
    use cce_core::{QueryIntent, SearchVerdict, ViewManifest};

    #[test]
    fn token_estimate_penalizes_code_punctuation() {
        assert!(estimate_tokens("fn x() { y(); }") > estimate_tokens("plain prose text"));
    }

    fn hit(
        document_id: &str,
        entity_id: &str,
        representation: cce_core::RetrievalRepresentation,
        address: Option<SourceAddress>,
        snippet_len: usize,
    ) -> SearchHit {
        SearchHit {
            document_id: document_id.to_owned(),
            entity_id: entity_id.to_owned(),
            region_id: None,
            symbol_name: Some(entity_id.to_owned()),
            representation,
            route: SearchRoute::Lexical,
            rank: 0,
            score: 1.0,
            contributing_routes: vec![SearchRoute::Lexical],
            address,
            evidence: Vec::new(),
            snippet: "x".repeat(snippet_len),
            verified_current: true,
            explanation: vec!["fixture".to_owned()],
        }
    }

    fn address(path: &str, start: u64, end: u64) -> Option<SourceAddress> {
        SourceAddress::new("repo_t", "snap_t", path, start..end, 1..=2).ok()
    }

    fn search_result(hits: Vec<SearchHit>) -> SearchResult {
        search_result_with_verdict(hits, VerdictState::default(), Vec::new(), Vec::new())
    }

    fn search_result_with_verdict(
        hits: Vec<SearchHit>,
        state: VerdictState,
        reasons: Vec<String>,
        distinguishing_terms: Vec<String>,
    ) -> SearchResult {
        SearchResult {
            request: SearchRequest {
                repository_id: "repo_t".to_owned(),
                snapshot_id: "snap_t".to_owned(),
                query: "fixture query".to_owned(),
                intent: None,
                limit: 50,
                require_fresh: false,
                routes: Vec::new(),
                filters: QueryFilters::default(),
            },
            plan: QueryPlan {
                intent: QueryIntent::NaturalLanguageBehavior,
                routes: vec![SearchRoute::Lexical],
                graph_policy: cce_core::GraphPolicy::Opportunistic,
                required_views: Vec::new(),
                reasons: Vec::new(),
            },
            manifest: ViewManifest {
                repository_id: "repo_t".to_owned(),
                snapshot_id: "snap_t".to_owned(),
                views: std::collections::BTreeMap::new(),
            },
            hits,
            missing_capabilities: Vec::new(),
            verdict: SearchVerdict {
                state,
                reasons,
                claim: ClaimFrame {
                    intent: QueryIntent::NaturalLanguageBehavior,
                    predicate: ClaimPredicate::Lookup,
                    required_witness: WitnessRequirement::Any,
                    subjects: Vec::new(),
                    distinguishing_terms,
                },
                witness: WitnessReport::default(),
                evidence_tiers: EvidenceTiers::default(),
                drill_downs: Vec::new(),
            },
            latency_ms: 0,
        }
    }

    fn packed_ids(pack: &ContextPack) -> Vec<&str> {
        pack.items.iter().map(|item| item.id.as_str()).collect()
    }

    /// Probe-pack with unlimited budget to learn the real token cost of a
    /// hit set — budgets are derived from measured tokens, not magic numbers.
    fn measured_tokens(search: &SearchResult, id: &str) -> usize {
        let probe = ContextPacker::new().pack(search, usize::MAX / 2);
        if id == "orientation" {
            probe
                .items
                .iter()
                .find(|item| item.kind == ContextItemKind::Orientation)
                .map_or(0, |item| item.estimated_tokens)
        } else {
            probe
                .items
                .iter()
                .find(|item| item.id == id)
                .map_or(0, |item| item.estimated_tokens)
        }
    }

    #[test]
    fn rejected_long_candidates_do_not_starve_short_evidence() {
        // Four oversized candidates in one file fail the budget check; a
        // later short candidate in the same file must still pack — the
        // rejections must not have consumed the four-slot file quota.
        let mut hits: Vec<SearchHit> = (0..4)
            .map(|index| {
                hit(
                    &format!("long_{index}"),
                    &format!("entity_{index}"),
                    cce_core::RetrievalRepresentation::RawCode,
                    address("src/a.rs", index * 100, index * 100 + 50),
                    200_000,
                )
            })
            .collect();
        hits.push(hit(
            "short",
            "entity_short",
            cce_core::RetrievalRepresentation::RawCode,
            address("src/a.rs", 900, 950),
            40,
        ));
        let search = search_result(hits);
        let pack = ContextPacker::new().pack(&search, 4_000);
        let ids = packed_ids(&pack);
        assert!(
            ids.contains(&"short"),
            "short evidence must pack after four rejected longs: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.starts_with("long_")),
            "rejected candidates must not appear: {ids:?}"
        );
    }

    #[test]
    fn failed_long_representation_frees_the_range_for_a_short_one() {
        // Same entity, same range: the long representation fails the budget,
        // the short representation of the identical range must still pack.
        let range = address("src/lib.rs", 0, 500);
        let hits = vec![
            hit(
                "long_repr",
                "entity_a",
                cce_core::RetrievalRepresentation::RawCode,
                range.clone(),
                200_000,
            ),
            hit(
                "short_repr",
                "entity_a",
                cce_core::RetrievalRepresentation::SymbolSummary,
                range,
                40,
            ),
        ];
        let search = search_result(hits);
        let pack = ContextPacker::new().pack(&search, 4_000);
        let ids = packed_ids(&pack);
        assert_eq!(
            ids.iter().filter(|id| **id == "short_repr").count(),
            1,
            "short representation of the range must pack: {ids:?}"
        );
        assert!(!ids.contains(&"long_repr"));
    }

    #[test]
    fn failed_addressless_candidate_does_not_consume_entity_dedup() {
        // Entity dedup for addressless hits: a long addressless candidate
        // fails on budget; a later short hit on the same entity must still
        // be admitted — the entity key was never committed.
        let hits = vec![
            hit(
                "long_addrless",
                "entity_b",
                cce_core::RetrievalRepresentation::KnowledgePage,
                None,
                200_000,
            ),
            hit(
                "short_addrless",
                "entity_b",
                cce_core::RetrievalRepresentation::KnowledgePage,
                None,
                40,
            ),
        ];
        let search = search_result(hits);
        let pack = ContextPacker::new().pack(&search, 4_000);
        let ids = packed_ids(&pack);
        assert!(
            ids.contains(&"short_addrless"),
            "same-entity short hit must pack: {ids:?}"
        );
        assert!(!ids.contains(&"long_addrless"));
    }

    #[test]
    fn admitted_quota_rules_still_hold() {
        // Successful duplicate range stays excluded; the fifth distinct
        // range in one file still hits the four-slot cap.
        let mut hits: Vec<SearchHit> = (0..4)
            .map(|index| {
                hit(
                    &format!("file_a_{index}"),
                    &format!("entity_a_{index}"),
                    cce_core::RetrievalRepresentation::RawCode,
                    address("src/a.rs", index * 100, index * 100 + 50),
                    40,
                )
            })
            .collect();
        hits.push(hit(
            "dup_range",
            "entity_dup",
            cce_core::RetrievalRepresentation::RawCode,
            address("src/a.rs", 0, 50),
            40,
        ));
        hits.push(hit(
            "fifth_range",
            "entity_fifth",
            cce_core::RetrievalRepresentation::RawCode,
            address("src/a.rs", 500, 550),
            40,
        ));
        let search = search_result(hits);
        let pack = ContextPacker::new().pack(&search, usize::MAX / 2);
        let ids = packed_ids(&pack);
        for index in 0..4 {
            assert!(ids.contains(&format!("file_a_{index}").as_str()));
        }
        assert!(!ids.contains(&"dup_range"), "duplicate range rejected");
        assert!(!ids.contains(&"fifth_range"), "file cap still enforced");
    }

    #[test]
    fn role_preselection_rejection_does_not_pollute_rank_fill() {
        // A Test-role candidate that fails the budget during role
        // preselection must not leave its range committed — the rank-order
        // fill pass still sees the range as free for a shorter hit.
        let hits = vec![
            hit(
                "long_test",
                "entity_t",
                cce_core::RetrievalRepresentation::TestBehavior,
                address("tests/t.rs", 0, 400),
                200_000,
            ),
            hit(
                "target_hit",
                "entity_u",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/u.rs", 0, 100),
                40,
            ),
        ];
        let search = search_result(hits);
        let pack = ContextPacker::new().pack(&search, 4_000);
        let ids = packed_ids(&pack);
        assert!(ids.contains(&"target_hit"), "rank fill unaffected: {ids:?}");
        assert!(!ids.contains(&"long_test"));
    }

    #[test]
    fn budget_edges_are_exact() {
        let hits = vec![
            hit(
                "a",
                "entity_a",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/a.rs", 0, 50),
                40,
            ),
            hit(
                "b",
                "entity_b",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/b.rs", 0, 50),
                40,
            ),
        ];
        let search = search_result(hits);

        // Zero budget: nothing packs, not even orientation.
        let pack = ContextPacker::new().pack(&search, 0);
        assert!(pack.items.is_empty() && pack.used_tokens == 0);

        // Orientation alone consumes the budget exactly.
        let orientation_tokens = measured_tokens(&search, "orientation");
        let pack = ContextPacker::new().pack(&search, orientation_tokens);
        assert_eq!(pack.items.len(), 1);
        assert_eq!(pack.items[0].kind, ContextItemKind::Orientation);
        assert_eq!(pack.used_tokens, orientation_tokens);

        // Exact fill: orientation + first hit precisely exhausts the budget.
        let a_tokens = measured_tokens(&search, "a");
        let exact = orientation_tokens + a_tokens;
        let pack = ContextPacker::new().pack(&search, exact);
        let ids = packed_ids(&pack);
        assert!(
            ids.contains(&"a") && !ids.contains(&"b"),
            "exact fill: {ids:?}"
        );
        assert_eq!(pack.used_tokens, exact);
        assert!(pack.used_tokens <= pack.budget_tokens);

        // used_tokens always equals the sum of item estimates.
        let sum: usize = pack.items.iter().map(|item| item.estimated_tokens).sum();
        assert_eq!(sum, pack.used_tokens);
    }

    #[test]
    fn search_verdict_passes_through_verbatim() {
        let search = search_result_with_verdict(
            vec![hit(
                "a",
                "entity_a",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/a.rs", 0, 50),
                40,
            )],
            VerdictState::WeakWitness,
            vec!["no artifact binds the claim's distinguishing terms".to_owned()],
            vec!["zebra".to_owned()],
        );
        let pack = ContextPacker::new().pack(&search, 4_000);
        let verdict = pack.search_verdict.expect("searchVerdict");
        assert_eq!(verdict, search.verdict, "verdict must be verbatim");
        assert_eq!(verdict.state, VerdictState::WeakWitness);
    }

    #[test]
    fn weak_witness_reasons_surface_as_evidence_uncertainties() {
        let search = search_result_with_verdict(
            vec![hit(
                "a",
                "entity_a",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/a.rs", 0, 50),
                40,
            )],
            VerdictState::WeakWitness,
            vec![
                "no artifact binds the claim's distinguishing terms".to_owned(),
                "no artifact binds the claim's distinguishing terms".to_owned(),
            ],
            Vec::new(),
        );
        let pack = ContextPacker::new().pack(&search, 4_000);
        let witness_gaps = pack
            .uncertainties
            .iter()
            .filter(|gap| gap.capability == "evidence_witness")
            .count();
        assert_eq!(
            witness_gaps, 1,
            "deduped, one witness gap: {:?}",
            pack.uncertainties
        );
        // It is an evidence gap, never a view failure.
        assert!(pack.missing_capabilities.is_empty());
        assert!(
            !pack
                .uncertainties
                .iter()
                .any(|gap| gap.capability == "lexical")
        );
    }

    #[test]
    fn sole_evidence_cut_by_budget_reports_the_gap() {
        // The only hit carrying the claim term is too big to ship:
        // delivered_terms empty, missing_terms reports it, omission
        // reason is budget, and nothing source-backed shipped.
        let hits = vec![hit(
            "sole",
            "entity_sole",
            cce_core::RetrievalRepresentation::RawCode,
            address("src/sole.rs", 0, 400),
            200_000,
        )];
        let search = search_result_with_verdict(
            hits,
            VerdictState::WeakWitness,
            Vec::new(),
            vec!["zebra".to_owned()],
        );
        let pack = ContextPacker::new().pack(&search, 4_000);
        let report = pack.delivery_report.expect("delivery report");
        assert!(report.included_item_ids.is_empty());
        assert_eq!(report.delivered_terms, Vec::<String>::new());
        assert_eq!(report.missing_terms, vec!["zebra".to_owned()]);
        assert_eq!(
            report.omitted_hits,
            vec![OmittedHit {
                document_id: "sole".to_owned(),
                reason: OmissionReason::Budget,
            }]
        );
        assert_eq!(
            report.witness_verification,
            WitnessVerification::NoSourceEvidence
        );
    }

    #[test]
    fn orientation_and_titles_never_count_as_term_coverage() {
        // A distinguishing term that appears only in the pack's
        // orientation/title furniture — never in a shipped snippet — must
        // stay missing. Orientation text carries routes/rationale, not
        // evidence.
        let hits = vec![hit(
            "filler",
            "entity_f",
            cce_core::RetrievalRepresentation::RawCode,
            address("src/f.rs", 0, 50),
            40,
        )];
        let mut search = search_result_with_verdict(
            hits,
            VerdictState::WeakWitness,
            Vec::new(),
            vec!["lexical".to_owned()],
        );
        // "lexical" literally appears in the orientation body (route
        // debug listing) — if orientation counted, the term would read
        // delivered.
        assert!(orientation(&search).to_lowercase().contains("lexical"));
        search.verdict.claim.distinguishing_terms = vec!["lexical".to_owned()];
        let pack = ContextPacker::new().pack(&search, 4_000);
        let report = pack.delivery_report.expect("delivery report");
        assert_eq!(
            report.missing_terms,
            vec!["lexical".to_owned()],
            "orientation text must not satisfy term coverage"
        );
    }

    #[test]
    fn term_in_undelivered_source_tail_is_not_delivered() {
        // The hit ships, but its snippet is the truncated prefix — a
        // claim term that exists only past the snippet boundary was not
        // actually delivered.
        let hits = vec![hit(
            "truncated",
            "entity_t",
            cce_core::RetrievalRepresentation::RawCode,
            address("src/t.rs", 0, 50_000),
            40, // snippet: "xxxx..." — no "zebra"
        )];
        let search = search_result_with_verdict(
            hits,
            VerdictState::Answered,
            Vec::new(),
            vec!["zebra".to_owned()],
        );
        let pack = ContextPacker::new().pack(&search, 4_000);
        let report = pack.delivery_report.expect("delivery report");
        assert_eq!(report.included_item_ids, vec!["truncated".to_owned()]);
        assert_eq!(report.missing_terms, vec!["zebra".to_owned()]);
        assert_eq!(report.delivered_terms, Vec::<String>::new());
        // Source-backed evidence did ship — gaps are reported, nothing
        // promoted to a completeness claim.
        assert_eq!(
            report.witness_verification,
            WitnessVerification::NotVerified
        );
    }

    #[test]
    fn delivered_terms_match_shipped_snippets() {
        let hits = vec![
            hit(
                "carries",
                "entity_c",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/c.rs", 0, 50),
                0,
            ),
            hit(
                "other",
                "entity_o",
                cce_core::RetrievalRepresentation::RawCode,
                address("src/o.rs", 0, 50),
                0,
            ),
        ];
        let mut search = search_result(hits);
        search.hits[0].snippet = "fn handle_zebra_reconnect()".to_owned();
        search.hits[1].snippet = "unrelated body".to_owned();
        search.verdict.claim.distinguishing_terms =
            vec!["zebra".to_owned(), "absent_term".to_owned()];
        let pack = ContextPacker::new().pack(&search, 4_000);
        let report = pack.delivery_report.expect("delivery report");
        assert_eq!(report.delivered_terms, vec!["zebra".to_owned()]);
        assert_eq!(report.missing_terms, vec!["absent_term".to_owned()]);
    }
}
