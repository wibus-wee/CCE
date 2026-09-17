# Benchmarking contract

CCE evaluates stages independently so a better answer model cannot hide a worse retriever.

## Harness architecture

`research/cce_research/` is layered so evidence flows in one direction:

| Module | Role |
|---|---|
| `schema.py` | Case/result contracts: gold evidence, `gold_facts`, `judged_files`, intent withholding, route pins, lineage fields |
| `adapters.py` | Command-template execution; normalizes context packs and raw search results into `RetrievedRange` |
| `metrics.py` | Per-case observations only — every metric is a pure function of (case, result) |
| `stats.py` | Inference: bootstrap CI, paired permutation test, Holm-Bonferroni, effect size, MDE |
| `pooling.py` | Gold-completeness management: adjudication queue + verdict application |
| `ablations.py` | Leave-one-route-out case generation and marginal route utility |
| `variants.py` | Metamorphic query variants and agreement scoring |
| `probes.py` | Position-permutation sensitivity and the staleness probe |
| `cli.py` | Orchestration: `run`, `evaluate`, `compare`, `ablate`, `variants`, `adjudicate`, `apply-judgments`, `permute`, `lint-gold`, `probe-staleness`, `utility` |

## Dataset schema

Each JSONL case records repository and immutable revision, query, intent, gold files, gold symbols, gold line ranges, supporting ranges, optional no-context label, and license/provenance metadata. Dataset builders must not redistribute source content unless its license permits it.

Additional optional fields:

- `supply_intent` (default `true`): when `false`, the adapter must not pass `intent` to the system. The system's own classifier decides; the resolved intent is recorded as `predicted_intent` and scored against the case's gold `intent`.
- `routes` (default `[]`): a non-empty list overrides the planner's route selection, passed through as repeated `--route` flags. Ablation rows reuse a query with a restricted route set.
- `gold_facts` (default `[]`): atomic claims the evidence must support, each with `evidence` ranges and/or `symbols`. Scored as `claim_support` — separates "found the file" from "found the answer" without an LLM judge.
- `judged_files` (default `[]`): paths an adjudicator reviewed and marked *not* relevant. Retrieved paths outside gold ∪ supporting ∪ judged are **unjudged**, not false positives.
- `derived_from` / `derivation`: lineage for generated cases (`ablate:<route>`, `variant:<transform>`).
- `answer_key` (optional): the string a correct consumer must extract from the pack; reserved for downstream answer probes.

## Result schema

Each result records the retrieved source-linked ranges plus the plan the system actually executed: `predicted_intent`, `plan_routes`, `graph_policy`, and `missing_capabilities`. Misrouted queries are attributable instead of silently averaged away. `abstained` means the system returned no source-linked evidence.

Result `metadata` carries the executed `command` and `engine_latency_ms` — the engine's own reported timing (`latencyMs` on search results and context packs), distinct from `query_ms`, which is subprocess wall-clock including spawn and model-session init. Latency comparisons should cite `engine_latency_ms` for engine compute and `query_ms` for end-to-end cost.

Adapter YAMLs may declare `dense: baseline|local` plus `embedding_model`/`embedding_dimensions`; the adapter appends the corresponding global CLI flags and derives `model_identity` as `local:<model>` when unset, so bundles stay attributable to the model that produced them.

## Required metrics

- ranked retrieval: Recall@5/10/20/50, MRR, nDCG@10, file success, symbol and line recall
- judgment-aware: `unjudged_rate@k` (top-k share with no verdict), `bpref@20` (penalizes only judged-irrelevant items; Buckley–Voorhees)
- claim-level: `claim_support` over `gold_facts`
- selective retrieval: abstention accuracy, no-context precision, false-positive rate
- planner: `intent_accuracy`, `intent_recall/<intent>`, `intent_precision/<intent>` over `supply_intent: false` cases
- architecture routing: `component_recall_at_5/20` and `component_mrr` — package-granularity Task→Component recall. `gold_components` annotates the packages a case's gold lives in; each run captures the system's `map` output once as `component_map` (package → rootDir) and resolves retrieved paths by longest-prefix match. Cases without `gold_components` are skipped, not zeroed. Annotation convention: a case's `gold_components` is the set of packages that own its `gold_files` — resolved by the same longest-prefix `rootDir` rule the metric applies (list names with `cce map`; e.g. `crates/cce-store/...` → `cce-store`, `apps/web/...` → `@cce/web`, top-level files → the workspace root package)
- budgeted packs: coverage at 2K/4K/8K, relevant-line density, unique gold entities per token, redundancy, relation coverage, citation correctness
- systems: cold index time, incremental p50/p95, stale window, query and rerank p50/p95, disk/LOC, peak memory, context tokens and model inference cost

All per-case metrics are also reported under `by_intent/<intent>/` groups.

## Mandatory ablations

Report lexical, dense, hybrid, summaries, routing, static symbols, selective graph expansion, reranking, knowledge retrieval, and tuned embeddings separately and grouped by query intent. `cce-research ablate` generates the full leave-one-route-out matrix from a baseline run's captured `plan_routes`; `cce-research utility` then reports each route's marginal metric contribution. The `cce-search` adapter exercises retrieval only; `cce` exercises the full context-pack stage. Never compare systems using different revisions, budgets, gold normalization, or answer models.

