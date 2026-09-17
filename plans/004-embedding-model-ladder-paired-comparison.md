# Plan 004: Paired embedding-model ladder comparison (e5-small vs Qwen3-0.6B vs e5-base)

## Priority

P1 — depends on plan 001 (dense passthrough + latency) and plan 003 (v5 dataset with vocabulary-gap cases). Renamed from "e5-base paired comparison": the architecture report (`research/papers/architecture_first_code_context_engine_report.pdf`, §13.1/§22.1) recommends a model **ladder** — `0.6B zero-shot baseline → agent-task SFT → larger ablation` — and explicitly warns against "先默认部署最大模型". So the experiment is "which local baseline earns its cost", not "does a bigger e5 help".

## Goal

On identical cases and repository revision, measure which embedding model improves retrieval on the NL↔identifier vocabulary gap — and at what latency/memory cost — or show the difference is below the harness's detectable effect. Both outcomes are publishable conclusions; either closes the debate with evidence.

## Background for the executor

- `research/cce_research/model_benchmark.py` is a per-model microbenchmark (embed latency/throughput), not retrieval quality.
- Retrieval quality needs paired per-case comparison: `cce-research compare` already implements bootstrap CIs + permutation p-values (`metrics.py::compare_bundles`, `cli.py::compare_bundles`).
- Model identity is part of `index_profile_hash` → snapshot identity (`config.rs:115`, `repository.rs:249-266`). Each model gets a distinct snapshot automatically; no data-directory surgery needed. Warm-query numbers stay comparable because each snapshot is self-contained.
- MDE honesty: `metrics.py::_paired_intent_deltas` reports minimum detectable effect. n≈30 cases gives limited power — a "no significant difference" result may mean the harness can't detect it, not that none exists. Report MDE explicitly.
- Report prediction to test (§22.1, `[待验证]`): "强通用 embedding + 好 hard negatives + reranker + curation" may matter more than model size. If the ladder shows <5pp deltas everywhere, the next lever is hard-negative SFT / reranker, not 4B models.

## Steps

1. **Verify model availability.** `target/release/cce models` (or fastembed docs) — confirm the fastembed model codes for:
   - `intfloat/multilingual-e5-small` (current default; `apps/cli/src/main.rs:59`)
   - `Qwen/Qwen3-Embedding-0.6B` (report's recommended local baseline, Table 4)
   - `intfloat/multilingual-e5-base` (user's candidate)
   If a model isn't in the fastembed registry, drop it from the matrix and record why in the comparison notes — do not add model-loading code for this experiment.
2. **Create the adapter yaml** for each additional model, cloning the e5-small-dense adapter from plan 001 with only `CCE_EMBEDDING_MODEL` changed:
   - `benchmarks/adapters/cce-dense-qwen3-06b.yaml` → `CCE_EMBEDDING_MODEL=Qwen/Qwen3-Embedding-0.6B`
   - `benchmarks/adapters/cce-dense-e5-base.yaml` → `CCE_EMBEDDING_MODEL=intfloat/multilingual-e5-base`
3. **Run all bundles on the same checkout and dataset** (`benchmarks/datasets/cce-self.jsonl`, v5 after plan 003):
   - `cce-dense-e5-small` (from plan 001) — baseline
   - `cce-dense-qwen3-06b`
   - `cce-dense-e5-base`
   - `cce-search` (sparse-only ablation context — how much is dense contributing at all)
   First run per model pays the one-time embedding-index build (~110s observed for e5-small at this repo size; record per-model index time + artifact size from `.cce/` snapshot dirs).
4. **Pairwise compare**: `compare` on (e5-small vs each candidate), per `docs/benchmarking.md` guardrails. Read the full report JSON — per-metric bootstrap CI, permutation p, Holm verdicts, per-intent deltas, MDE.
5. **Report table** (in the executor's verdict, not a doc file): per variant — `line_recall_at_5/20`, `file_success_at_5`, `mrr`, `relevant_line_density`, `abstain` correctness on no_context cases, mean engine `latency_ms` (post-001), index build time, model dims/memory, delta vs e5-small with CI.
6. **Slice the vocab-gap cases.** The v5 `vocab-gap-*` and CJK cases are the measurement surface for this question — report their per-case deltas separately from the aggregate.

## Files

- `benchmarks/adapters/cce-dense-*.yaml` (one per model)
- `research/output/` artifacts (generated, gitignored)

## Verification

- `uv run --project research cce-research compare benchmarks/datasets/cce-self.jsonl research/output/e5-small.jsonl research/output/qwen3-06b.jsonl` exits 0 and reports non-null CIs.
- Same for e5-base pair.
- Manifest `config_summary` in each bundle shows the intended model name (catches silent default fallback — plan 001's whole point).

## Done when

Both pairwise comparisons ran to completion with model identity recorded; a table of deltas + CIs + per-case vocab-gap deltas exists; latency/cost column filled.

## Maintenance notes

- Re-run when v5+ cases land or retrieval routing changes; numbers are tied to dataset revision.
- Keep raw bundle JSONL files — they're the audit trail for any claimed delta.

## Escape hatch

- If ONNX runtime can't load a model on this machine (arm64 asset issues): record the failure, proceed with the models that load; the comparison still has value with 2 arms.
- If MDE exceeds plausible deltas (underpowered): say so plainly; the honest output is "cannot distinguish at n≈30 — need more cases or accept inconclusive", not a forced winner.
