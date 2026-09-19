use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{GraphPolicy, QueryIntent, SearchRoute, SearchVerdict, SourceAddress};

/// The role a packed context item plays for the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemKind {
    /// High-level repository/task orientation.
    Orientation,
    /// Likely entry point for the task.
    EntryPoint,
    /// Verbatim source code.
    Source,
    /// API/type contract or signature.
    Contract,
    /// A path through the relation graph.
    RelationPath,
    /// Test code or behavior evidence.
    Test,
    /// Configuration or build metadata.
    Config,
    /// Commit/history evidence.
    History,
    /// Generated knowledge content.
    Knowledge,
}

/// Why and how a context item was retrieved, with source linkage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextProvenance {
    /// Human-readable reason this item was included.
    pub why_retrieved: String,
    /// Route that produced the item.
    pub route: SearchRoute,
    /// Rank in the underlying result list.
    pub rank: usize,
    /// Score assigned by the producing route.
    pub score: f64,
    /// Snapshot the item was retrieved from.
    pub snapshot_id: String,
    /// Whether content was verified against the current worktree.
    pub verified_current: bool,
    /// Symbol name when the item is symbol-shaped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    /// Primary source address backing the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_address: Option<SourceAddress>,
    /// Additional supporting source addresses.
    #[serde(default)]
    pub evidence_addresses: Vec<SourceAddress>,
}

/// A single packed context item with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    /// Item identifier (document id).
    pub id: String,
    /// Role the item plays.
    pub kind: ContextItemKind,
    /// Short display title.
    pub title: String,
    /// Item body text.
    pub body: String,
    /// Estimated token cost of the item.
    pub estimated_tokens: usize,
    /// Retrieval provenance.
    pub provenance: ContextProvenance,
}

/// A capability gap surfaced to the caller instead of silently degrading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Uncertainty {
    /// Name of the missing/partial capability.
    pub capability: String,
    /// Explanation of what is missing and why.
    pub message: String,
    /// What the caller could do to satisfy the capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<String>,
}

/// Why a retrieved hit did not ship inside the pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReason {
    /// Remaining token budget could not fit the rendered body.
    Budget,
    /// The same source range already shipped under another representation.
    DuplicateRange,
    /// The file's four-range packing cap was already spent.
    FileCap,
    /// The entity already shipped and this hit carried no distinct range.
    DuplicateEntity,
}

/// One retrieved hit that never made it into the pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OmittedHit {
    /// Document id of the omitted hit.
    pub document_id: String,
    /// Why it was omitted.
    pub reason: OmissionReason,
}

/// What the delivery layer can state about witness completeness — the
/// packer observes shipped bodies, not the corpus, so the vocabulary is
/// deliberately limited to gap statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WitnessVerification {
    /// Packing cannot re-run corpus witness checks; delivery gaps are
    /// reported, nothing is asserted complete.
    NotVerified,
    /// No source-backed evidence item shipped at all — every retrieved
    /// source citation was cut by budget or quota.
    NoSourceEvidence,
}

/// What this pack actually shipped versus what retrieval found — the
/// delivery gaps a corpus-level verdict cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryReport {
    /// Document ids that shipped as items.
    pub included_item_ids: Vec<String>,
    /// Hits that never shipped, each with its final admission reason.
    pub omitted_hits: Vec<OmittedHit>,
    /// Claim distinguishing terms present in delivered evidence snippets.
    pub delivered_terms: Vec<String>,
    /// Claim distinguishing terms absent from every delivered snippet.
    pub missing_terms: Vec<String>,
    /// Weak delivery-side verification statement — never a completeness
    /// claim.
    pub witness_verification: WitnessVerification,
}