## Statistical methodology

`cce-research compare` computes paired per-case deltas and reports, per metric:

- bootstrap 95% CI over resampled cases;
- paired permutation (randomization) p-value — preferred over bootstrap significance at small n (Smucker–Allan–Carterette 2007; Urbano et al. 2019);
- Holm-Bonferroni significance across the whole reported metric family — a single metric table contains enough comparisons that ~1 spurious "significant" per run is expected without correction;
- Cohen's d_z effect size;
- the minimum detectable effect (MDE): the smallest true delta resolvable at 80% power. When |observed| < MDE the honest verdict is "unresolvable at this n", not "no difference".

`compare --gate` exits nonzero when a guardrail metric (default: `file_success@20`, `file_recall@20`, `abstention_accuracy`, `no_context_precision`) regresses with Holm-corrected significance. Guardrails block; diagnostics inform.

## Gold completeness and adjudication

Gold sets are never complete. `unjudged_rate@k` measures how much of the top-k lacks a verdict; `bpref` scores only judged nonrelevant evidence. Workflow:

```bash
# pool unjudged top-20 paths across one or more result bundles
uv run --project research cce-research adjudicate benchmarks/datasets/cce-self.jsonl \
  research/output/cce-self.jsonl research/output/cce-self-search.jsonl \
  research/output/adjudication-queue.jsonl
# fill "verdict": "relevant"|"irrelevant" per row, then:
uv run --project research cce-research apply-judgments benchmarks/datasets/cce-self.jsonl \
  research/output/adjudication-queue.jsonl benchmarks/datasets/cce-self-adjudicated.jsonl
```

`relevant` verdicts promote paths into `gold_files`; `irrelevant` verdicts go to `judged_files`. Re-run evaluation on the adjudicated revision.

`lint-gold` verifies every gold path is reachable under the union route set — unreachable gold means bad annotation, not a system defect.

## Metamorphic variants and probes

`variants` emits perturbed copies of each query (inflection: "breaks"→"break"; mixed-language CJK↔EN; verbose framing; keyword scramble) sharing the parent's gold. `variant-agreement` reports how often variants reproduce the parent outcome — disagreement is brittleness evidence.

`permute` shuffles each result's item order and reports the per-case spread of order-sensitive metrics. It answers the packer-contract question empirically: if mrr/ndcg barely move under permutation, those metrics were measuring ordering, not content.

`probe-staleness` writes a marker file into the repository, queries it, rewrites it, and re-queries — reporting whether fresh content surfaces and whether stale content is still served (`verified_current`).

## External projects

Adapters live under `benchmarks/adapters/`. They normalize public benchmarks without copying incompatible datasets into this repository. Every result bundle includes tool version, dependency lockfile hash, model identity/revision, configuration, dataset revision, hardware, raw per-case outcomes, and aggregate confidence intervals.

Systems listed in `benchmarks/systems.yaml` are not assigned scores until an adapter, pinned revision, license review, and reproducible result bundle exist. `adapter_required` and `compliance_review_required` are deliberate non-results, not zero scores. CCE's self-dataset is a smoke/regression suite, not evidence of superiority over external systems.

Run the local reproducible path with:

```bash
cargo build --release --locked -p cce-cli
uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce.yaml . WORKTREE research/output/cce-self.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/cce-self.jsonl --output research/output/cce-self-metrics.json --confusion
# retrieval stage only:
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search.yaml . WORKTREE research/output/cce-self-search.jsonl
# paired comparison (CI + permutation p + Holm + effect size + MDE):
uv run --project research cce-research compare benchmarks/datasets/cce-self.jsonl research/output/baseline.jsonl research/output/candidate.jsonl
# regression gate for CI:
uv run --project research cce-research compare benchmarks/datasets/cce-self.jsonl research/output/baseline.jsonl research/output/candidate.jsonl --gate
```

### Dense model comparison

`benchmarks/adapters/cce-(search-)dense-<model>.yaml` variants pin a local embedding model via `dense: local` + `embedding_model: <code>` (`cce models` lists valid codes). Each model gets its own snapshot — embedding model identity is part of `index_profile_hash` — so arms share `.cce-benchmark` without wiping. Warm each model's index once (`cce --dense local --embedding-model <code> index .`; the first run downloads into `<data>/models/`, the documented network opt-in), then run all arms on the same dataset revision and `compare` pairwise. Model identity travels in the bundle manifest via `model_identity: local:<code>`.

First ladder result (cce-self-v5, n=29, search stage): dense beats sparse decisively — recall@20 +0.12 (p=.03), nDCG@10 +0.09 (p=.01), file_success@20 .62→.79 — at ~3s/query subprocess cost dominated by per-process ONNX session init (see `engine_latency_ms` for engine-only time). Between dense models the suite is underpowered: e5-base and jina-v2-base-code show positive point estimates on the vocab-gap subset (recall@5 .40→.60/.70) but all CIs include zero at MDE≈0.10–0.16. Qwen3-Embedding-0.6B is not in the fastembed registry and was dropped from the matrix. Conclusion: keep e5-small as default; revisit when the suite grows or a reranker lands.

Published bundles must replace `WORKTREE` with an immutable commit and record CPU, memory, operating system, model endpoint/revision, cold versus warm cache state, and lockfile digests.
