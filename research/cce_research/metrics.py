from __future__ import annotations

import math
from collections import defaultdict
from dataclasses import dataclass
from statistics import mean

from .schema import BenchmarkCase, CaseResult, LineRange, RetrievedRange
from .stats import Comparison, bootstrap_ci, collapse_clusters, compare_metrics


def case_clusters(cases: list[BenchmarkCase]) -> dict[str, str]:
    """case_id -> cluster root. Derived cases (`::var-*`, `::b*` budget
    curves, pinned ablations) replicate their parent — they are not
    independent evidence. Every statistic that treats rows as iid would
    over-weight variant-heavy families and report dishonestly narrow CIs;
    collapsing to cluster means makes a family exactly one observation.
    Transitive lineage (variant of a variant) resolves to the ultimate
    root; a missing parent id still groups siblings correctly."""
    roots = {case.case_id: (case.derived_from or case.case_id) for case in cases}

    def root_of(case_id: str) -> str:
        seen: set[str] = set()
        current = case_id
        while current in roots and roots[current] != current and current not in seen:
            seen.add(current)
            current = roots[current]
        return current

    return {case_id: root_of(case_id) for case_id in roots}

# Metrics where a *larger* value is worse: a guardrail regression is a
# significant positive delta, not a negative one. Everything not listed
# here is higher-is-better. `unjudged_rate` is deliberately absent — it
# measures gold completeness, not system quality, so gating on it would
# penalize a system merely for surfacing unverdicted evidence.
LOWER_IS_BETTER = frozenset(
    {
        "decoy_hit_rate@5",
        "decoy_hit_rate@10",
        "decoy_hit_rate@20",
        "decoy_hit_rate@50",
        "false_positive_rate",
        "redundancy@5",
        "redundancy@10",
        "query_ms",
        "index_ms",
    }
)


@dataclass(frozen=True)
class MetricSummary:
    value: float
    ci_low: float
    ci_high: float
    samples: int


