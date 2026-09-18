# Plan 007: Earn the graph-flow mechanism — `replace` arm reaches parity with `union` on the adversarial set

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 3f5b930..HEAD -- crates/cce-engine/src/retrieval.rs benchmarks/adapters/ benchmarks/datasets/cce-self.jsonl`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: MED
- **Depends on**: none (001–006 all DONE; this is roadmap phase 1 of 3)
- **Category**: direction
- **Planned at**: commit `3f5b930`, 2026-09-18

## Why this matters

The retrieval ranking layer accreted bounded additive priors —
corroboration join, intra-candidate cluster support, champion file-vote,
and the symbol-evidence join — each justified by a single n=29 A/B. The
v6 experiment (`apply_graph_flow`, `CCE_FLOW`-gated) proved they all
approximate one mechanism: **query-seeded propagation over the typed
entity graph**. The six-arm A/B showed `union ≡ off` on quality (the flow
pass computes the same evidence the priors compute) but `replace` lost
~.02–.05 of sparse recall/symbol tail coverage — real enough that
`off` remains the shipped default.

This plan closes that residual so `replace` earns parity and the four
priors can be deleted. This is not a tuning exercise: the consolidated
pass is the substrate Phase 2 (evidence paths → principled abstention)
and Phase 3 (world-model surface: dataflow, structural operators,
federation) both build on. Until `replace` is earned, every downstream
plan inherits a forked mechanism.

## Current state

- `crates/cce-engine/src/retrieval.rs` — the ranking pipeline and the
  experimental flow pass (~2900 lines).
- `crates/cce-store/src/metadata.rs` — graph reads
  (`relations_for_entity`, `entities_by_ids`, `entity_relation_degree`).
- `benchmarks/adapters/cce-search-*-flow-{off,replace}.yaml` — the A/B
  arm definitions (`environment: CCE_FLOW`).
- `benchmarks/datasets/cce-self.jsonl` — v6.0 adversarial dataset, 51
  cases (15 `no_context`, decoy-judged near-misses).
- `docs/benchmarking.md` — full protocol and the v6 verdict; append new
  rounds there following the existing round format.
- `research/cce_research/` — the evaluation harness
  (`run`/`evaluate`/`compare --gate`).

The mechanism and its gate (retrieval.rs, current):

```rust
// ~line 629
let flow_mode = std::env::var("CCE_FLOW").unwrap_or_default();
if !matches!(flow_mode.as_str(), "replace") {
    apply_corroboration(...);   // four legacy priors run when not replacing
    ...
    symbol_evidence_join(...)?;
}
if matches!(flow_mode.as_str(), "union" | "replace") {
    apply_graph_flow(self.store(), &request.snapshot_id, &request.query,
                     &mut candidates, verified_fresh, request.limit)?;
}
```

The pass constants (retrieval.rs ~1637):

```rust
const FLOW_SEEDS: usize = 32;
const FLOW_SEED_DEGREE: usize = 64;
const FLOW_HOPS: usize = 2;
const FLOW_HOP_DECAY: f64 = 0.5;
const FLOW_BONUS: f64 = 0.25;
const FLOW_EMIT_MAX: usize = 12;
const FLOW_FRONTIER: usize = 16;
```

Two readouts already exist: candidate received-mass bonus
(`FLOW_BONUS`-capped) and symbol `entity:` emission at pool-boundary mass
with a callable-kind named lane (~lines 1883–1990).

The three named residuals from the v6 verdict (all inside
`apply_graph_flow` / its call site):

1. **Expansion-surfaced entities never seed flow.** Expansion emits
   `entity:`-keyed hits and records `ExpansionEvidence` (struct at
   retrieval.rs ~line 72), but seeds come only from doc-keyed candidate
   entity ids — the corroboration-join subsumption is incomplete.
2. **Emission breadth.** Sparse `replace` lost tail coverage; the
   emission eligibility set (symbol-kind filter + `FLOW_EMIT_MAX` +
   two-lane interleave) is narrower than the legacy symbol-evidence
   join's pointer rule.
3. **Contains-hop seeding asymmetry.** `flow_edge_weights(Contains) =
   (0.3, 0.9)`: file→member dilutes, member→file aggregates. File-level
   candidates therefore cannot seed their member symbols — exactly the
   granularity gap the legacy file-vote approximated.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Build | `cargo +stable-2026-07-16 build --release -p cce` | exit 0 |
| Lint | `cargo +stable-2026-07-16 clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| Format | `cargo +stable-2026-07-16 fmt --all -- --check` | exit 0 |
| Unit tests | `cargo +stable-2026-07-16 test --workspace --all-features` | all pass |
| Dataset check | `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl` | exit 0 |
| Benchmark arm | `uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/<arm>.yaml . WORKTREE research/output/<name>.jsonl` | exit 0, result file written |
| Compare | `uv run --project research cce-research compare research/output/<a>.jsonl research/output/<b>.jsonl --gate` | `GATE PASS` |

