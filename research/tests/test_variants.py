from cce_research.schema import BenchmarkCase, CaseResult, Provenance, RetrievedRange
from cce_research.variants import generate_variants, transform, variant_agreement


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
        "query": "What breaks if ContextPack stops carrying snapshot identity?",
        "intent": "impact",
        "gold_files": ["src/a.rs"],
        "provenance": _provenance(),
    }
    fields.update(overrides)
    return BenchmarkCase.model_validate(fields)


def test_inflection_transform_rewrites_breaks() -> None:
    out = transform("What breaks if X changes?", "inflection", 0)
    assert "break " in out or "break?" in out or out.endswith("break")
    assert "breaks" not in out


def test_mixed_language_swaps_cjk_term() -> None:
    assert transform("search 偶发返回陈旧结果", "mixed-language", 0) == "search 偶发return陈旧结果"


def test_verbose_transform_always_changes() -> None:
    out = transform("how does X work", "verbose", 0)
    assert out.startswith("please help me understand")


def test_variants_share_parent_gold_and_lineage() -> None:
    case = _case()
    derived = generate_variants([case])
    assert derived
    for variant in derived:
        assert variant.derived_from == "c1"
        assert variant.derivation.startswith("variant:")
        assert variant.gold_files == case.gold_files
        assert variant.query != case.query


def test_variants_skip_no_context_cases() -> None:
    case = _case(no_context=True, gold_files=[], query="anything")
    assert generate_variants([case]) == []


def test_variant_agreement_counts_mismatched_outcomes() -> None:
    case = _case()
    hit = RetrievedRange(
        path="src/a.rs", start_line=1, end_line=5, route="lexical",
        rank=1, score=1.0, estimated_tokens=5,
    )
    baseline = [CaseResult(
        case_id="c1", system="t", system_revision="WORKTREE",
        dataset_revision="test-v1", retrieved=[hit], query_ms=1.0,
    )]
    variant = CaseResult(
        case_id="c1::var-verbose", system="t", system_revision="WORKTREE",
        dataset_revision="test-v1", retrieved=[], query_ms=1.0,
    )
    report = variant_agreement([case], baseline, [variant])
    assert report["agreement"] == 0.0
    assert report["variants"] == 1.0
