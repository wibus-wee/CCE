# Benchmarking contract

CCE evaluates stages independently so a better answer model cannot hide a worse retriever.

## Dataset schema

Each JSONL case records repository and immutable revision, query, intent, gold files, gold symbols, gold line ranges, supporting ranges, optional no-context label, and license/provenance metadata. Dataset builders must not redistribute source content unless its license permits it.

## Required metrics

- ranked retrieval: Recall@5/10/20/50, MRR, nDCG@10, file success, symbol and line recall
- selective retrieval: abstention accuracy, no-context precision, false-positive rate
- budgeted packs: coverage at 2K/4K/8K, relevant-line density, unique gold entities per token, redundancy, relation coverage, citation correctness
- systems: cold index time, incremental p50/p95, stale window, query and rerank p50/p95, disk/LOC, peak memory, context tokens and provider cost

## Mandatory ablations

Report lexical, dense, hybrid, summaries, routing, static symbols, selective graph expansion, reranking, knowledge retrieval, and tuned embeddings separately and grouped by query intent. Never compare systems using different revisions, budgets, gold normalization, or answer models.

## External projects

Adapters live under `benchmarks/adapters/`. They normalize public benchmarks without copying incompatible datasets into this repository. Every result bundle includes tool version, dependency lockfile hash, model identity/revision, configuration, dataset revision, hardware, raw per-case outcomes, and aggregate confidence intervals.

Systems listed in `benchmarks/systems.yaml` are not assigned scores until an adapter, pinned revision, license review, and reproducible result bundle exist. `adapter_required` and `compliance_review_required` are deliberate non-results, not zero scores. CCE's self-dataset is a smoke/regression suite, not evidence of superiority over external systems.

Run the local reproducible path with:

```bash
cargo build --release --locked -p cce-cli
uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce.yaml . WORKTREE research/output/cce-self.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/cce-self.jsonl --output research/output/cce-self-metrics.json
```

Published bundles must replace `WORKTREE` with an immutable commit and record CPU, memory, operating system, model endpoint/revision, cold versus warm cache state, and lockfile digests.
