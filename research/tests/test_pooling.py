from cce_research.pooling import adjudication_queue, apply_judgments
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
        "gold_files": ["src/gold.rs"],
        "provenance": _provenance(),
    }
    fields.update(overrides)
    return BenchmarkCase.model_validate(fields)


def _hit(path: str, rank: int) -> RetrievedRange:
    return RetrievedRange(
        path=path, start_line=1, end_line=5, route="lexical",
        rank=rank, score=1.0, estimated_tokens=5,
    )


def _result(case_id: str, retrieved: list[RetrievedRange]) -> CaseResult:
    return CaseResult(
        case_id=case_id, system="t", system_revision="WORKTREE",
        dataset_revision="test-v1", retrieved=retrieved, query_ms=1.0,
    )


def test_queue_excludes_gold_and_judged_paths() -> None:
    case = _case(judged_files=["src/no.rs"])
    result = _result("c1", [_hit("src/gold.rs", 1), _hit("src/no.rs", 2), _hit("src/?.rs", 3)])
    queue = adjudication_queue([case], [("run", [result])])
    assert [entry.path for entry in queue] == ["src/?.rs"]


def test_queue_ranks_shared_unjudged_first() -> None:
    case = _case()
    run_a = _result("c1", [_hit("src/shared.rs", 5), _hit("src/only-a.rs", 1)])
    run_b = _result("c1", [_hit("src/shared.rs", 9)])
    queue = adjudication_queue([case], [("a", [run_a]), ("b", [run_b])])
    assert queue[0].path == "src/shared.rs"
    assert queue[0].occurrences == 2
    # best_rank is the best list position across runs (delivery order), not
    # the engine's rank field.
    assert queue[0].best_rank == 1


def test_apply_judgments_promotes_relevant_and_records_irrelevant() -> None:
    case = _case()
    updated = apply_judgments(
        [case],
        [("c1", "src/extra.rs", "relevant"), ("c1", "src/noise.rs", "irrelevant")],
    )
    assert "src/extra.rs" in updated[0].gold_files
    assert "src/noise.rs" in updated[0].judged_files


def test_apply_judgments_rejects_unknown_case() -> None:
    try:
        apply_judgments([_case()], [("nope", "src/x.rs", "relevant")])
    except ValueError as error:
        assert "unknown case" in str(error)
    else:
        raise AssertionError("expected rejection of unknown case id")