Use `target/release/cce` for benchmark arms (adapter YAMLs reference it).
Do NOT run benchmark arms concurrently with cargo jobs — provider
subprocesses (rust-analyzer) have a 600s timeout and will block on a
cargo build lock; that is a documented trap in `docs/benchmarking.md`.

## Scope

**In scope** (the only files you should modify):
- `crates/cce-engine/src/retrieval.rs`
- `benchmarks/adapters/` (new arm YAMLs only, if a third arm is needed)
- `benchmarks/datasets/cce-self.jsonl` (only if adjudication pool needs a
  routine top-up; follow the documented adjudication workflow)
- `docs/benchmarking.md` (append the result round)
- `research/output/*.jsonl` (generated run outputs, gitignored or not —
  follow existing convention)

**Out of scope** (do NOT touch, even though they look related):
- `apply_corroboration`, `apply_structural_features`, `symbol_evidence_join`,
  `prf_expansion_pass` — deleting the legacy priors is the *reward* for
  earning parity, not a step in this plan. Leave them in place until the
  gate passes; removal is a separate trivial follow-up.
- `crates/cce-engine/src/engine.rs` view materialization.
- `CCE_FLOW` default value — `off` stays shipped until parity is proven.
- Any HTTP/daemon surface (`apps/daemon`, `apps/gateway`).
- `graph_policy` / planner routing.

## Git workflow

- Branch: `advisor/007-graph-flow-parity` (or work directly on the user's
  branch if instructed — match how prior `retrieval:` commits landed).
- Commit style (from `git log`): lowercase `area: summary`, e.g.
  `retrieval: seed flow with expansion-surfaced entities`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Reproduce the residual

Run the two sparse arms and the dense baseline on the v6.0 dataset and
confirm the v6 findings still hold (union≈off on quality; replace trails
on `symbol_recall@20`/`recall@20` tail):

```bash
cargo +stable-2026-07-16 build --release -p cce
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl \
  benchmarks/adapters/cce-search-sparse-flow-off.yaml . WORKTREE \
  research/output/v7-sparse-off.jsonl
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl \
  benchmarks/adapters/cce-search-sparse-flow-replace.yaml . WORKTREE \
  research/output/v7-sparse-replace.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl \
  research/output/v7-sparse-off.jsonl
uv run --project research cce-research compare \
  research/output/v7-sparse-off.jsonl research/output/v7-sparse-replace.jsonl
```

**Verify**: the compare table shows `replace` trailing on
`symbol_recall@20` and/or `recall@20` (expected negative deltas, roughly
−.02 to −.05). If `replace` already matches `off` on everything, skip to
Step 5 — the residual is already closed and the plan only needs the gate.

### Step 2: Seed flow with expansion-surfaced entities

In `apply_graph_flow`, the seed list is built only from doc-keyed
candidate entity ids. Extend it: `ExpansionEvidence.entities` /
`regions` / `paths` (populated during structural expansion, passed in or
recomputed) contribute seeds at their recorded propagated score, capped
by the same `FLOW_SEEDS` budget (expansion seeds append after candidate
seeds; total stays ≤ 32). This completes the corroboration-join
subsumption the v6 notes name as the next lever.

The function signature currently is:

```rust
fn apply_graph_flow(
    store: &MetadataStore,
    snapshot_id: &str,
    query: &str,
    candidates: &mut HashMap<String, Candidate>,
    verified_fresh: bool,
    limit: usize,
) -> Result<()>
```

Add a parameter for the expansion evidence (or a `&[(String, f64)]`
extra-seed slice) and update the single call site.

**Verify**: `cargo +stable-2026-07-16 test -p cce-engine retrieval` → all
pass. Then re-run the sparse-replace arm and confirm
`symbol_recall@20`/`recall@20` deltas vs `off` shrink or invert.

### Step 3: Widen emission breadth to match the pointer rule

The legacy `symbol_evidence_join` emits a symbol when it has ≥2
referrers OR query-name overlap alone. The flow emission filter is a
symbol-kind set plus two-lane interleave of `FLOW_EMIT_MAX=12`. Compare
the emitted sets on a failing sparse case (the benchmark's per-case
`hits` list is in the result JSONL — diff `entity:` hits between arms).

Tune toward coverage parity using only mechanism-level levers — do NOT
hand-fit constants to cases: candidate levers are `FLOW_EMIT_MAX`
(12→16/24), the kind set (add `Variable`/`Field` if claim facts name
them), and the named-lane predicate (overlap≥1 vs ≥2 referrers
equivalent: a node receiving mass from ≥2 distinct seed senders is the
flow analogue of consensus — this is already computable from `received`
edge counts if tracked per-sender; if not tracked, add a `HashMap<String,
usize>` sender-count alongside received mass).

