from __future__ import annotations

import math
import random
from collections import defaultdict
from dataclasses import dataclass
from statistics import mean

from .schema import BenchmarkCase, CaseResult, LineRange, RetrievedRange


@dataclass(frozen=True)
class MetricSummary:
    value: float
    ci_low: float
    ci_high: float
    samples: int


def evaluate(cases: list[BenchmarkCase], results: list[CaseResult]) -> dict[str, MetricSummary]:
    by_case = {case.case_id: case for case in cases}
    result_by_case = {result.case_id: result for result in results}
    if missing := sorted(set(by_case) - set(result_by_case)):
        raise ValueError(f"missing results for {len(missing)} cases: {missing[:5]}")

    observations: dict[str, list[float]] = defaultdict(list)
    for case_id, case in by_case.items():
        result = result_by_case[case_id]
        for cutoff in (5, 10, 20, 50):
            observations[f"recall@{cutoff}"].append(range_recall(case, result.retrieved[:cutoff]))
            observations[f"file_recall@{cutoff}"].append(
                file_recall(case, result.retrieved[:cutoff])
            )
        observations["mrr"].append(reciprocal_rank(case, result.retrieved))
        observations["ndcg@10"].append(ndcg(case, result.retrieved[:10]))
        observations["symbol_recall@20"].append(symbol_recall(case, result.retrieved[:20]))
        observations["line_recall@20"].append(range_recall(case, result.retrieved[:20]))
        observations["file_success@20"].append(file_success(case, result.retrieved[:20]))
        observations["abstention_accuracy"].append(float(result.abstained == case.no_context))
        observations["relevant_line_density"].append(relevant_line_density(case, result.retrieved))
        observations["citation_correctness"].append(citation_correctness(result.retrieved))
        observations["query_ms"].append(result.query_ms)

    summaries = {name: bootstrap(values) for name, values in sorted(observations.items())}
    predictions = [result_by_case[case_id].abstained for case_id in by_case]
    labels = [by_case[case_id].no_context for case_id in by_case]
    predicted_no_context = sum(predictions)
    true_no_context = sum(
        prediction and label for prediction, label in zip(predictions, labels, strict=True)
    )
    no_context_cases = sum(labels)
    precision = true_no_context / predicted_no_context if predicted_no_context else 1.0
    false_positive_rate = (
        sum(
            (not prediction) and label
            for prediction, label in zip(predictions, labels, strict=True)
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
    gains = [float(relevant(case, candidate)) for candidate in retrieved]
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


def bootstrap(values: list[float], samples: int = 2000, seed: int = 0xCCE) -> MetricSummary:
    if not values:
        return MetricSummary(0.0, 0.0, 0.0, 0)
    generator = random.Random(seed)
    estimates = sorted(mean(generator.choices(values, k=len(values))) for _ in range(samples))
    return MetricSummary(
        value=mean(values),
        ci_low=estimates[int(samples * 0.025)],
        ci_high=estimates[min(samples - 1, int(samples * 0.975))],
        samples=len(values),
    )
