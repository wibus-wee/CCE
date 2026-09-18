# Plan 009: World-model surface — structural operators, source→sink dataflow, and federated query fan-out

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 3f5b930..HEAD -- crates/cce-core/src/retrieval.rs crates/cce-engine/src/retrieval.rs crates/cce-engine/src/engine.rs crates/cce-engine/src/snapshot_diff.rs crates/cce-engine/src/export.rs apps/daemon/src/main.rs apps/gateway/src/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: L
- **Risk**: MED
- **Depends on**: plans/008-evidence-paths-abstention.md (dataflow reuses
  `EvidencePath`; abstention semantics shape what "no answer" means at
  the query surface)
- **Category**: direction
- **Planned at**: commit `3f5b930`, 2026-09-18

## Why this matters

Phases 1–2 turn the ranking layer into one principled propagation
mechanism over the typed entity graph. This phase exposes that graph as
a *product surface*: the snapshot world model becomes something users
query directly, not just the engine's internal evidence. Three
deliverables, one substrate:

1. **Structural query operators** (`type:symbol`, `pat:`, entity kinds)
   — deterministic entity-level search, Sourcegraph-parity capability
   the current document-granularity pipeline can't express.
2. **Source→sink dataflow** — the `dataflow` view is `Partial` today:
   SCIP def/ref substrate exists (4429 edges on cce-self) but no taint
   walk. Seeded propagation with a constrained edge set IS taint
   analysis — the same mechanism as ranking flow with different seeds
   and readouts.
3. **Federated fan-out** — the gateway routes per-repo; cross-repo
   aggregation is the last unbuilt lane of the central-service shape.

The snapshot-diff (`engine.architecture_diff`) and export endpoints
(`/v1/export*`) already landed — the graph is becoming a first-class
artifact. This plan is where "code intelligence" stops meaning "search".

## Current state

- `crates/cce-core/src/retrieval.rs:208` — `parse_query_filters` extracts
  `lang:`/`path:`/`type:` into `QueryFilters` (`hit_type` supports
  `diff`/`commit`/`file`; no entity-level types).
- `crates/cce-engine/src/retrieval.rs` — `search()` pipeline, flow pass,
  `EvidencePath` (added by plan 008).
- `crates/cce-engine/src/engine.rs` — `dataflow_view_status` (~line
  1834): `Partial` iff `scip_edges > 0`, message "no source→sink taint
  analysis yet"; `architecture_diff(base, head)` exists.
- `crates/cce-engine/src/snapshot_diff.rs` (484 lines) — entity/relation
  delta between committed snapshots; `/v1/diff/architecture` on daemon.
- `crates/cce-engine/src/export.rs` (214 lines) — paginated
  `entities_page`/`relations_page`/`regions_page`; `/v1/export*` on
  daemon (snapshot + cursor pagination, ≤10k/page).
- `crates/cce-engine/src/planner.rs` — `QueryIntent::PreciseDataflow`
  → `GraphPolicy::DataflowRequired`; retrieval.rs ~line 600 emits
  `missing_capabilities` when dataflow isn't `Ready`.
- `apps/gateway/src/main.rs` — `/{repo}/v1/{*path}` proxies verbatim;
  registry (`require_repo`) resolves id/slug/name.
- `apps/gateway/src/mcp.rs` — `/{repo}/mcp` JSON-RPC → worker `/v1/*`.
- `apps/daemon/src/main.rs` — OpenApiRouter routes; new endpoints follow
  the `#[utoipa::path]` + `routes!` pattern (see `files`/`file_source`
  handlers ~line 471).

Trust/contract constraints to honor (`AGENTS.md`):
- Deterministic facts, framework-derived relations, and model inference
  must remain distinguishable — every relation already carries
  `origin`/`confidence`/`extractor`.
- A stale/unavailable view is reported explicitly; never silently fall
  back.
- TypeScript must not reimplement retrieval/indexing logic — WebUI
  changes are display-only.
