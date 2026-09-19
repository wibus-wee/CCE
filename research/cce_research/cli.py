from __future__ import annotations

import hashlib
import json
import os
import platform
import random
import re
import sys
from collections import defaultdict
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path
from statistics import mean

import typer
from rich.console import Console
from rich.table import Table

from .ablations import marginal_utility, route_ablation_cases
from .adapters import METRICS_VERSION as ADAPTER_METRICS_VERSION
from .adapters import Adapter
from .metrics import LOWER_IS_BETTER, compare, evaluate, intent_confusion
from .pooling import adjudication_queue, apply_judgments
from .probes import permute_results, position_sensitivity, run_staleness_probe
from .schema import (
    BenchmarkCase,
    CaseResult,
    LineRange,
    ResultBundleManifest,
    load_jsonl,
)
from .variants import generate_variants, variant_agreement

app = typer.Typer(no_args_is_help=True)
console = Console()

# Metrics where a Holm-significant regression fails `compare --gate`.
# Diagnostic metrics (ndcg, mrr, density...) inform but never block.
DEFAULT_GUARDRAILS = (
    "file_success@20,file_recall@20,abstention_accuracy,"
    "no_context_precision,decoy_hit_rate@20"
)


def _filter_tags(
    cases: list[BenchmarkCase], include_tag: str | None, exclude_tag: str | None
) -> list[BenchmarkCase]:
    """Held-out protocol support: `--include-tag dev` evaluates only the
    tuning slice, `--exclude-tag adversarial` hides the trap suite from a
    report, etc. Filtering cases (not results) is what makes a split
    meaningful — result rows for excluded cases are simply ignored."""
    if include_tag:
        cases = [case for case in cases if include_tag in case.tags]
    if exclude_tag:
        cases = [case for case in cases if exclude_tag not in case.tags]
    return cases


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
    include_tag: str | None = None,
    exclude_tag: str | None = None,
) -> None:
    cases = list(load_jsonl(dataset, BenchmarkCase))
    cases = _filter_tags(cases, include_tag, exclude_tag)
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
    include_tag: str | None = None,
    exclude_tag: str | None = None,
) -> None:
    """Paired per-case deltas (candidate - baseline): bootstrap CI, paired
    permutation p-value, Holm-corrected significance across the whole
    metric family, effect size, and the suite's minimum detectable effect.

    --gate exits nonzero when a guardrail metric regresses with
    Holm-corrected significance.
    """
    cases = list(load_jsonl(dataset, BenchmarkCase))
    cases = _filter_tags(cases, include_tag, exclude_tag)
    baseline_results = list(load_jsonl(baseline, CaseResult))
    candidate_results = list(load_jsonl(candidate, CaseResult))
    deltas = compare(cases, baseline_results, candidate_results)

    guardrails = {name.strip() for name in guardrail.split(",") if name.strip()}
    table = Table("Metric", "Delta", "95% CI", "p(perm)", "d", "MDE", "N", "Sig")
    for name, metric in deltas.items():
        regressed = name in guardrails and metric.significant and (
            metric.delta > 0 if name in LOWER_IS_BETTER else metric.delta < 0
        )
        flag = "[red]![/red]" if regressed else ("*" if metric.significant else "")
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
            if name in deltas
            and deltas[name].significant
            and (deltas[name].delta > 0 if name in LOWER_IS_BETTER else deltas[name].delta < 0)
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
    # The adapter substitutes {repository} verbatim into subprocess commands
    # whose cwd is the repository itself — a relative arg like ".." would
    # re-resolve against that cwd and index the parent directory. Pin the
    # absolute path once here.
    repository = repository.resolve()
    component_map = adapter.component_map(repository)
    if not component_map:
        console.print(
            "[yellow]![/yellow] adapter produced no component map; "
            "component_* metrics will be skipped"
        )
    adapter.start_session(repository)
    raw_path = output.with_suffix(output.suffix + ".raw.jsonl")
    try:
        with output.open("w", encoding="utf-8") as handle, raw_path.open(
            "w", encoding="utf-8"
        ) as raw:
            for case in cases:
                result = adapter.run(case, repository, system_revision, raw_sink=raw)
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
        metrics_version=ADAPTER_METRICS_VERSION,
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


def _repo_lines(repository: Path, path: str) -> list[str] | None:
    """Repository-relative source lines, or None when the path is missing
    or non-text (binary/deleted). Used by lint-dataset's rot checks."""
    target = repository / path
    if not target.is_file():
        return None
    try:
        return target.read_text(encoding="utf-8").splitlines()
    except (UnicodeDecodeError, OSError):
        return None


