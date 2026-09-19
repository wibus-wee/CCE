"""Statistical inference for paired benchmark comparisons.

Design follows the IR evaluation literature:
- paired per-case deltas (candidate - baseline) are the unit of evidence;
- bootstrap CIs estimate the sampling distribution of the mean delta;
- a paired permutation (randomization) test provides the p-value — preferred
  over bootstrap significance at small n because bootstrap is biased toward
  small p (Smucker, Allan & Carterette 2007; Urbano et al. 2019);
- Holm-Bonferroni controls the family-wise error rate across the many
  metrics reported in one comparison;
- the minimum detectable effect (MDE) states the smallest true delta the
  suite can resolve at 80% power, separating "no difference" from
  "cannot tell at this n".
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass, field
from statistics import mean, stdev

BOOTSTRAP_SAMPLES = 2000
PERMUTATION_SAMPLES = 2000
SEED = 0xCCE


@dataclass(frozen=True)
class Comparison:
    """One metric's paired comparison outcome."""

    metric: str
    delta: float
    ci_low: float
    ci_high: float
    samples: int
    p_value: float | None
    effect_size: float | None
    mde: float | None
    significant: bool = field(default=False)


def collapse_clusters(values: list[float], clusters: list[str]) -> list[float]:
    """Collapse per-case values to per-cluster means. Derived cases
    (variants, budget curves, pinned ablations) replicate their parent —
    they are not independent evidence, so a family contributes exactly
    one observation (its mean). This is the unit-level fix for
    pseudo-replication: without it, variant-heavy families inflate both
    the effective sample size and the weight of one underlying case."""
    groups: dict[str, list[float]] = {}
    order: list[str] = []
    for value, cluster in zip(values, clusters, strict=True):
        if cluster not in groups:
            groups[cluster] = []
            order.append(cluster)
        groups[cluster].append(value)
    return [mean(groups[cluster]) for cluster in order]


def bootstrap_ci(
    values: list[float],
    samples: int = BOOTSTRAP_SAMPLES,
    seed: int = SEED,
    clusters: list[str] | None = None,
) -> tuple[float, float, float]:
    """Mean, and percentile bootstrap 95% CI of the mean. When `clusters`
    is given (aligned with `values`), the analysis unit is the cluster
    mean — families are resampled as indivisible blocks, so CIs reflect
    the number of *independent* cases, not the inflated row count."""
    if clusters is not None:
        values = collapse_clusters(values, clusters)
    if not values:
        return (0.0, 0.0, 0.0)
    generator = random.Random(seed)
    estimates = sorted(mean(generator.choices(values, k=len(values))) for _ in range(samples))
    return (
        mean(values),
        estimates[int(samples * 0.025)],
        estimates[min(samples - 1, int(samples * 0.975))],
    )


def permutation_pvalue(
    baseline: list[float],
    candidate: list[float],
    samples: int = PERMUTATION_SAMPLES,
    seed: int = SEED,
    clusters: list[str] | None = None,
) -> float | None:
    """Two-sided paired permutation p-value for mean(candidate - baseline).

    Under the null, swapping each pair's labels is equally likely; the
    p-value is the share of relabelings whose |mean delta| is at least the
    observed one. Returns None when fewer than two pairs exist. When
    `clusters` is given, pairs collapse to cluster means first — swapping
    within a family is not a valid randomization unit.
    """
    if clusters is not None:
        pairs = [
            (left, right)
            for left, right in zip(
                collapse_clusters(baseline, clusters),
                collapse_clusters(candidate, clusters),
                strict=True,
            )
        ]
    else:
        pairs = [(left, right) for left, right in zip(baseline, candidate, strict=True)]
    if len(pairs) < 2:
        return None
    observed = abs(mean(right - left for left, right in pairs))
    if observed == 0.0:
        return 1.0
    generator = random.Random(seed)
    extreme = 0
    for _ in range(samples):
        swapped = sum(
            (right - left) if generator.random() < 0.5 else (left - right)
            for left, right in pairs
        ) / len(pairs)
        if abs(swapped) >= observed - 1e-12:
            extreme += 1
    return (extreme + 1) / (samples + 1)


def effect_size(deltas: list[float]) -> float | None:
    """Cohen's d_z for paired deltas; None when undefined (n<2 or no spread)."""
    if len(deltas) < 2:
        return None
    spread = stdev(deltas)
    return mean(deltas) / spread if spread > 0 else None


