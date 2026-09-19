use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{RelationKind, RelationOrigin, SourceAddress};

/// Coarse classification of what a query is trying to accomplish, used by
/// the planner to pick routes and graph policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryIntent {
    /// Looking up a specific named entity (symbol, type, file).
    ExactEntity,
    /// Free-form "where/how does X behave" question.
    NaturalLanguageBehavior,
    /// Localizing the code responsible for a reported issue.
    IssueLocalization,
    /// Following a call/data path forward from a starting point.
    Trace,
    /// Finding what would be affected by changing an entity.
    Impact,
    /// Understanding module/architecture structure.
    Architecture,
    /// Asking about when or why code changed.
    History,
    /// Requires source-to-sink dataflow evidence; explicitly refused when
    /// no dataflow view is available.
    PreciseDataflow,
    /// Intent could not be classified.
    Unknown,
}

/// How much graph expansion the planner is allowed to apply to a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphPolicy {
    /// No graph expansion.
    None,
    /// Follow outgoing edges from matched entities.
    OutgoingTrace,
    /// Follow incoming edges to matched entities.
    IncomingImpact,
    /// Expand along module/architecture boundaries.
    ArchitectureBoundary,
    /// Requires evidence-backed dataflow edges; refused when unavailable.
    DataflowRequired,
    /// Undirected expansion under inferred intent: runs as a corroborating
    /// tail route — fusion, not policy, decides whether expanded hits rank.
    Opportunistic,
}

/// Which retrieval channel produced (or should produce) a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchRoute {
    /// Planner decided no retrieval is needed (e.g. abstention cases).
    NoRetrieval,
    /// Exact symbol/entity lookup.
    ExactSymbol,
    /// Full-text (`SQLite` FTS5) search.
    Lexical,
    /// Dense embedding over raw code documents.
    DenseRaw,
    /// Dense embedding over generated summary documents.
    DenseSummary,
    /// Fusion of lexical and dense channels.
    Hybrid,
    /// Structural/graph-driven retrieval.
    Structural,
    /// Retrieval over generated knowledge pages.
    Knowledge,
    /// Retrieval over commit/history documents.
    History,
    /// Query-time regex over stored commit patches (`type:diff`).
    Diff,
    /// Zoekt trigram index candidates (external sidecar index; file hits
    /// resolve back to snapshot documents through matched line numbers).
    Zoekt,
    /// Result ordering produced by the reranker stage.
    Reranked,
}

/// What form a retrieval document's content takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalRepresentation {
    /// Verbatim source text.
    RawCode,
    /// A symbol signature without its body.
    Signature,
    /// Compact per-file descriptor (path, imports, top-level signatures) used
    /// for architecture routing instead of whole-file text.
    FileDescriptor,
    /// Summary of a single symbol.
    SymbolSummary,
    /// Summary of a file's role.
    RoleSummary,
    /// Summary of a module.
    ModuleSummary,
    /// Summary of a call/data flow.
    FlowSummary,
    /// Summary of test behavior.
    TestBehavior,
    /// Summary derived from a commit.
    CommitSummary,
    /// Changed paths plus extracted added/removed lines from a commit diff.
    /// Deterministic evidence (not a generated summary); the full patch text
    /// lives in the artifact store under `body_artifact_digest`.
    CommitDiff,
    /// A generated knowledge page.
    KnowledgePage,
}

/// A single retrievable document: content plus provenance linking it back to
/// source addresses and (for generated content) its producing model/step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalDocument {
    /// Document identifier.
    pub id: String,
    /// Entity this document is attached to.
    pub entity_id: String,
    /// Snapshot the document was built against.
    pub snapshot_id: String,
    /// Which representation the body carries.
    pub representation: RetrievalRepresentation,
    /// Content-addressed digest of the body payload.
    pub body_artifact_digest: String,
    /// Canonical region this document's content is drawn from, when it maps
    /// to a concrete source range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    /// Source address of the document's primary range, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// Embedding profile used for dense indexing, when embedded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_profile: Option<String>,
    /// Identity of the model or deterministic step that generated the body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    /// Additional source addresses supporting this document.
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    /// Precomputed search terms (identifier splits, folds) for lexical match.
    #[serde(default)]
    pub terms: Vec<String>,
}