- Network access opt-in.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Build | `cargo +stable-2026-07-16 build --release --workspace` | exit 0 |
| Lint | `cargo +stable-2026-07-16 clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| Format | `cargo +stable-2026-07-16 fmt --all -- --check` | exit 0 |
| Tests | `cargo +stable-2026-07-16 test --workspace --all-features` | all pass |
| Web build | `pnpm web:build` | exit 0 |
| Benchmark | `uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/<arm>.yaml . WORKTREE research/output/<name>.jsonl` | result file |
| Compare | `uv run --project research cce-research compare <a> <b> --gate` | `GATE PASS` |

## Scope

**In scope** (the only files you should modify):
- `crates/cce-core/src/retrieval.rs` — `QueryFilters`/`parse_query_filters`
  extension (entity-kind and pattern operators)
- `crates/cce-engine/src/retrieval.rs` — structural-operator route +
  dataflow walk
- `crates/cce-engine/src/engine.rs` — `dataflow_view_status` upgrade
  (Partial → Ready when the taint substrate runs)
- `crates/cce-store/src/metadata.rs` — entity queries the operators need
  (kind-filtered, name-pattern) — additive methods only
- `apps/daemon/src/main.rs` — new route handlers + utoipa annotations
- `apps/gateway/src/main.rs`, `apps/gateway/src/mcp.rs` — aggregation
  endpoint + any new passthrough tools
- `apps/cli/src/main.rs` — CLI flags for new operators (if added)
- `apps/web/src/` — display-only rendering (design system governs;
  read AGENTS.md "Web UI design system" first)
- `docs/benchmarking.md`, `docs/architecture.md`, `deploy/README.md`,
  `docs/service.md` — doc updates where behavior is described
- `research/output/*.jsonl`

**Out of scope**:
- Ranking math / priors (plan 007) and abstention internals (plan 008).
- Multi-hop taint *sanitizer modeling* — v1 of dataflow is reachability
  with provenance, not CodeQL parity. Record it as partial if sanitizers
  matter.
- Auth/tenancy on the gateway — a separate infrastructure plan.
- K8s manifests.
- `apps/mcp` stdio binary — stays engine-embedded for local use.

## Git workflow

- Branch: `advisor/009-world-model-surface` (or repo convention).
- Commit style: `retrieval: type:symbol entity route`, per `git log`.
- Do NOT push or open a PR unless instructed.

## Steps

This plan has three semi-independent tracks (A: operators, B: dataflow,
C: federation). Each lands green independently; order A → B → C is
recommended because A's entity-level result shape is reused by B's
output and C aggregates A's wire types.

### Track A — Structural operators

#### Step A1: Extend the query filter grammar

In `crates/cce-core/src/retrieval.rs`, `parse_query_filters` currently
produces `QueryFilters { path_prefix, language, hit_type }`. Add
`hit_type` values `symbol`, `entity`, and a new operator `pat:<pattern>`
(entity-name substring/glob). Extend `QueryFilters` with
`entity_kind: Option<String>` (accept `type:symbol`, `type:entity`, and
`kind:<EntityKind>` — e.g. `kind:function`) and `name_pattern:
Option<String>` for `pat:`. Keep additive serde-compatible fields;
existing `diff`/`commit`/`file` handling untouched.

**Verify**: `cargo +stable-2026-07-16 test -p cce-core` → pass plus new
parse tests: `pat:View*`, `type:symbol`, `kind:method` extract correctly;
`type:diff` still parses.

#### Step A2: Entity-level search route

Add a `SearchRoute` variant (e.g. `Entity`) — or reuse `Structural` with
a filter-driven branch, whichever is less invasive given `SearchRoute`'s
serde derive and planner tables. Implementation: when
`filters.entity_kind.is_some() || filters.name_pattern.is_some()` OR
`hit_type == "symbol"`, run a store query over `entities` (kind filter +
name LIKE/GLOB + optional path/language join through `region_id` →
regions) and emit `entity:` hits directly — bypassing document FTS.
Score: deterministic order (exact-name > prefix > substring, then
qualified_name depth) — no RRF needed for a typed lookup; this is a
*lookup*, not retrieval.

Store support: `metadata.rs` has `entities_for_snapshot` (full scan) —
add a filtered `entities_query(snapshot_id, kind, name_pattern,
limit)` method with a SQL `WHERE kind=? AND name LIKE ?` (escaping
LIKE wildcards; document semantics).

**Verify**: `cargo +stable-2026-07-16 test -p cce-engine` → pass; new
test: `type:symbol ViewManifest` returns the entity, not documents
mentioning it.

#### Step A3: Wire through surfaces

- CLI: `cce search` already passes query text — operators work
  automatically once parsed; verify `cce search . "kind:function stale"`
  end-to-end.
- Daemon: `/v1/search` passes `filters` — verify via curl.
- MCP: `cce_search` forwards `query` — works automatically.
- OpenAPI: `SearchInput.filters` schema regenerates from the type —
  confirm `/openapi.json` includes the new filter fields.

**Verify**: `curl -s -X POST :7734/v1/search -d '{"query":"type:symbol
IndexReport","limit":5}'` returns `entity:` hits with `IndexReport` at
rank 1.

### Track B — Source→sink dataflow

#### Step B1: The taint walk as constrained propagation

New engine method `taint_paths(source_query, sink_query, max_hops)`
(export it from `engine.rs` next to `architecture_diff`):

1. Resolve seeds: entities matching `source_query` (name/qualified-name
   — reuse the A2 entity query) = sources; `sink_query` = sinks.
2. Run `apply_graph_flow`-style propagation **restricted to
   `Calls`/`References` edges in the forward direction** (callee
   reaches), `max_hops` ≤ 6, degree cap ~64 — a bounded reachability
   walk, not a ranking.
3. Emit `EvidencePath`s (plan 008's type) that terminate at a sink
   entity. Each path carries its edge sequence with confidence — that IS
   the answer ("source reaches sink via calls a→b→c").

Return `TaintReport { paths: Vec<EvidencePath>, sources: usize, sinks:
usize, truncated: bool }`.

**Verify**: unit test on a fixture graph — source fn → Calls → mid →
Calls → sink fn yields one path with 2 edges; disconnected → zero paths.

#### Step B2: View promotion + endpoint

- `dataflow_view_status`: when `taint_paths` exists and SCIP substrate
  >0 edges, promote `Partial` → `Ready` with capability
  `source_sink_reachability` `derived`; keep the honest message that
  sanitizers are unmodeled (reachability, not sanitization-aware taint —
  say so in the capability `reason`).
- Daemon: `GET /v1/taint?source=<q>&sink=<q>` handler +
  `#[utoipa::path]` annotation (query params like `FileQuery` — same
  pattern), returning `TaintReport`.
- `GraphPolicy::DataflowRequired` queries can now route through
  `taint_paths` instead of immediately reporting missing — wire it in
  `search()` where the dataflow-required branch currently pushes
  `missing_capabilities`: attempt the walk; empty paths → keep the
  missing-capability report (honest), paths → include a synthesized hit
  or capability note naming the path count.

**Verify**: `curl ':7734/v1/taint?source=main&sink=write_all'` returns a
path list or an honest empty; `cargo test -p cce-engine` passes.

### Track C — Gateway federation

#### Step C1: Aggregation endpoint

`POST /v1/search/all` on the gateway: body = same `SearchInput` shape as
the worker (reuse the daemon's wire struct — copy the small struct into
gateway or lift it to `cce-core`; prefer lifting: gateway already
depends on `cce-core`). Fan out `POST {worker}/v1/search` to every
registered repo's worker **concurrently** (`futures::future::join_all`
or `FuturesUnordered`), collect `SearchResult`s, and merge:

- Hits carry `repositoryId`/`snapshotId` in their `SourceAddress` — set
  `hit.document_id` namespaced `"{slug}:{document_id}"` if the worker
  doesn't already namespace it (check: worker sets `repository_id` on
  `SourceAddress` — verify and document).
- Merge order: interleave by fused score — scores aren't cross-repo
  comparable, so a round-robin-by-rank interleave per repo is the honest
  merge (document the semantics in the utoipa summary).
- Per-repo failures degrade to that repo being absent + a top-level
  `errors: Vec<{repo, message}>` field — never fail the whole fan-out.
- Timeout: 30s per worker; a hung worker degrades, not blocks.

Register on the OpenApiRouter with a `#[utoipa::path]` annotation.

**Verify**: two-repos setup (compose or two local daemons) →
`curl -X POST :7735/v1/search/all -d '{"query":"snapshot"}'` returns
hits from both with per-repo attribution.

#### Step C2: MCP surface for federation

`/{repo}/mcp` tools are per-repo. Add an optional `repo: "all"` tool
variant or a separate `POST /mcp` root endpoint whose `cce_search` maps
to `/v1/search/all` — pick the simpler one (root `/mcp` with `repo`
argument is cleaner; implement that).

**Verify**: JSON-RPC `tools/call cce_search` on the root endpoint
returns merged structuredContent.

#### Step C3: Docs + benchmark note

Update `docs/service.md` (the federation lane is now real — it was
"contract-shaping only"), `deploy/README.md` (aggregation endpoint),
and append the measurement note to `docs/benchmarking.md` if new cases
were exercised.

**Verify**: docs describe what was verified, not what was intended.

## Test plan

- `cce-core`: filter parsing — `pat:`, `type:symbol`, `kind:`,
  combinations with `lang:`/`path:`; `type:diff` regression.
- `cce-engine`: entity route (kind/name filters), taint walk (path
  found/not-found/hop cap), abstention interplay (a `PreciseDataflow`
  query with no paths still reports missing-capability).
- `cce-gateway`: fan-out merge logic — unit-test the interleave/degrade
  functions with stub `SearchResult`s (extract merge into a pure
  function for testability).
- E2E: steps A3/B2/C1 have curl-level verifications.
- `cargo +stable-2026-07-16 test --workspace` → all pass.

## Done criteria

- [ ] `cargo +stable-2026-07-16 clippy --workspace --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo +stable-2026-07-16 test --workspace --all-features` exits 0
- [ ] `pnpm web:build` exits 0 (if web files touched)
- [ ] `type:symbol`/`kind:`/`pat:` queries return entity-level results via CLI + daemon
- [ ] `/v1/taint` returns paths with edge provenance, or honest empty
- [ ] `dataflow` view reports `Ready` with `source_sink_reachability` capability when SCIP substrate exists
- [ ] `POST /v1/search/all` merges ≥2 workers' results with per-repo attribution and per-repo error tolerance
- [ ] `/openapi.json` regenerated includes all new routes
- [ ] Docs updated (service.md, deploy/README.md, benchmarking.md as applicable)
- [ ] No files outside the in-scope list modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- Code at cited locations doesn't match excerpts (drift since 3f5b930).
- A verification fails twice after a reasonable fix attempt.
- Track B appears to require changing propagation math from plans
  007/008 — report instead of forking the mechanism.
- `SearchResult`/`SearchInput` wire changes turn out non-additive
  (breaking) — the daemon and gateway both serve these types; a
  breaking change needs a versioning decision, not this plan.
- The taint walk needs sanitizer modeling to be honest — if
  reachability alone is judged misleading for the `PreciseDataflow`
  contract, keep the view `Partial` and report; do not ship a
  security claim the implementation doesn't back.

## Maintenance notes

- Entity-route results are `entity:`-keyed hits — the same keying
  expansion/emission use; consumers (WebUI `SearchResults`) already
  render `entity:` hits — check the hit renderer handles an
  entity-sourced hit's snippet/address fields.
- Taint v1 is reachability: sink *categories* (eval/exec/sql/fs) as a
  named library of sink patterns is the natural v2 — keep
  `TaintReport` extensible.
- Federation merge semantics (round-robin-by-rank) should be revisited
  when a calibrated score exists — note the current choice is honest
  but crude.
- `parse_query_filters` is also used by `cce grep` and `cce diff`
  argument paths — new operators must not break those (they ignore
  unknown tokens; verify).
- A reviewer should scrutinize: LIKE-injection in `pat:` (escape `%`,
  `_`, `\`); the dataflow `Ready` claim wording (reachability ≠ taint
  analysis — the capability `reason` must say so); fan-out timeout
  behavior under a dead worker; and that merged results keep
  provenance (a merged hit must still name its repo+snapshot).
