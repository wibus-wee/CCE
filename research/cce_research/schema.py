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
    no_context: bool = False
    budget_tokens: int = Field(default=8192, ge=256, le=128_000)
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
    index_ms: float | None = Field(default=None, ge=0)
    query_ms: float = Field(ge=0)
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


def load_jsonl[ModelT: BaseModel](path: Path, model: type[ModelT]) -> Iterator[ModelT]:
    import json

    with path.open(encoding="utf-8") as handle:
        for number, line in enumerate(handle, start=1):
            if line.strip():
                try:
                    yield model.model_validate(json.loads(line))
                except Exception as error:
                    raise ValueError(f"{path}:{number}: {error}") from error
