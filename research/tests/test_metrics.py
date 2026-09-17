from cce_research.adapters import Adapter, normalize_payload, predicted_intent
from cce_research.metrics import (
    bootstrap,
    case_observations,
    compare,
    component_mrr,
    component_recall,
    evaluate,
    overlaps,
    path_component,
)
from cce_research.schema import (
    BenchmarkCase,
    CaseResult,
    LineRange,
    Provenance,
    RetrievedRange,
)


def _provenance() -> Provenance:
    return Provenance(
        source_url="https://example.com/repo",
        dataset_revision="test-v1",
        license_spdx="Apache-2.0",
        redistribution="allowed",
        construction_method="test fixture",
    )


def _case(**overrides: object) -> BenchmarkCase:
    fields: dict[str, object] = {
        "case_id": "c1",
        "repository": "example/repo",
        "revision": "WORKTREE",
        "query": "q",
        "intent": "impact",
        "gold_files": ["src/a.rs"],
        "provenance": _provenance(),
    }
    fields.update(overrides)
    return BenchmarkCase.model_validate(fields)


def _result(case_id: str = "c1", **overrides: object) -> CaseResult:
    fields: dict[str, object] = {
        "case_id": case_id,
        "system": "test",
        "system_revision": "WORKTREE",
        "dataset_revision": "test-v1",
        "retrieved": [
            RetrievedRange(
                path="src/a.rs",
                start_line=1,
                end_line=10,
                route="lexical",
                rank=1,
                score=1.0,
                estimated_tokens=10,
            )
        ],
        "query_ms": 1.0,
    }
    fields.update(overrides)
    return CaseResult.model_validate(fields)


def test_overlap_is_path_and_line_aware() -> None:
    assert overlaps(
        LineRange(path="a.rs", start_line=10, end_line=20),
        LineRange(path="a.rs", start_line=20, end_line=30),
    )
    assert not overlaps(
        LineRange(path="a.rs", start_line=10, end_line=20),
        LineRange(path="b.rs", start_line=10, end_line=20),
    )


def test_bootstrap_is_deterministic() -> None:
    assert bootstrap([0.0, 1.0], samples=100) == bootstrap([0.0, 1.0], samples=100)


def test_evaluate_groups_metrics_by_intent() -> None:
    cases = [_case(case_id="c1", intent="impact"), _case(case_id="c2", intent="trace")]
    results = [_result("c1"), _result("c2")]
    summary = evaluate(cases, results)
    assert "by_intent/impact/recall@20" in summary
    assert "by_intent/trace/recall@20" in summary
    assert summary["by_intent/impact/recall@20"].samples == 1


def test_intent_accuracy_only_scored_when_intent_withheld() -> None:
    supplied = _case(case_id="c1", supply_intent=True)
    withheld = _case(case_id="c2", supply_intent=False)
    results = [
        _result("c1", predicted_intent="trace"),
        _result("c2", predicted_intent="impact"),
    ]
    summary = evaluate([supplied, withheld], results)
    assert summary["intent_accuracy"].value == 1.0
    assert summary["intent_accuracy"].samples == 1


def test_intent_accuracy_counts_classifier_misses() -> None:
    withheld = _case(case_id="c1", supply_intent=False)
    result = _result("c1", predicted_intent="natural_language_behavior")
    summary = evaluate([withheld], [result])
    assert summary["intent_accuracy"].value == 0.0


def test_compare_reports_paired_deltas() -> None:
    case = _case()
    weak = _result(retrieved=[], abstained=True)
    strong = _result()
    deltas = compare([case], [weak], [strong])
    assert deltas["recall@20"].delta == 1.0
    assert deltas["mrr"].delta == 1.0


def test_bpref_ignores_unjudged_but_penalizes_judged_irrelevant() -> None:
    from cce_research.metrics import bpref

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    # Gold at rank 2, one unjudged item above it: unjudged costs nothing.
    case = _case(gold_files=["src/gold.rs"])
    assert bpref(case, [hit("src/unknown.rs", 1), hit("src/gold.rs", 2)]) == 1.0
    # Same shape but the item above is adjudicated irrelevant: penalty.
    judged = _case(gold_files=["src/gold.rs"], judged_files=["src/unknown.rs"])
    assert bpref(judged, [hit("src/unknown.rs", 1), hit("src/gold.rs", 2)]) == 0.0


def test_unjudged_rate_counts_only_unverdicted_paths() -> None:
    from cce_research.metrics import unjudged_rate

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    case = _case(gold_files=["src/gold.rs"], judged_files=["src/no.rs"])
    rate = unjudged_rate(case, [hit("src/gold.rs", 1), hit("src/no.rs", 2), hit("src/?.rs", 3)])
    assert rate == 1 / 3
    assert unjudged_rate(case, []) == 0.0


def test_claim_support_scores_facts_not_files() -> None:
    from cce_research.metrics import claim_support
    from cce_research.schema import GoldFact

    case = _case(
        gold_facts=[
            GoldFact(
                claim="status is written via set_view_status",
                evidence=[LineRange(path="src/store.rs", start_line=10, end_line=30)],
                symbols=["set_view_status"],
            ),
            GoldFact(claim="unreachable claim", evidence=[], symbols=["missing_fn"]),
        ]
    )
    hit = RetrievedRange(
        path="src/other.rs", start_line=1, end_line=5, symbol="set_view_status",
        route="lexical", rank=1, score=1.0, estimated_tokens=5,
    )
    assert claim_support(case, [hit]) == 0.5
    assert claim_support(case, []) == 0.0