**Verify**: sparse `replace` `symbol_recall@20` delta vs `off` ≥ −0.01
in the re-run compare.

### Step 4: Contains-hop seeding

File-level candidates (the dominant doc granularity) should seed their
member symbols: when a seed entity is `EntityKind::File` (or the file
kind the schema uses — check `EntityKind` variants in
`crates/cce-core/src/entity.rs`), its outgoing `Contains` edges should
feed mass to members at a *moderated* rate, not the (0.3) conductance
used for generic contains forward flow. Concretely: either add a
distinct seed-injection edge set (seed file → members, fixed share) or
raise `Contains` forward conductance for seed nodes only. The goal:
member symbols of a strong file candidate become flow-reachable in
hop 1, which is what the legacy file-vote prior approximated.

**Verify**: dense and sparse `replace` arms: `claim_support` delta vs
`off` ≥ 0 (ns tolerated), no new `decoy_hit_rate@20` regression.

### Step 5: Gate on the full adversarial set

Run both sparse arms AND the dense arms (add `union` if a `flow-union`
yaml doesn't exist — copy `cce-search-daemon-dense-jina-code-flow-replace.yaml`
with `CCE_FLOW: "union"`):

```bash
uv run --project research cce-research compare \
  research/output/v7-sparse-off.jsonl research/output/v7-sparse-replace.jsonl --gate
uv run --project research cce-research compare \
  research/output/v7-dense-off.jsonl research/output/v7-dense-replace.jsonl --gate
```

Parity criterion (the decision, not a hope): `replace` ≥ `off`/`union`
on `recall@{5,10,20}`, `symbol_recall@20`, `claim_support`,
`file_success@20` with no significant negative delta; `decoy_hit_rate@20`
and `false_positive_rate` not worse. `query_ms` regression ≤ +25%.

**Verify**: `GATE PASS` on both compares.

### Step 6: Record the round

Append a v7 section to `docs/benchmarking.md` following the existing
round format (arms table, deltas, verdict stated plainly, residuals
named honestly). If parity is earned, note the follow-up: deleting the
four priors is a separate mechanical plan (out of scope here).

**Verify**: `git diff docs/benchmarking.md` shows one new `###` section.

## Test plan

- Unit: extend the existing flow tests near
  `symbol_evidence_surfaces_pointed_symbols` (retrieval.rs ~line 2751):
  - expansion-seeded entity participates in hop-1 flow (seed from
    `ExpansionEvidence`, not candidates)
  - file-entity seed reaches a member symbol through `Contains`
  - emission lane preserves the callable-kind partition under a
    widened `FLOW_EMIT_MAX`
- The adversarial dataset IS the characterization suite — per-case
  before/after hit diffs (Step 3) are the regression evidence.
- Verification: `cargo +stable-2026-07-16 test -p cce-engine` → all pass.

## Done criteria

- [ ] `cargo +stable-2026-07-16 clippy --workspace --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo +stable-2026-07-16 test --workspace --all-features` exits 0
- [ ] Sparse `replace` vs `off` compare: `GATE PASS`, no significant negative delta on recall@k/symbol_recall/claim_support
- [ ] Dense `replace` vs `off` compare: `GATE PASS`
- [ ] `decoy_hit_rate@20` and `false_positive_rate` not worse than `off` in either arm
- [ ] `docs/benchmarking.md` has a new results round
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- The code at the locations in "Current state" doesn't match the excerpts
  (the codebase has drifted since this plan was written).
- A step's verification fails twice after a reasonable fix attempt.
- `replace` parity appears to require a *non-mechanism* change (a new
  hand-tuned prior rather than flow parameters) — that defeats the point;
  stop and report.
- The sparse residual is traced to something outside `apply_graph_flow`
  (e.g. expansion itself differs between modes — it doesn't, but verify).
- Dataset cases fail validation or the adjudication pool needs >30 new
  judgments — that's a separate adjudication round, not this plan.

## Maintenance notes

- After parity: the four legacy priors (`apply_corroboration`,
  `apply_structural_features`'s graph-side bonuses,
  `symbol_evidence_join`, and the file-vote/cluster blocks) become
  deletable. That deletion is a separate plan — do it while the v7
  numbers are fresh so a regression bisects cleanly.
- `ExpansionEvidence` may become redundant once its entities seed flow
  directly; check whether the bonus readout still needs it.
- `CCE_FLOW` env gating is experimental scaffolding; once `replace` is
  default, remove the env var and the `off`/`union` branches (with the
  prior deletion).
- A reviewer should scrutinize: any constant changed without a measured
  before/after; new per-sender bookkeeping's memory on the ≤32-seed
  universe (bounded, fine); and that no case-specific special-casing
  slipped in — the mechanism must stay case-blind.