/// A token-budgeted bundle of context items plus explicit uncertainty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextPack {
    /// Repository the pack was built from.
    pub repository_id: String,
    /// Snapshot the pack was built against.
    pub snapshot_id: String,
    /// Original query text.
    pub query: String,
    /// Classified query intent.
    pub intent: QueryIntent,
    /// Routes the executed plan selected; empty when unknown.
    #[serde(default)]
    pub plan_routes: Vec<SearchRoute>,
    /// Graph expansion policy applied, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_policy: Option<GraphPolicy>,
    /// Token budget requested.
    pub budget_tokens: usize,
    /// Tokens actually consumed by the packed items.
    pub used_tokens: usize,
    /// Engine-side wall time for the whole context call — search,
    /// backlink expansion, and packing — in ms. Distinct from
    /// caller-observed latency, which also includes transport.
    #[serde(default)]
    pub latency_ms: u64,
    /// Wall time of the search stage alone, when measured. Segment of
    /// `latency_ms`, never a replacement for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_latency_ms: Option<u64>,
    /// The search-stage evidence verdict, verbatim. `None` means the
    /// producer did not supply one — never an implicit `Answered`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_verdict: Option<SearchVerdict>,
    /// What this pack shipped versus what retrieval found — delivery
    /// gaps the corpus-level verdict cannot see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_report: Option<DeliveryReport>,
    /// Packed items in priority order.
    pub items: Vec<ContextItem>,
    /// Explicit capability gaps affecting this pack.
    #[serde(default)]
    pub uncertainties: Vec<Uncertainty>,
    /// Capability names that could not be satisfied at all.
    #[serde(default)]
    pub missing_capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ClaimFrame, ClaimPredicate, EvidenceTiers, VerdictState, WitnessReport, WitnessRequirement,
    };

    fn fixture_pack() -> ContextPack {
        ContextPack {
            repository_id: "repo".to_owned(),
            snapshot_id: "snap".to_owned(),
            query: "q".to_owned(),
            intent: QueryIntent::NaturalLanguageBehavior,
            plan_routes: Vec::new(),
            graph_policy: None,
            budget_tokens: 1024,
            used_tokens: 10,
            latency_ms: 5,
            search_latency_ms: Some(3),
            search_verdict: Some(SearchVerdict {
                state: VerdictState::WeakWitness,
                reasons: vec!["no artifact binds the claim".to_owned()],
                claim: ClaimFrame {
                    intent: QueryIntent::NaturalLanguageBehavior,
                    predicate: ClaimPredicate::Lookup,
                    required_witness: WitnessRequirement::Any,
                    subjects: Vec::new(),
                    distinguishing_terms: vec!["reconnect".to_owned()],
                },
                witness: WitnessReport::default(),
                evidence_tiers: EvidenceTiers::default(),
                drill_downs: vec![crate::DrillDown {
                    query: "reconnect implementation".to_owned(),
                    reason: "missing term".to_owned(),
                }],
            }),
            delivery_report: Some(DeliveryReport {
                included_item_ids: vec!["d1".to_owned()],
                omitted_hits: vec![OmittedHit {
                    document_id: "d2".to_owned(),
                    reason: OmissionReason::Budget,
                }],
                delivered_terms: Vec::new(),
                missing_terms: vec!["reconnect".to_owned()],
                witness_verification: WitnessVerification::NoSourceEvidence,
            }),
            items: Vec::new(),
            uncertainties: Vec::new(),
            missing_capabilities: Vec::new(),
        }
    }

    #[test]
    fn pack_round_trip_preserves_verdict_and_delivery() {
        let pack = fixture_pack();
        let json = serde_json::to_value(&pack).expect("serialize");
        // Wire names are camelCase; the verdict keeps its search shape.
        assert!(json.get("searchVerdict").is_some());
        assert!(json.get("deliveryReport").is_some());
        assert_eq!(
            json["deliveryReport"]["omittedHits"][0]["reason"],
            serde_json::json!("budget")
        );
        assert_eq!(
            json["deliveryReport"]["witnessVerification"],
            serde_json::json!("no_source_evidence")
        );
        let restored: ContextPack = serde_json::from_value(json).expect("deserialize");
        assert_eq!(restored, pack);
        let verdict = restored.search_verdict.expect("verdict");
        assert_eq!(verdict.state, VerdictState::WeakWitness);
        assert_eq!(verdict.reasons, vec!["no artifact binds the claim"]);
        assert_eq!(verdict.drill_downs.len(), 1, "drill-downs survive");
    }

    #[test]
    fn legacy_pack_without_new_fields_reads_as_none() {
        // A producer predating the delivery report omits the keys; the
        // consumer must get None — never an implied Answered verdict.
        let json = serde_json::json!({
            "repositoryId": "repo",
            "snapshotId": "snap",
            "query": "q",
            "intent": "natural_language_behavior",
            "budgetTokens": 1024,
            "usedTokens": 0,
            "items": [],
        });
        let pack: ContextPack = serde_json::from_value(json).expect("legacy pack parses");
        assert!(pack.search_verdict.is_none());
        assert!(pack.delivery_report.is_none());
        assert!(pack.search_latency_ms.is_none());
    }
}