/// A search request scoped to one repository snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    /// Repository to search in.
    pub repository_id: String,
    /// Snapshot to search against.
    pub snapshot_id: String,
    /// Raw query text.
    pub query: String,
    /// Optional pre-classified intent; the planner classifies when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<QueryIntent>,
    /// Maximum number of hits to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// When true, refuse results from stale snapshots.
    #[serde(default)]
    pub require_fresh: bool,
    /// Route allowlist; empty means planner chooses.
    #[serde(default)]
    pub routes: Vec<SearchRoute>,
    /// Structured hit filters. `key:value` tokens in `query`
    /// (`lang:rust`, `path:crates/…`) are parsed into this field; callers
    /// may also set it directly, in which case the query is not parsed.
    #[serde(default)]
    pub filters: QueryFilters,
}

/// Conjunctive hit filters parsed from `key:value` query tokens or set
/// directly by API callers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryFilters {
    /// Repository-relative path prefix (`path:crates/cce-engine`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    /// Entity language (`lang:rust`) matched exactly, case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Result kind override (`type:diff`, `type:commit`, `type:file`).
    /// `diff` routes the query to a regex scan over stored commit patches,
    /// `commit` restricts retrieval to the history route, `file` is the
    /// default.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub hit_type: Option<String>,
}

impl QueryFilters {
    /// Whether any filter is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.path_prefix.is_none() && self.language.is_none() && self.hit_type.is_none()
    }
}

/// Extracts `lang:`/`path:`/`type:` tokens into structured filters.
///
/// Returns the cleaned query text and the filters. Unknown `key:` tokens
/// (and unknown `type:` values) stay in the query text untouched.
#[must_use]
pub fn parse_query_filters(query: &str) -> (String, QueryFilters) {
    let mut filters = QueryFilters::default();
    let mut kept = Vec::new();
    for token in query.split_whitespace() {
        let Some((key, value)) = token.split_once(':') else {
            kept.push(token);
            continue;
        };
        if value.is_empty() {
            kept.push(token);
            continue;
        }
        match key {
            "lang" => filters.language = Some(value.to_ascii_lowercase()),
            "path" => filters.path_prefix = Some(value.to_owned()),
            "type" if matches!(value, "diff" | "commit" | "file") => {
                filters.hit_type = Some(value.to_owned());
            }
            _ => kept.push(token),
        }
    }
    (kept.join(" "), filters)
}

const fn default_limit() -> usize {
    20
}

/// One scored retrieval result with full provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Identifier of the retrieved document.
    pub document_id: String,
    /// Entity the document belongs to.
    pub entity_id: String,
    /// Canonical region the hit's content was drawn from, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    /// Symbol name when the hit is symbol-shaped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    /// Representation of the hit's content.
    pub representation: RetrievalRepresentation,
    /// Route that produced (or last re-ranked) this hit.
    pub route: SearchRoute,
    /// 1-based position in the returned list.
    pub rank: usize,
    /// Final score used for ordering.
    pub score: f64,
    /// All routes that contributed to this hit under fusion.
    #[serde(default)]
    pub contributing_routes: Vec<SearchRoute>,
    /// Primary source address of the hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// Supporting evidence addresses.
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    /// Display snippet for the hit.
    pub snippet: String,
    /// Whether the hit's content was verified against the current worktree.
    pub verified_current: bool,
    /// Human-readable reasons explaining why this hit was returned.
    #[serde(default)]
    pub explanation: Vec<String>,
}

/// How the kernel's evidence verdict came out — the structured signal an
/// external verifier reads to decide whether the returned evidence can
/// constitute an answer.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VerdictState {
    /// Strict-tier evidence supports at least one hit and the claim's
    /// witness typing found no gaps.
    Answered,
    /// Hits exist, but the claim's witness typing has gaps a consumer
    /// should check before treating them as an answer.
    WeakWitness,
    /// The kernel could not constitute an answer from its evidence —
    /// also the zero value: an uncomputed verdict asserts nothing.
    #[default]
    Abstained,
}

/// What the query asks of the repository, expressed in witness terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimPredicate {
    /// A definition or declaration of the named subject.
    Definition,
    /// Implemented behavior — requires code-tier binding evidence.
    Implementation,
    /// Relations between entities (trace, impact, architecture).
    Relation,
    /// Historical change evidence.
    History,
    /// Unclassified — no witness requirement is asserted.
    Lookup,
}