def minimum_detectable_effect(
    deltas: list[float], alpha: float = 0.05, power: float = 0.8
) -> float | None:
    """Smallest |true mean delta| detectable at `power` with a two-sided
    `alpha` test, via the normal approximation: (z_a + z_power) * se.
    Tells the user the resolution limit of the current suite."""
    if len(deltas) < 2:
        return None
    z_alpha = _inverse_normal_cdf(1 - alpha / 2)
    z_power = _inverse_normal_cdf(power)
    return (z_alpha + z_power) * stdev(deltas) / math.sqrt(len(deltas))


def holm_bonferroni(pvalues: dict[str, float], alpha: float = 0.05) -> dict[str, bool]:
    """Holm step-down correction over a family of p-values. Returns which
    hypotheses remain significant after controlling FWER at `alpha`."""
    ordered = sorted(pvalues.items(), key=lambda item: item[1])
    m = len(ordered)
    significant: dict[str, bool] = {}
    rejected_all = True
    for index, (name, p) in enumerate(ordered):
        threshold = alpha / (m - index)
        if rejected_all and p <= threshold:
            significant[name] = True
        else:
            rejected_all = False
            significant[name] = False
    return significant


def compare_metrics(
    baseline: dict[str, list[float]],
    candidate: dict[str, list[float]],
    alpha: float = 0.05,
    seed: int = SEED,
    clusters: dict[str, list[str]] | None = None,
) -> dict[str, Comparison]:
    """Full paired comparison for every metric present in both systems'
    per-case observation lists. `baseline`/`candidate` map metric name to a
    list of per-case values aligned by case order. `clusters` maps metric
    name to cluster labels aligned with that metric's values — metrics
    skipped per case (claim_support, component_*, budget_compliant) have
    shorter value lists, so one global label list would misalign."""
    shared = sorted(set(baseline) & set(candidate))
    raw: dict[str, Comparison] = {}
    pvalues: dict[str, float] = {}
    for name in shared:
        left, right = baseline[name], candidate[name]
        metric_clusters = clusters.get(name) if clusters else None
        deltas = [right_v - left_v for left_v, right_v in zip(left, right, strict=True)]
        value, low, high = bootstrap_ci(deltas, seed=seed, clusters=metric_clusters)
        p = permutation_pvalue(left, right, seed=seed, clusters=metric_clusters)
        effective = (
            len(collapse_clusters(deltas, metric_clusters))
            if metric_clusters
            else len(deltas)
        )
        raw[name] = Comparison(
            metric=name,
            delta=value,
            ci_low=low,
            ci_high=high,
            samples=effective,
            p_value=p,
            effect_size=effect_size(
                collapse_clusters(deltas, metric_clusters) if metric_clusters else deltas
            ),
            mde=minimum_detectable_effect(
                collapse_clusters(deltas, metric_clusters) if metric_clusters else deltas,
                alpha=alpha,
            ),
        )
        if p is not None:
            pvalues[name] = p
    for name, rejected in holm_bonferroni(pvalues, alpha).items():
        comparison = raw[name]
        raw[name] = Comparison(
            metric=comparison.metric,
            delta=comparison.delta,
            ci_low=comparison.ci_low,
            ci_high=comparison.ci_high,
            samples=comparison.samples,
            p_value=comparison.p_value,
            effect_size=comparison.effect_size,
            mde=comparison.mde,
            significant=rejected,
        )
    return raw


def _inverse_normal_cdf(p: float) -> float:
    """Acklam's rational approximation of the standard normal quantile."""
    if not 0.0 < p < 1.0:
        raise ValueError("p must be in (0, 1)")
    a = [-3.969683028665376e01, 2.209460984245205e02, -2.759285104469687e02,
         1.383577518672690e02, -3.066479806614716e01, 2.506628277459239e00]
    b = [-5.447609879822406e01, 1.615858368580409e02, -1.556989798598866e02,
         6.680131188771972e01, -1.328068155288572e01]
    c = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e00,
         -2.549732539343734e00, 4.374664141464968e00, 2.938163982698783e00]
    d = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e00,
         3.754408661907416e00]
    low, high = 0.02425, 1 - 0.02425
    if p < low:
        q = math.sqrt(-2 * math.log(p))
        return (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / (
            (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1
        )
    if p <= high:
        q = p - 0.5
        r = q * q
        return (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q / (
            ((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1
        )
    q = math.sqrt(-2 * math.log(1 - p))
    return -(((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / (
        (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1
    )
