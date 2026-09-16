"""Route ablation: leave-one-route-out (LORO) case generation and marginal
route utility.

A case's executed plan routes are captured in `CaseResult.plan_routes`. For
every case we emit one derived case per executed route, pinning
`routes = plan_routes - {that route}`. Comparing the ablation bundle against
the full-plan baseline yields each route's marginal contribution to the
metrics — direct evidence for "is this route earning its keep or adding
noise".

Derived cases share gold/provenance with the parent and carry
`derived_from`/`derivation` lineage so `evaluate` can group or exclude them.
"""

from __future__ import annotations

from collections import defaultdict
from statistics import mean

from .metrics import case_observations
from .schema import BenchmarkCase, CaseResult


def route_ablation_cases(
    cases: list[BenchmarkCase], results: list[CaseResult]
) -> list[BenchmarkCase]:
    """One derived case per (case, executed route): that route removed.

    Source of truth for the route set: `CaseResult.plan_routes` when the
    case did not pin routes, else the pinned set. Cases whose plan used a
    single route are skipped (removing it produces an empty retrieval,
    which is a degenerate ablation).
    """
    result_by_case = {result.case_id: result for result in results}
    derived: list[BenchmarkCase] = []
    for case in cases:
        result = result_by_case.get(case.case_id)
        base_routes = list(case.routes) if case.routes else (
            list(result.plan_routes) if result else []
        )
        if len(base_routes) < 2:
            continue
        for removed in base_routes:
            remaining = [route for route in base_routes if route != removed]
            derived.append(
                case.model_copy(
                    update={
                        "case_id": f"{case.case_id}::abl-{removed}",
                        "routes": remaining,
                        "supply_intent": True,
                        "derived_from": case.case_id,
                        "derivation": f"ablate:{removed}",
                        "tags": [*case.tags, "ablation"],
                    }
                )
            )
    return derived


def marginal_utility(
    cases: list[BenchmarkCase],
    baseline: list[CaseResult],
    ablations: list[CaseResult],
    metric: str = "file_recall@20",
) -> dict[str, dict[str, float]]:
    """Mean metric delta per removed route, aggregated over parent cases.

    Returns {route: {"mean_delta": Δ, "cases": n, "worst_delta": min Δ}} —
    a negative mean means the route was contributing recall; near-zero means
    the route is redundant for that case set.
    """
    case_by_id = {case.case_id: case for case in cases}
    baseline_by_case = {result.case_id: result for result in baseline}
    # Derived case ids are `parent::abl-<route>`; map back to the parent case.
    per_route: dict[str, list[float]] = defaultdict(list)
    for result in ablations:
        parent_id, _, suffix = result.case_id.rpartition("::abl-")
        if not parent_id or not suffix:
            continue
        parent = case_by_id.get(parent_id)
        base_result = baseline_by_case.get(parent_id)
        if parent is None or base_result is None:
            continue
        base = case_observations(parent, base_result).get(metric)
        ablated = case_observations(parent, result).get(metric)
        if base is None or ablated is None:
            continue
        per_route[suffix].append(ablated - base)
    return {
        route: {
            "mean_delta": mean(deltas),
            "worst_delta": min(deltas),
            "cases": float(len(deltas)),
        }
        for route, deltas in sorted(per_route.items())
    }


def executable_routes(results: list[CaseResult]) -> list[str]:
    """Union of routes observed across a run — the ablation axis."""
    routes: set[str] = set()
    for result in results:
        routes.update(result.plan_routes)
    return sorted(routes)
