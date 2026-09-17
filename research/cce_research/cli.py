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

from .ablations import marginal_utility, route_ablation_cases
from .adapters import Adapter
from .metrics import compare, evaluate, intent_confusion
from .pooling import adjudication_queue, apply_judgments
from .probes import permute_results, position_sensitivity, run_staleness_probe
from .schema import BenchmarkCase, CaseResult, ResultBundleManifest, load_jsonl
from .variants import generate_variants, variant_agreement

app = typer.Typer(no_args_is_help=True)
console = Console()

# Metrics where a Holm-significant regression fails `compare --gate`.
# Diagnostic metrics (ndcg, mrr, density...) inform but never block.
DEFAULT_GUARDRAILS = "file_success@20,file_recall@20,abstention_accuracy,no_context_precision"


@app.command("validate-dataset")
def validate_dataset(dataset: Path) -> None:
    cases = list(load_jsonl(dataset, BenchmarkCase))
    revisions = {case.provenance.dataset_revision for case in cases}
    console.print(f"Validated {len(cases)} cases across {len(revisions)} dataset revisions.")


@app.command("evaluate")
def evaluate_results(
    dataset: Path,
    results: Path,
    output: Path | None = None,
    confusion: bool = False,
) -> None:
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
    if confusion:
        matrix = intent_confusion(cases, outcomes)
        confusion_table = Table("gold \\ predicted", "predicted", "count")
        for gold, row in sorted(matrix.items()):
            for predicted, count in sorted(row.items()):
                confusion_table.add_row(gold, predicted, str(count))
        console.print(confusion_table)
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            json.dumps({key: asdict(value) for key, value in summary.items()}, indent=2),
            encoding="utf-8",
        )


@app.command("compare")
def compare_results(
    dataset: Path,
    baseline: Path,
    candidate: Path,
    output: Path | None = None,
    gate: bool = False,
    guardrail: str = DEFAULT_GUARDRAILS,
) -> None:
    """Paired per-case deltas (candidate - baseline): bootstrap CI, paired
    permutation p-value, Holm-corrected significance across the whole
    metric family, effect size, and the suite's minimum detectable effect.

    --gate exits nonzero when a guardrail metric regresses with
    Holm-corrected significance.
    """
    cases = list(load_jsonl(dataset, BenchmarkCase))
    baseline_results = list(load_jsonl(baseline, CaseResult))
    candidate_results = list(load_jsonl(candidate, CaseResult))
    deltas = compare(cases, baseline_results, candidate_results)

    guardrails = {name.strip() for name in guardrail.split(",") if name.strip()}
    table = Table("Metric", "Delta", "95% CI", "p(perm)", "d", "MDE", "N", "Sig")
    for name, metric in deltas.items():
        flag = "[red]![/red]" if name in guardrails and metric.significant and metric.delta < 0 else (
            "*" if metric.significant else ""
        )
        table.add_row(
            name,
            f"{metric.delta:+.4f}",
            f"[{metric.ci_low:+.4f}, {metric.ci_high:+.4f}]",
            f"{metric.p_value:.4f}" if metric.p_value is not None else "-",
            f"{metric.effect_size:+.2f}" if metric.effect_size is not None else "-",
            f"{metric.mde:.4f}" if metric.mde is not None else "-",
            str(metric.samples),
            flag,
        )
    console.print(table)

    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            json.dumps({key: asdict(value) for key, value in deltas.items()}, indent=2),
            encoding="utf-8",
        )
    if gate:
        failures = [
            name
            for name in guardrails
            if name in deltas and deltas[name].significant and deltas[name].delta < 0
        ]
        if failures:
            console.print(f"[red]GATE FAIL[/red]: guardrail regression in {sorted(failures)}")
            raise typer.Exit(code=1)
        console.print("[green]GATE PASS[/green]: no significant guardrail regression")


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
    component_map = adapter.component_map(repository)
    if not component_map:
        console.print(
            "[yellow]![/yellow] adapter produced no component map; "
            "component_* metrics will be skipped"
        )
    adapter.start_session(repository)
    try:
        with output.open("w", encoding="utf-8") as handle:
            for case in cases:
                result = adapter.run(case, repository, system_revision)
                result.component_map = component_map
                handle.write(result.model_dump_json() + "\n")
                console.print(f"[green]✓[/green] {case.case_id}")
    finally:
        adapter.shutdown()
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
        component_map_packages=len(component_map) or None,
    )
    manifest_path = output.with_suffix(output.suffix + ".manifest.json")
    manifest_path.write_text(manifest.model_dump_json(indent=2) + "\n", encoding="utf-8")


@app.command("adjudicate")
def adjudicate(
    dataset: Path,
    results: list[Path],
    output: Path,
    depth: int = 20,
) -> None:
    """Emit the adjudication queue: unjudged paths surfaced in the top
    `depth` of each run, most-shared first. Fill `verdict` per row
    (relevant|irrelevant), then `apply-judgments` folds them into a new
    dataset revision."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    bundles = [(path.stem, list(load_jsonl(path, CaseResult))) for path in results]
    queue = adjudication_queue(cases, bundles, depth=depth)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for entry in queue:
            handle.write(
                json.dumps(
                    {
                        "case_id": entry.case_id,
                        "path": entry.path,
                        "best_rank": entry.best_rank,
                        "occurrences": entry.occurrences,
                        "bundles": list(entry.bundles),
                        "routes": list(entry.routes),
                        "verdict": "",
                    }
                )
                + "\n"
            )
    console.print(f"{len(queue)} unjudged paths queued to {output}")


@app.command("apply-judgments")
def apply(dataset: Path, judgments: Path, output: Path) -> None:
    """Fold adjudicated verdicts into a new dataset file. Rows with empty or
    unknown verdicts are ignored."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    verdicts = []
    for line in judgments.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        if row.get("verdict") in ("relevant", "irrelevant"):
            verdicts.append((row["case_id"], row["path"], row["verdict"]))
    updated = apply_judgments(cases, verdicts)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for case in updated:
            handle.write(case.model_dump_json() + "\n")
    console.print(f"applied {len(verdicts)} judgments -> {output}")


