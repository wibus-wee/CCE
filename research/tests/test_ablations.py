from cce_research.ablations import marginal_utility, route_ablation_cases
from cce_research.schema import BenchmarkCase, CaseResult, Provenance, RetrievedRange


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


def _result(case_id: str, routes: list[str], retrieved: list[RetrievedRange]) -> CaseResult:
    return CaseResult(
        case_id=case_id, system="t", system_revision="WORKTREE",
        dataset_revision="test-v1", retrieved=retrieved,
        plan_routes=routes, query_ms=1.0,
    )


def _hit(path: str) -> RetrievedRange:
    return RetrievedRange(
        path=path, start_line=1, end_line=5, route="structural",
        rank=1, score=1.0, estimated_tokens=5,
    )


def test_ablation_cases_remove_one_route_each() -> None:
    case = _case()
    baseline = [_result("c1", ["lexical", "structural", "hybrid"], [_hit("src/a.rs")])]
    derived = route_ablation_cases([case], baseline)
    by_id = {d.case_id: d for d in derived}
    assert set(by_id) == {"c1::abl-lexical", "c1::abl-structural", "c1::abl-hybrid"}
    assert by_id["c1::abl-structural"].routes == ["lexical", "hybrid"]
    assert by_id["c1::abl-structural"].derivation == "ablate:structural"
    assert by_id["c1::abl-structural"].derived_from == "c1"


def test_ablation_skips_single_route_plans() -> None:
    case = _case()
    baseline = [_result("c1", ["lexical"], [_hit("src/a.rs")])]
    assert route_ablation_cases([case], baseline) == []


def test_ablation_respects_pinned_route_sets() -> None:
    case = _case(routes=["lexical", "exact_symbol"])
    baseline = [_result("c1", ["lexical", "exact_symbol", "hybrid"], [_hit("src/a.rs")])]
    derived = route_ablation_cases([case], baseline)
    assert {d.case_id for d in derived} == {"c1::abl-lexical", "c1::abl-exact_symbol"}


def test_marginal_utility_reports_per_route_delta() -> None:
    case = _case()
    baseline = [_result("c1", ["lexical", "structural"], [_hit("src/a.rs")])]
    ablated = [_result("c1::abl-structural", ["lexical"], [])]
    table = marginal_utility([case], baseline, ablated, metric="file_recall@20")
    assert table["structural"]["mean_delta"] == -1.0
    assert table["structural"]["cases"] == 1.0
