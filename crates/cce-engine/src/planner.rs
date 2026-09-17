use cce_core::{GraphPolicy, QueryIntent, SearchRoute, ViewKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// The planner's output: which routes to run, how much graph expansion is
/// allowed, which views are required, and why.
pub struct QueryPlan {
    /// Classified intent (caller-supplied wins over guessing).
    pub intent: QueryIntent,
    /// Retrieval routes to execute.
    pub routes: Vec<SearchRoute>,
    /// Graph expansion policy.
    pub graph_policy: GraphPolicy,
    /// Views that must be Ready for the plan to be satisfiable.
    pub required_views: Vec<ViewKind>,
    /// Human-readable rationale for the plan.
    pub reasons: Vec<String>,
}

/// Deterministic query planner: text → `QueryPlan`. Heuristics only label
/// intent; they never silently narrow routes.
#[derive(Debug, Clone, Default)]
pub struct QueryPlanner;

impl QueryPlanner {
    /// Create a planner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Plan routes and graph policy for a query.
    #[must_use]
    pub fn plan(&self, query: &str, requested_intent: Option<QueryIntent>) -> QueryPlan {
        let normalized = query.to_ascii_lowercase();
        let Some(intent) = requested_intent else {
            // Inferred intent is a label, never a routing decision: keyword
            // guesses must not escalate into narrow routes or graph expansion.
            let guessed = classify(query, &normalized);
            return union_plan(
                guessed,
                vec![
                    "intent not supplied; conservative union recall without graph expansion"
                        .to_owned(),
                    format!("keyword classifier guessed {guessed:?} (label only, not routed on)"),
                ],
            );
        };
        match intent {
            QueryIntent::ExactEntity => QueryPlan {
                intent,
                routes: vec![SearchRoute::ExactSymbol, SearchRoute::Lexical],
                graph_policy: GraphPolicy::None,
                required_views: vec![ViewKind::Symbols, ViewKind::Lexical],
                reasons: vec!["identifier-shaped or definition/reference query".to_owned()],
            },
            QueryIntent::NaturalLanguageBehavior | QueryIntent::IssueLocalization => {
                QueryPlan {
                    intent,
                    routes: vec![
                        SearchRoute::Lexical,
                        SearchRoute::DenseRaw,
                        SearchRoute::DenseSummary,
                        SearchRoute::Hybrid,
                    ],
                    graph_policy: GraphPolicy::None,
                    required_views: vec![ViewKind::Lexical, ViewKind::Dense],
                    reasons: vec![match intent {
                    QueryIntent::IssueLocalization => {
                        "issue localization shares the behavior plan; no reranker is configured"
                    }
                    _ => "behavior query crosses natural-language and source-code vocabularies",
                }
                .to_owned()],
                }
            }
            QueryIntent::Trace => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::ExactSymbol,
                    SearchRoute::Lexical,
                    SearchRoute::DenseRaw,
                    SearchRoute::Structural,
                ],
                graph_policy: GraphPolicy::OutgoingTrace,
                required_views: vec![ViewKind::Symbols, ViewKind::Lexical, ViewKind::Graph],
                reasons: vec![
                    "trace query requires direction-preserving structural expansion".to_owned(),
                ],
            },
            QueryIntent::Impact => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::ExactSymbol,
                    SearchRoute::Lexical,
                    SearchRoute::DenseRaw,
                    SearchRoute::Structural,
                ],
                graph_policy: GraphPolicy::IncomingImpact,
                required_views: vec![ViewKind::Symbols, ViewKind::Lexical, ViewKind::Graph],
                reasons: vec![
                    "impact query traverses incoming references and containment".to_owned(),
                ],
            },
            QueryIntent::Architecture => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::Knowledge,
                    SearchRoute::DenseSummary,
                    SearchRoute::Structural,
                    SearchRoute::Lexical,
                ],
                graph_policy: GraphPolicy::ArchitectureBoundary,
                required_views: vec![ViewKind::Knowledge, ViewKind::Graph, ViewKind::Lexical],
                reasons: vec![
                    "architecture query needs hierarchy plus representative source evidence"
                        .to_owned(),
                ],
            },
            QueryIntent::History => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::History,
                    SearchRoute::Lexical,
                    SearchRoute::DenseSummary,
                ],
                graph_policy: GraphPolicy::None,
                required_views: vec![ViewKind::History, ViewKind::Lexical],
                reasons: vec![
                    "why/history query must not infer rationale from current source alone"
                        .to_owned(),
                ],
            },
            QueryIntent::PreciseDataflow => QueryPlan {
                intent,
                routes: vec![SearchRoute::ExactSymbol, SearchRoute::Structural],
                graph_policy: GraphPolicy::DataflowRequired,
                required_views: vec![ViewKind::Dataflow, ViewKind::Symbols],
                reasons: vec![
                    "precise dataflow requires an evidence-backed analysis view".to_owned(),
                ],
            },
            QueryIntent::Unknown => union_plan(
                QueryIntent::Unknown,
                vec![
                    "unknown intent uses conservative union recall without graph expansion"
                        .to_owned(),
                ],
            ),
        }
    }
}