/// The evidence tier a claim requires before it can be constituted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WitnessRequirement {
    /// The claim subject must be *defined* — an entity or file bearing
    /// the name. Mentions do not satisfy it.
    Definition,
    /// The claim's distinguishing terms must co-bind inside at least one
    /// coherent code artifact.
    CodeBinding,
    /// At least one claim subject must participate in typed relation
    /// edges — a relation claim's witness is the edge set itself.
    Relation,
    /// At least one claim subject must appear in commit-class documents
    /// — a history claim's witness is the recorded change history.
    History,
    /// Any strict-tier evidence suffices; no witness typing asserted.
    Any,
}

/// Document class of an artifact. Prose can corroborate a claim but
/// cannot witness that code exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentClass {
    /// Source code, configuration, and manifest artifacts.
    Code,
    /// Prose documents — plans, docs, research notes.
    Prose,
}

/// Classify an artifact path by document class: prose formats can
/// corroborate a claim but cannot witness that code exists.
#[must_use]
pub fn document_class(path: &str) -> DocumentClass {
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown" | "mdx" | "rst" | "txt" | "adoc" | "org" | "tex") => {
            DocumentClass::Prose
        }
        _ => DocumentClass::Code,
    }
}

/// What the query claims, decomposed into the pieces a witness check can
/// evaluate: the named subjects, the claim-specific vocabulary, and the
/// evidence tier the claim requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimFrame {
    /// Classified intent the claim was derived under.
    pub intent: QueryIntent,
    /// What the query asks of the repository.
    pub predicate: ClaimPredicate,
    /// The evidence tier this claim requires.
    pub required_witness: WitnessRequirement,
    /// Verbatim or name-shaped subjects the query states.
    #[serde(default)]
    pub subjects: Vec<String>,
    /// Rare, claim-specific content terms — the vocabulary carrying the
    /// claim's specificity. Absence of these terms in returned evidence
    /// is a scope gap, not a ranking detail.
    #[serde(default)]
    pub distinguishing_terms: Vec<String>,
}

/// A definition-tier witness: an entity or file whose name carries the
/// checked term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinedWitness {
    /// Witnessing entity identifier.
    pub entity_id: String,
    /// Entity name as stored.
    pub name: String,
    /// Entity classification.
    pub kind: crate::EntityKind,
    /// Path of the witnessing definition, when it maps to source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Edge counts grouped by provenance tier.
///
/// The engineering contract's three-way split (deterministic facts,
/// framework-derived relations, model inference) plus the precision
/// gradient inside "deterministic".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelationProvenance {
    /// Compiler-grade edges: `Compiler`, `Scip`, `Lsp` origins — the
    /// strongest relation evidence.
    pub precise: usize,
    /// Syntax-level edges: `TreeSitter` — structural, not name-matched.
    pub syntactic: usize,
    /// Framework/build-derived edges: `BuildSystem`, `FrameworkRule` —
    /// deterministic but rule-derived, not compiler-attested.
    pub derived: usize,
    /// Model-inferred edges — never source truth, only corroboration.
    pub inferred: usize,
}

/// Typed-edge summary across a term's definition entities — a relation
/// claim's witness is the edge set itself.
///
/// Counts are bounded samples (per-entity edge fetches are capped):
/// enough to attest that relations exist and of which kinds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelationWitness {
    /// Inbound + outbound edges sampled across the term's definitions.
    pub edges: usize,
    /// Distinct edge kinds present.
    #[serde(default)]
    pub kinds: Vec<RelationKind>,
    /// Origins present — deterministic facts vs model inference stay
    /// split, so inference-only structure is never passed as source truth.
    #[serde(default)]
    pub origins: Vec<RelationOrigin>,
    /// Edges grouped by provenance tier — evidence quality at a glance
    /// without re-classifying the origin list.
    #[serde(default)]
    pub provenance: RelationProvenance,
}

/// Per-term witness data: defined (entities/files) vs mentioned
/// (documents). Mention is not definition — the distinction this report
/// exists to surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermWitness {
    /// The checked claim term.
    pub term: String,
    /// Definition-tier witnesses — empty when the term is mention-only.
    #[serde(default)]
    pub defined: Vec<DefinedWitness>,
    /// Corpus document frequency — mentions across all document classes.
    pub mentions: i64,
    /// Paths where the term is mentioned; drill-down material for a
    /// consuming verifier.
    #[serde(default)]
    pub mention_paths: Vec<String>,
    /// Typed-edge summary across this term's definitions — zero edges
    /// means the term participates in no recorded relation.
    #[serde(default)]
    pub relations: RelationWitness,
    /// Commit-class document ids (commit summary/diff) containing the
    /// term — the history claim's witness surface. Empty means the term
    /// never appears in the recorded change history.
    #[serde(default)]
    pub history_documents: Vec<String>,
}

