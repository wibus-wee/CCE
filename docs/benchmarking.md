# Benchmarking contract

CCE evaluates stages independently so a better answer model cannot hide a worse retriever.

## Dataset schema

Each JSONL case records repository and immutable revision, query, intent, gold files, gold symbols, gold line ranges, supporting ranges, optional no-context label, and license/provenance metadata. Dataset builders must not redistribute source content unless its license permits it.

## Required metrics

- ranked retrieval: Recall@5/10/20/50, MRR, nDCG@10, file success, symbol and line recall
- selective retrieval: abstention accuracy, no-context precision, false-positive rate
- budgeted packs: coverage at 2K/4K/8K, relevant-line density, unique gold entities per token, redundancy, relation coverage, citation correctness
- systems: cold index time, incremental p50/p95, stale window, query and rerank p50/p95, disk/LOC, peak memory, context tokens, and local compute cost

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

Published bundles must replace `WORKTREE` with an immutable commit and record CPU, memory, operating system, model/bundle revision, cold versus warm cache state, and lockfile digests.

The metadata-only JS/TS production regression set pins Vue core and combines exact symbol navigation with multi-file architecture questions:

```bash
git clone https://github.com/vuejs/core.git /tmp/vue-core
git -C /tmp/vue-core checkout 40423896796ff7438a1aa70ee3cf22fe5b0ac224
CCE_DATA_DIR=/tmp/vue-core-cce target/release/cce \
  --scip-auto --scip-typescript /opt/scip-typescript/bin/scip-typescript index /tmp/vue-core
CCE_DATA_DIR=/tmp/vue-core-cce uv run --project research cce-research run \
  benchmarks/datasets/vue-core-js-ts.jsonl benchmarks/adapters/cce.yaml \
  /tmp/vue-core 40423896796ff7438a1aa70ee3cf22fe5b0ac224 \
  research/output/vue-core-js-ts.jsonl
uv run --project research cce-research evaluate \
  benchmarks/datasets/vue-core-js-ts.jsonl research/output/vue-core-js-ts.jsonl \
  --output research/output/vue-core-js-ts-metrics.json
```

Embedding benchmarks use repository-isolated splits and run each model in a clean process so peak RSS is not contaminated by a previously loaded model. They report overall and per-query-kind Recall@1/5/10/20, MRR, nDCG@10, indexing throughput, single-query p50/p95, and peak RSS. Run the pinned base suite with:

```bash
uv run --project research --extra models cce-research benchmark-models \
  research/output/corpus/test.jsonl \
  benchmarks/embedding-models.yaml \
  research/output/base-models.json
```

Base-versus-tuned comparisons must use the same held-out repository split. A lower training loss is not a promotion result.

ANN recall gates must cover repeated independent HNSW builds; a single passing construction can hide graph-build variance. CCE's synthetic smoke records minimum, mean, maximum, and raw per-build Recall@10, while production promotion additionally requires held-out repository queries plus p95 latency and peak RSS.

Cross-encoder benchmarks operate on one positive plus the same repository-local hard negatives per query. Each model runs in a clean process and reports Recall@1/5/10, MRR, nDCG@10, pairs/s, per-query p50/p95, peak RSS, per-query-kind metrics, and raw case ranks:

```bash
uv run --project research --extra models cce-research benchmark-rerankers \
  research/output/corpus/test.jsonl \
  benchmarks/reranker-models.yaml \
  research/output/base-rerankers.json \
  --candidates 8
```

Use an identical candidate count and held-out file for a tuned model. A runtime end-to-end ablation must additionally compare CCE with `--reranker disabled` and `--reranker local`, because candidate construction, query-centered snippets, fusion weight, and model ranking can each change the final order.
