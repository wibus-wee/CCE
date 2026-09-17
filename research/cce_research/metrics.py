from __future__ import annotations

import math
from collections import defaultdict
from dataclasses import dataclass
from statistics import mean

from .schema import BenchmarkCase, CaseResult, LineRange, RetrievedRange
from .stats import Comparison, bootstrap_ci, compare_metrics


@dataclass(frozen=True)
class MetricSummary:
    value: float
    ci_low: float
    ci_high: float
    samples: int


def evaluate(cases: list[BenchmarkCase], results: list[CaseResult]) -> dict[str, MetricSummary]:
    """Per-metric summaries: overall, plus `by_intent/<intent>/<metric>`
    groups, plus `intent_accuracy` and per-class intent precision/recall
    over cases that withhold intent."""
    by_case = {case.case_id: case for case in cases}
    result_by_case = {result.case_id: result for result in results}
    if missing := sorted(set(by_case) - set(result_by_case)):
        raise ValueError(f"missing results for {len(cases)} cases: {missing[:5]}")

    observations: dict[str, list[float]] = defaultdict(list)
    confusion = intent_confusion(cases, results)
    for case_id, case in by_case.items():
        result = result_by_case[case_id]
        per_case = case_observations(case, result)
        for name, value in per_case.items():
            observations[name].append(value)
            observations[f"by_intent/{case.intent}/{name}"].append(value)
        if not case.supply_intent:
            observations["intent_accuracy"].append(
                float(result.predicted_intent == case.intent)
            )

    summaries = {name: bootstrap(values) for name, values in sorted(observations.items())}
    for gold_intent, predictions in confusion.items():
        total = sum(predictions.values())
        correct = predictions.get(gold_intent, 0)
        summaries[f"intent_recall/{gold_intent}"] = MetricSummary(
            correct / total if total else 0.0,
            correct / total if total else 0.0,
            correct / total if total else 0.0,
            total,
        )
    predicted_totals: dict[str, int] = defaultdict(int)
    predicted_correct: dict[str, int] = defaultdict(int)
    for gold_intent, predictions in confusion.items():
        for predicted, count in predictions.items():
            predicted_totals[predicted] += count
            if predicted == gold_intent:
                predicted_correct[predicted] += count
    for predicted, total in predicted_totals.items():
        value = predicted_correct[predicted] / total
        summaries[f"intent_precision/{predicted}"] = MetricSummary(value, value, value, total)

    predictions_flags = [result_by_case[case_id].abstained for case_id in by_case]
    labels = [by_case[case_id].no_context for case_id in by_case]
    predicted_no_context = sum(predictions_flags)
    true_no_context = sum(
        prediction and label for prediction, label in zip(predictions_flags, labels, strict=True)
    )
    no_context_cases = sum(labels)
    precision = true_no_context / predicted_no_context if predicted_no_context else 1.0
    false_positive_rate = (
        sum(
            (not prediction) and label
            for prediction, label in zip(predictions_flags, labels, strict=True)
        )
        / no_context_cases
        if no_context_cases
        else 0.0
    )
    summaries["no_context_precision"] = MetricSummary(precision, precision, precision, len(cases))
    summaries["false_positive_rate"] = MetricSummary(
        false_positive_rate, false_positive_rate, false_positive_rate, len(cases)
    )
    return dict(sorted(summaries.items()))


def compare(
    cases: list[BenchmarkCase],
    baseline: list[CaseResult],
    candidate: list[CaseResult],
) -> dict[str, Comparison]:
    """Paired per-case deltas (candidate - baseline) with bootstrap CI,
    permutation p-value, Holm-corrected significance, effect size, and the
    minimum detectable effect of the current suite. Systems must cover the
    same cases."""
    by_case = {case.case_id: case for case in cases}
    baseline_by_case = {result.case_id: result for result in baseline}
    candidate_by_case = {result.case_id: result for result in candidate}
    shared = sorted(
        set(by_case) & set(baseline_by_case) & set(candidate_by_case)
    )
    if missing := sorted(set(by_case) - set(shared)):
        raise ValueError(f"case coverage differs across systems: {missing[:5]}")

    left_obs: dict[str, list[float]] = defaultdict(list)
    right_obs: dict[str, list[float]] = defaultdict(list)
    for case_id in shared:
        case = by_case[case_id]
        left = case_observations(case, baseline_by_case[case_id])
        right = case_observations(case, candidate_by_case[case_id])
        for name in left.keys() & right.keys():
            left_obs[name].append(left[name])
            right_obs[name].append(right[name])
    return compare_metrics(dict(left_obs), dict(right_obs))


def intent_confusion(
    cases: list[BenchmarkCase], results: list[CaseResult]
) -> dict[str, dict[str, int]]:
    """gold intent -> predicted intent -> count, over withheld-intent cases."""
    result_by_case = {result.case_id: result for result in results}
    confusion: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for case in cases:
        if case.supply_intent:
            continue
        result = result_by_case.get(case.case_id)
        predicted = (result.predicted_intent if result else None) or "unresolved"
        confusion[case.intent][predicted] += 1
    return {gold: dict(row) for gold, row in confusion.items()}


