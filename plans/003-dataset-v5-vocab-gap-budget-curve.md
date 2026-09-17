# Plan 003: Extend cce-self dataset to v5 — vocabulary-gap, budget-curve, and dataflow-refusal cases

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git rev-parse --short HEAD` into "Planned at"
> (advisor shell was unavailable; field left unfilled). Then
> `git diff --stat e25751c..HEAD -- benchmarks/datasets/cce-self.jsonl`
> plus confirm the cited source ranges below still match the code
> (`git diff e25751c..HEAD -- crates/cce-engine/src/`).

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (pairs naturally with 001 but does not require it)
- **Category**: tests (benchmark dataset)
- **Planned at**: commit `e25751c`, 2026-09-17

## Why this matters

`benchmarks/datasets/cce-self.jsonl` (revision `cce-self-v4`, 15 cases) does
not exercise the system's claimed bottleneck or two documented metric
families:

1. **No vocabulary-gap cases.** The known hard problem is NL query phrasing
   vs identifier naming (the semantic gap dense retrieval exists to close).
   Every current case either names its symbols or asks about concepts whose
   terms appear verbatim in the gold files. Without vocab-gap cases, a
   stronger embedding model has nothing to win on — and `compare`'s own MDE
   calculation will show the suite is underpowered at n=15.
2. **No budget curve.** `docs/benchmarking.md` requires "coverage at
   2K/4K/8K" but every case pins a single `budget_tokens`. `budget_tokens`
   is already a per-case field — the curve needs only derived rows.
3. **`precise_dataflow` intent is uncovered**, including the contract that it
   must surface a missing-capability message rather than silently degrade.

After this plan the dataset has 29 cases at revision `cce-self-v5`, covering
all three gaps, and `lint-gold` proves every gold target is reachable under
the union route set.

## Current state

- `benchmarks/datasets/cce-self.jsonl` — one JSON object per line, schema in
  `research/cce_research/schema.py:72-115` (`BenchmarkCase`,
  `extra="forbid"`). Every case carries
  `provenance.dataset_revision`; a result bundle requires exactly one
  revision (`cli.py:146-148`), so **all** rows bump to `cce-self-v5`
  together.
- Field reference (from existing rows): `case_id`, `repository`
  (`wibus-wee/CCE`), `revision` (`WORKTREE`), `query`, `intent`,
  `gold_files`, `gold_symbols`, `gold_ranges`, `supporting_ranges`,
  `gold_facts` (`{claim, evidence: [{path,start_line,end_line}], symbols}`),
  `judged_files`, `no_context`, `budget_tokens`, `supply_intent`, `routes`,
  `derived_from`, `derivation`, `answer_key`, `provenance`, `tags`.
- `derived_from`/`derivation` mark generated rows (e.g. `"budget:2048"`) —
  same mechanism `variants`/`ablate` already use.
- Gold evidence below was verified against the source at planning time;
  **re-verify each range before committing it** (drift check above).

Candidate new cases (queries chosen so gold terms do NOT appear verbatim in
the query — that is the point of the vocab gap):

| case_id | query | intent | gold file(s) | gold symbols / evidence |
|---|---|---|---|---|
| `self-fusion-rrf` | `How are hits from different retrieval channels merged into a single ranking?` | `natural_language_behavior` | `crates/cce-engine/src/retrieval.rs` | `add_candidate`, `ranked_candidates`; evidence lines ~14 (`RRF_K`), ~526-556 |
| `self-fusion-rrf-cjk` | `多条检索通道的命中是怎么合并成一个排序的` (supply_intent:false) | `natural_language_behavior` | same | same |
| `self-dense-profile-guard` | `Why does a query fail when the embedding model differs from the one used at index time?` | `natural_language_behavior` | `crates/cce-engine/src/dense.rs` | `DenseIndex`; evidence ~295-301 (profile mismatch error) |
| `self-e5-prefix` | `where are the query and passage prefixes added for E5-family embeddings` | `natural_language_behavior` | `crates/cce-engine/src/dense.rs` | `LocalEmbedder`, `embed`; evidence ~166-170, ~202-212 |
| `self-pack-file-cap-cjk` | `上下文打包时怎么防止同一个文件把预算吃光` (supply_intent:false) | `natural_language_behavior` | `crates/cce-engine/src/context.rs` | `ContextPacker`, `pack`; evidence ~247-253 (per-file range cap) |
| `self-call-confidence` | `调用边的置信度为什么低于包依赖边` (supply_intent:false) | `natural_language_behavior` | `crates/cce-engine/src/relations.rs` | `call_relations`, `resolve_callee`; evidence ~295-341 (confidence 0.6, spelling resolution) |
| `self-dataflow-refusal` | `Show the source-to-sink path for user input reaching the database` | `precise_dataflow` | `crates/cce-engine/src/retrieval.rs`, `crates/cce-engine/src/planner.rs` | `QueryPlanner`; evidence retrieval.rs ~394-404 (missing-capability push), planner.rs ~124-132 (DataflowRequired plan) |

Budget-curve derived rows (`derived_from` = parent, `derivation` =
`"budget:<n>"`, `tags` + `"budget-curve"`; identical gold):

- `self-sqlite-artifact-boundary::b2048` and `::b4096` (parent budget 8192)
- `self-impact-source-address::b2048` and `::b4096`
- `self-trace-context-pack::b2048` and `::b4096`
- `self-snapshot-freshness::b2048` (parent budget 4096)

Authoring conventions to match existing rows:

- `provenance`: `source_url` `https://github.com/wibus-wee/CCE`,
  `dataset_revision` `cce-self-v5`, `license_spdx` `Apache-2.0`,
  `redistribution` `allowed`, `construction_method` describing the case
  (e.g. `"maintainer-authored vocabulary-gap question; gold verified against
  source; query wording excludes gold identifiers"`).
