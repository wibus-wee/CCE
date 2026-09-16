from cce_research.adapters import Adapter, normalize_payload, predicted_intent
from cce_research.metrics import bootstrap, compare, evaluate, overlaps
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
    assert deltas["recall@20"].value == 1.0
    assert deltas["mrr"].value == 1.0


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