def case_observations(case: BenchmarkCase, result: CaseResult) -> dict[str, float]:
    """All per-case metric values; shared by evaluate() and compare() so both
    always score the same definitions."""
    observations: dict[str, float] = {}
    for cutoff in (5, 10, 20, 50):
        observations[f"recall@{cutoff}"] = range_recall(case, result.retrieved[:cutoff])
        observations[f"file_recall@{cutoff}"] = file_recall(case, result.retrieved[:cutoff])
        observations[f"unjudged_rate@{cutoff}"] = unjudged_rate(
            case, result.retrieved[:cutoff]
        )
    observations["mrr"] = reciprocal_rank(case, result.retrieved)
    observations["ndcg@10"] = ndcg(case, result.retrieved[:10])
    observations["symbol_recall@20"] = symbol_recall(case, result.retrieved[:20])
    observations["line_recall@20"] = range_recall(case, result.retrieved[:20])
    observations["file_success@20"] = file_success(case, result.retrieved[:20])
    observations["bpref@20"] = bpref(case, result.retrieved[:20])
    if case.gold_facts:
        observations["claim_support"] = claim_support(case, result.retrieved)
    if case.expect_missing_capability:
        expected = case.expect_missing_capability.lower()
        observations["capability_contract"] = float(
            any(expected in capability.lower() for capability in result.missing_capabilities)
        )
    observations["abstention_accuracy"] = float(result.abstained == case.no_context)
    observations["relevant_line_density"] = relevant_line_density(case, result.retrieved)
    observations["citation_correctness"] = citation_correctness(result.retrieved)
    if case.gold_components and result.component_map:
        observations["component_recall_at_5"] = component_recall(
            case, result.retrieved[:5], result.component_map
        )
        observations["component_recall_at_20"] = component_recall(
            case, result.retrieved[:20], result.component_map
        )
        observations["component_mrr"] = component_mrr(
            case, result.retrieved, result.component_map
        )
    observations["query_ms"] = result.query_ms
    if result.index_ms is not None:
        observations["index_ms"] = result.index_ms
    return observations


def overlaps(left: LineRange, right: LineRange) -> bool:
    return (
        left.path == right.path
        and left.start_line <= right.end_line
        and right.start_line <= left.end_line
    )


