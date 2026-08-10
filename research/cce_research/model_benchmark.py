"""Repository-level embedding quality, latency, and memory benchmark."""

from __future__ import annotations

import hashlib
import json
import multiprocessing
import os
import platform
import resource
import statistics
import time
from dataclasses import asdict, dataclass
from importlib import import_module
from pathlib import Path
from typing import Any

import numpy as np
import yaml

from .schema import TrainingExample, load_jsonl


@dataclass(frozen=True)
class EmbeddingModelSpec:
    name: str
    model: str
    revision: str | None = None
    code_revision: str | None = None
    query_prefix: str = ""
    document_prefix: str = ""
    trust_remote_code: bool = False
    backend: str = "torch"
    file_name: str | None = None


@dataclass(frozen=True)
class ModelMeasurement:
    model: str
    revision: str | None
    queries: int
    documents: int
    dimensions: int
    recall_at_1: float
    recall_at_5: float
    recall_at_10: float
    recall_at_20: float
    mrr: float
    ndcg_at_10: float
    metrics_by_query_kind: dict[str, dict[str, float]]
    case_ranks: list[dict[str, str | int]]
    index_seconds: float
    documents_per_second: float
    query_batch_seconds: float
    queries_per_second: float
    single_query_p50_ms: float
    single_query_p95_ms: float
    peak_memory_bytes: int


def load_model_specs(path: Path) -> list[EmbeddingModelSpec]:
    payload = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or not isinstance(payload.get("models"), list):
        raise ValueError(f"{path}: expected a models list")
    output: list[EmbeddingModelSpec] = []
    for raw in payload["models"]:
        if not isinstance(raw, dict):
            raise ValueError(f"{path}: every model must be an object")
        output.append(
            EmbeddingModelSpec(
                name=str(raw["name"]),
                model=str(raw["model"]),
                revision=str(raw["revision"]) if raw.get("revision") else None,
                code_revision=(
                    str(raw["code_revision"]) if raw.get("code_revision") else None
                ),
                query_prefix=str(raw.get("query_prefix", "")),
                document_prefix=str(raw.get("document_prefix", "")),
                trust_remote_code=bool(raw.get("trust_remote_code", False)),
                backend=str(raw.get("backend", "torch")),
                file_name=str(raw["file_name"]) if raw.get("file_name") else None,
            )
        )
    if len({model.name for model in output}) != len(output):
        raise ValueError("benchmark model names must be unique")
    return output


def benchmark_models(
    dataset: Path,
    model_config: Path,
    output: Path,
    batch_size: int = 16,
    limit: int | None = None,
) -> list[ModelMeasurement]:
    if batch_size < 1:
        raise ValueError("batch_size must be positive")
    examples = list(load_jsonl(dataset, TrainingExample))
    if limit is not None:
        examples = examples[:limit]
    if not examples:
        raise ValueError("benchmark dataset is empty")
    specs = load_model_specs(model_config)
    measurements = [benchmark_model_isolated(spec, examples, batch_size) for spec in specs]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "dataset": str(dataset.resolve()),
                "datasetSha256": sha256_file(dataset),
                "modelConfigSha256": sha256_file(model_config),
                "repositories": sorted({item.positive.repository for item in examples}),
                "queryKinds": sorted({item.query_kind for item in examples}),
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


def benchmark_model(
    spec: EmbeddingModelSpec,
    examples: list[TrainingExample],
    batch_size: int,
) -> ModelMeasurement:
    sentence_transformers: Any = require_sentence_transformers()
    model_kwargs: dict[str, Any] = {}
    if spec.backend == "onnx":
        model_kwargs["provider"] = "CPUExecutionProvider"
        if spec.file_name:
            model_kwargs["file_name"] = spec.file_name
    if spec.code_revision:
        model_kwargs["code_revision"] = spec.code_revision
    model = sentence_transformers.SentenceTransformer(
        spec.model,
        revision=spec.revision,
        trust_remote_code=spec.trust_remote_code,
        backend=spec.backend,
        model_kwargs=model_kwargs,
    )
    documents_by_repository: dict[str, dict[str, str]] = {}
    examples_by_repository: dict[str, list[TrainingExample]] = {}
    for example in examples:
        examples_by_repository.setdefault(example.positive.repository, []).append(example)
        corpus = documents_by_repository.setdefault(example.positive.repository, {})
        corpus[example.positive.document_id] = example.positive.text
        for negative in example.negatives:
            corpus[negative.document_id] = negative.text

    ranks: list[int] = []
    ranks_by_query_kind: dict[str, list[int]] = {}
    case_ranks: list[dict[str, str | int]] = []
    total_documents = 0
    dimensions = 0
    index_seconds = 0.0
    query_batch_seconds = 0.0
    query_latencies: list[float] = []
    for repository, repository_examples in sorted(examples_by_repository.items()):
        corpus = documents_by_repository[repository]
        document_ids = sorted(corpus)
        document_texts = [spec.document_prefix + corpus[identifier] for identifier in document_ids]
        started = time.perf_counter()
        document_embeddings = encode(model, document_texts, batch_size)
        index_seconds += time.perf_counter() - started
        queries = [spec.query_prefix + example.query for example in repository_examples]
        started = time.perf_counter()
        query_embeddings = encode(model, queries, batch_size)
        query_batch_seconds += time.perf_counter() - started
        if dimensions == 0:
            dimensions = int(document_embeddings.shape[1])
        total_documents += len(document_ids)
        index_by_id = {identifier: index for index, identifier in enumerate(document_ids)}
        for query_embedding, example in zip(query_embeddings, repository_examples, strict=True):
            scores = document_embeddings @ query_embedding
            order = np.argsort(-scores, kind="stable")
            gold_index = index_by_id[example.positive.document_id]
            rank = int(np.flatnonzero(order == gold_index)[0]) + 1
            ranks.append(rank)
            ranks_by_query_kind.setdefault(example.query_kind, []).append(rank)
            case_ranks.append(
                {
                    "example_id": example.example_id,
                    "repository": repository,
                    "query_kind": example.query_kind,
                    "positive_document_id": example.positive.document_id,
                    "rank": rank,
                    "candidate_documents": len(document_ids),
                }
            )
        for query in queries[: min(20, len(queries))]:
            started = time.perf_counter()
            encode(model, [query], 1)
            query_latencies.append((time.perf_counter() - started) * 1000)
    metrics = evaluate_ranks(ranks)
    peak_memory_bytes = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if platform.system() != "Darwin":
        peak_memory_bytes *= 1024
    return ModelMeasurement(
        model=spec.name,
        revision=spec.revision,
        queries=len(examples),
        documents=total_documents,
        dimensions=dimensions,
        recall_at_1=metrics["recall@1"],
        recall_at_5=metrics["recall@5"],
        recall_at_10=metrics["recall@10"],
        recall_at_20=metrics["recall@20"],
        mrr=metrics["mrr"],
        ndcg_at_10=metrics["ndcg@10"],
        metrics_by_query_kind={
            kind: evaluate_ranks(kind_ranks)
            for kind, kind_ranks in sorted(ranks_by_query_kind.items())
        },
        case_ranks=case_ranks,
        index_seconds=index_seconds,
        documents_per_second=total_documents / index_seconds if index_seconds else 0.0,
        query_batch_seconds=query_batch_seconds,
        queries_per_second=len(examples) / query_batch_seconds if query_batch_seconds else 0.0,
        single_query_p50_ms=percentile(query_latencies, 0.5),
        single_query_p95_ms=percentile(query_latencies, 0.95),
        peak_memory_bytes=peak_memory_bytes,
    )


