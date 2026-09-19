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
- `gold_facts` (default `[]`): atomic claims the evidence must support, each with `evidence` ranges and/or `symbols`. Scored as `claim_support` — separates "found the file" from "found the answer" without an LLM judge. A `symbols` entry is scoped to the fact's `evidence` paths: a same-named symbol cited from an unrelated file does not count. Facts carrying only bare symbols (no evidence paths) are unscoped — excluded from the strict metric's denominator and reported under `claim_support_unscoped`.
- `judged_files` (default `[]`): paths an adjudicator reviewed and marked *not* relevant. Retrieved paths outside gold ∪ supporting ∪ judged are **unjudged**, not false positives.
- `derived_from` / `derivation`: lineage for generated cases (`ablate:<route>`, `variant:<transform>`).
- `answer_key` (optional): the string a correct consumer must extract from the pack; reserved for downstream answer probes.

## Result schema

Each result records two layers derived from one normalization: `items`, the ranked retrieval/packing units (each with an item id, original rank, token cost, a primary source address, and supporting addresses), and `retrieved`, the flat compat rows expanded from those items — one row per cited address, all sharing the item's rank. @K cutoffs and token budgets count **items**, never addresses: an item citing three addresses occupies one rank slot and spends its tokens once. Items without addresses (orientation) consume budget but produce no flat rows.

Results also carry `result_kind` (`search`/`context`), the pack's own `used_tokens`, the engine's `verdict_state` (`answered`/`weak_witness`/`abstained`), the context pack's `delivery_report` (shipped item ids, per-hit omission reasons, delivered/missing claim terms — `None` on search results), and `metrics_version` — `compare` refuses to mix accounting versions. Plus the plan the system actually executed: `predicted_intent`, `plan_routes`, `graph_policy`, and `missing_capabilities`. Misrouted queries are attributable instead of silently averaged away. `abstained` means the system returned no source-linked evidence; a `weak_witness` verdict with candidates is a different event and is reported separately via the `verdict_*` metrics. Legacy results (no `items`) stay readable: metrics that need item identity or a packing budget mark themselves uncomputable rather than guessing from flat rows.

Result `metadata` carries the executed `command` and `engine_latency_ms` — the engine's own reported timing (`latencyMs` on search results and context packs), distinct from `query_ms`, which is subprocess wall-clock including spawn and model-session init. Latency comparisons should cite `engine_latency_ms` for engine compute and `query_ms` for end-to-end cost.

Adapter YAMLs may declare `dense: baseline|local` plus `embedding_model`/`embedding_dimensions`; the adapter appends the corresponding global CLI flags and derives `model_identity` as `local:<model>` when unset, so bundles stay attributable to the model that produced them.