/// Safe default: every cheap retrieval channel runs and reciprocal-rank
/// fusion sorts it out. Knowledge/History are opportunistic — they only
/// contribute when those documents exist, so they are not required views.
fn union_plan(intent: QueryIntent, reasons: Vec<String>) -> QueryPlan {
    QueryPlan {
        intent,
        routes: vec![
            SearchRoute::ExactSymbol,
            SearchRoute::Lexical,
            SearchRoute::DenseRaw,
            SearchRoute::DenseSummary,
            SearchRoute::Hybrid,
            SearchRoute::Knowledge,
            SearchRoute::History,
        ],
        graph_policy: GraphPolicy::None,
        required_views: vec![ViewKind::Symbols, ViewKind::Lexical, ViewKind::Dense],
        reasons,
    }
}

/// `original` keeps its casing for identifier detection; `normalized` is the
/// lowercase form used for keyword matching.
fn classify(original: &str, query: &str) -> QueryIntent {
    // Single ASCII keywords match on word boundaries so identifiers like
    // `commit_snapshot` or `workflow` don't trip "commit"/"flow". Multiword
    // phrases and CJK terms keep substring semantics. Ambiguous probes
    // ("why"/"为什么", "break"/"breaks") need a second anchor word so they do
    // not capture behavior or non-conditional uses.
    if has_any_word(query, &["taint", "taints"])
        || contains_any(
            query,
            &[
                "source to sink",
                "source→sink",
                "污点",
                "用户输入",
                "shell 参数",
            ],
        )
    {
        return QueryIntent::PreciseDataflow;
    }
    // History asks how the code came to be. Provenance words (commit, blame,
    // 历史, 提交) stand alone, but a bare "why"/"为什么" probe is ambiguous —
    // "why is call confidence lower" is about current behavior, not history —
    // so it only counts paired with a change anchor ("why was it added",
    // "为什么引入").
    if has_any_word(
        query,
        &["history", "commit", "commits", "committed", "blame"],
    ) || contains_any(query, &["历史", "什么时候改", "何时改", "谁改", "提交"])
        || (has_any_word(query, &["why"])
            && has_any_word(
                query,
                &[
                    "added",
                    "changed",
                    "introduced",
                    "removed",
                    "deprecated",
                    "renamed",
                    "deleted",
                    "modified",
                    "reverted",
                ],
            ))
        || (contains_any(query, &["为什么", "为何"])
            && contains_any(
                query,
                &["修改", "改动", "引入", "添加", "删除", "重命名", "弃用"],
            ))
    {
        return QueryIntent::History;
    }
    // "What breaks if/when X changes" is the canonical impact phrasing; the
    // break verb is word-boundary matched and needs a conditional anchor so
    // "line break" or "break statement" alone does not escalate. Matching the
    // words separately keeps inflections ("breaks if") from slipping through.
    if has_any_word(query, &["impact", "affected"])
        || (has_any_word(query, &["break", "breaks"]) && has_any_word(query, &["if", "when"]))
        || contains_any(query, &["影响", "会破坏", "哪些地方会"])
    {
        return QueryIntent::Impact;
    }
    if has_any_word(query, &["trace", "traces", "lifecycle"])
        || contains_any(
            query,
            &["data movement", "调用链", "链路", "流向", "生命周期"],
        )
    {
        return QueryIntent::Trace;
    }
    if has_any_word(
        query,
        &["architecture", "subsystem", "boundary", "responsibility"],
    ) || contains_any(query, &["架构", "模块", "职责", "边界"])
    {
        return QueryIntent::Architecture;
    }
    if has_any_word(
        query,
        &[
            "bug",
            "fail",
            "fails",
            "failure",
            "error",
            "broken",
            "occasionally",
        ],
    ) || contains_any(query, &["错误", "失败", "偶发", "不生效"])
    {
        return QueryIntent::IssueLocalization;
    }
    if is_exact_entity_query(original, query) {
        return QueryIntent::ExactEntity;
    }
    QueryIntent::NaturalLanguageBehavior
}

/// Exact-entity routing requires code-shaped tokens (`snake_case`,
/// `CamelCase`, `::` paths, `file.ext`) — natural-language questions like "where is
/// snapshot freshness decided" must not be misrouted just because they are
/// all alphanumeric words.
fn is_exact_entity_query(original: &str, query: &str) -> bool {
    let tokens = original
        .split(|character: char| character.is_whitespace())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return false;
    }
    let code_tokens = tokens.iter().filter(|token| looks_like_code(token)).count();
    if code_tokens * 2 >= tokens.len() {
        return true;
    }
    code_tokens > 0
        && (has_any_word(
            query,
            &[
                "defined",
                "definition",
                "references",
                "implemented",
                "decided",
            ],
        ) || contains_any(
            query,
            &["where is", "在哪定义", "谁引用", "在哪里", "定义", "引用"],
        ))
}