def range_recall(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    if case.no_context:
        return float(not retrieved)
    if not case.gold_ranges:
        return file_recall(case, retrieved)
    covered = sum(
        any(overlaps(gold, candidate) for candidate in retrieved) for gold in case.gold_ranges
    )
    return covered / len(case.gold_ranges)


def file_recall(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    gold = set(case.gold_files) | {item.path for item in case.gold_ranges}
    if not gold:
        return float(case.no_context and not retrieved)
    found = {item.path for item in retrieved}
    return len(gold & found) / len(gold)


def file_success(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    gold = set(case.gold_files) | {item.path for item in case.gold_ranges}
    return float(gold <= {item.path for item in retrieved}) if gold else float(not retrieved)


def reciprocal_rank(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    for index, candidate in enumerate(retrieved, start=1):
        if relevant(case, candidate):
            return 1.0 / index
    return 0.0


def ndcg(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    # Relevance gain counts once per (path, gold unit): without deduping,
    # several retrieved ranges inside one gold file inflate DCG past ideal
    # and nDCG exceeds 1.0.
    scored: set[tuple[str, int | None]] = set()
    gains: list[float] = []
    for candidate in retrieved:
        gain = 0.0
        for gold_index, gold in enumerate(case.gold_ranges):
            key: tuple[str, int | None] = (candidate.path, gold_index)
            if key not in scored and overlaps(gold, candidate):
                scored.add(key)
                gain = 1.0
        if candidate.path in case.gold_files:
            key = (candidate.path, None)
            if key not in scored:
                scored.add(key)
                gain = 1.0
        gains.append(gain)
    dcg = sum(gain / math.log2(index + 2) for index, gain in enumerate(gains))
    gold_items = len(case.gold_ranges) if case.gold_ranges else len(set(case.gold_files))
    ideal_relevant = min(gold_items, len(retrieved))
    ideal = sum(1.0 / math.log2(index + 2) for index in range(ideal_relevant))
    return dcg / ideal if ideal else 0.0


def symbol_recall(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    gold = set(case.gold_symbols)
    if not gold:
        return 1.0
    found = {candidate.symbol for candidate in retrieved if candidate.symbol}
    return len(gold & found) / len(gold)


def relevant(case: BenchmarkCase, candidate: RetrievedRange) -> bool:
    return candidate.path in case.gold_files or any(
        overlaps(gold, candidate) for gold in case.gold_ranges
    )


def judged_paths(case: BenchmarkCase) -> set[str]:
    """Every path with a known relevance verdict: gold, supporting, and
    adjudicated-irrelevant. Anything else retrieved is 'unjudged'."""
    return (
        set(case.gold_files)
        | {item.path for item in case.gold_ranges}
        | {item.path for item in case.supporting_ranges}
        | set(case.judged_files)
    )


def unjudged_rate(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """Fraction of retrieved paths with no relevance verdict. High values
    flag gold incompleteness — those items need adjudication, and metrics
    that treat them as irrelevant are biased (TREC pooling lesson)."""
    if not retrieved:
        return 0.0
    judged = judged_paths(case)
    return sum(item.path not in judged for item in retrieved) / len(retrieved)


def bpref(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """Binary preference: robust to incomplete gold because only *judged*
    nonrelevant items count against a relevant hit (Buckley & Voorhees).
    bpref = (1/R) * Σ_r (1 - min(nonrel_before_r, R)/R)."""
    gold = set(case.gold_files) | {item.path for item in case.gold_ranges}
    if not gold:
        return float(not retrieved)
    judged_nonrelevant = set(case.judged_files) - gold
    total_relevant = len(gold)
    nonrel_before = 0
    score = 0.0
    # Each gold path earns gain once, at its first occurrence — repeated
    # ranges inside one file must not inflate the score past 1.0.
    scored_gold: set[str] = set()
    scored_nonrel: set[str] = set()
    for candidate in retrieved:
        if candidate.path in gold:
            if candidate.path not in scored_gold:
                scored_gold.add(candidate.path)
                score += 1.0 - min(nonrel_before, total_relevant) / total_relevant
        elif candidate.path in judged_nonrelevant and candidate.path not in scored_nonrel:
            scored_nonrel.add(candidate.path)
            nonrel_before += 1
        # Unjudged items are neither reward nor penalty — that is the point.
    return score / total_relevant


def claim_support(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """Fraction of `gold_facts` supported by retrieved evidence: any overlap
    with a fact's evidence ranges, or a retrieved symbol in the fact's
    `symbols`. Deterministic claim-level recall — separates 'found the file'
    from 'found the answer'."""
    if not case.gold_facts:
        return 1.0
    found_symbols = {candidate.symbol for candidate in retrieved if candidate.symbol}
    supported = 0
    for fact in case.gold_facts:
        if any(
            any(overlaps(evidence, candidate) for candidate in retrieved)
            for evidence in fact.evidence
        ) or any(symbol in found_symbols for symbol in fact.symbols):
            supported += 1
    return supported / len(case.gold_facts)


def relevant_line_density(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    total = sum(item.end_line - item.start_line + 1 for item in retrieved)
    if total == 0:
        return float(case.no_context)
    relevant_lines = 0
    for item in retrieved:
        if item.path in case.gold_files and not case.gold_ranges:
            relevant_lines += item.end_line - item.start_line + 1
            continue
        for gold in case.gold_ranges:
            if item.path == gold.path:
                relevant_lines += max(
                    0, min(item.end_line, gold.end_line) - max(item.start_line, gold.start_line) + 1
                )
    return min(1.0, relevant_lines / total)


def citation_correctness(retrieved: list[RetrievedRange]) -> float:
    return mean([float(item.citation_verified) for item in retrieved]) if retrieved else 1.0


def path_component(path: str, component_map: dict[str, str]) -> str | None:
    """Resolve a repo-relative path to its package via longest rootDir
    prefix match. rootDir ''/'.' is the workspace root package and matches
    everything, so deeper packages always win over it."""
    best: str | None = None
    best_depth = -1
    for name, root in component_map.items():
        prefix = "" if root in ("", ".") else root.rstrip("/") + "/"
        if path == root.rstrip("/") or path.startswith(prefix):
            depth = len(prefix)
            if depth > best_depth:
                best = name
                best_depth = depth
    return best


def retrieved_components(
    retrieved: list[RetrievedRange], component_map: dict[str, str]
) -> list[str | None]:
    return [path_component(item.path, component_map) for item in retrieved]


def component_recall(
    case: BenchmarkCase, retrieved: list[RetrievedRange], component_map: dict[str, str]
) -> float:
    """Task→Component Recall@K at package granularity: share of
    `gold_components` whose package produced at least one top-K hit."""
    gold = set(case.gold_components)
    found = {component for component in retrieved_components(retrieved, component_map)}
    found.discard(None)
    return len(gold & found) / len(gold)


def component_mrr(
    case: BenchmarkCase, retrieved: list[RetrievedRange], component_map: dict[str, str]
) -> float:
    """Reciprocal rank of the first hit inside any gold component — the
    'time to first correct component' of the report, as an MRR."""
    gold = set(case.gold_components)
    for index, component in enumerate(
        retrieved_components(retrieved, component_map), start=1
    ):
        if component in gold:
            return 1.0 / index
    return 0.0


def bootstrap(values: list[float], samples: int = 2000, seed: int = 0xCCE) -> MetricSummary:
    value, low, high = bootstrap_ci(values, samples=samples, seed=seed)
    return MetricSummary(value=value, ci_low=low, ci_high=high, samples=len(values))
