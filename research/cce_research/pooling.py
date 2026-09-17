"""Judgment pooling: detect gold-set incompleteness and drive adjudication.

Gold sets are never complete — a system that retrieves a genuinely relevant
file the annotator missed is penalized by naive metrics (the TREC pooling
lesson, and SWE-bench's "40% superset of oracle files" observation). This
module:

1. builds an adjudication queue: unjudged paths surfaced by system runs,
   ranked by how often/how high they appear across bundles;
2. applies human judgments back into the dataset: `relevant` verdicts are
   promoted into `gold_files`, `irrelevant` into `judged_files`.

Until adjudicated, `unjudged_rate@k` (metrics.py) reports how much of the
top-k has no verdict, and `bpref` scores only judged nonrelevant evidence.
"""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field
from typing import Literal

from .metrics import judged_paths
from .schema import BenchmarkCase, CaseResult

Verdict = Literal["relevant", "irrelevant"]


@dataclass(frozen=True)
class QueueEntry:
    """One path awaiting a relevance verdict for one case."""

    case_id: str
    path: str
    best_rank: int
    occurrences: int
    bundles: tuple[str, ...] = field(compare=False)
    routes: tuple[str, ...] = field(compare=False)


@dataclass
class _Accum:
    best_rank: int
    occurrences: int
    bundles: set[str]
    routes: set[str]


def adjudication_queue(
    cases: list[BenchmarkCase],
    bundles: list[tuple[str, list[CaseResult]]],
    depth: int = 20,
) -> list[QueueEntry]:
    """Unjudged paths in the top-`depth` of each run, pooled across bundles.

    Paths retrieved by several systems rank first — they are the most likely
    to be true-but-missed relevance (or shared blind spots). Gold,
    supporting, and already-judged paths are excluded.
    """
    by_case = {case.case_id: case for case in cases}
    seen: dict[tuple[str, str], _Accum] = defaultdict(
        lambda: _Accum(best_rank=depth + 1, occurrences=0, bundles=set(), routes=set())
    )
    for bundle_name, results in bundles:
        for result in results:
            case = by_case.get(result.case_id)
            if case is None:
                continue
            judged = judged_paths(case)
            for index, item in enumerate(result.retrieved[:depth], start=1):
                if item.path in judged:
                    continue
                entry = seen[(result.case_id, item.path)]
                entry.occurrences += 1
                entry.best_rank = min(entry.best_rank, index)
                entry.bundles.add(bundle_name)
                entry.routes.add(item.route)
    queue = [
        QueueEntry(
            case_id=case_id,
            path=path,
            best_rank=data.best_rank,
            occurrences=data.occurrences,
            bundles=tuple(sorted(data.bundles)),
            routes=tuple(sorted(data.routes)),
        )
        for (case_id, path), data in seen.items()
    ]
    # Most-shared evidence first, then highest-ranked.
    return sorted(queue, key=lambda entry: (-entry.occurrences, entry.best_rank, entry.case_id))


def apply_judgments(
    cases: list[BenchmarkCase], judgments: list[tuple[str, str, Verdict]]
) -> list[BenchmarkCase]:
    """Fold adjudication verdicts into cases. `relevant` promotes a path to
    `gold_files`; `irrelevant` records it in `judged_files`. Paths judged
    relevant that were already gold are skipped silently.
    """
    by_case: dict[str, BenchmarkCase] = {case.case_id: case for case in cases}
    for case_id, path, verdict in judgments:
        case = by_case.get(case_id)
        if case is None:
            raise ValueError(f"judgment references unknown case {case_id}")
        if verdict == "relevant":
            if path not in case.gold_files and path not in case.judged_files:
                case.gold_files.append(path)
        else:
            if path not in case.gold_files and path not in case.judged_files:
                case.judged_files.append(path)
    return list(by_case.values())
