from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

Intent = Literal[
    "exact_entity",
    "natural_language_behavior",
    "issue_localization",
    "trace",
    "impact",
    "architecture",
    "history",
    "precise_dataflow",
]

Route = Literal[
    "no_retrieval",
    "exact_symbol",
    "lexical",
    "dense_raw",
    "dense_summary",
    "hybrid",
    "structural",
    "knowledge",
    "history",
    "reranked",
]


class LineRange(BaseModel):
    model_config = ConfigDict(extra="forbid")

    path: str
    start_line: int = Field(ge=1)
    end_line: int = Field(ge=1)
    symbol: str | None = None

    @model_validator(mode="after")
    def ordered(self) -> LineRange:
        if self.end_line < self.start_line:
            raise ValueError("end_line must be >= start_line")
        return self


class Provenance(BaseModel):
    model_config = ConfigDict(extra="forbid")

    source_url: str
    dataset_revision: str
    license_spdx: str
    redistribution: Literal["allowed", "metadata_only", "prohibited"]
    construction_method: str


class GoldFact(BaseModel):
    """An atomic claim the case's evidence must support. A fact counts as
    supported when any retrieved range overlaps one of `evidence` ranges or
    carries one of `symbols` — deterministic, no LLM judge required.
    This distinguishes "found the file" from "found the answer"."""

    model_config = ConfigDict(extra="forbid")

    claim: str
    evidence: list[LineRange] = Field(default_factory=list)
    symbols: list[str] = Field(default_factory=list)


class BenchmarkCase(BaseModel):
    model_config = ConfigDict(extra="forbid")

    case_id: str
    repository: str
    revision: str
    query: str
    intent: Intent
    gold_files: list[str] = Field(default_factory=list)
    gold_symbols: list[str] = Field(default_factory=list)
    gold_ranges: list[LineRange] = Field(default_factory=list)
    supporting_ranges: list[LineRange] = Field(default_factory=list)
    # Atomic facts the evidence must support (claim-level recall). Finer
    # than file recall: the right file at the wrong lines still fails.
    gold_facts: list[GoldFact] = Field(default_factory=list)
    # Paths an adjudicator has reviewed and marked NOT relevant. Retrieved
    # items outside gold ∪ supporting ∪ judged are "unjudged" — they feed
    # `unjudged_rate` and the adjudication queue instead of counting as
    # false positives (gold sets are never complete; pooling lesson).
    judged_files: list[str] = Field(default_factory=list)
    # Packages (L2 component proxy from `cce map`) the gold evidence lives
    # in — e.g. "cce-store". Feeds component_recall@k / component_mrr, the
    # package-granularity stand-in for Task→Component Recall. Empty = metric
    # skipped for the case.
    gold_components: list[str] = Field(default_factory=list)
    no_context: bool = False
    budget_tokens: int = Field(default=8192, ge=256, le=128_000)
    # When false the adapter must not pass the intent to the system; the
    # system's own classifier decides and `predicted_intent` is scored against
    # `intent`. This is how planner/classifier quality is measured.
    supply_intent: bool = True
    # Non-empty routes override the planner's route selection (ablation rows).
    routes: list[Route] = Field(default_factory=list)
    # Derived-case lineage: `derived_from` is the parent case_id,
    # `derivation` names the transform (e.g. "ablate:structural",
    # "variant:inflection", "permute:3").
    derived_from: str | None = None
    derivation: str | None = None
    # Optional extraction key for downstream answer probes: the string a
    # correct consumer must be able to produce from the pack.
    answer_key: str | None = None
    provenance: Provenance
    tags: list[str] = Field(default_factory=list)

    @model_validator(mode="after")
    def has_gold_or_abstention(self) -> BenchmarkCase:
        if not self.no_context and not (self.gold_files or self.gold_symbols or self.gold_ranges):
            raise ValueError("non-abstention cases require gold evidence")
        return self


class RetrievedRange(LineRange):
    route: str
    rank: int = Field(ge=1)
    score: float
    estimated_tokens: int = Field(ge=0)
    citation_verified: bool = False


class CaseResult(BaseModel):
    model_config = ConfigDict(extra="forbid")

    case_id: str
    system: str
    system_revision: str
    dataset_revision: str
    retrieved: list[RetrievedRange]
    abstained: bool = False
    # Plan actually executed, as reported by the system. `predicted_intent` is
    # what the planner resolved (supplied or classified); `plan_routes` and
    # `graph_policy` record the routing decision so misrouting is attributable.
    predicted_intent: str | None = None
    plan_routes: list[str] = Field(default_factory=list)
    graph_policy: str | None = None
    missing_capabilities: list[str] = Field(default_factory=list)
    index_ms: float | None = Field(default=None, ge=0)
    query_ms: float = Field(ge=0)
    # Package name → rootDir map captured once per run from `cce map`;
    # shared by every result so retrieved paths resolve to components.
    component_map: dict[str, str] = Field(default_factory=dict)
    peak_memory_bytes: int | None = Field(default=None, ge=0)
    index_bytes: int | None = Field(default=None, ge=0)
    metadata: dict[str, str | int | float | bool | None] = Field(default_factory=dict)


class ResultBundleManifest(BaseModel):
    model_config = ConfigDict(extra="forbid")

    schema_version: int = 1
    system: str
    system_revision: str
    dataset_revision: str
    dataset_sha256: str
    adapter_sha256: str
    results_sha256: str
    model_identity: str
    model_revision: str
    generated_at: str
    operating_system: str
    machine: str
    processor: str
    cpu_count: int | None
    python_version: str
    dependency_lock_sha256: dict[str, str]
    environment_keys: list[str]
    # Package count in the component_map captured for this run; None when
    # the adapter could not produce one (no map support, map failed).
    component_map_packages: int | None = None


def load_jsonl[ModelT: BaseModel](path: Path, model: type[ModelT]) -> Iterator[ModelT]:
    import json

    with path.open(encoding="utf-8") as handle:
        for number, line in enumerate(handle, start=1):
            if line.strip():
                try:
                    yield model.model_validate(json.loads(line))
                except Exception as error:
                    raise ValueError(f"{path}:{number}: {error}") from error