- `tags`: include `"self"` plus topical tags; CJK rows add `"cjk"`;
  `supply_intent:false` rows add `"inferred-intent"`.
- `gold_facts` entries: `claim` = one atomic sentence; `evidence` = line
  ranges verified above; `symbols` = symbol names that must appear on a hit.
- `budget_tokens`: 4096 for symbol-level cases, 8192 for multi-file cases;
  derived rows keep parent gold and set only `budget_tokens`, `case_id`,
  `derived_from`, `derivation`, `tags`.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Validate | `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl` | "Validated 29 cases across 1 dataset revisions" |
| Gold feasibility | `cargo build --release --locked -p cce-cli` then `uv run --project research cce-research lint-gold benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search.yaml .` | "all gold evidence reachable under union routes" |
| Full run (optional) | `uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce.yaml . WORKTREE research/output/cce-self-v5.jsonl` | 29 `✓` lines |

## Scope

**In scope**:

- `benchmarks/datasets/cce-self.jsonl` (edit in place: bump all
  `dataset_revision` values, append rows)

**Out of scope**:

- `research/cce_research/*.py` — the schema already supports everything
  used here; no code changes.
- Do not add cases whose gold requires `map`/`explain`/`impact` — those
  endpoints return non-range shapes the adapter cannot normalize yet
  (that is a separate plan).
- Do not fill `judged_files` by guessing; adjudication verdicts come from a
  pooled run + human review (`cce-research adjudicate`), which is a
  follow-up activity, not this plan.
- Do not split the dataset into a new filename — the docs' commands and
  single-revision rule assume this file.

## Git workflow

- Branch: `advisor/003-dataset-v5`
- One commit; e.g. `benchmark: cce-self-v5 adds vocab-gap, budget-curve, dataflow cases`
- Do NOT push or open a PR.

## Steps

### Step 1: Bump the dataset revision

In `cce-self.jsonl`, set every row's `provenance.dataset_revision` to
`cce-self-v5`. (The bundle-level single-revision rule means partial bumps
break `run`.)

**Verify**: `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl` → "Validated 15 cases across 1 dataset revisions."

### Step 2: Append the seven content cases

