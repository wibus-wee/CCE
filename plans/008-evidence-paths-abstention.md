# Plan 008: Evidence paths — principled abstention and fact-granularity emission from graph flow

> **STALE — reconciled 2026-09-19.** This is a historical design, not a
> ready-to-execute plan. Evidence paths and SearchVerdict already exist in
> the current worktree; its baseline numbers and commands are outdated.
> Continue context delivery work through [017](017-preserve-verdict-through-context.md).
> Revalidate the abstention policy separately; do not enable a disabled gate
> merely to follow this document. See the [current index](README.md).

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 3f5b930..HEAD -- crates/cce-engine/src/retrieval.rs crates/cce-engine/src/engine.rs crates/cce-core/src/retrieval.rs benchmarks/datasets/cce-self.jsonl`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: MED
- **Depends on**: plans/007-graph-flow-parity.md (the consolidated flow
  pass must be the default mechanism — building path semantics on a
  deprecated fork wastes the work)
- **Category**: direction
- **Planned at**: commit `3f5b930`, 2026-09-18

## Why this matters

The v6.0 adversarial baseline measured the system's biggest honesty gap:
**`false_positive_rate` = 0.60** — on queries for things that don't exist
(websocket handler, LSP, distributed lock, cross-repo search), the engine
returns a ranked list 60% of the time instead of saying nothing. Along
with `decoy_hit_rate@20` = 0.282 and `symbol_recall@20` ≈ .52, this is the
trust blocker for agent-facing use: an agent that can't distinguish
"no evidence" from "weak ranking" will consume fabricated context.

The root cause is structural, not tunable: **RRF fused scores carry no
evidence semantics**. A score of 0.028 is excellent on one query and
meaningless on another — no threshold can separate "found" from "didn't
find" across queries (verified: scores are cross-query incomparable by
construction). Abstention therefore cannot be a score floor. It must be a
*graph property*: no corroborated path from query evidence to a candidate.

This plan makes the propagation pass (plan 007) emit **typed evidence
paths** — seed → edges → evidence node — instead of only mass, and turns
"no corroborated path exists" into the abstain signal, while the same
path machinery upgrades claim-support granularity (symbol-level
emissions carry their justifying edges) and name disambiguation (the
correct `open` is the one a path reaches).

## Current state

- `crates/cce-engine/src/retrieval.rs` — `search()` pipeline,
  `apply_graph_flow` (propagation), `symbol_evidence_join`, `SearchResult`.
- `crates/cce-engine/src/engine.rs` — `dataflow_view_status` (~line 1834)
  returns `Partial` when SCIP edges exist; `Unavailable` otherwise.
- `crates/cce-core/src/retrieval.rs` — `QueryFilters`,
  `parse_query_filters` (line 208) extracts `lang:`/`path:`/`type:`.
- `crates/cce-engine/src/planner.rs` — `QueryIntent::PreciseDataflow`
  detection ("taint", "污点", "source→sink") →
  `GraphPolicy::DataflowRequired`; retrieval.rs ~line 600 already pushes
  `missing_capabilities` when the dataflow view isn't ready.
- `apps/daemon/src/main.rs` — `/v1/search` returns `SearchResult` with
  `hits` + `missingCapabilities` (camelCase on the wire).
- `research/cce_research/adapters.py:457` — `abstained = not retrieved`:
  the harness reads an empty hit list as abstention. No other abstain
  channel exists today.
- `benchmarks/datasets/cce-self.jsonl` — 51 cases; the `no_context`
  (15) and adversarial (22) cases are the measurement surface.
- `docs/benchmarking.md` — protocol; the v6.0 adversarial baseline round
  reports FPR 0.60 / decoy 0.282 — quote it as the baseline in your new
  round.

Baseline numbers to beat (dense arm, cce-search adapter, n=51):

```
false_positive_rate   0.60   (near-miss queries returning hits)
decoy_hit_rate@20     0.282  (adjudicated-irrelevant share of top-20)
symbol_recall@20     ~0.52   (fact-granularity coverage)
```

`SearchResult` shape (retrieval.rs ~line 30):

```rust
pub struct SearchResult {
    pub plan: QueryPlan,
    pub manifest: ViewManifest,
    pub hits: Vec<SearchHit>,
    pub missing_capabilities: Vec<String>,
    ...
}
```

Flow readouts (post-007): candidate bonus + symbol `entity:` emission.
The propagation loop tracks `mass`/`received` per node and `traversed`
per (edge, direction) — the edges that carried mass are already known;
this plan *records* them as paths rather than only summing mass.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Build | `cargo +stable-2026-07-16 build --release -p cce` | exit 0 |
| Lint | `cargo +stable-2026-07-16 clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo +stable-2026-07-16 test --workspace --all-features` | all pass |
| Dataset check | `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl` | exit 0 |
| Benchmark arm | `uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/<arm>.yaml . WORKTREE research/output/<name>.jsonl` | result file written |
| Compare | `uv run --project research cce-research compare research/output/<a>.jsonl research/output/<b>.jsonl --gate` | `GATE PASS` |

Same operational trap as plan 007: never run benchmark arms concurrently
with cargo jobs (provider subprocess 600s timeout on the build lock).

## Scope

**In scope** (the only files you should modify):
- `crates/cce-engine/src/retrieval.rs` — path recording, abstention
  decision, emission payloads
- `crates/cce-core/src/retrieval.rs` — `SearchHit`/`SearchResult` fields
  if a path provenance field is added (keep serde-compatible: additive
  optional fields only)
- `crates/cce-engine/src/engine.rs` — only if the abstention signal
  needs a view-level companion flag
- `apps/daemon/src/main.rs` — only if a wire field changes (additive)
- `apps/web/src/` — ONLY the rendering of a new optional field, if added
  (the design system in `apps/web/src/ui/` governs components; read
  AGENTS.md "Web UI design system" section first)
- `docs/benchmarking.md` — append the result round
- `research/output/*.jsonl`

**Out of scope**:
- `apply_graph_flow`'s propagation math (weights, hops, normalization)
  — plan 007 owns that. This plan records paths through the *existing*
  loop, it does not retune it.
- Deleting legacy priors — separate follow-up after 007.
- MCP tool surface (`apps/gateway/src/mcp.rs`) — new fields pass through
  `structuredContent` automatically; no change needed.
- `no_context` dataset cases — adding cases is a separate adjudication
  round; this plan is measured against the existing 51.
- Score normalization / display formatting.

## Git workflow

- Branch: `advisor/008-evidence-paths` (or the repo's convention).
- Commit style: `retrieval: evidence paths in flow propagation`, per
  `git log` lowercase `area: summary`.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Record mass-carrying paths in the propagation loop

Inside `apply_graph_flow` (retrieval.rs ~1748–1990), the loop already
marks `traversed` edges. Extend bookkeeping so each receiving node keeps
its **strongest inbound evidence path**: `(seed_entity_id,
[(source, target, RelationKind, confidence)...], received_mass)`.
Implementation shape: a `HashMap<String, EvidencePath>` updated when a
node's `received` total improves; a path is the predecessor's path plus
the traversed edge (bounded at `FLOW_HOPS` edges — path length ≤ 2 today,
keep the cap generic). Deterministic: ties break on `(mass desc, path
lexicographic)`.

Introduce `struct EvidencePath { seed: String, edges: Vec<(String,
String, RelationKind, f64)>, received: f64 }` near the other flow types.

**Verify**: `cargo +stable-2026-07-16 test -p cce-engine retrieval` →
pass. Add a unit test: a seed → `Calls` → member path is recorded with
the right edge kind and confidence.

### Step 2: Emit paths on emitted hits

When flow emits `entity:` hits (readout 2), attach the winning path to
`SearchHit.explanation` as a structured line — e.g.
`"flow path: seed_fn → Calls(0.85) → emit_fn"`. Keep it text (the field
is already `Vec<String>`) — no schema change needed yet. For candidate
bonuses (readout 1), append the path to the existing
`"graph flow corroboration"` explanation line.

**Verify**: run `target/release/cce --json search . "a query that hits"`
and confirm explanations carry `flow path:` lines with edge kinds.

### Step 3: The abstention decision

Define the abstain condition at the end of `search()` — after all routes
and passes:

**Abstain when** the fused head contains no corroborated evidence:
concretely, no candidate received flow mass AND no emitted `entity:` hit
exists AND the top-K fused scores show no route agreement (a candidate
present in ≥2 distinct routes counts as agreement — track per-candidate
route count, already available via the candidates map's hit routes).

When abstaining: return `hits: []` plus
`missing_capabilities.push("no corroborated evidence path — query may
target absent functionality")`. Do NOT add a threshold on fused score —
that was the rejected approach.

Guard: only abstain when at least one retrieval route actually ran and
returned zero corroboration — a system with all views unavailable must
report `missing_capabilities` for the views (existing behavior), not
"abstain".

**Verify**: `cargo +stable-2026-07-16 test -p cce-engine` → pass, plus a
new unit test: a query with only single-route low-evidence hits produces
`hits: []` + the missing-capability message; a multi-route-hit query
does not.

### Step 4: Measure on the adversarial set

Run the dense search arm (same adapter as the v6.0 baseline:
`cce-search-daemon-dense-jina-code.yaml` or its successor) and evaluate:

```bash
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl \
  benchmarks/adapters/cce-search-daemon-dense-jina-code.yaml . WORKTREE \
  research/output/v8-dense-paths.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl \
  research/output/v8-dense-paths.jsonl
```

Targets: `false_positive_rate` ≤ 0.25 (from 0.60), `decoy_hit_rate@20`
≤ 0.15 (from 0.282), `recall@20`/`file_success@20` not significantly
worse vs the post-007 baseline. If recall drops significantly, the
abstain condition is over-firing — inspect which cases emptied
(per-case `hits` in the result JSONL) and tighten the "route agreement"
clause before any other lever.

**Verify**: the three metrics land inside the targets or you can name
the specific cases that still fail and why (record them).

### Step 5: Disambiguation via paths (the `open` case)

The name-collision cases (`self-adv-open-*`) show the wrong `open` ranked
first because document-level scoring can't resolve same-name ambiguity.
With paths: when multiple same-name entities compete, the one reached by
a corroborated path (flow mass through *qualified* edges — `Calls`,
`References`, `Contains` from a topically-seeded parent) wins; an
unreached same-name entity gets no path bonus and a path-less candidate
should rank below a path-carrying one at equal fused score — implement
as a final tiebreak in `ranked_candidates` (retrieval.rs ~2061): path
presence/quality as the last sort key.

**Verify**: the `self-adv-open-*` cases in the v8 run show the qualified
`open` (e.g. `ArtifactStore::open` in artifact context) ranking above
the bare `open`. Record per-case rank movement in the benchmark round.

### Step 6: Record the round

Append a v8 section to `docs/benchmarking.md`: baseline (0.60/0.282) →
new numbers, abstention semantics stated plainly (what "abstain" means
operationally: empty hits + missing-capability), residual failures named.

**Verify**: one new `###` section.

## Test plan

- Unit tests in `retrieval.rs` (model after existing flow tests near
  `symbol_evidence_surfaces_pointed_symbols`):
  - path recorded on emission (kind, confidence, seed)
  - abstain: no-corroboration query → empty hits + missing capability
  - no-abstain: multi-route corroborated candidate → hits preserved
  - all-views-unavailable → view missing_capabilities, not abstain
  - same-name disambiguation: qualified entity outranks bare one when a
    path reaches it
- Dataset: the adversarial set is the acceptance suite (Step 4).
- Verification: `cargo +stable-2026-07-16 test -p cce-engine` → all pass.

## Done criteria

- [ ] `cargo +stable-2026-07-16 clippy --workspace --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo +stable-2026-07-16 test --workspace --all-features` exits 0
- [ ] `false_positive_rate` ≤ 0.25 on the v6.0 dataset (from 0.60)
- [ ] `decoy_hit_rate@20` ≤ 0.15 (from 0.282)
- [ ] `compare --gate` vs post-007 baseline: PASS (no significant recall/file_success regression)
- [ ] Emitted `entity:` hits carry `flow path:` explanations
- [ ] `self-adv-open-*` cases: qualified entity outranks bare same-name
- [ ] `docs/benchmarking.md` round appended
- [ ] No files outside the in-scope list modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- Code at cited locations doesn't match excerpts (drift since 3f5b930).
- A verification fails twice after a reasonable fix attempt.
- Abstention seems to require a *score threshold* — that's the rejected
  design; report instead of implementing it.
- Path recording measurably regresses `query_ms` > 50% — the bookkeeping
  should be cheap on a ≤32-seed universe; if it isn't, the data
  structure is wrong, stop and reconsider.
- Plan 007 is not landed (flow still `off`-gated experimental): the
  paths built here must ride the shipped mechanism.

## Maintenance notes

- The abstain condition will need a wire-visible flag eventually —
  `abstained: bool` on `SearchResult` is the honest schema (the harness
  infers it from empty hits today; making it explicit is better). Adding
  it is an additive serde field — acceptable within scope if needed, but
  prefer inferring from `hits.is_empty()` + missing_capabilities first.
- Future dataflow work (plan 009) reuses `EvidencePath` directly — a
  source→sink taint walk is seeded propagation with a constrained edge
  set; keep the struct general (no retrieval-specific naming inside it).
- `ExpansionEvidence` and path recording overlap conceptually; when the
  legacy priors are deleted (post-007 follow-up), unify the bookkeeping.
- A reviewer should scrutinize: the abstain condition's over-fire risk
  on legitimately-weak queries (the metric that catches it is recall@k
  on non-no_context cases — must stay flat); explanation text staying
  honest (a path line must only appear when a path was recorded);
  abstain behavior on CJK/vocab-gap queries (a real-answer query with a
  vocabulary gap looks like a no-context query — check those cases
  specifically).
