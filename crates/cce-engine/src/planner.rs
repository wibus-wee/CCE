use cce_core::{QueryIntent, SearchRoute, ViewKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphPolicy {
    None,
    OutgoingTrace,
    IncomingImpact,
    ArchitectureBoundary,
    DataflowRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPlan {
    pub intent: QueryIntent,
    pub routes: Vec<SearchRoute>,
    pub graph_policy: GraphPolicy,
    pub rerank: bool,
    pub required_views: Vec<ViewKind>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct QueryPlanner;

impl QueryPlanner {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn plan(&self, query: &str, requested_intent: Option<QueryIntent>) -> QueryPlan {
        let normalized = query.to_ascii_lowercase();
        let intent = requested_intent.unwrap_or_else(|| classify(&normalized));
        match intent {
            QueryIntent::ExactEntity => QueryPlan {
                intent,
                routes: vec![SearchRoute::ExactSymbol, SearchRoute::Lexical],
                graph_policy: GraphPolicy::None,
                rerank: false,
                required_views: vec![ViewKind::Symbols, ViewKind::Lexical],
                reasons: vec!["identifier-shaped or definition/reference query".to_owned()],
            },
            QueryIntent::NaturalLanguageBehavior => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::Lexical,
                    SearchRoute::DenseRaw,
                    SearchRoute::DenseSummary,
                    SearchRoute::Hybrid,
                ],
                graph_policy: GraphPolicy::None,
                rerank: false,
                required_views: vec![ViewKind::Lexical, ViewKind::Dense],
                reasons: vec!["behavior query crosses natural-language and source-code vocabularies".to_owned()],
            },
            QueryIntent::IssueLocalization => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::Lexical,
                    SearchRoute::DenseRaw,
                    SearchRoute::DenseSummary,
                    SearchRoute::Hybrid,
                ],
                graph_policy: GraphPolicy::None,
                rerank: true,
                required_views: vec![ViewKind::Lexical, ViewKind::Dense],
                reasons: vec!["issue localization benefits from tests, exact error terms, and semantic recall".to_owned()],
            },
            QueryIntent::Trace => QueryPlan {
                intent,
                routes: vec![
                    SearchRoute::ExactSymbol,
                    SearchRoute::Lexical,
                    SearchRoute::DenseRaw,
                    SearchRoute::Structural,
                ],
                graph_policy: GraphPolicy::OutgoingTrace,
                rerank: false,
                required_views: vec![ViewKind::Symbols, ViewKind::Lexical, ViewKind::Graph],
                reasons: vec!["trace query requires direction-preserving structural expansion".to_owned()],
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
                rerank: false,
                required_views: vec![ViewKind::Symbols, ViewKind::Lexical, ViewKind::Graph],
                reasons: vec!["impact query traverses incoming references and containment".to_owned()],
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
                rerank: true,
                required_views: vec![ViewKind::Knowledge, ViewKind::Graph, ViewKind::Lexical],
                reasons: vec!["architecture query needs hierarchy plus representative source evidence".to_owned()],
            },
            QueryIntent::History => QueryPlan {
                intent,
                routes: vec![SearchRoute::History, SearchRoute::Lexical, SearchRoute::DenseSummary],
                graph_policy: GraphPolicy::None,
                rerank: false,
                required_views: vec![ViewKind::History, ViewKind::Lexical],
                reasons: vec!["why/history query must not infer rationale from current source alone".to_owned()],
            },
            QueryIntent::PreciseDataflow => QueryPlan {
                intent,
                routes: vec![SearchRoute::ExactSymbol, SearchRoute::Structural],
                graph_policy: GraphPolicy::DataflowRequired,
                rerank: false,
                required_views: vec![ViewKind::Dataflow, ViewKind::Symbols],
                reasons: vec!["precise dataflow requires an evidence-backed analysis view".to_owned()],
            },
            QueryIntent::Unknown => QueryPlan {
                intent: QueryIntent::NaturalLanguageBehavior,
                routes: vec![SearchRoute::Lexical, SearchRoute::DenseRaw, SearchRoute::Hybrid],
                graph_policy: GraphPolicy::None,
                rerank: false,
                required_views: vec![ViewKind::Lexical, ViewKind::Dense],
                reasons: vec!["unknown intent uses conservative hybrid recall without graph expansion".to_owned()],
            },
        }
    }
}

fn classify(query: &str) -> QueryIntent {
    if contains_any(
        query,
        &[
            "taint",
            "source to sink",
            "source→sink",
            "污点",
            "用户输入",
            "shell 参数",
        ],
    ) {
        return QueryIntent::PreciseDataflow;
    }
    if contains_any(
        query,
        &[
            "why ",
            "history",
            "commit",
            "为什么",
            "为何",
            "历史",
            "什么时候改",
        ],
    ) {
        return QueryIntent::History;
    }
    if contains_any(
        query,
        &[
            "impact",
            "affected",
            "break if",
            "影响",
            "会破坏",
            "哪些地方会",
        ],
    ) {
        return QueryIntent::Impact;
    }
    if contains_any(
        query,
        &[
            "trace",
            "flow",
            "lifecycle",
            "data movement",
            "调用链",
            "链路",
            "流向",
            "生命周期",
        ],
    ) {
        return QueryIntent::Trace;
    }
    if contains_any(
        query,
        &[
            "architecture",
            "subsystem",
            "boundary",
            "responsibility",
            "架构",
            "模块",
            "职责",
            "边界",
        ],
    ) {
        return QueryIntent::Architecture;
    }
    if contains_any(
        query,
        &[
            "bug",
            "fail",
            "error",
            "broken",
            "occasionally",
            "错误",
            "失败",
            "偶发",
            "不生效",
        ],
    ) {
        return QueryIntent::IssueLocalization;
    }
    if contains_any(
        query,
        &[
            "defined",
            "definition",
            "references",
            "where is",
            "在哪定义",
            "谁引用",
            "在哪里",
        ],
    ) || identifier_ratio(query) > 0.72
    {
        return QueryIntent::ExactEntity;
    }
    QueryIntent::NaturalLanguageBehavior
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn identifier_ratio(value: &str) -> f64 {
    let non_space = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    if non_space == 0 {
        return 0.0;
    }
    let identifier = value
        .chars()
        .filter(|character| character.is_alphanumeric() || *character == '_' || *character == ':')
        .count();
    identifier as f64 / non_space as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_impact_without_universal_graph_expansion() {
        let plan = QueryPlanner::new().plan("改 Attempt.status 会影响哪些 UI？", None);
        assert_eq!(plan.intent, QueryIntent::Impact);
        assert_eq!(plan.graph_policy, GraphPolicy::IncomingImpact);
    }

    #[test]
    fn routes_plain_behavior_without_graph() {
        let plan = QueryPlanner::new().plan("用户退出之后消息仍然恢复", None);
        assert_eq!(plan.graph_policy, GraphPolicy::None);
    }
}
