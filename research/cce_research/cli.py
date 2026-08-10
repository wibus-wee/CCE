from __future__ import annotations

import hashlib
import json
import os
import platform
import sys
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from .adapters import Adapter
from .metrics import evaluate
from .model_benchmark import benchmark_models
from .reranker_benchmark import benchmark_rerankers
from .schema import BenchmarkCase, CaseResult, ResultBundleManifest, load_jsonl
from .train_reranker import (
    RerankerTrainingRecipe,
    export_reranker_onnx,
    promote_reranker_bundle,
    train_reranker,
)
from .train_retriever import TrainingRecipe, export_onnx, train_retriever
from .training_data import build_training_dataset

app = typer.Typer(no_args_is_help=True)
console = Console()


@app.command("validate-dataset")
def validate_dataset(dataset: Path) -> None:
    cases = list(load_jsonl(dataset, BenchmarkCase))
    revisions = {case.provenance.dataset_revision for case in cases}
    console.print(f"Validated {len(cases)} cases across {len(revisions)} dataset revisions.")


@app.command("evaluate")
def evaluate_results(dataset: Path, results: Path, output: Path | None = None) -> None:
    cases = list(load_jsonl(dataset, BenchmarkCase))
    outcomes = list(load_jsonl(results, CaseResult))
    summary = evaluate(cases, outcomes)
    table = Table("Metric", "Value", "95% CI", "N")
    for name, metric in summary.items():
        table.add_row(
            name,
            f"{metric.value:.4f}",
            f"[{metric.ci_low:.4f}, {metric.ci_high:.4f}]",
            str(metric.samples),
        )
    console.print(table)
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            json.dumps({key: asdict(value) for key, value in summary.items()}, indent=2),
            encoding="utf-8",
        )


@app.command("run")
def run_adapter(
    dataset: Path,
    adapter_file: Path,
    repository: Path,
    system_revision: str,
    output: Path,
) -> None:
    adapter = Adapter.load(adapter_file)
    cases = list(load_jsonl(dataset, BenchmarkCase))
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for case in cases:
            result = adapter.run(case, repository, system_revision)
            handle.write(result.model_dump_json() + "\n")
            console.print(f"[green]✓[/green] {case.case_id}")
    dataset_revisions = {case.provenance.dataset_revision for case in cases}
    if len(dataset_revisions) != 1:
        raise ValueError("a result bundle must contain exactly one dataset revision")
    lockfiles = [
        repository / "Cargo.lock",
        repository / "pnpm-lock.yaml",
        repository / "research" / "uv.lock",
    ]
    manifest = ResultBundleManifest(
        system=adapter.name,
        system_revision=system_revision,
        dataset_revision=next(iter(dataset_revisions)),
        dataset_sha256=sha256(dataset),
        adapter_sha256=sha256(adapter_file),
        results_sha256=sha256(output),
        model_identity=adapter.model_identity,
        model_revision=adapter.model_revision,
        generated_at=datetime.now(UTC).isoformat(),
        operating_system=platform.platform(),
        machine=platform.machine(),
        processor=platform.processor(),
        cpu_count=os.cpu_count(),
        python_version=sys.version.split()[0],
        dependency_lock_sha256={
            str(path.relative_to(repository)): sha256(path) for path in lockfiles if path.is_file()
        },
        environment_keys=sorted(adapter.environment),
    )
    manifest_path = output.with_suffix(output.suffix + ".manifest.json")
    manifest_path.write_text(manifest.model_dump_json(indent=2) + "\n", encoding="utf-8")


@app.command("plot-recall")
def plot_recall(metrics_files: list[Path], output: Path) -> None:
    import matplotlib.pyplot as plt

    for path in metrics_files:
        payload = json.loads(path.read_text(encoding="utf-8"))
        cutoffs = [5, 10, 20, 50]
        values = [payload[f"recall@{cutoff}"]["value"] for cutoff in cutoffs]
        plt.plot(cutoffs, values, marker="o", label=path.stem)
    plt.xlabel("K")
    plt.ylabel("Range recall")
    plt.ylim(0, 1)
    plt.grid(alpha=0.25)
    plt.legend()
    output.parent.mkdir(parents=True, exist_ok=True)
    plt.savefig(output, bbox_inches="tight", dpi=180)


@app.command("build-training-data")
def build_training_data_command(
    manifest: Path,
    workspace: Path,
    output: Path,
    cce_binary: Path = Path("target/release/cce"),
    hard_negatives: int = 7,
) -> None:
    summary = build_training_dataset(
        manifest,
        workspace,
        output,
        cce_binary,
        hard_negatives,
    )
    console.print_json(json.dumps(asdict(summary)))