@app.command("ablate")
def ablate(dataset: Path, baseline: Path, output: Path) -> None:
    """Generate a leave-one-route-out dataset: for every case, one derived
    case per executed route with that route removed. Run the derived dataset
    with the same adapter, then `utility` compares it to the baseline."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    baseline_results = list(load_jsonl(baseline, CaseResult))
    derived = route_ablation_cases(cases, baseline_results)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for case in derived:
            handle.write(case.model_dump_json() + "\n")
    console.print(f"{len(derived)} ablation cases -> {output}")


@app.command("utility")
def utility(dataset: Path, baseline: Path, ablations: Path, metric: str = "file_recall@20") -> None:
    """Marginal route utility: mean metric delta when each route is removed
    (negative = the route was contributing)."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    baseline_results = list(load_jsonl(baseline, CaseResult))
    ablation_results = list(load_jsonl(ablations, CaseResult))
    table_rows = marginal_utility(cases, baseline_results, ablation_results, metric=metric)
    table = Table("route removed", f"mean Δ {metric}", "worst Δ", "cases")
    for route, row in table_rows.items():
        table.add_row(
            route,
            f"{row['mean_delta']:+.4f}",
            f"{row['worst_delta']:+.4f}",
            str(int(row["cases"])),
        )
    console.print(table)


@app.command("variants")
def variants(dataset: Path, output: Path, seed: int = 0xCCE) -> None:
    """Generate metamorphic query variants (inflection / mixed-language /
    verbose / keyword-scramble) sharing each case's gold."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    derived = generate_variants(cases, seed=seed)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for case in derived:
            handle.write(case.model_dump_json() + "\n")
    console.print(f"{len(derived)} variant cases -> {output}")


@app.command("variant-agreement")
def variant_agreement_cmd(
    dataset: Path,
    baseline: Path,
    variant_results: Path,
    metric: str = "file_success@20",
) -> None:
    """How often metamorphic variants reproduce the parent's outcome."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    baseline_results = list(load_jsonl(baseline, CaseResult))
    results = list(load_jsonl(variant_results, CaseResult))
    report = variant_agreement(cases, baseline_results, results, metric=metric)
    console.print(report)


@app.command("permute")
def permute(
    dataset: Path,
    results: Path,
    permutations: int = 10,
    seed: int = 0xCCE,
) -> None:
    """Position-sensitivity report: shuffle each result's item order and
    measure how much order-sensitive metrics (mrr, ndcg@10) move. High
    spread = the metric mostly reflects ordering, not content."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    outcomes = list(load_jsonl(results, CaseResult))
    runs = permute_results(outcomes, permutations=permutations, seed=seed)
    report = position_sensitivity(cases, runs)
    table = Table("metric", "mean under permutation", "mean per-case spread")
    for name in report.per_metric_mean:
        table.add_row(
            name,
            f"{report.per_metric_mean[name]:.4f}",
            f"{report.per_metric_spread.get(name, 0.0):.4f}",
        )
    console.print(table)


@app.command("lint-gold")
def lint_gold(
    dataset: Path,
    adapter_file: Path,
    repository: Path,
    limit: int = 50,
) -> None:
    """Feasibility check: run every case with the full union route set and
    report gold evidence the system cannot surface at all — bad annotations,
    not system failures."""
    adapter = Adapter.load(adapter_file)
    cases = list(load_jsonl(dataset, BenchmarkCase))
    union_routes = [
        "exact_symbol", "lexical", "dense_raw", "dense_summary",
        "hybrid", "structural", "knowledge", "history",
    ]
    failures = 0
    try:
        for case in cases:
            probe = case.model_copy(update={"routes": union_routes, "supply_intent": True})
            result = adapter.run(probe, repository, "LINT")
            reachable = {item.path for item in result.retrieved[:limit]}
            gold = set(case.gold_files) | {item.path for item in case.gold_ranges}
            if unreachable := sorted(gold - reachable):
                failures += 1
                console.print(f"[red]{case.case_id}[/red]: unreachable gold {unreachable}")
            if case.no_context and result.retrieved:
                console.print(
                    f"[yellow]{case.case_id}[/yellow]: no-context case still retrieved "
                    f"{len(result.retrieved)} items under union routes"
                )
    finally:
        adapter.shutdown()
    if failures:
        raise typer.Exit(code=1)
    console.print("[green]all gold evidence reachable under union routes[/green]")


@app.command("probe-staleness")
def probe_staleness(adapter_file: Path, repository: Path) -> None:
    """Mutate-then-query freshness probe: write a marker file, index it,
    rewrite it, and check whether fresh content surfaces and stale content
    is served."""
    adapter = Adapter.load(adapter_file)
    try:
        report = run_staleness_probe(adapter, repository)
    finally:
        adapter.shutdown()
    console.print(report)


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