/// How tightly a binding was attested — the granularity of the coherent
/// scope carrying every distinguishing term.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BindingScope {
    /// The terms co-occur inside one code entity's source span — the
    /// tightest witness: a single function/type/module carries the
    /// whole claim vocabulary.
    Entity,
    /// The terms co-occur in the same file but inside no single entity
    /// — proximity, not coherence. `File` is also the zero value: an
    /// unmarked binding asserts the weaker claim.
    #[default]
    File,
}

/// An artifact binding every distinguishing term in one coherent scope —
/// the only evidence shape that can witness a multi-term claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoundArtifact {
    /// Repository-relative path.
    pub path: String,
    /// Document class of the artifact.
    pub class: DocumentClass,
    /// Granularity of the coherent scope attesting the conjunction.
    #[serde(default)]
    pub scope: BindingScope,
    /// The binding entity's name when the scope is `entity` — the
    /// drill-down anchor a verifier wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// The binding evidence lives in test code — a `#[cfg(test)]`
    /// module, a `tests/` tree, or a `test_` function. Tests assert
    /// vocabulary; they do not implement it, so a test-only binding
    /// corroborates rather than witnesses (the same contract prose
    /// carries).
    #[serde(default)]
    pub test: bool,
}

/// What the evidence actually contains relative to the claim: per-term
/// definition/mention data, the coherent-scope binding set, and the
/// claim vocabulary absent from every returned hit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WitnessReport {
    /// Per checked term: definitions vs mentions.
    #[serde(default)]
    pub terms: Vec<TermWitness>,
    /// Artifacts binding ALL distinguishing terms. An empty set, or one
    /// holding only prose, means no code artifact can witness the claim
    /// as asked.
    #[serde(default)]
    pub binding_artifacts: Vec<BoundArtifact>,
    /// Distinguishing terms absent from every returned hit document —
    /// the hits do not carry the claim's specific vocabulary.
    #[serde(default)]
    pub scope_gaps: Vec<String>,
}

/// One deterministic follow-up retrieval derived from a witness gap —
/// a verbatim-runnable query plus the gap it closes. The kernel
/// suggests; the consumer decides whether to run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DrillDown {
    /// The suggested query, ready to pass back to `search` — uses only
    /// the query language's own filters (`type:`, `path:`).
    pub query: String,
    /// The witness gap this query addresses.
    pub reason: String,
}

/// Counts of final candidates by evidence tier.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTiers {
    /// Candidates admitted by a strict-tier pass (literal query-term
    /// evidence).
    pub strict: usize,
    /// Candidates admitted only by inferred-vicinity passes (dense,
    /// expansion, PRF, flow).
    pub weak: usize,
}