Write each case from the table above as one JSON line. Before committing
each `evidence` range and `gold_symbols` entry, open the cited file and
confirm the lines/symbols still exist and say what the claim asserts —
adjust line numbers to the live code. Rules:

- `gold_ranges` stays `[]` (file/symbol-level gold); precise ranges live in
  `gold_facts.evidence`.
- For `supply_intent:false` rows, also sanity-check the query against the
  classifier's keyword surface (`crates/cce-engine/src/planner.rs`,
  `classify`, ~lines 167-231): the intended `intent` is what the classifier
  *should* produce; a mismatch is a legitimate measurement, not a data bug —
  but do not ship a case whose gold intent can never be predicted (e.g.
  don't label a bare symbol name `impact`).
- `self-dataflow-refusal`: keep `no_context: false` — the system still
  returns structural evidence; the scored behavior is that
  `missingCapabilities` reports the unavailable dataflow view. Add
  `"tags": ["self", "dataflow", "capability-contract"]`.

**Verify**: `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl` → "Validated 22 cases".

### Step 3: Append the seven budget-curve rows

Duplicate each parent row, changing only `case_id` (append `::b2048` /
`::b4096`), `budget_tokens`, `derived_from`, `derivation`, `tags`
(+`"budget-curve"`), and `construction_method` (note it is a budget
derivation of the parent).

**Verify**: validate-dataset → "Validated 29 cases across 1 dataset revisions."

### Step 4: Feasibility gate

```bash
cargo build --release --locked -p cce-cli
uv run --project research cce-research lint-gold benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search.yaml .
```

Expected: `all gold evidence reachable under union routes`. If a new case's
gold is unreachable, fix the annotation (wrong path/symbol/range) — never
"fix" it by deleting the case without noting why in the commit message.

### Step 5 (optional, needs human verdicts): adjudication pass

After any full `run`, emit
`uv run --project research cce-research adjudicate benchmarks/datasets/cce-self.jsonl research/output/*.jsonl research/output/adjudication.jsonl`,
have a maintainer fill `verdict` per row, then
`apply-judgments` to fold them into a `cce-self-v5.1`. This step requires
human relevance decisions — if no adjudicator is available, leave it out of
the commit and note it in the PR description.

## Test plan

- `validate-dataset` (schema conformance) and `lint-gold` (feasibility) are
  the dataset's test suite; both must pass.
- `uv run --project research pytest research/tests` must still pass (no
  code changed, but cheap to confirm).

## Done criteria

- [ ] `validate-dataset` reports 29 cases, exactly one revision (`cce-self-v5`)
- [ ] `lint-gold` reports all gold reachable
- [ ] At least 4 cases have `supply_intent: false` among the new rows, and
      `precise_dataflow` appears as a gold intent for the first time
- [ ] Budget coverage: `self-sqlite-artifact-boundary`,
      `self-impact-source-address`, `self-trace-context-pack` each exist at
      2048/4096/8192
- [ ] `git diff` touches only `benchmarks/datasets/cce-self.jsonl`
- [ ] `plans/README.md` status row updated

## STOP conditions

- A cited evidence range no longer matches the code AND the intent of the
  case can't be re-anchored within the same file — report rather than
  re-pointing gold at different semantics.
- `lint-gold` reveals gold that is structurally unreachable (e.g. a symbol
  the parser never emits) — fix annotation if it's a typo, report if the
  case concept itself can't be surfaced.
- Schema validation rejects a field you believe is documented — the schema
  is `extra="forbid"`; report the mismatch instead of renaming fields.

## Maintenance notes

- `unjudged_rate@k` will rise on the new cases until adjudication runs;
  that's the metric doing its job (measuring gold incompleteness), not a
  regression.
- The vocab-gap rows are the measurement surface for the e5-base comparison
  (plan 004): if `dense` routes contribute nothing, these cases isolate it.
- When the atlas eval surface lands (separate plan), add `map`/`explain`/
  `impact` cases as a `cce-self-v6` rather than editing these rows — keep
  bundle comparability per revision.