/// Whole-word keyword match: tokens are runs of alphanumerics plus `_`, so
/// `commit_snapshot` does not match the keyword `commit`.
fn has_any_word(query: &str, words: &[&str]) -> bool {
    query
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .any(|token| words.contains(&token))
}

/// A token that looks like a code identifier or path rather than a natural
/// language word: contains `_`, `::`, an interior lower→upper transition
/// (camelCase), or a `name.ext` file shape.
fn looks_like_code(token: &str) -> bool {
    if token.contains('_') || token.contains("::") {
        return true;
    }
    let mut previous_lower = false;
    for character in token.chars() {
        if previous_lower && character.is_uppercase() {
            return true;
        }
        previous_lower = character.is_lowercase();
    }
    token.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && !extension.is_empty()
            && extension.len() <= 5
            && extension.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferred_intent_labels_without_routing() {
        // The classifier may label the query, but an inferred intent must
        // never escalate into narrow routes or graph expansion.
        let plan = QueryPlanner::new().plan("改 Attempt.status 会影响哪些 UI？", None);
        assert_eq!(plan.intent, QueryIntent::Impact);
        assert_eq!(plan.graph_policy, GraphPolicy::None);
        assert!(plan.routes.contains(&SearchRoute::Lexical));
        assert!(!plan.routes.contains(&SearchRoute::Structural));
    }

    #[test]
    fn explicit_impact_keeps_incoming_expansion() {
        let plan = QueryPlanner::new().plan(
            "改 Attempt.status 会影响哪些 UI？",
            Some(QueryIntent::Impact),
        );
        assert_eq!(plan.graph_policy, GraphPolicy::IncomingImpact);
        assert!(plan.routes.contains(&SearchRoute::Structural));
    }

    #[test]
    fn issue_localization_shares_behavior_plan() {
        let issue = QueryPlanner::new().plan("search fails", Some(QueryIntent::IssueLocalization));
        let behavior =
            QueryPlanner::new().plan("search fails", Some(QueryIntent::NaturalLanguageBehavior));
        assert_eq!(issue.routes, behavior.routes);
        assert_eq!(issue.graph_policy, GraphPolicy::None);
    }

    #[test]
    fn routes_plain_behavior_without_graph() {
        let plan = QueryPlanner::new().plan("用户退出之后消息仍然恢复", None);
        assert_eq!(plan.graph_policy, GraphPolicy::None);
    }

    #[test]
    fn natural_language_where_is_not_exact_entity() {
        let plan = QueryPlanner::new().plan("where is snapshot freshness decided?", None);
        assert_eq!(plan.intent, QueryIntent::NaturalLanguageBehavior);
    }

    #[test]
    fn where_is_with_identifier_is_exact_entity() {
        let plan = QueryPlanner::new().plan("where is SnapshotIdentity defined?", None);
        assert_eq!(plan.intent, QueryIntent::ExactEntity);
        let plan = QueryPlanner::new().plan("MetadataStore::commit_snapshot", None);
        assert_eq!(plan.intent, QueryIntent::ExactEntity);
        let plan = QueryPlanner::new().plan("engine.rs", None);
        assert_eq!(plan.intent, QueryIntent::ExactEntity);
    }

    #[test]
    fn inflected_breaks_if_is_impact() {
        // "break if" as a literal phrase misses the canonical inflected
        // phrasing; the break verb plus a conditional anchor must match.
        let plan = QueryPlanner::new().plan(
            "What breaks if ContextPack stops carrying snapshot identity?",
            None,
        );
        assert_eq!(plan.intent, QueryIntent::Impact);
        // Bare "break" without a conditional anchor stays behavior.
        let plan = QueryPlanner::new().plan("how does the line break rendering work", None);
        assert_eq!(plan.intent, QueryIntent::NaturalLanguageBehavior);
    }

    #[test]
    fn bare_why_probe_is_behavior_not_history() {
        // 为什么/why alone asks about current behavior; history needs a
        // change or provenance anchor.
        let plan = QueryPlanner::new().plan("为什么函数调用关系的置信度低于导入关系", None);
        assert_eq!(plan.intent, QueryIntent::NaturalLanguageBehavior);
        let plan =
            QueryPlanner::new().plan("why is call confidence lower than import confidence", None);
        assert_eq!(plan.intent, QueryIntent::NaturalLanguageBehavior);
    }

    #[test]
    fn why_with_change_anchor_is_history() {
        let plan = QueryPlanner::new().plan("why was the quarantine path added?", None);
        assert_eq!(plan.intent, QueryIntent::History);
        let plan = QueryPlanner::new().plan("为什么引入 quarantine 路径", None);
        assert_eq!(plan.intent, QueryIntent::History);
    }
}
