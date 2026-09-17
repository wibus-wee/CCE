# Plan 005: Task→Component Recall@K metric (Layer-0-lite at package granularity)

## Priority

P1 — independent of 001/002; case annotation benefits from landing alongside 003 (annotate whatever `cce-self.jsonl` contains at execution time).

## Goal

Implement the architecture report's headline Layer-0 metric — **Task→Component Recall@K** ("任务是否先被路由到正确 subsystem", report §14.1/§23) — at the granularity the engine actually supports today: L2 packages from `codebase_map`. This is L0-lite: packages are the deterministic proxy until real L3 component clustering exists.

## Background for the executor

- Report §14.1 names `Task→Component Recall@K` and `Time-to-First-Correct-Component` as first-class metrics; §23 makes architecture accuracy — not Recall@20 — the release gate.
- `codebase_map` (`atlas.rs::codebase_map`, `search.rs::CodebaseMap/PackageNode`) already returns deterministic packages with `rootDir` — the component proxy needs no engine changes.
- `BenchmarkCase` (`research/cce_research/schema.py`) is strict-schema but additive optional fields are the established pattern (`gold_facts`, `derived_from`, `answer_keys`).
- `CaseResult.metadata` is `dict[str, str|int|float|bool|None]` — enough to carry a compact package map.
- `retrieval_metrics` groups everything by intent automatically; component metrics inherit that for free.

## Steps

1. **Schema** (`research/cce_research/schema.py`):
   - `BenchmarkCase`: add `gold_components: list[str] = []` — package names the task's gold evidence lives in (e.g. `["cce-store"]`); empty = metric skipped for the case.
   - `CaseResult`: add `component_map: dict[str, str] = {}` — package name → `rootDir`, populated once per run.
2. **Adapter** (`research/cce_research/adapters.py` + `cli.py::run_adapter`): after the per-case loop, when the command supports it, run `target/release/cce map --json` once against the same repository; parse `packages[]` into `{name: rootDir}`; stamp the same dict on every `CaseResult.component_map`. On `map` failure/non-support: log, leave empty — never fail the run. Record `component_map_packages: <count>` in manifest metadata.
3. **Metrics** (`research/cce_research/metrics.py::retrieval_metrics`): for cases with `gold_components` and a non-empty `component_map`:
   - Map each retrieved path to its package by longest-prefix match against `rootDir` values (normalize separators; repo-relative paths both sides).
   - `component_recall_at_5` / `component_recall_at_20` = |gold_components ∩ predicted_components(top-K)| / |gold_components|.
   - `first_correct_component_rank` — rank of the first retrieved item whose package ∈ gold_components (0 if none); report as `component_mrr` for report-parity with "Time-to-First-Correct-Component".
   - Skip (not zero) cases lacking `gold_components` — consistent with how `gold_facts_accuracy` handles missing annotations.
4. **Annotate the dataset**: for every case in `benchmarks/datasets/cce-self.jsonl`, derive `gold_components` deterministically from `gold_files`/`supporting_files` owning crate dir (`crates/cce-store/...` → `cce-store`; `apps/cli/...` → `cce-cli`; `research/...` → `cce-research`; top-level files → `repository-root` or the workspace package — pick one convention, document it in `docs/benchmarking.md`). No-context cases stay empty.
5. **Docs**: `docs/benchmarking.md` — one row in the metrics table + the annotation convention.

## Files

- `research/cce_research/schema.py` (two optional fields)
- `research/cce_research/adapters.py` (map fetch helper + fallback)
- `research/cce_research/cli.py` (invoke once per run)
- `research/cce_research/metrics.py` (two metrics + tests)
- `benchmarks/datasets/cce-self.jsonl` (gold_components on all cases)
- `docs/benchmarking.md` (metric + convention)

## Verification

- `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`.
- `uv run --project research pytest research/tests -q`.
- `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl`.
- `cargo build --release --locked -p cce-cli` then the standard run + `evaluate` — metrics JSON contains `component_recall_at_5`, `component_mrr` (per-intent + overall), and they're sane vs `file_success_at_5` (component recall ≥ file recall in most cases since it's coarser — investigate any inversion).
- Unit test: synthetic case with known component_map, retrieved paths spanning gold/non-gold packages.

## Done when

Evaluating any bundle on the annotated dataset emits `component_recall_at_*`/`component_mrr`; a case with empty `gold_components` produces no component metrics; docs updated.

## Maintenance notes

- When real L3 components land (report §5.3 affinity clustering), keep this package-granularity metric as the deterministic floor and add a second `component_*` series at L3 — don't silently redefine the existing metric.
- If `codebase_map` package naming changes, the annotation convention doc is the contract.

## Escape hatch

- If `cce map` is too slow/fragile to call per run on this repo, fall back to a static `component_map` embedded in the adapter yaml — record the tradeoff; the metric math is identical.
- If path→package prefix matching is ambiguous (nested crates), longest-prefix wins; log unmatched paths in run summary.
