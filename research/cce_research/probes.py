"""Probes that answer questions aggregate metrics cannot.

1. Position permutation: the packer now reorders hits into role quotas, so
   "rank" inside a pack no longer means "best evidence first". Permuting
   each result's item order and re-scoring order-sensitive metrics (mrr,
   ndcg) quantifies how much of a metric is *order* versus *content*. If
   permuted scores barely move, the metric was measuring ordering noise;
   if they collapse, order carried real signal. This is the offline half of
   a lost-in-the-middle probe — the downstream-consumer half needs a model
   and stays opt-in.

2. Staleness probe: CCE's differentiator is explicit view freshness
   (`verified_current`). The probe writes a marker file into the repository,
   runs a fresh query through the adapter, mutates the file, re-runs, and
   reports whether fresh content surfaces and whether stale hits linger.
"""

from __future__ import annotations

import random
import time
from dataclasses import dataclass, field
from statistics import mean, pstdev
from typing import TYPE_CHECKING

from pathlib import Path

from .metrics import case_observations
from .schema import BenchmarkCase, CaseResult, Provenance

if TYPE_CHECKING:
    from .adapters import Adapter


ORDER_SENSITIVE = ("mrr", "ndcg@10")


@dataclass(frozen=True)
class PermutationReport:
    """Per-case score spread of order-sensitive metrics under permutation."""

    per_metric_mean: dict[str, float]
    per_metric_spread: dict[str, float]
    permutations: int
    cases: int


def permute_results(
    results: list[CaseResult], permutations: int, seed: int = 0xCCE
) -> list[list[CaseResult]]:
    """`permutations` shuffled copies of each result's retrieved list."""
    generator = random.Random(seed)
    shuffled_runs: list[list[CaseResult]] = []
    for _ in range(permutations):
        run: list[CaseResult] = []
        for result in results:
            items = list(result.retrieved)
            generator.shuffle(items)
            run.append(result.model_copy(update={"retrieved": items}))
        shuffled_runs.append(run)
    return shuffled_runs


def position_sensitivity(
    cases: list[BenchmarkCase],
    permuted_runs: list[list[CaseResult]],
    metrics: tuple[str, ...] = ORDER_SENSITIVE,
) -> PermutationReport:
    """Mean and per-case spread (population stdev over permutations) of each
    order-sensitive metric. High spread = the metric mostly reflects pack
    ordering, not content."""
    case_by_id = {case.case_id: case for case in cases}
    per_metric_values: dict[str, list[float]] = {name: [] for name in metrics}
    per_metric_spreads: dict[str, list[float]] = {name: [] for name in metrics}
    for case in cases:
        series: dict[str, list[float]] = {name: [] for name in metrics}
        for run in permuted_runs:
            result = next((r for r in run if r.case_id == case.case_id), None)
            if result is None:
                continue
            observations = case_observations(case, result)
            for name in metrics:
                if name in observations:
                    series[name].append(observations[name])
        for name in metrics:
            values = series[name]
            if not values:
                continue
            per_metric_values[name].append(mean(values))
            per_metric_spreads[name].append(pstdev(values))
    return PermutationReport(
        per_metric_mean={
            name: mean(values) for name, values in per_metric_values.items() if values
        },
        per_metric_spread={
            name: mean(spreads) for name, spreads in per_metric_spreads.items() if spreads
        },
        permutations=len(permuted_runs),
        cases=len(cases),
    )


@dataclass
class StalenessReport:
    marker_found_on_create: bool
    marker_found_after_edit: bool
    stale_marker_survives: bool
    verified_current_after_edit: bool | None
    create_query_ms: float
    edit_query_ms: float
    notes: list[str] = field(default_factory=list)


def run_staleness_probe(
    adapter: Adapter, repository_root: Path, system_revision: str = "WORKTREE"
) -> StalenessReport:
    """Mutate-then-query probe for index freshness.

    Creates `.cce-probe/marker-<ts>.md` inside the repository, queries for a
    unique token, rewrites the file with a different token, queries again.
    Restores (deletes) the marker file in `finally`. Reports freshness
    signals; the caller decides whether the behavior is acceptable.
    """
    # Marker lives at the repository root: a `.cce-probe/` directory would be
    # excluded by the repo's own `/.cce*/` ignore rules and never indexed.
    marker = repository_root / f"cce-probe-marker-{int(time.time())}.md"
    token_a = f"cceprobe{int(time.time())}a"
    token_b = f"cceprobe{int(time.time())}b"

    def synthetic_case(query: str) -> BenchmarkCase:
        return BenchmarkCase(
            case_id="staleness-probe",
            repository=str(repository_root),
            revision=system_revision,
            query=query,
            intent="exact_entity",
            gold_files=[str(marker.relative_to(repository_root))],
            provenance=Provenance(
                source_url="local-probe",
                dataset_revision="probe",
                license_spdx="Proprietary",
                redistribution="prohibited",
                construction_method="staleness probe (synthetic)",
            ),
        )

    report = StalenessReport(
        marker_found_on_create=False,
        marker_found_after_edit=False,
        stale_marker_survives=False,
        verified_current_after_edit=None,
        create_query_ms=0.0,
        edit_query_ms=0.0,
    )
    try:
        marker.write_text(f"token {token_a}\n", encoding="utf-8")
        first = adapter.run(synthetic_case(token_a), repository_root, system_revision)
        report.create_query_ms = first.query_ms
        report.marker_found_on_create = any(
            item.path.endswith(marker.name) for item in first.retrieved
        )

        marker.write_text(f"token {token_b}\n", encoding="utf-8")
        second = adapter.run(synthetic_case(token_b), repository_root, system_revision)
        report.edit_query_ms = second.query_ms
        marker_hits = [
            item for item in second.retrieved if item.path.endswith(marker.name)
        ]
        report.marker_found_after_edit = bool(marker_hits)
        report.verified_current_after_edit = (
            all(item.citation_verified for item in marker_hits) if marker_hits else None
        )
        # A stale index would still surface the marker for the OLD token.
        stale = adapter.run(synthetic_case(token_a), repository_root, system_revision)
        report.stale_marker_survives = any(
            item.path.endswith(marker.name) for item in stale.retrieved
        )
        if report.stale_marker_survives:
            report.notes.append(
                "old token still retrieves the marker after rewrite — index may be serving stale content"
            )
    finally:
        marker.unlink(missing_ok=True)
    return report