@app.command("lint-dataset")
def lint_dataset(dataset: Path, repository: Path) -> None:
    """Static gold audit — no system run. WORKTREE-pinned datasets rot
    silently as code drifts; this catches it without measuring the
    system at all: missing gold files, out-of-bounds ranges, gold symbols
    absent from every gold file, judged∩gold conflicts, and duplicate
    queries annotated with inconsistent gold."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    repository = repository.resolve()
    failures = 0
    file_cache: dict[str, list[str] | None] = {}

    def lines_of(path: str) -> list[str] | None:
        if path not in file_cache:
            file_cache[path] = _repo_lines(repository, path)
        return file_cache[path]

    def check_range(case_id: str, item: LineRange) -> int:
        file_lines = lines_of(item.path)
        if file_lines is not None and item.end_line > len(file_lines):
            console.print(
                f"[red]{case_id}[/red]: range {item.path}:{item.start_line}-{item.end_line} "
                f"exceeds file length {len(file_lines)}"
            )
            return 1
        return 0

    for case in cases:
        gold_paths = (
            set(case.gold_files)
            | {item.path for item in case.gold_ranges}
            | {item.path for item in case.supporting_ranges}
        )
        for path in sorted(gold_paths):
            if lines_of(path) is None:
                console.print(f"[red]{case.case_id}[/red]: missing gold file {path}")
                failures += 1
        for item in [*case.gold_ranges, *case.supporting_ranges]:
            failures += check_range(case.case_id, item)
        for fact in case.gold_facts:
            for item in fact.evidence:
                failures += check_range(case.case_id, item)
        for symbol in case.gold_symbols:
            if "/" in symbol or "." in symbol:
                continue
            pattern = re.compile(rf"\b{re.escape(symbol)}\b")
            if gold_paths and not any(
                pattern.search(line)
                for path in gold_paths
                for line in (lines_of(path) or [])
            ):
                console.print(
                    f"[red]{case.case_id}[/red]: gold symbol {symbol!r} absent from "
                    "every gold file"
                )
                failures += 1
        if conflict := sorted(set(case.judged_files) & gold_paths):
            console.print(
                f"[red]{case.case_id}[/red]: judged_files overlap gold {conflict}"
            )
            failures += 1

    # Duplicate queries only contradict when they come from *different*
    # case families: siblings legitimately narrow their parent's gold
    # (route pins and budgets change what is reachable). Compare one
    # representative per cluster.
    from .metrics import case_clusters

    cluster_of = case_clusters(cases)
    by_query: dict[str, list[BenchmarkCase]] = defaultdict(list)
    for case in cases:
        by_query[case.query].append(case)
    for query in sorted(by_query):
        families: dict[str, BenchmarkCase] = {}
        for case in by_query[query]:
            families.setdefault(cluster_of[case.case_id], case)
        distinct = {
            tuple(sorted(
                set(case.gold_files) | {item.path for item in case.gold_ranges}
            ))
            for case in families.values()
        }
        if len(distinct) > 1:
            console.print(
                f"[yellow]duplicate query {query!r} with inconsistent gold across "
                f"families[/yellow]: {[case.case_id for case in families.values()]}"
            )
    if failures:
        console.print(f"[red]{failures} annotation failure(s)[/red]")
        raise typer.Exit(code=1)
    console.print("[green]gold annotations consistent with the worktree[/green]")


_PERTURB_OPS = ("pluralize", "truncate", "double-last", "swap-last")


def _perturb_symbol(symbol: str, op: str) -> str:
    """One-edit mutations that stay lookalike but should not name anything
    real: 'SnapshotIdentity' -> 'SnapshotIdentitys', 'open' -> 'openn',
    'search' -> 'serach'."""
    if op == "pluralize":
        return f"{symbol}s"
    if op == "truncate":
        return symbol[:-1] if len(symbol) > 2 else f"{symbol}x"
    if op == "double-last":
        return f"{symbol}{symbol[-1]}"
    if op == "swap-last":
        return symbol[:-2] + symbol[-1] + symbol[-2] if len(symbol) > 2 else f"{symbol}x"
    raise ValueError(f"unknown perturb op {op}")


def _repo_corpus(repository: Path) -> str:
    """All repository text as one searchable blob — the ground truth for
    'this mutated symbol is provably absent'."""
    parts: list[str] = []
    for path in repository.rglob("*"):
        if not path.is_file():
            continue
        relative = path.relative_to(repository)
        if any(
            part in {".git", ".cce", "target", "node_modules", "__pycache__", ".venv"}
            for part in relative.parts
        ):
            continue
        try:
            parts.append(path.read_text(encoding="utf-8"))
        except (UnicodeDecodeError, OSError):
            continue
    return "\n".join(parts)


@app.command("perturb")
def perturb(
    dataset: Path,
    repository: Path,
    output: Path,
    seed: int = 0xCCE,
) -> None:
    """Fresh near-miss abstention probes. Mutate each gold case's first
    symbol into a lookalike name, verify the mutation is *provably absent*
    from the repository (full-corpus word-boundary scan — no wrong labels),
    and pre-judge the parent's gold files as the decoys. Regenerate with a
    new seed whenever the trap surface feels memorized: the mutations are
    seeded, so each regeneration is a fresh but reproducible trap set."""
    cases = list(load_jsonl(dataset, BenchmarkCase))
    corpus = _repo_corpus(repository.resolve())
    generator = random.Random(seed)
    derived: list[BenchmarkCase] = []
    for case in cases:
        if case.no_context or not case.gold_symbols or case.derived_from:
            continue
        symbol = case.gold_symbols[0]
        ops = list(_PERTURB_OPS)
        generator.shuffle(ops)
        mutation = None
        for op in ops:
            candidate = _perturb_symbol(symbol, op)
            if candidate != symbol and not re.search(rf"\b{re.escape(candidate)}\b", corpus):
                mutation = (op, candidate)
                break
        if mutation is None:
            continue
        op, typo = mutation
        derived.append(
            case.model_copy(
                update={
                    "case_id": f"{case.case_id}::ptb-{op}",
                    "query": f"Where is `{typo}` defined?",
                    "gold_files": [],
                    "gold_symbols": [],
                    "gold_ranges": [],
                    "supporting_ranges": [],
                    "gold_facts": [],
                    "gold_components": [],
                    "judged_files": list(case.gold_files),
                    "no_context": True,
                    "derived_from": case.case_id,
                    "derivation": f"perturb:{op}",
                    "tags": [*case.tags, "perturb", "adversarial"],
                }
            )
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for case in derived:
            handle.write(case.model_dump_json() + "\n")
    console.print(f"{len(derived)} provably-absent perturb cases -> {output}")


@app.command("audit")
def audit(
    dataset: Path,
    results: Path,
    adapter_file: Path,
    repository: Path,
    sample: int = 10,
    seed: int = 0xCCE,
) -> None:
    """Replay-integrity check: re-run a seeded random sample of a result
    bundle's cases through the live system and compare retrieved path sets
    against the claimed results. A hand-edited or replayed bundle shows
    systematic mismatches; a legitimately nondeterministic system shows
    moderate Jaccard drift. Diagnostic, not a gate — interpret alongside
    the system's determinism properties."""
    adapter = Adapter.load(adapter_file)
    cases = {case.case_id: case for case in load_jsonl(dataset, BenchmarkCase)}
    claimed = {result.case_id: result for result in load_jsonl(results, CaseResult)}
    shared = sorted(set(cases) & set(claimed))
    picks = random.Random(seed).sample(shared, min(sample, len(shared)))
    repository = repository.resolve()
    table = Table("case", "claimed", "fresh", "jaccard", "abstain-flip")
    scores: list[float] = []
    try:
        for case_id in picks:
            fresh = adapter.run(cases[case_id], repository, "AUDIT")
            old_paths = {item.path for item in claimed[case_id].retrieved}
            new_paths = {item.path for item in fresh.retrieved}
            union = old_paths | new_paths
            jaccard = len(old_paths & new_paths) / len(union) if union else 1.0
            flip = claimed[case_id].abstained != fresh.abstained
            scores.append(jaccard)
            flag = " [red]![/red]" if jaccard < 0.5 or flip else ""
            table.add_row(
                case_id,
                str(len(old_paths)),
                str(len(new_paths)),
                f"{jaccard:.2f}",
                "flip" if flip else "",
                flag,
            )
    finally:
        adapter.shutdown()
    console.print(table)
    if scores:
        console.print(
            f"mean path-set Jaccard over {len(scores)} replays: {mean(scores):.3f}"
        )


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
