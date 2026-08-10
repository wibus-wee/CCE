"""Repository-held-out cross-encoder quality and runtime benchmark."""

from __future__ import annotations

import json
import multiprocessing
import os
import platform
import resource
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import numpy as np
import yaml

from .model_benchmark import (
    evaluate_ranks,
    percentile,
    require_sentence_transformers,
    sha256_file,
)
from .schema import TrainingExample, load_jsonl


@dataclass(frozen=True)
class RerankerModelSpec:
    name: str
    model: str
    revision: str | None = None
    trust_remote_code: bool = False
    backend: str = "torch"
    file_name: str | None = None


@dataclass(frozen=True)
class RerankerMeasurement:
    model: str
    revision: str | None
    queries: int
    pairs: int
    recall_at_1: float
    recall_at_5: float
    recall_at_10: float
    mrr: float
    ndcg_at_10: float
    metrics_by_query_kind: dict[str, dict[str, float]]
    case_ranks: list[dict[str, str | int]]
    pairs_per_second: float
    query_p50_ms: float
    query_p95_ms: float
    peak_memory_bytes: int


def load_reranker_specs(path: Path) -> list[RerankerModelSpec]:
    payload = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or not isinstance(payload.get("models"), list):
        raise ValueError(f"{path}: expected a models list")
    specs = [
        RerankerModelSpec(
            name=str(item["name"]),
            model=str(item["model"]),
            revision=str(item["revision"]) if item.get("revision") else None,
            trust_remote_code=bool(item.get("trust_remote_code", False)),
            backend=str(item.get("backend", "torch")),
            file_name=str(item["file_name"]) if item.get("file_name") else None,
        )
        for item in payload["models"]
        if isinstance(item, dict)
    ]
    if len(specs) != len(payload["models"]):
        raise ValueError(f"{path}: every model must be an object")
    if len({spec.name for spec in specs}) != len(specs):
        raise ValueError("benchmark reranker names must be unique")
    return specs


def benchmark_rerankers(
    dataset: Path,
    model_config: Path,
    output: Path,
    batch_size: int = 8,
    candidates: int = 16,
    limit: int | None = None,
) -> list[RerankerMeasurement]:
    if batch_size < 1 or candidates < 2:
        raise ValueError("batch_size must be positive and candidates must be at least two")
    examples = list(load_jsonl(dataset, TrainingExample))
    if limit is not None:
        examples = examples[:limit]
    if not examples:
        raise ValueError("benchmark dataset is empty")
    measurements = [
        benchmark_reranker_isolated(spec, examples, batch_size, candidates)
        for spec in load_reranker_specs(model_config)
    ]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "dataset": str(dataset.resolve()),
                "datasetSha256": sha256_file(dataset),
                "modelConfigSha256": sha256_file(model_config),
                "candidatePolicy": "one-positive-plus-repository-local-mined-hard-negatives",
                "maximumCandidates": candidates,
                "repositories": sorted({item.positive.repository for item in examples}),
                "hardware": {
                    "platform": platform.platform(),
                    "machine": platform.machine(),
                    "processor": platform.processor(),
                    "cpuCount": os.cpu_count(),
                },
                "measurements": [asdict(measurement) for measurement in measurements],
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return measurements


def benchmark_reranker(
    spec: RerankerModelSpec,
    examples: list[TrainingExample],
    batch_size: int,
    candidates: int,
) -> RerankerMeasurement:
    sentence_transformers: Any = require_sentence_transformers()

    model_kwargs: dict[str, Any] = {}
    if spec.backend == "onnx":
        model_kwargs["provider"] = "CPUExecutionProvider"
        if spec.file_name:
            model_kwargs["file_name"] = spec.file_name
    model = sentence_transformers.CrossEncoder(
        spec.model,
        revision=spec.revision,
        trust_remote_code=spec.trust_remote_code,
        backend=spec.backend,
        model_kwargs=model_kwargs,
        max_length=512,
    )
    ranks: list[int] = []
    ranks_by_kind: dict[str, list[int]] = {}
    case_ranks: list[dict[str, str | int]] = []
    latencies: list[float] = []
    pair_count = 0
    started_all = time.perf_counter()
    for example in examples:
        documents = [example.positive, *example.negatives[: candidates - 1]]
        pairs = [(example.query, document.text) for document in documents]
        started = time.perf_counter()
        scores = np.asarray(
            model.predict(pairs, batch_size=batch_size, show_progress_bar=False), dtype=np.float32
        ).reshape(-1)
        latencies.append((time.perf_counter() - started) * 1000)
        order = np.argsort(-scores, kind="stable")
        rank = int(np.flatnonzero(order == 0)[0]) + 1
        ranks.append(rank)
        ranks_by_kind.setdefault(example.query_kind, []).append(rank)
        pair_count += len(pairs)
        case_ranks.append(
            {
                "example_id": example.example_id,
                "repository": example.positive.repository,
                "query_kind": example.query_kind,
                "rank": rank,
                "candidate_documents": len(pairs),
            }
        )
    elapsed = time.perf_counter() - started_all
    metrics = evaluate_ranks(ranks)
    peak_memory_bytes = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if platform.system() != "Darwin":
        peak_memory_bytes *= 1024
    return RerankerMeasurement(
        model=spec.name,
        revision=spec.revision,
        queries=len(examples),
        pairs=pair_count,
        recall_at_1=metrics["recall@1"],
        recall_at_5=metrics["recall@5"],
        recall_at_10=metrics["recall@10"],
        mrr=metrics["mrr"],
        ndcg_at_10=metrics["ndcg@10"],
        metrics_by_query_kind={
            kind: evaluate_ranks(kind_ranks) for kind, kind_ranks in sorted(ranks_by_kind.items())
        },
        case_ranks=case_ranks,
        pairs_per_second=pair_count / elapsed if elapsed else 0.0,
        query_p50_ms=percentile(latencies, 0.5),
        query_p95_ms=percentile(latencies, 0.95),
        peak_memory_bytes=peak_memory_bytes,
    )


def benchmark_reranker_isolated(
    spec: RerankerModelSpec,
    examples: list[TrainingExample],
    batch_size: int,
    candidates: int,
) -> RerankerMeasurement:
    context = multiprocessing.get_context("spawn")
    receiving, sending = context.Pipe(duplex=False)
    process = context.Process(
        target=benchmark_worker,
        args=(sending, spec, examples, batch_size, candidates),
        daemon=False,
    )
    process.start()
    sending.close()
    process.join()
    if not receiving.poll():
        raise RuntimeError(f"benchmark worker for {spec.name} exited with {process.exitcode}")
    ok, payload = receiving.recv()
    receiving.close()
    if not ok:
        raise RuntimeError(f"benchmark worker for {spec.name} failed: {payload}")
    return RerankerMeasurement(**payload)


def benchmark_worker(
    connection: Any,
    spec: RerankerModelSpec,
    examples: list[TrainingExample],
    batch_size: int,
    candidates: int,
) -> None:
    try:
        measurement = benchmark_reranker(spec, examples, batch_size, candidates)
        connection.send((True, asdict(measurement)))
    except Exception as error:
        connection.send((False, f"{type(error).__name__}: {error}"))
    finally:
        connection.close()
