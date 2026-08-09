"""Embedding benchmark entry point. Install the `models` extra before use."""

from __future__ import annotations

import time
from dataclasses import dataclass
from importlib import import_module
from typing import Any


@dataclass(frozen=True)
class ModelMeasurement:
    model: str
    documents: int
    dimensions: int
    elapsed_seconds: float
    documents_per_second: float


def benchmark_sentence_transformer(
    model_name: str, texts: list[str], batch_size: int = 32
) -> ModelMeasurement:
    try:
        sentence_transformers: Any = import_module("sentence_transformers")
    except ImportError as error:
        raise RuntimeError("install cce-research[models] to benchmark embedding models") from error
    model = sentence_transformers.SentenceTransformer(model_name, trust_remote_code=False)
    started = time.perf_counter()
    embeddings = model.encode(
        texts, batch_size=batch_size, normalize_embeddings=True, show_progress_bar=True
    )
    elapsed = time.perf_counter() - started
    return ModelMeasurement(
        model=model_name,
        documents=len(texts),
        dimensions=int(embeddings.shape[1]),
        elapsed_seconds=elapsed,
        documents_per_second=len(texts) / elapsed if elapsed else 0.0,
    )