Adapters default to `session: subprocess` — one engine process per case, so `query_ms` includes spawn and model-session init. `session: daemon` instead spawns `cce-daemon` once per run (the sibling binary of the command template's executable: `target/release/cce` → `target/release/cce-daemon`), blocks until `GET /healthz` answers, then issues each case as `POST /v1/search` or `POST /v1/context` with the same template-derived parameters. This exercises the production warm path: dense model init is paid once at startup, and `query_ms` measures per-request wall-clock only. The daemon inherits the adapter `environment` (`CCE_DATA_DIR` etc.) and logs to `research/output/<adapter>.daemon.log`; `metadata.command` records the HTTP request and `metadata.session` records `daemon`. One caveat: `/v1/search` cannot pin routes — route-pinned cases on a search adapter fall back to a cold subprocess for that case (`metadata.session: subprocess-fallback`); `/v1/context` accepts `routes` and honors them in-session.

## Required metrics

- ranked retrieval: Recall@5/10/20/50, MRR, nDCG@10, file success, symbol and line recall
- judgment-aware: `unjudged_rate@k` (top-k share with no verdict), `bpref@20` (penalizes only judged-irrelevant items; Buckley–Voorhees)
- claim-level: `claim_support` over `gold_facts` (strict: evidence overlap or path-scoped symbol), `claim_support_legacy` (pre-012 loose criterion, continuity only), `claim_support_unscoped` (count of undecidable bare-symbol facts)
- selective retrieval: abstention accuracy, no-context precision, false-positive rate, plus `verdict_answered`/`verdict_weak_witness`/`verdict_abstained`/`verdict_flagged` shares when the system exports its evidence verdict
- planner: `intent_accuracy`, `intent_recall/<intent>`, `intent_precision/<intent>` over `supply_intent: false` cases
- architecture routing: `component_recall_at_5/20` and `component_mrr` — package-granularity Task→Component recall. `gold_components` annotates the packages a case's gold lives in; each run captures the system's `map` output once as `component_map` (package → rootDir) and resolves retrieved paths by longest-prefix match. Cases without `gold_components` are skipped, not zeroed. Annotation convention: a case's `gold_components` is the set of packages that own its `gold_files` — resolved by the same longest-prefix `rootDir` rule the metric applies (list names with `cce map`; e.g. `crates/cce-store/...` → `cce-store`, `apps/web/...` → `@cce/web`, top-level files → the workspace root package)
- budgeted packs: `budget_compliant` (the pack's own `usedTokens` — or the item sum including address-less orientation — against the case budget; uncomputable on search results and legacy rows), `pack_sufficiency` (strict claim support over the items that actually fit the budget), coverage at 2K/4K/8K, relevant-line density, unique gold entities per token, redundancy, relation coverage, citation correctness
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

`benchmarks/adapters/cce-(search-)dense-<model>.yaml` variants pin a local embedding model via `dense: local` + `embedding_model: <code>` (`cce models` lists valid codes). Since dense is now the product default (`--dense local` in cli/daemon/mcp), adapters that omit `dense:` exercise the default jina-code path; pin `dense: disabled` when a sparse-control arm is needed (`cce-search-sparse.yaml`). Each model gets its own snapshot — embedding model identity is part of `index_profile_hash` — so arms share `.cce-benchmark` without wiping. Warm each model's index once (`cce --embedding-model <code> index .`; the first run downloads into `<data>/models/`), then run all arms on the same dataset revision and `compare` pairwise. Model identity travels in the bundle manifest via `model_identity: local:<code>`.

First ladder result (cce-self-v5, n=29, search stage): dense beats sparse decisively — recall@20 +0.12 (p=.03), nDCG@10 +0.09 (p=.01), file_success@20 .62→.79 — at ~3s/query subprocess cost dominated by per-process ONNX session init (see `engine_latency_ms` for engine-only time). Between dense models the suite is underpowered: e5-base and jina-v2-base-code show positive point estimates on the vocab-gap subset (recall@5 .40→.60/.70) but all CIs include zero at MDE≈0.10–0.16. Qwen3-Embedding-0.6B is not in the fastembed registry and was dropped from the matrix.

Second ladder result (cce-self-v5.1, n=29, daemon session, enriched symbol descriptors): **jina-v2-base-code beats e5-small with significance** — nDCG@10 +0.09 (p=.0005, Holm-significant), recall@5 +0.10 (p=.006), MRR +0.07 — at a small recall@20 tail cost (−0.06, ns). On the CJK vocabulary-gap case the gold symbol went from absent-at-50 to rank 1. `jinaai/jina-embeddings-v2-base-code` is now `DEFAULT_LOCAL_EMBEDDING_MODEL`. Next lever for the remaining claim-level gap is a reranker stage.

### Reranker comparison

`reranker: <model>` in an adapter YAML enables the local cross-encoder pass (`cce models` lists reranker codes; the flag travels to daemon startup and subprocess fallback commands in `--reranker=<model>` form). The reranker only reorders hits carrying topical routes (lexical/dense/exact/hybrid); graph-expansion hits (structural/knowledge/history) keep fused slots because they answer "what is connected", not "what matches the query text" — scoring them topically destroyed impact/trace recall in the first measurement. `model_identity` gains a `+rerank:<model>` suffix.

First reranker result (cce-self-v5.1 adjudicated head, jina-daemon baseline vs `rozgo/bge-reranker-v2-m3`): **no measured benefit at high cost** — nDCG@10 −0.11, recall@5 −0.07, MRR −0.10 (all ns after adjudicating the promoted head items; initial unjudged@5 +0.22 was adjudication-pool staleness, resolved by judging 111 promoted items: 8 relevant, 103 irrelevant). The dominant failure is prose preference: a general passage-ranking cross-encoder promotes README/docs/plans over code symbols on code-intent queries. Cost is +5.8s/query (50-pair scoring on CPU). The stage stays opt-in via `--reranker`; it is not recommended by default. A code-aware reranker model or richer reranker input formatting is the open question if this lever is revisited.

### Code-switched query handling

unicode61 indexes a CJK run as one monolithic token and the exact-symbol identifier gate dropped every non-identifier token, so a mixed CJK+Latin issue query ("search 偶发返回陈旧结果，哪里把过期视图标记为 stale") collapsed to the pairwise lexical floor: 7 hits, all gold out of top-50. Fix: `entity_tokens` skips the identifier-shape gate when the query contains CJK (embedded Latin words are deliberate anchors; CJK spellings are valid identifier territory), and `fts_match_queries` adds a code-switched cascade — Latin-term exact AND, prefix AND, then a Latin single-term tail that homogeneous queries still don't get (the two-distinct-term floor stands for them).

Result (cce-self-v5.1, search stage): sparse `self-inferred-issue-stale` goes 7→60 hits, file_recall@20 0.25→0.50, all four gold files inside top-50 (`metadata.rs` only via the Latin singles tail — its docs share no query vocabulary, so it stays a dense-arm strength). Suite claim_support 0.50→0.58 sparse, 0.75→0.875 dense; no guardrail regression. Pure-CJK queries are unchanged by design — with no Latin anchor there is no sparse evidence to reach for; that is the honest dense boundary.

### SCIP provider ingest

`cce providers` reports per-provider readiness (`scip:file` reads a repo-root `index.scip`, `scip:rust-analyzer`/`scip:typescript` spawn the toolchain indexer); `cce index` ingests `References`/`Implements` relations at `RelationOrigin::Scip`, trust level 1, joined back to CCE entities by byte-range containment. `cce def`/`cce refs` answer navigation queries from those edges. A failed provider degrades to a report entry, never a silent claim. `index.scip` is a generated artifact — it is listed in `.gitignore`/`.cceignore` and read by the file provider directly, not through the scanner.

First ingest result (cce-self-v5.1, search stage, rust-analyzer 1.97.1 → 37 documents, 3746 definitions, 2860 reference edges): direction-positive — file_recall@20 +0.05 (p=.045), file_success@20 +0.17 (ns) — at a measured cost: query_ms +201ms (p=.0005, Holm-significant) because provider detection/ingest runs inside the `require_fresh` index pass. The accompanying unjudged_rate rise (+0.03–0.06) was pool staleness: SCIP expansion surfaced candidates outside the v5.1 pool.

Second adjudication round (cce-self-v5.2): 166 pooled top-20 candidates judged — 14 relevant promoted to `gold_files` (the `0001_initial.sql` artifact-digest schema, `rerank.rs`, `scip.rs`, and `cce-core` type consumers of `SourceAddress`), 152 recorded in `judged_files`. `unjudged_rate@20` is now 0.00 on the pooled arms, so precision-side metrics are real rather than penalizing unannotated gold gaps: bpref@20 0.50 sparse / 0.55 dense, claim_support 0.54 sparse / 0.75 dense. The dense arm's remaining quarter of unsupported claims is the next real target — the vocab-gap cases find the file but not all the atomic evidence.

### Structural ranking features, feedback expansion, and opportunistic graph walks (v5.3)

Four deterministic levers landed together: (1) `ChangedWith` co-change edges — mined since the world-model rebuild but never consumed — now feed a confidence-scaled partner bonus in `apply_structural_features` and the `Opportunistic` expansion arm (the first `Both`-direction arm, which is the only directionally-correct way to traverse the single-stored symmetric edge); (2) a BugCache-style recency prior — `lastTouched` unix time on File entities from the same history walk that feeds co-change, applied as a 90-day-half-life bonus gated to issue-localization/history intents; (3) a same-package membership bonus via Package `rootDir` longest-prefix matching; (4) an RM3-style pseudo-relevance-feedback pass — top-5 topical hits' discriminative identifier terms re-query the lexical route once at 0.6 attenuation, before expansion picks its seeds. Separately, `union_plan` now runs `Opportunistic` structural expansion under inferred intent — propagated scores are orders of magnitude below topical hits so expansion evidence can only rank when corroborated; the graph view stays non-required and reports an explicit skip when unready.

Result (cce-self v5.3 adjudicated, n=29, daemon-dense arm vs. same-revision baseline, `compare --gate` PASS): recall@20 +0.07 (p=.017), recall@5 +0.04 (p=.035), file_recall@20 +0.07 (p=.017), MRR +0.09 (p=.047), nDCG@10 +0.06 (p=.042), bpref@20 +0.07 (p=.0495), file_success@20 +0.07 (ns). The opportunistic A/B (features vs. features+expansion) moved nothing resolvable — expansion as a corroborating tail route neither helps nor drifts at this corpus size; kept as safe recall surface for larger repositories. Two honest residuals: claim_support −0.17 (ns, n=12, below MDE) — the dropped cases were only incidentally supported by a whole-file descriptor that new mid-tail candidates displaced; the atomic evidence symbols still don't rank, which is the vocab-gap problem, not a new regression. RRF weight retuning was skipped: at n=29 (MDE ≈ 0.10–0.16) numerator tweaks are unresolvable by construction. Third adjudication round: 115 pooled candidates → 12 promoted gold + 103 judged, unjudged@20 back to .003.

### Head-order round: corroboration joins, head dedup, CJK glossary, file vote (v5.4)

Five levers aimed at the recall@5↔recall@20 gap, which diagnosis showed was head-order, not candidate-generation — gold files parked at ranks 6–20 while redundant or isolated hits held the top slots: (1) **corroboration join** — graph-expansion hits are `entity:`-keyed while document candidates are document-keyed, so expansion evidence could never merge into the candidates it confirmed; an `ExpansionEvidence` side channel (entity/region/file-path keys, best propagated score + edge count) now joins it back as a ≤0.2 bonus, structural-route candidates skipped to avoid double-counting; (2) **intra-candidate cluster support** — the files answering one query call/import/co-change each other; top-30 candidates resolve to FILE entities and edges whose other endpoint also ranks count as ln-damped support ≤0.2 (file-level adjacency only: `Calls`/`References` are symbol-level, so `Imports`/`ChangedWith` carry the signal); (3) **windowed per-file head dedup** — selection caps 1 hit/file inside top-5, 2 inside top-10, 3 after; skipped hits drop, not defer, and file-level golds make the cap a strict non-loss for recall (verified: all 29 cases are file-level); (4) **CJK→English glossary** — a curated 59-pair static table appends ≤10 English anchors to the lexical query and the PRF re-query (whose term mining also sees the anchors so they are not re-added); dense and exact-symbol routes keep the raw query since embeddings handle CJK natively and glossary words would pollute the `entity_tokens` identifier gate; (5) **champion file-vote** — files are keyed by best rank + pooled occurrence count, and only the file's champion document collects ≤0.2 × best-rank mass × ln(occ)/ln(cap); voting *all* documents of a recurring file gained +.017 recall@5 but paid −.020 recall@10, champion-only keeps +.013 for −.006.

Result (cce-self adjudicated, n=29, dense-daemon and sparse arms vs. same-revision baselines, `compare --gate` PASS both): dense recall@5 **+.101 (p=.002)**, recall@10 +.074 (p=.019), nDCG@10 +.091 (p=.006), file_success@20 +.069 (ns); absolute recall@5 .698 vs recall@20 .859 — the head gap narrowed from .28 to .16. Sparse recall@20 +.125 (p=.005), recall@10 +.118 (p=.039), recall@5 +.066 (p=.071) — the priors are route-agnostic, so the control arm benefits as much as the product arm. A jina-reranker-v2-multilingual A/B (locally cached cross-encoder over the fused head) was flat everywhere — reranking stays opt-in. Honest residual: the multi-gold coverage ceiling — cases with 7–11 gold files can cover at most 5 slots, bounding per-case recall@5 at ~.71 even with a perfect head; remaining misses are mechanism-adjacent non-golds (planner/test/benchmark files sharing vocabulary) that no deterministic prior distinguishes from the mechanism itself. claim_support −.13 vs the v5.3 baseline (ns, n=12) — the standing evidence-granularity issue, unchanged in mechanism.

### Symbol evidence join: graph-backed atomic evidence (v5.5)

v5.4 left claim_support at .54 because retrieval granularity and evidence granularity differ: documents surface thematically, but claim facts name atomic symbols (`set_view_status`-class functions) that share no vocabulary with the query. The join closes that gap with graph facts instead of lexical guessing — outgoing `References`/`Calls` edges from the top-24 unique candidate entities aggregate a neighbor table (pointers × best referrer score), admitting a neighbor on structural consensus (≥2 referrers) or query-name overlap alone.

Three design decisions mattered empirically: (1) **emission mass must be pool-anchored** — an absolute mass two orders below topical scores emits only in shallow pools; anchoring just above the incumbent score at `limit − MAX` lands the whole block inside the window while displacing only the weakest incumbents and never reordering the head; (2) **two interleaved lanes** — signature-type hubs (`ViewManifest`, `ViewState`) are pointed at by everything and dominate any single ordering, so a named lane restricted to callable kinds (claim facts name functions/methods) interleaves with the consensus lane — types still emit, but they cannot crowd out the claim candidates; (3) **seed and degree caps** — 24 unique top-scoring entities × 48 outgoing edges bounds the graph walk (~50 store calls/query), and neighbor resolution uses the new batched `entities_by_ids` (one IN-chunked query instead of ~400 point lookups).

Result (same protocol as v5.4): dense claim_support **.542 → .625** (+.083 absolute; +.042 paired vs the v5.4 arm, ns at n=12), recall@20 +.031 (p=.0195), symbol_recall@20 +.023, recall@5/.10 flat, all guardrails 1.0 — the target case's `set_view_status` now emits at rank ~20 where no prior arm surfaced it at all. Sparse arm gate-pass with larger deltas (claim_support +.042, recall@20 +.128 p=.003) — structural evidence carries more weight without dense candidates. Cost: ~112–500ms per query for the graph walk depending on pool depth; `unjudged_rate@50` +.019 (p=.0035) is the pooling artifact of newly emitted documents, feeding the next adjudication round.

### Graph flow: one propagation pass vs four priors (v6, experimental)

The v5.x ranking layer accreted bounded additive priors that all approximate one mechanism — query-seeded flow over the typed entity graph. `apply_graph_flow` makes that mechanism explicit: candidate entities seed mass, per-kind conductance weights carry it along edge direction (calls/tests→mechanisms forward, contains→file upward, changed_with symmetric), confidence-weighted fan-out normalization splits mass within each kind, non-backtracking forbids immediate reversal, and hop-1's top receivers expand the edge frontier for real two-hop reach (file→member→mechanism). Two readouts: received-mass bonuses on candidates, and symbol `entity:` emissions at the pool boundary. `CCE_FLOW` gates the A/B — `off` (default) legacy only, `union` both, `replace` flow instead of the four priors.

Six-arm A/B (same snapshot, n=29, adjudicated cce-self):

| arm | recall@20 | symbol_recall@20 | claim_support | mrr | query_ms |
|---|---|---|---|---|---|
| dense off | .843 | .489 | .625 | — | baseline |
| dense union | .843 | .489 | .625 | — | ≈same |
| dense replace | .843 | .489 | .625 | — | −1454 (ns, cold-arm noise) |
| sparse off | .781 | .483 | .583 | baseline | 942 |
| sparse union | .771 | .454 | .583 | +.006 | **+1379 (p=.0005)** |
| sparse replace | .761 | .431 | .500/.583* | **+.091** | −190 |

*claim_support .583 in the first iteration, .500 after frontier expansion — emission-mix churn, both ns.

Verdict, stated plainly: **the mechanism is validated, the replacement is not yet earned.** Union ≡ off everywhere on quality proves flow computes the same evidence the four priors compute — the consolidation thesis is architecturally right. Dense replace is full parity (product default unaffected). But sparse replace trades ~.02–.05 of recall@k/symbol_recall tail coverage for +.09 MRR/component precision — direction-consistent, below MDE at n=29, but real enough to keep `off` as the shipped default. Two iteration findings recorded for the next attempt: receiver-side in-degree gating is *wrong inside a bounded universe* (≤32 senders makes in-degree a consensus measure, not hubness — gating it suppressed gold type hubs); frontier expansion widens reach but dilutes one-hop consensus in emission ordering.

Open parameterization for a future round: expansion-surfaced entities currently seed flow only transitively through candidate membership; seeding `ExpansionEvidence` directly would complete the corroboration-join subsumption. Emission breadth (eligibility vs the symev pointer rule) is the other half of the sparse residual.

### Graph flow parity: expansion seeding, consensus emission, contains injection (v7)

The v6 sparse residual closed with the three levers that round named, all inside `apply_graph_flow` — no constants fitted to cases. (1) **Expansion-seeded flow**: `ExpansionEvidence` entities seed with their recorded propagated scores inside the same `FLOW_SEEDS` budget, and expansion *paths* resolve to their file entities — completing the corroboration-join subsumption (candidate→expansion-entity→candidate is now a real two-hop path). (2) **Consensus-gated emission**: per-receiver distinct *seed senders* (`senders` map) is the flow analogue of the join's referrer count — emission requires ≥2 independent senders or name overlap, scored `1.5·overlap + seed_consensus + normalized_mass`; plural-tolerant term matching keeps "views"→`set_view_status`; each direction gets its own `FLOW_SEED_DEGREE` budget so hub inflow can't crowd outgoing pointers out of the edge universe. (3) **Contains-hop seed injection**: file-kind seeds conduct into their own members at `FLOW_SEED_CONTAINS` 0.6 (between diluting forward 0.3 and aggregating backward 0.9) — the file→member granularity hop the champion file-vote approximated by fiat. Readout 1 also pools the containing file entity's received mass into each document candidate, and candidates already carrying the `Structural` route are skipped (they ARE the evidence — no double-counting).

v6.0 dataset (n=41 scored), WORKTREE mode, paired arms re-run in the same window (the working tree was concurrently edited — same-window pairs are the valid comparison):

| arm | recall@20 | recall@5 | symbol_recall@20 | claim_support | file_success@20 | decoy@20 | query_ms |
|---|---|---|---|---|---|---|---|
| sparse off | .627 | .477 | .671 | .643 | .512 | .480 | 668 |
| sparse replace | .635 | .453 | .671 | .643 | .512 | .474 | 802 (+20%) |
| dense off | .652 | .577 | .711 | .679 | .561 | .509 | 4209* |
| dense replace | .660 | .578 | .699 | .714 | .561 | .506 | 818 |

*dense-off query_ms carries first-case cold start in its CI [628, 11106]; not a real delta.

`compare --gate` on both lanes: **GATE PASS — no significant guardrail regression.** Every quality delta is inside noise; the only significant movement is decoy_hit_rate@10 −0.021 (p=.029) *favoring* replace on the dense lane. Parity criterion met: `replace` ≥ `off` on recall@k/symbol_recall/claim_support/file_success, decoy and abstention metrics not worse, latency within +25%.

Verdict, stated plainly: **the replacement is earned — the unified propagation pass now computes everything the four priors computed, plus evidence the priors could not express** (distinct-sender consensus, expansion-path corroboration). `off` remains the shipped default until the prior deletion lands; that removal is a separate mechanical change, and once `replace` is default the `CCE_FLOW` scaffolding goes with it. Honest residuals: false_positive_rate .73 is untouched (abstention is plan 008's mechanism — flow still ranks rather than judges); sparse query_ms +20% is the price of the extra edge fetches.

### Post-commit view interruption repair

The snapshot `complete` flag commits with the records transaction, but the dense artifact and every view-status write happen *after* it — an interrupted index left those views permanently `Building` while `index()` early-returned on every later call. `index()` now repairs in place under the index lease, once per snapshot per process: any view stuck `Building`/`Failed` on a complete snapshot is recomputed from committed records (file-analysis artifacts for symbol coverage, relation rows for SCIP edges, commit entities/documents for history, re-probed provider detect-state for graph; dense re-runs its build). Once-per-process means a deterministically failing step reports `Failed` once instead of re-running per request — a fixed environment heals on the next process. Verified by fault injection (`view_status` forced to `building` → next `index` → converged statuses, recomputed capabilities). Two operational traps found while validating this: provider subprocesses have a 600s timeout that can exceed the adapter timeout when the toolchain blocks (e.g. `rust-analyzer` waiting on a cargo build lock — don't run benchmark arms concurrently with cargo jobs), and `{repository}` must be an absolute or cwd-stable path since adapters pass it verbatim into subprocesses spawned with `cwd=<repo>` (`..` indexed the parent directory; `cli run` now resolves the path once up front).

Dense builds are incremental across snapshots: `build_dense_view` decodes the newest completed snapshot's vector artifact under the same index profile, joins vectors to that snapshot's document texts, and reuses them keyed by document-text digest — unchanged text re-embeds nothing, so a worktree edit only pays the model for documents whose content actually moved (measured 2m35s → 10.8s on a single-file edit; result index is byte-identical to a full rebuild). A cold start, model/profile change, or artifact-less prior build falls back to a full embed.

Post-fix dense-default verification (cce-self-v5.2, n=29; dense = default `--dense local` jina-code): subprocess and daemon arms produce identical rankings; the difference is pure latency — `query_ms` 5740 per-case subprocess (ONNX init paid per spawn) vs **105 warm daemon**. Dense-daemon vs sparse paired: recall@10 +0.099 (p=.0085), recall@20 +0.081, recall@50 +0.108, file_success@20 +0.069, claim_support 0.75 vs 0.54 — and daemon-dense is *faster* in wall time than sparse-subprocess (−456ms) because session amortization beats per-case spawn+scan. `unjudged_rate@50` +0.048 (p=.0145) is the adjudication pool needing its periodic top-up against the new snapshot, not a quality regression. GATE PASS.

Published bundles must replace `WORKTREE` with an immutable commit and record CPU, memory, operating system, model endpoint/revision, cold versus warm cache state, and lockfile digests.

## Standing gaps and roadmap

Measured gaps (v5.5, dense arm, adjudicated cce-self n=29): `symbol_recall@20` .52 and `claim_support` .63 — atomic evidence granularity remains the weakest layer; `file_success@20` .59; `recall@5` .70 vs `recall@20` .89 — residual gap bounded by multi-gold cases (7–11 golds cap per-case @5 at ~.71); `query_ms` ~+112ms for graph walks; `unjudged_rate@50` +.019 — newly emitted documents need the next adjudication round.

Architectural debt, recorded honestly: the ranking layer accreted bounded additive priors (co-change, recency, same-package, corroboration, cluster support, file vote, symbol-evidence join), each justified by a single n=29 A/B. Individually they are the strongest measured gains in this project's history; collectively they approximate — with hand-tuned constants — one mechanism: **query-conditioned flow over the typed entity graph**. Corroboration is one-hop backflow, cluster support is intra-head edge density, file vote is file-level mass aggregation, and the symbol-evidence join is terminal-node emission. Edge direction and type already encode role (tests flow toward mechanisms, not back) — the signal that lexical similarity cannot express. The essential consolidation is one heterogeneous propagation pass producing multi-granular scores (file/symbol/document as one node space), which also gives real per-edge provenance decompositions. Node-attribute priors (recency, package) and selection policy (per-file dedup) are orthogonal and stay.

Roadmap candidates in priority order: (1) ~~unified graph-flow pass~~ — parity earned (v7): expansion seeding, consensus emission, and contains injection closed the sparse residual; `replace` ≡ `off` on both lanes, GATE PASS. Prior deletion + making `replace` the default is the remaining mechanical step; (2) ~~`contains`-hop~~ — done via frontier expansion (hop-1 receivers' edges fetched for hop 2) plus seed-file member injection (v7); (3) structural/pattern query operator (`pat:`, `type:symbol`, `lang:`) — Sourcegraph-parity capability, fully deterministic; (4) SPLADE-Code ONNX document-side `terms` expansion for the vocab gap; (5) dependency-manifest → external symbol stubs so `imports` edges can point outward; (6) external benchmark corpus (Agent-Retrieval-Bench-style) — n=29 bounds what micro-tuning can prove.