@app.command("train-retriever")
def train_retriever_command(
    dataset: Path,
    output: Path,
    base_model: str,
    base_revision: str,
    code_revision: str | None = None,
    query_prefix: str = "query: ",
    document_prefix: str = "passage: ",
    hard_negatives: int = 7,
    epochs: float = 1.0,
    batch_size: int = 8,
    gradient_accumulation_steps: int = 4,
    max_sequence_length: int = 512,
    max_train_examples: int | None = None,
    max_validation_examples: int | None = None,
    trust_remote_code: bool = False,
) -> None:
    summary = train_retriever(
        TrainingRecipe(
            base_model=base_model,
            base_revision=base_revision,
            code_revision=code_revision,
            dataset_directory=str(dataset),
            output_directory=str(output),
            query_prefix=query_prefix,
            document_prefix=document_prefix,
            same_repository_hard_negatives=hard_negatives,
            epochs=epochs,
            batch_size=batch_size,
            gradient_accumulation_steps=gradient_accumulation_steps,
            max_sequence_length=max_sequence_length,
            max_train_examples=max_train_examples,
            max_validation_examples=max_validation_examples,
            trust_remote_code=trust_remote_code,
        )
    )
    console.print_json(json.dumps(asdict(summary)))


@app.command("export-onnx")
def export_onnx_command(
    model: Path,
    output: Path,
    optimization: str = "O3",
    quantization: str | None = None,
) -> None:
    exported = export_onnx(model, output, optimization, quantization)
    console.print(str(exported))


@app.command("benchmark-models")
def benchmark_models_command(
    dataset: Path,
    models: Path,
    output: Path,
    batch_size: int = 16,
    limit: int | None = None,
) -> None:
    measurements = benchmark_models(dataset, models, output, batch_size, limit)
    table = Table("Model", "R@1", "R@5", "MRR", "nDCG@10", "doc/s", "query p95 ms")
    for measurement in measurements:
        table.add_row(
            measurement.model,
            f"{measurement.recall_at_1:.4f}",
            f"{measurement.recall_at_5:.4f}",
            f"{measurement.mrr:.4f}",
            f"{measurement.ndcg_at_10:.4f}",
            f"{measurement.documents_per_second:.1f}",
            f"{measurement.single_query_p95_ms:.1f}",
        )
    console.print(table)


@app.command("train-reranker")
def train_reranker_command(
    dataset: Path,
    output: Path,
    base_model: str,
    base_revision: str,
    code_revision: str | None = None,
    hard_negatives: int = 7,
    epochs: float = 1.0,
    batch_size: int = 8,
    gradient_accumulation_steps: int = 4,
    max_sequence_length: int = 512,
    max_train_examples: int | None = None,
    max_validation_examples: int | None = None,
    trust_remote_code: bool = False,
) -> None:
    summary = train_reranker(
        RerankerTrainingRecipe(
            base_model=base_model,
            base_revision=base_revision,
            dataset_directory=str(dataset),
            output_directory=str(output),
            code_revision=code_revision,
            hard_negatives=hard_negatives,
            epochs=epochs,
            batch_size=batch_size,
            gradient_accumulation_steps=gradient_accumulation_steps,
            max_sequence_length=max_sequence_length,
            max_train_examples=max_train_examples,
            max_validation_examples=max_validation_examples,
            trust_remote_code=trust_remote_code,
        )
    )
    console.print_json(json.dumps(asdict(summary)))


@app.command("export-reranker-onnx")
def export_reranker_onnx_command(
    model: Path,
    output: Path,
    runtime_model: str,
    source_revision: str,
    max_sequence_length: int = 512,
) -> None:
    summary = export_reranker_onnx(
        model, output, runtime_model, source_revision, max_sequence_length
    )
    console.print_json(json.dumps(asdict(summary)))


@app.command("promote-reranker-bundle")
def promote_reranker_bundle_command(
    source: Path,
    output: Path,
    runtime_model: str,
    source_revision: str,
) -> None:
    summary = promote_reranker_bundle(source, output, runtime_model, source_revision)
    console.print_json(json.dumps(asdict(summary)))


@app.command("benchmark-rerankers")
def benchmark_rerankers_command(
    dataset: Path,
    models: Path,
    output: Path,
    batch_size: int = 8,
    candidates: int = 16,
    limit: int | None = None,
) -> None:
    measurements = benchmark_rerankers(dataset, models, output, batch_size, candidates, limit)
    table = Table("Model", "R@1", "R@5", "MRR", "nDCG@10", "pairs/s", "query p95 ms")
    for measurement in measurements:
        table.add_row(
            measurement.model,
            f"{measurement.recall_at_1:.4f}",
            f"{measurement.recall_at_5:.4f}",
            f"{measurement.mrr:.4f}",
            f"{measurement.ndcg_at_10:.4f}",
            f"{measurement.pairs_per_second:.1f}",
            f"{measurement.query_p95_ms:.1f}",
        )
    console.print(table)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


if __name__ == "__main__":
    app()
