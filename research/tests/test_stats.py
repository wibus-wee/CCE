from cce_research.stats import (
    bootstrap_ci,
    compare_metrics,
    effect_size,
    holm_bonferroni,
    minimum_detectable_effect,
    permutation_pvalue,
)


def test_permutation_pvalue_detects_clear_signal() -> None:
    baseline = [0.0] * 15
    candidate = [1.0] * 15
    p = permutation_pvalue(baseline, candidate)
    assert p is not None and p < 0.01


def test_permutation_pvalue_is_null_on_identical_runs() -> None:
    values = [0.5, 0.7, 0.3, 0.9]
    assert permutation_pvalue(values, list(values)) == 1.0


def test_permutation_is_deterministic() -> None:
    baseline = [0.2, 0.5, 0.4, 0.6]
    candidate = [0.3, 0.5, 0.6, 0.7]
    assert permutation_pvalue(baseline, candidate) == permutation_pvalue(baseline, candidate)


def test_holm_controls_family_wise() -> None:
    # One tiny p among many borderline: only the smallest survives.
    result = holm_bonferroni({"a": 0.001, "b": 0.03, "c": 0.04}, alpha=0.05)
    assert result == {"a": True, "b": False, "c": False}


def test_holm_rejects_all_when_all_tiny() -> None:
    result = holm_bonferroni({"a": 0.001, "b": 0.002, "c": 0.004}, alpha=0.05)
    assert all(result.values())


def test_mde_shrinks_with_more_cases() -> None:
    deltas = [0.05, -0.02, 0.03, 0.0]
    few = minimum_detectable_effect(deltas)
    many = minimum_detectable_effect(deltas * 10)
    assert few is not None and many is not None and many < few


def test_effect_size_sign_and_scale() -> None:
    assert effect_size([1.0, 1.0, 1.0]) is None  # no spread
    positive = effect_size([0.5, 1.0, 1.5, 0.8])
    negative = effect_size([-0.5, -1.0, -1.5, -0.8])
    assert positive is not None and positive > 0
    assert negative is not None and negative < 0


def test_compare_metrics_marks_holm_significance() -> None:
    baseline = {"recall@20": [0.0] * 12, "query_ms": [10.0] * 12}
    candidate = {"recall@20": [1.0] * 12, "query_ms": [10.0] * 12}
    report = compare_metrics(baseline, candidate)
    assert report["recall@20"].significant
    assert report["recall@20"].delta == 1.0
    # Identical runs: permutation p = 1.0, never significant.
    assert report["query_ms"].p_value == 1.0
    assert not report["query_ms"].significant


def test_bootstrap_ci_orders() -> None:
    value, low, high = bootstrap_ci([0.0, 0.5, 1.0, 0.5])
    assert low <= value <= high