def test_evaluate_emits_intent_precision_and_recall() -> None:
    withheld_a = _case(case_id="a", supply_intent=False, intent="impact")
    withheld_b = _case(case_id="b", supply_intent=False, intent="impact")
    withheld_c = _case(case_id="c", supply_intent=False, intent="trace")
    results = [
        _result("a", predicted_intent="impact"),
        _result("b", predicted_intent="natural_language_behavior"),
        _result("c", predicted_intent="trace"),
    ]
    summary = evaluate([withheld_a, withheld_b, withheld_c], results)
    assert summary["intent_recall/impact"].value == 0.5
    assert summary["intent_precision/natural_language_behavior"].value == 0.0
    assert summary["intent_precision/impact"].value == 1.0


def test_ndcg_never_exceeds_one_with_duplicate_gold_paths() -> None:
    case = _case()
    duplicated = [
        RetrievedRange(
            path="src/a.rs",
            start_line=offset,
            end_line=offset + 5,
            route="lexical",
            rank=index + 1,
            score=1.0,
            estimated_tokens=10,
        )
        for index, offset in enumerate(range(1, 60, 6))
    ]
    summary = evaluate([case], [_result(retrieved=duplicated)])
    assert summary["ndcg@10"].value <= 1.0


def test_compare_rejects_mismatched_coverage() -> None:
    cases = [_case(case_id="c1"), _case(case_id="c2")]
    try:
        compare(cases, [_result("c1")], [_result("c1"), _result("c2")])
    except ValueError as error:
        assert "coverage" in str(error)
    else:
        raise AssertionError("compare must reject missing case coverage")


def test_build_command_omits_intent_when_withheld() -> None:
    adapter = Adapter(
        name="t",
        command=["cce", "context", "{repository}", "{query}", "{intent_args}", "{route_args}"],
        timeout_seconds=10,
        environment={},
        model_identity="none",
        model_revision="none",
    )
    supplied = adapter.build_command(_case(), __import__("pathlib").Path("/repo"))
    assert supplied[-2:] == ["--intent", "impact"]
    withheld_cmd = adapter.build_command(
        _case(supply_intent=False), __import__("pathlib").Path("/repo")
    )
    assert "--intent" not in withheld_cmd


def test_build_command_expands_route_overrides() -> None:
    adapter = Adapter(
        name="t",
        command=["cce", "{route_args}"],
        timeout_seconds=10,
        environment={},
        model_identity="none",
        model_revision="none",
    )
    command = adapter.build_command(
        _case(routes=["lexical", "exact_symbol"]), __import__("pathlib").Path("/repo")
    )
    assert command == ["cce", "--route", "lexical", "--route", "exact_symbol"]


def test_normalize_payload_handles_search_results() -> None:
    payload = {
        "plan": {"intent": "impact", "routes": ["lexical"], "graphPolicy": "incoming_impact"},
        "hits": [
            {
                "documentId": "d1",
                "entityId": "e1",
                "symbolName": "SourceAddress",
                "route": "exact_symbol",
                "rank": 1,
                "score": 0.5,
                "address": {"path": "src/a.rs", "startLine": 3, "endLine": 9},
                "verifiedCurrent": True,
            }
        ],
    }
    retrieved = normalize_payload(payload)
    assert retrieved[0].path == "src/a.rs"
    assert retrieved[0].symbol == "SourceAddress"
    assert predicted_intent(payload) == "impact"


def test_normalize_payload_handles_context_packs() -> None:
    payload = {
        "intent": "natural_language_behavior",
        "items": [
            {
                "kind": "source",
                "estimatedTokens": 42,
                "provenance": {
                    "route": "lexical",
                    "rank": 2,
                    "score": 0.25,
                    "sourceAddress": {"path": "src/b.rs", "startLine": 1, "endLine": 5},
                    "verifiedCurrent": True,
                },
            }
        ],
    }
    retrieved = normalize_payload(payload)
    assert retrieved[0].path == "src/b.rs"
    assert predicted_intent(payload) == "natural_language_behavior"


def _component_map() -> dict[str, str]:
    return {"cce-store": "crates/cce-store", "cce-engine": "crates/cce-engine", "root": "."}


def test_path_component_longest_prefix_wins() -> None:
    component_map = {
        "cce-store": "crates/cce-store",
        "nested": "crates/cce-store/nested",
        "root": ".",
    }
    assert path_component("crates/cce-store/nested/x.rs", component_map) == "nested"
    assert path_component("crates/cce-store/src/a.rs", component_map) == "cce-store"
    assert path_component("README.md", component_map) == "root"
    assert path_component("other/a.rs", {k: v for k, v in component_map.items() if k != "root"}) is None


def test_component_metrics_use_map() -> None:
    case = _case(gold_components=["cce-store"])
    result = _result(
        retrieved=[
            RetrievedRange(
                path="crates/cce-engine/src/a.rs",
                start_line=1,
                end_line=5,
                route="lexical",
                rank=1,
                score=1.0,
                estimated_tokens=5,
            ),
            RetrievedRange(
                path="crates/cce-store/src/b.rs",
                start_line=1,
                end_line=5,
                route="lexical",
                rank=2,
                score=0.9,
                estimated_tokens=5,
            ),
        ],
        component_map=_component_map(),
    )
    assert component_recall(case, result.retrieved[:1], result.component_map) == 0.0
    assert component_recall(case, result.retrieved[:5], result.component_map) == 1.0
    assert component_mrr(case, result.retrieved, result.component_map) == 0.5


def test_component_metrics_skip_when_unannotated_or_unmapped() -> None:
    case = _case()
    result = _result(component_map=_component_map())
    observations = case_observations(case, result)
    assert "component_recall_at_5" not in observations
    case = _case(gold_components=["cce-store"])
    result = _result(component_map={})
    observations = case_observations(case, result)
    assert "component_recall_at_5" not in observations