/// The kernel's evidence verdict: what the evidence is and where it
/// fails witness typing. The kernel reports; the consumer judges — a
/// verifier adapter reads this object instead of re-deriving it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchVerdict {
    /// Verdict state.
    pub state: VerdictState,
    /// Human-readable reasons for an abstention or weak-witness flag.
    #[serde(default)]
    pub reasons: Vec<String>,
    /// The claim the verdict was evaluated against.
    pub claim: ClaimFrame,
    /// What the evidence contains relative to the claim.
    pub witness: WitnessReport,
    /// Evidence-tier composition of the candidate set.
    pub evidence_tiers: EvidenceTiers,
    /// Deterministic follow-up queries derived from witness gaps — the
    /// drill-down surface. Empty when no gap suggests a next step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drill_downs: Vec<DrillDown>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verdict is the kernel↔verifier wire contract: struct fields
    /// stay camelCase (retrieval convention), enum values `snake_case`, and
    /// every section a drill-down consumer needs is present.
    #[test]
    fn verdict_serializes_external_consumer_shape() {
        let verdict = SearchVerdict {
            state: VerdictState::WeakWitness,
            reasons: vec!["weak_witness: `WebSocket` is mention-only".to_owned()],
            claim: ClaimFrame {
                intent: QueryIntent::NaturalLanguageBehavior,
                predicate: ClaimPredicate::Implementation,
                required_witness: WitnessRequirement::CodeBinding,
                subjects: vec!["WebSocket".to_owned()],
                distinguishing_terms: vec!["reconnect".to_owned()],
            },
            witness: WitnessReport {
                terms: vec![TermWitness {
                    term: "WebSocket".to_owned(),
                    defined: Vec::new(),
                    mentions: 4,
                    mention_paths: vec!["plans/009.md".to_owned()],
                    relations: RelationWitness {
                        edges: 3,
                        kinds: vec![RelationKind::Calls],
                        origins: vec![RelationOrigin::ModelInference],
                        provenance: RelationProvenance {
                            precise: 0,
                            syntactic: 0,
                            derived: 0,
                            inferred: 3,
                        },
                    },
                    history_documents: vec!["commit:abc123".to_owned()],
                }],
                binding_artifacts: vec![
                    BoundArtifact {
                        path: "docs/rfc.md".to_owned(),
                        class: DocumentClass::Prose,
                        scope: BindingScope::File,
                        entity: None,
                        test: false,
                    },
                    BoundArtifact {
                        path: "src/gateway.rs".to_owned(),
                        class: DocumentClass::Code,
                        scope: BindingScope::Entity,
                        entity: Some("validate".to_owned()),
                        test: false,
                    },
                ],
                scope_gaps: vec!["reconnect".to_owned()],
            },
            evidence_tiers: EvidenceTiers {
                strict: 2,
                weak: 33,
            },
            drill_downs: vec![DrillDown {
                query: "type:commit WebSocket".to_owned(),
                reason: "`WebSocket` has no commit-class witness".to_owned(),
            }],
        };
        let json = serde_json::to_value(&verdict).expect("serialize");
        assert_eq!(json["state"], "weak_witness");
        assert_eq!(json["claim"]["predicate"], "implementation");
        assert_eq!(json["claim"]["requiredWitness"], "code_binding");
        assert_eq!(json["claim"]["distinguishingTerms"][0], "reconnect");
        assert_eq!(
            json["witness"]["terms"][0]["mentionPaths"][0],
            "plans/009.md"
        );
        assert_eq!(json["witness"]["bindingArtifacts"][0]["class"], "prose");
        assert_eq!(json["witness"]["bindingArtifacts"][0]["scope"], "file");
        assert!(
            json["witness"]["bindingArtifacts"][0]
                .get("entity")
                .is_none()
        );
        assert_eq!(json["witness"]["bindingArtifacts"][1]["scope"], "entity");
        assert_eq!(json["witness"]["bindingArtifacts"][1]["entity"], "validate");
        assert_eq!(json["witness"]["scopeGaps"][0], "reconnect");
        assert_eq!(json["witness"]["terms"][0]["relations"]["edges"], 3);
        assert_eq!(
            json["witness"]["terms"][0]["relations"]["kinds"][0],
            "calls"
        );
        assert_eq!(
            json["witness"]["terms"][0]["relations"]["origins"][0],
            "model_inference"
        );
        assert_eq!(
            json["witness"]["terms"][0]["relations"]["provenance"]["inferred"],
            3
        );
        assert_eq!(
            json["witness"]["terms"][0]["historyDocuments"][0],
            "commit:abc123"
        );
        assert_eq!(json["evidenceTiers"]["strict"], 2);
        assert_eq!(json["drillDowns"][0]["query"], "type:commit WebSocket");
        let roundtrip: SearchVerdict = serde_json::from_value(json).expect("deserialize");
        assert_eq!(roundtrip, verdict);
    }

    /// `Abstained` is the zero value: an uncomputed verdict asserts
    /// nothing — an adapter must never read silence as an answer.
    #[test]
    fn verdict_default_is_abstained() {
        assert_eq!(VerdictState::default(), VerdictState::Abstained);
    }

    /// Document class drives witness typing; the classification itself
    /// must be stable for consumers re-implementing the check.
    #[test]
    fn document_class_partitions_prose_from_code() {
        assert_eq!(document_class("plans/009.md"), DocumentClass::Prose);
        assert_eq!(document_class("docs/RFC.MDX"), DocumentClass::Prose);
        assert_eq!(document_class("src/lock.rs"), DocumentClass::Code);
        assert_eq!(document_class("Cargo.toml"), DocumentClass::Code);
        assert_eq!(document_class("Makefile"), DocumentClass::Code);
    }
}
