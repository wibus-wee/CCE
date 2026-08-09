from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class RetrievalEvent:
    document_id: str
    stage: str
    tokens: int


def summarize(events: list[RetrievalEvent]) -> dict[str, float | int]:
    stages = {
        "retrieved",
        "shown_to_model",
        "opened_by_agent",
        "cited_or_used",
        "edited_or_affected",
    }
    counts = {
        stage: len({event.document_id for event in events if event.stage == stage})
        for stage in stages
    }
    retrieved = counts["retrieved"]
    return {
        **{f"unique_{stage}": count for stage, count in counts.items()},
        "tokens": sum(event.tokens for event in events),
        "retrieved_to_used_ratio": counts["cited_or_used"] / retrieved if retrieved else 0.0,
    }
