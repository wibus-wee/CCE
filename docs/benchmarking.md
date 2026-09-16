# Benchmarking contract

CCE evaluates stages independently so a better answer model cannot hide a worse retriever.

## Dataset schema

Each JSONL case records repository and immutable revision, query, intent, gold files, gold symbols, gold line ranges, supporting ranges, optional no-context label, and license/provenance metadata. Dataset builders must not redistribute source content unless its license permits it.

Two optional fields control how the planner is exercised:

- `supply_intent` (default `true`): when `false`, the adapter must not pass `intent` to the system. The system's own classifier decides; the resolved intent is recorded on the result as `predicted_intent` and scored against the case's gold `intent` as `intent_accuracy`. Use this to measure routing/classifier quality instead of bypassing it.
- `routes` (default `[]`): a non-empty list overrides the planner's route selection, passed through as repeated `--route` flags. Ablation rows reuse a query with a restricted route set and their own `case_id` (e.g. `tags: ["ablation", "lexical-only"]`).

## Result schema

Each result records the retrieved source-linked ranges plus the plan the system actually executed: `predicted_intent`, `plan_routes`, `graph_policy`, and `missing_capabilities`. Misrouted queries are attributable instead of silently averaged away. `abstained` means the system returned no source-linked evidence.

## Required metrics

- ranked retrieval: Recall@5/10/20/50, MRR, nDCG@10, file success, symbol and line recall
- selective retrieval: abstention accuracy, no-context precision, false-positive rate
- planner: intent accuracy over `supply_intent: false` cases
- budgeted packs: coverage at 2K/4K/8K, relevant-line density, unique gold entities per token, redundancy, relation coverage, citation correctness
- systems: cold index time, incremental p50/p95, stale window, query and rerank p50/p95, disk/LOC, peak memory, context tokens and model inference cost

All ranked metrics are also reported under `by_intent/<intent>/` groups.

## Mandatory ablations

Report lexical, dense, hybrid, summaries, routing, static symbols, selective graph expansion, reranking, knowledge retrieval, and tuned embeddings separately and grouped by query intent. Route ablations run through the `routes` case field and the `cce-search` adapter (retrieval stage only); the `cce` adapter exercises the full context-pack stage. Never compare systems using different revisions, budgets, gold normalization, or answer models.

## System comparison

`cce-research compare` computes paired per-case deltas between two result bundles with bootstrap confidence intervals over resampled cases. Both bundles must cover the same case set; paired tests are the only sanctioned way to claim a regression or improvement.

## External projects

Adapters live under `benchmarks/adapters/`. They normalize public benchmarks without copying incompatible datasets into this repository. Every result bundle includes tool version, dependency lockfile hash, model identity/revision, configuration, dataset revision, hardware, raw per-case outcomes, and aggregate confidence intervals.

Systems listed in `benchmarks/systems.yaml` are not assigned scores until an adapter, pinned revision, license review, and reproducible result bundle exist. `adapter_required` and `compliance_review_required` are deliberate non-results, not zero scores. CCE's self-dataset is a smoke/regression suite, not evidence of superiority over external systems.

Run the local reproducible path with:

```bash
cargo build --release --locked -p cce-cli
uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce.yaml . WORKTREE research/output/cce-self.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/cce-self.jsonl --output research/output/cce-self-metrics.json
# retrieval stage only:
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search.yaml . WORKTREE research/output/cce-self-search.jsonl
# paired comparison between two bundles:
uv run --project research cce-research compare benchmarks/datasets/cce-self.jsonl research/output/baseline.jsonl research/output/candidate.jsonl
```

Published bundles must replace `WORKTREE` with an immutable commit and record CPU, memory, operating system, model endpoint/revision, cold versus warm cache state, and lockfile digests.