def evaluate(cases: list[BenchmarkCase], results: list[CaseResult]) -> dict[str, MetricSummary]:
    """Per-metric summaries: overall, plus `by_intent/<intent>/<metric>`
    and `by_tag/<tag>/<metric>` groups, plus `intent_accuracy` and
    per-class intent precision/recall over cases that withhold intent.
    All bootstrapped statistics collapse derived-case families to cluster
    means first — see `case_clusters`."""
    by_case = {case.case_id: case for case in cases}
    result_by_case = {result.case_id: result for result in results}
    if missing := sorted(set(by_case) - set(result_by_case)):
        raise ValueError(f"missing results for {len(cases)} cases: {missing[:5]}")

    cluster_of = case_clusters(cases)
    observations: dict[str, list[float]] = defaultdict(list)
    metric_clusters: dict[str, list[str]] = defaultdict(list)
    confusion = intent_confusion(cases, results)
    for case_id, case in by_case.items():
        result = result_by_case[case_id]
        per_case = case_observations(case, result)
        for name, value in per_case.items():
            for group in [name, f"by_intent/{case.intent}/{name}"]:
                observations[group].append(value)
                metric_clusters[group].append(cluster_of[case_id])
            for tag in case.tags:
                observations[f"by_tag/{tag}/{name}"].append(value)
                metric_clusters[f"by_tag/{tag}/{name}"].append(cluster_of[case_id])
        if not case.supply_intent:
            observations["intent_accuracy"].append(
                float(result.predicted_intent == case.intent)
            )
            metric_clusters["intent_accuracy"].append(cluster_of[case_id])

    summaries = {
        name: bootstrap(collapse_clusters(values, metric_clusters[name]))
        for name, values in sorted(observations.items())
    }
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

    ordered_ids = list(by_case)
    cluster_labels = [cluster_of[case_id] for case_id in ordered_ids]
    predictions_flags = collapse_clusters(
        [float(result_by_case[case_id].abstained) for case_id in ordered_ids],
        cluster_labels,
    )
    labels = collapse_clusters(
        [float(by_case[case_id].no_context) for case_id in ordered_ids],
        cluster_labels,
    )
    predicted_no_context = sum(predictions_flags)
    # no_context is constant within a derived family, so the product of
    # cluster means equals the mean of per-case products.
    true_no_context = sum(
        prediction * label
        for prediction, label in zip(predictions_flags, labels, strict=True)
    )
    no_context_cases = sum(labels)
    precision = true_no_context / predicted_no_context if predicted_no_context else 1.0
    false_positive_rate = (
        sum(
            (1.0 - prediction) * label
            for prediction, label in zip(predictions_flags, labels, strict=True)
        )
        / no_context_cases
        if no_context_cases
        else 0.0
    )
    effective = len(predictions_flags)
    summaries["no_context_precision"] = MetricSummary(precision, precision, precision, effective)
    summaries["false_positive_rate"] = MetricSummary(
        false_positive_rate, false_positive_rate, false_positive_rate, effective
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
    versions = {result.metrics_version for result in baseline} | {
        result.metrics_version for result in candidate
    }
    if len(versions) > 1:
        raise ValueError(
            "cannot compare results recorded under different metrics_version "
            f"{sorted(versions)}: flat-row and item-aware rank/budget semantics "
            "are not commensurable — re-collect one side under matching "
            "accounting"
        )
    by_case = {case.case_id: case for case in cases}
    baseline_by_case = {result.case_id: result for result in baseline}
    candidate_by_case = {result.case_id: result for result in candidate}
    shared = sorted(
        set(by_case) & set(baseline_by_case) & set(candidate_by_case)
    )
    if missing := sorted(set(by_case) - set(shared)):
        raise ValueError(f"case coverage differs across systems: {missing[:5]}")

    cluster_of = case_clusters(cases)
    left_obs: dict[str, list[float]] = defaultdict(list)
    right_obs: dict[str, list[float]] = defaultdict(list)
    metric_clusters: dict[str, list[str]] = defaultdict(list)
    for case_id in shared:
        case = by_case[case_id]
        left = case_observations(case, baseline_by_case[case_id])
        right = case_observations(case, candidate_by_case[case_id])
        for name in left.keys() & right.keys():
            left_obs[name].append(left[name])
            right_obs[name].append(right[name])
            metric_clusters[name].append(cluster_of[case_id])
    return compare_metrics(dict(left_obs), dict(right_obs), clusters=dict(metric_clusters))


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


def observed_ranges(result: CaseResult, cutoff: int | None = None) -> list[RetrievedRange]:
    """The first `cutoff` *items'* citations as flat rows.

    Item-aware results (metrics_version 2) take the top-k items and then
    expand each item's addresses — an item citing three addresses still
    occupies one rank slot. Legacy results have no item identity, so the
    flat rows are sliced as before (address-per-row semantics); metrics
    that depend on item identity or budgets report themselves
    uncomputable instead of guessing."""
    if result.items:
        items = result.items[:cutoff] if cutoff is not None else result.items
        return [row for item in items for row in item.to_ranges()]
    rows = result.retrieved
    return rows[:cutoff] if cutoff is not None else rows


def case_observations(case: BenchmarkCase, result: CaseResult) -> dict[str, float]:
    """All per-case metric values; shared by evaluate() and compare() so both
    always score the same definitions."""
    observations: dict[str, float] = {}
    for cutoff in (5, 10, 20, 50):
        observations[f"recall@{cutoff}"] = range_recall(case, observed_ranges(result, cutoff))
        observations[f"file_recall@{cutoff}"] = file_recall(case, observed_ranges(result, cutoff))
        observations[f"unjudged_rate@{cutoff}"] = unjudged_rate(
            case, observed_ranges(result, cutoff)
        )
        observations[f"decoy_hit_rate@{cutoff}"] = decoy_hit_rate(
            case, observed_ranges(result, cutoff)
        )
    observations["mrr"] = reciprocal_rank(case, observed_ranges(result))
    observations["ndcg@10"] = ndcg(case, observed_ranges(result, 10))
    if case.gold_symbols:
        # Skipped on unannotated cases rather than reported as 1.0 — the
        # aggregate's N then equals the annotated subset size.
        observations["symbol_recall@20"] = symbol_recall(case, observed_ranges(result, 20))
    observations["line_recall@20"] = range_recall(case, observed_ranges(result, 20))
    observations["file_success@20"] = file_success(case, observed_ranges(result, 20))
    observations["bpref@20"] = bpref(case, observed_ranges(result, 20))
    for cutoff in (1, 3, 5):
        observations[f"file_hit@{cutoff}"] = file_hit(case, observed_ranges(result, cutoff))
    for cutoff in (5, 10):
        observations[f"redundancy@{cutoff}"] = redundancy(case, observed_ranges(result, cutoff))
    if case.gold_facts:
        supported, undecidable = claim_support(case, observed_ranges(result))
        if supported is not None:
            observations["claim_support"] = supported
            observations["claim_support_legacy"] = claim_support_legacy(
                case, observed_ranges(result)
            )
        if undecidable:
            observations["claim_support_unscoped"] = float(undecidable)
        pack = pack_sufficiency(case, result)
        if pack is not None:
            observations["pack_sufficiency"] = pack
    if case.expect_missing_capability:
        expected = case.expect_missing_capability.lower()
        observations["capability_contract"] = float(
            any(expected in capability.lower() for capability in result.missing_capabilities)
        )
    observations["abstention_accuracy"] = float(result.abstained == case.no_context)
    if result.verdict_state is not None:
        # The engine's own verdict is a separate event from "no returned
        # rows": weak_witness keeps candidates, abstained keeps none.
        # One-hot flags let aggregates report each state's share.
        observations["verdict_answered"] = float(result.verdict_state == "answered")
        observations["verdict_weak_witness"] = float(result.verdict_state == "weak_witness")
        observations["verdict_abstained"] = float(result.verdict_state == "abstained")
        observations["verdict_flagged"] = float(result.verdict_state != "answered")
    budget = spent_tokens(result)
    if budget is not None:
        # Protocol honesty: the declared budget is a contract, not a
        # hint. A pack that exceeds it gains recall by smuggling tokens.
        # The pack's own accounting (top-level usedTokens, else the sum
        # over items including address-less orientation) is authoritative
        # — legacy flat rows double-counted per address and are skipped.
        observations["used_tokens"] = float(budget)
        observations["budget_compliant"] = float(budget <= case.budget_tokens)
    observations["relevant_line_density"] = relevant_line_density(case, observed_ranges(result))
    observations["citation_correctness"] = citation_correctness(observed_ranges(result))
    if case.gold_components and result.component_map:
        observations["component_recall_at_3"] = component_recall(
            case, observed_ranges(result, 3), result.component_map
        )
        observations["component_recall_at_5"] = component_recall(
            case, observed_ranges(result, 5), result.component_map
        )
        observations["component_recall_at_20"] = component_recall(
            case, observed_ranges(result, 20), result.component_map
        )
        observations["component_mrr"] = component_mrr(
            case, observed_ranges(result), result.component_map
        )
    observations["query_ms"] = result.query_ms
    if result.index_ms is not None:
        observations["index_ms"] = result.index_ms
    return observations


def spent_tokens(result: CaseResult) -> int | None:
    """Tokens the packed result consumed, or None when uncomputable.

    Only context packs carry a packing budget. Item-aware results use
    the reported `usedTokens` when present and fall back to the item
    sum — orientation items have no addresses but still consume budget,
    which is exactly what flat-row summation missed. Legacy flat results
    cannot be trusted (each row repeats its item's full cost), so they
    report None instead of a wrong number."""
    if result.result_kind != "context":
        return None
    if result.used_tokens is not None:
        return result.used_tokens
    if result.items:
        return sum(item.estimated_tokens for item in result.items)
    return None


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


def file_hit(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """Any gold file inside the top-k — the skim-depth success signal.
    `file_success` demands *all* gold (a pack-sufficiency bar); `file_hit`
    asks whether a human skimming the first few results sees anything
    relevant at all. Complements MRR, which is rank-weighted rather than
    a hard depth cutoff."""
    gold = set(case.gold_files) | {item.path for item in case.gold_ranges}
    if not gold:
        return float(case.no_context and not retrieved)
    return float(bool(gold & {item.path for item in retrieved}))


def redundancy(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """Share of top-k slots repeating a path already seen above them —
    wasted attention for a human skimming, wasted tokens for a pack.
    Skipped on no_context cases: every hit there is already a convicted
    decoy, so charging redundancy too would double-count one failure."""
    if not retrieved or case.no_context:
        return 0.0
    seen: set[str] = set()
    duplicates = 0
    for item in retrieved:
        if item.path in seen:
            duplicates += 1
        else:
            seen.add(item.path)
    return duplicates / len(retrieved)


def pack_sufficiency(case: BenchmarkCase, result: CaseResult) -> float | None:
    """Fraction of gold_facts whose evidence survived budget-constrained
    packing — the downstream-sufficiency question: can a consumer assert
    every required fact from what actually fit the token budget?

    Only context results carry a packing stage, and only item-aware
    results record what actually got packed: `items` is already the
    post-budget set, so this is strict claim support over their
    citations — no re-packing against flat rows (which double-counted
    each address's share). Returns None when uncomputable."""
    if result.result_kind != "context" or not result.items:
        return None
    packed = [row for item in result.items for row in item.to_ranges()]
    supported, _ = claim_support(case, packed)
    return supported


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
    that treat them as irrelevant are biased (TREC pooling lesson).
    On a no_context case nothing is relevant, so every hit already carries
    a verdict: there is no adjudication debt to report."""
    if not retrieved or case.no_context:
        return 0.0
    judged = judged_paths(case)
    return sum(item.path not in judged for item in retrieved) / len(retrieved)


def decoy_hit_rate(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """Fraction of retrieved paths that are confirmed-irrelevant — the
    known-wrong share of the result list. Complements `unjudged_rate`:
    unjudged items might still be relevant; decoy hits are confirmed not.
    Unlike bpref, decoys count wherever they rank, not only before gold.
    On a no_context case *every* surfaced path is a false positive by
    definition — no adjudication is needed to convict it, so the rate is
    1.0 whenever anything is retrieved."""
    if not retrieved:
        return 0.0
    if case.no_context:
        return 1.0
    gold = set(case.gold_files) | {item.path for item in case.gold_ranges}
    judged_nonrelevant = set(case.judged_files) - gold
    return sum(item.path in judged_nonrelevant for item in retrieved) / len(retrieved)


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


def claim_support(
    case: BenchmarkCase, retrieved: list[RetrievedRange]
) -> tuple[float | None, int]:
    """Strict fact support: fraction of *decidable* `gold_facts` backed by
    retrieved evidence, plus the count of undecidable ones.

    A fact is supported when a retrieved range overlaps one of its
    `evidence` ranges, or when a retrieved citation carries one of its
    `symbols` *on a path the fact's evidence localizes* — a same-named
    symbol in an unrelated file does not support the claim. A fact with
    no evidence paths and only bare symbols cannot scope the symbol
    check: it is undecidable under the strict rule and excluded from the
    denominator (reported via the returned count, never silently
    converted to a pass or a fail). Returns (None, n) when nothing is
    decidable."""
    decidable = 0
    undecidable = 0
    supported = 0
    for fact in case.gold_facts:
        evidence_paths = {evidence.path for evidence in fact.evidence}
        evidence_hit = any(
            any(overlaps(evidence, candidate) for candidate in retrieved)
            for evidence in fact.evidence
        )
        if not evidence_paths:
            # Bare-symbol (or empty) facts cannot be located: no path set
            # to scope the symbol against. Undecidable, not a failure.
            undecidable += 1
            continue
        decidable += 1
        scoped_symbol_hit = any(
            candidate.symbol in fact.symbols and candidate.path in evidence_paths
            for candidate in retrieved
        )
        supported += int(evidence_hit or scoped_symbol_hit)
    if not decidable:
        return None, undecidable
    return supported / decidable, undecidable


def claim_support_legacy(case: BenchmarkCase, retrieved: list[RetrievedRange]) -> float:
    """The pre-012 loose criterion kept for continuity reporting only:
    any evidence overlap or any bare symbol name anywhere. Never mix
    with `claim_support` — it accepts wrong-file same-name symbols."""
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