def benchmark_model_isolated(
    spec: EmbeddingModelSpec,
    examples: list[TrainingExample],
    batch_size: int,
) -> ModelMeasurement:
    """Measure each model in a clean process so peak RSS is not order-dependent."""

    context = multiprocessing.get_context("spawn")
    receiving, sending = context.Pipe(duplex=False)
    process = context.Process(
        target=benchmark_worker,
        args=(sending, spec, examples, batch_size),
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
    return ModelMeasurement(**payload)


def benchmark_worker(
    connection: Any,
    spec: EmbeddingModelSpec,
    examples: list[TrainingExample],
    batch_size: int,
) -> None:
    try:
        measurement = benchmark_model(spec, examples, batch_size)
        connection.send((True, asdict(measurement)))
    except Exception as error:
        connection.send((False, f"{type(error).__name__}: {error}"))
    finally:
        connection.close()


def benchmark_sentence_transformer(
    model_name: str, texts: list[str], batch_size: int = 32
) -> ModelMeasurement:
    """Compatibility helper retained for throughput smoke tests."""

    if not texts:
        raise ValueError("texts must not be empty")
    document = TrainingExample.model_validate(
        {
            "example_id": "example_smoke",
            "dataset_revision": "smoke",
            "split": "test",
            "query": texts[0],
            "query_kind": "documentation",
            "positive": smoke_document("positive", texts[0]),
            "negatives": [smoke_document(str(index), text) for index, text in enumerate(texts[1:])]
            or [smoke_document("negative", "unrelated negative")],
        }
    )
    return benchmark_model(
        EmbeddingModelSpec(name=model_name, model=model_name), [document], batch_size
    )


def evaluate_ranks(ranks: list[int]) -> dict[str, float]:
    if not ranks:
        raise ValueError("ranks must not be empty")
    return {
        "recall@1": statistics.fmean(rank <= 1 for rank in ranks),
        "recall@5": statistics.fmean(rank <= 5 for rank in ranks),
        "recall@10": statistics.fmean(rank <= 10 for rank in ranks),
        "recall@20": statistics.fmean(rank <= 20 for rank in ranks),
        "mrr": statistics.fmean(1.0 / rank for rank in ranks),
        "ndcg@10": statistics.fmean(
            1.0 / np.log2(rank + 1) if rank <= 10 else 0.0 for rank in ranks
        ),
    }


def encode(model: Any, texts: list[str], batch_size: int) -> np.ndarray[Any, np.dtype[np.float32]]:
    embeddings = model.encode(
        texts,
        batch_size=batch_size,
        normalize_embeddings=True,
        show_progress_bar=False,
        convert_to_numpy=True,
    )
    return np.asarray(embeddings, dtype=np.float32)


def percentile(values: list[float], quantile: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * quantile)))
    return ordered[index]


def smoke_document(identifier: str, text: str) -> dict[str, str | int | None]:
    return {
        "document_id": identifier,
        "repository": "smoke",
        "revision": "0" * 40,
        "path": f"{identifier}.txt",
        "symbol": identifier,
        "language": None,
        "start_byte": 0,
        "end_byte": max(1, len(text.encode())),
        "text": text,
        "content_sha256": hashlib.sha256(text.encode()).hexdigest(),
    }


def require_sentence_transformers() -> Any:
    try:
        return import_module("sentence_transformers")
    except ImportError as error:
        raise RuntimeError("install cce-research[models] to benchmark embedding models") from error


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()
