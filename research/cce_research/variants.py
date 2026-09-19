"""Metamorphic query variants: same gold, perturbed query.

The keyword classifier's known fragility is surface form ("break" vs
"breaks", English vs CJK phrasing). Variants measure robustness directly:
each derived case reuses the parent's gold and is expected to retrieve the
same evidence. `variant_agreement` reports how often a variant reproduces
the parent's outcome; large disagreement = the system is sensitive to
phrasing it should not care about.

Transforms are deterministic and offline — no LLM paraphraser, matching the
engine's own constraint surface.
"""

from __future__ import annotations

import random
import re
from statistics import mean

from .metrics import case_observations
from .schema import BenchmarkCase, CaseResult

# Inflection and synonym swaps known to matter for this codebase's planner
# keyword surface. Ordered; each rule applies at most once per query.
SWAPS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\bbreaks\b", re.IGNORECASE), "break"),
    (re.compile(r"\bfails\b", re.IGNORECASE), "fail"),
    (re.compile(r"\baffects\b", re.IGNORECASE), "affect"),
    (re.compile(r"\bwhere is\b", re.IGNORECASE), "where can I find"),
    (re.compile(r"\bwhere does\b", re.IGNORECASE), "where do"),
    (re.compile(r"\bhow does\b", re.IGNORECASE), "in what way does"),
    (re.compile(r"\bwhat breaks\b", re.IGNORECASE), "what would break"),
    (re.compile(r"\bwhy does\b", re.IGNORECASE), "for what reason does"),
]

CJK_GLOSSARY: dict[str, str] = {
    "返回": "return",
    "标记": "mark",
    "影响": "affect",
    "哪些": "which",
    "失败": "failure",
    "错误": "error",
    "陈旧": "stale",
    "过期": "stale",
}

STOPWORDS = {
    "the", "a", "an", "is", "does", "do", "to", "of", "in", "for", "if",
    "what", "which", "how", "why", "where", "when", "would", "can", "i",
    "me", "my", "this", "that",
}


def transform(query: str, kind: str, seed: int) -> str:
    if kind == "inflection":
        for pattern, replacement in SWAPS:
            if pattern.search(query):
                return pattern.sub(replacement, query, count=1)
        return query
    if kind == "mixed-language":
        for source, target in CJK_GLOSSARY.items():
            if source in query:
                return query.replace(source, target, 1)
        return query
    if kind == "verbose":
        return f"please help me understand: {query}?"
    if kind == "keyword-scramble":
        words = query.split()
        content = [word for word in words if word.lower() not in STOPWORDS]
        if len(content) < 2:
            return query
        generator = random.Random(f"{seed}:{query}")
        generator.shuffle(content)
        return " ".join(content)
    if kind == "assertive":
        return f"I'm certain this exists in the codebase: {query}"
    if kind == "hedged":
        return f"not sure this exists, but if it does: {query}"
    raise ValueError(f"unknown variant transform {kind}")


TRANSFORMS = (
    "inflection",
    "mixed-language",
    "verbose",
    "keyword-scramble",
    "assertive",
    "hedged",
)


# Prefix-only transforms that never remove query content — safe to apply
# to abstention cases, where they double as sycophancy probes: "I'm
# certain this exists" must not flip an honest empty answer, and "please
# help me understand" must not conjure a capability.
NO_CONTEXT_SAFE_TRANSFORMS = ("verbose", "assertive", "hedged")


def generate_variants(
    cases: list[BenchmarkCase],
    transforms: tuple[str, ...] = TRANSFORMS,
    seed: int = 0xCCE,
    skip_no_context: bool = True,
) -> list[BenchmarkCase]:
    """Derived cases: one per (case, transform) where the transform actually
    changes the query. `derived_from`/`derivation` carry the lineage.
    Abstention (`no_context`) cases are only mutated by prefix-only
    transforms — mutating or dropping query terms could make an
    unanswerable query answerable, which would invalidate the gold."""
    derived: list[BenchmarkCase] = []
    for case in cases:
        if case.no_context and skip_no_context:
            kinds: tuple[str, ...] = NO_CONTEXT_SAFE_TRANSFORMS
        else:
            kinds = transforms
        for index, kind in enumerate(kinds):
            query = transform(case.query, kind, seed + index)
            if query == case.query:
                continue
            derived.append(
                case.model_copy(
                    update={
                        "case_id": f"{case.case_id}::var-{kind}",
                        "query": query,
                        "derived_from": case.case_id,
                        "derivation": f"variant:{kind}",
                        "tags": [*case.tags, "variant"],
                    }
                )
            )
    return derived


def variant_agreement(
    cases: list[BenchmarkCase],
    baseline: list[CaseResult],
    variant_results: list[CaseResult],
    metric: str = "file_success@20",
) -> dict[str, float]:
    """How often a variant reproduces its parent's outcome on `metric`.

    Returns {"agreement": fraction, "mean_abs_delta": mean |Δ|,
    "variants": n}. Agreement near 1 with small deltas = the system is
    phrasing-insensitive; low agreement exposes brittle routing.
    """
    case_by_id = {case.case_id: case for case in cases}
    baseline_by_case = {result.case_id: result for result in baseline}
    agree = 0
    deltas: list[float] = []
    total = 0
    for result in variant_results:
        parent_id, _, _ = result.case_id.rpartition("::var-")
        if not parent_id:
            continue
        parent = case_by_id.get(parent_id)
        base_result = baseline_by_case.get(parent_id)
        if parent is None or base_result is None:
            continue
        base = case_observations(parent, base_result).get(metric)
        variant = case_observations(parent, result).get(metric)
        if base is None or variant is None:
            continue
        total += 1
        deltas.append(variant - base)
        if variant == base:
            agree += 1
    return {
        "agreement": agree / total if total else 1.0,
        "mean_abs_delta": mean(abs(delta) for delta in deltas) if deltas else 0.0,
        "variants": float(total),
    }
