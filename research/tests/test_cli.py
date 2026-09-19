from cce_research.cli import _filter_tags, _perturb_symbol
from cce_research.schema import BenchmarkCase, Provenance


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


def test_perturb_symbol_mutations_stay_lookalike() -> None:
    assert _perturb_symbol("SnapshotIdentity", "pluralize") == "SnapshotIdentitys"
    assert _perturb_symbol("open", "truncate") == "ope"
    assert _perturb_symbol("open", "double-last") == "openn"
    assert _perturb_symbol("search", "swap-last") == "searhc"
    # Tiny symbols degrade gracefully instead of vanishing.
    assert _perturb_symbol("id", "truncate") == "idx"
    assert _perturb_symbol("id", "swap-last") == "idx"


def test_filter_tags_include_and_exclude() -> None:
    dev = _case(case_id="dev", tags=["dev"])
    trap = _case(case_id="trap", tags=["adversarial"])
    plain = _case(case_id="plain")
    cases = [dev, trap, plain]
    assert [c.case_id for c in _filter_tags(cases, "adversarial", None)] == ["trap"]
    assert [c.case_id for c in _filter_tags(cases, None, "adversarial")] == ["dev", "plain"]
    assert [c.case_id for c in _filter_tags(cases, "dev", "adversarial")] == ["dev"]
    assert _filter_tags(cases, None, None) == cases
