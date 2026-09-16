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
from .metrics import compare, evaluate
from .schema import BenchmarkCase, CaseResult, ResultBundleManifest, load_jsonl

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


@app.command("compare")
def compare_results(dataset: Path, baseline: Path, candidate: Path, output: Path | None = None) -> None:
    """Paired per-case deltas (candidate - baseline) with bootstrap CIs.
    Both result bundles must cover the same cases in `dataset`."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    baseline_results = list(load_jsonl(baseline, CaseResult))
    candidate_results = list(load_jsonl(candidate, CaseResult))
    deltas = compare(cases, baseline_results, candidate_results)
    table = Table("Metric", "Delta", "95% CI", "N")
    for name, metric in deltas.items():
        table.add_row(
            name,
            f"{metric.value:+.4f}",
            f"[{metric.ci_low:+.4f}, {metric.ci_high:+.4f}]",
            str(metric.samples),
        )
    console.print(table)
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            json.dumps({key: asdict(value) for key, value in deltas.items()}, indent=2),
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


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


if __name__ == "__main__":
    app()
