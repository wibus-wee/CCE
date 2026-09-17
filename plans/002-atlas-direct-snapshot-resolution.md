# Plan 002: Atlas APIs resolve the current snapshot directly instead of running a dummy search

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git rev-parse --short HEAD` and record it in
> "Planned at" (the advisor's shell was unavailable; the field was left
> unfilled). Then
> `git diff --stat e25751c..HEAD -- crates/cce-engine/src/atlas.rs crates/cce-engine/tests/integration.rs docs/api.md`
> On any mismatch with the excerpts below, treat as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug (behavioral quirk + wasted work)
- **Planned at**: commit `e25751c`, 2026-09-17

## Why this matters

`codebase_map`, `explain_component`, and `impact_analysis` each call
`current_snapshot_id()`, which runs a **full search pipeline** — planner,
union-route candidate generation, RRF fusion — with the hard-coded query
`"architecture orientation"` purely to read `result.request.snapshot_id`
(`atlas.rs:343-356`).

Two concrete costs:

1. **Surprising side effect**: the dummy search runs with
   `require_fresh: false`, but `resolve_index` falls through to
   `self.index().await` when no committed snapshot exists
   (`retrieval.rs:48-71`). So `cce map` on an **unindexed** repository
   silently triggers a complete index build — a read-shaped command doing
   write-shaped work, contrary to the read-only expectations set by
   `cce status`.
2. **Wasted work per call**: every map/explain/impact invocation pays for
   lexical search, entity lookups, and fusion it never uses.

After this plan, atlas calls resolve the snapshot the same way the
`require_fresh=false` path does — identify the repository, read
`current_snapshot`, verify completeness — and return a clear
`ViewUnavailable` error when nothing is indexed.

## Current state

- `crates/cce-engine/src/atlas.rs` — the three atlas APIs and the helper to
  replace. Current helper (lines 341-356):

  ```rust
      /// Snapshot id for the current committed index, reusing the search
      /// path's freshness handling without running a retrieval.
      async fn current_snapshot_id(&self) -> Result<String> {
          let result = self
              .search(SearchRequest {
                  repository_id: String::new(),
                  snapshot_id: String::new(),
                  query: "architecture orientation".to_owned(),
                  intent: None,
                  limit: 1,
                  require_fresh: false,
                  routes: Vec::new(),
              })
              .await?;
          Ok(result.request.snapshot_id)
      }
  ```

  Called from `codebase_map` (line 82), `explain_component` (line 168),
  `impact_analysis` (line 261). After the change, `SearchRequest`/`SearchResult`
  imports in this file may become unused — clean them up.

- `crates/cce-engine/src/retrieval.rs` — the non-fresh resolution path to
  mirror (lines 48-62):

  ```rust
      async fn resolve_index(&self, require_fresh: bool) -> Result<ResolvedIndex> {
          if !require_fresh {
              let anchor = RepositoryScanner::new(self.config().clone()).identify()?;
              if let Some(snapshot_id) = self.store().current_snapshot(&anchor.identity.id)? {
                  if self.store().snapshot_is_complete(&snapshot_id)? {
                      return Ok(ResolvedIndex { ... verified_fresh: false });
                  }
              }
          }
          let report = self.index().await?;   // <-- the silent index build
          ...
      }
  ```

- `crates/cce-engine/src/engine.rs` — `CceEngine` exposes `self.store()`,
  `self.config()`; `status()` (lines 92-114) already demonstrates the
  identify → `current_snapshot` → error-when-absent pattern:

  ```rust
          let scanned = RepositoryScanner::new(self.config.clone()).scan(Some(&self.store))?;
          let current = self
              .store
              .current_snapshot(&scanned.identity.id)?
              .ok_or_else(|| CceError::ViewUnavailable {
                  view: "manifest".to_owned(),
                  reason: "repository has not been indexed".to_owned(),
              })?;
  ```

  Note `status()` uses `scan` (full hashing) because it must detect staleness;
  atlas only needs `identify()` (cheap, no file hashing) because serving the
  last committed snapshot is the documented behavior.

- `crates/cce-store/src/metadata.rs` — `MetadataStore::current_snapshot` and
  `snapshot_is_complete` signatures (used above).

- `crates/cce-engine/tests/integration.rs` — test patterns; `engine(dir)`
  helper at lines 32-34, `fixture_repo()` at 21-30, `workspace_fixture()` at
  181-214. Atlas behavior tests already exist:
  `package_graph_comes_from_build_manifests` (299-333) and
  `typed_relations_carry_provenance_and_confidence` (245-297) — both index
  first, so they keep passing.

- `docs/api.md` — check whether it documents map/explain/impact triggering
  an index; if it does, update the wording. (Advisor could not verify.)

Conventions: `#![forbid(unsafe_code)]`; errors via `cce_core::CceError`;
atlas types already import `cce_core::{EntityKind, RelationKind, Result,
SearchRequest}` — `SearchRequest` import becomes removable.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0 |
| lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| tests | `cargo test --workspace --all-features` | all pass |
| focused test | `cargo test -p cce-engine --test integration` | all pass incl. new test |

## Scope

**In scope**:

- `crates/cce-engine/src/atlas.rs`
- `crates/cce-engine/tests/integration.rs` (add one test)
- `docs/api.md` (only if it documents index-on-demand for atlas endpoints)

**Out of scope**:

- `crates/cce-engine/src/retrieval.rs` — do not change `resolve_index`; the
  search path's index-on-miss behavior is intentional for `search`/`context`.
- `apps/daemon/src/main.rs` — the `/v1/map`, `/v1/explain/{name}`,
  `/v1/impact/{name}` handlers call the same engine methods and inherit the
  fix; no handler changes needed.
- Any change to `violations` (still intentionally empty — L3 component
  inference is a later, separate layer per `docs/architecture.md:54`).

## Git workflow

- Branch: `advisor/002-atlas-direct-snapshot`
- Commit message style: imperative, e.g.
  `atlas: resolve current snapshot directly instead of dummy search`
- Do NOT push or open a PR.

## Steps

### Step 1: Replace the dummy search with direct snapshot resolution

In `atlas.rs`, rewrite `current_snapshot_id` to:

```rust
    /// Snapshot id of the last committed index. Atlas reads committed state;
    /// it never triggers indexing — an unindexed repository is an explicit
    /// error, matching `status` semantics.
    async fn current_snapshot_id(&self) -> Result<String> {
        let anchor = RepositoryScanner::new(self.config().clone()).identify()?;
        match self.store().current_snapshot(&anchor.identity.id)? {
            Some(snapshot_id)
                if self.store().snapshot_is_complete(&snapshot_id)? =>
            {
                Ok(snapshot_id)
            }
            _ => Err(cce_core::CceError::ViewUnavailable {
                view: "atlas".to_owned(),
                reason: "repository has not been indexed; run `cce index`".to_owned(),
            }),
        }
    }
```

Add `RepositoryScanner` to the `use crate::...` import; remove `SearchRequest`
from the `cce_core` import if unused; remove the now-stale doc comment about
"reusing the search path".

**Verify**: `cargo clippy --workspace --all-targets --all-features -- -D warnings` → exit 0 (no unused imports).

### Step 2: Update docs if they promise index-on-demand

Read `docs/api.md`. If any endpoint description says map/explain/impact
index on demand or implies they work on unindexed repos, correct the wording
to "requires a completed index; returns 409 otherwise" (the daemon maps
`ViewUnavailable` → 409 CONFLICT already, `daemon/main.rs:93`).

**Verify**: `grep -n "index" docs/api.md` — confirm no stale claim remains.

### Step 3: Regression test — atlas on an unindexed repo errors

In `crates/cce-engine/tests/integration.rs`, add:

```rust
#[tokio::test]
async fn atlas_on_unindexed_repository_is_an_explicit_error() {
    let repo = fixture_repo();   // files written, never indexed
    let engine = engine(repo.path());

    for outcome in [
        engine.codebase_map().await.map(|_| ()),
        engine.explain_component("anything").await.map(|_| ()),
        engine.impact_analysis("anything").await.map(|_| ()),
    ] {
        let error = outcome.expect_err("atlas must fail without an index");
        assert!(
            matches!(error, cce_core::CceError::ViewUnavailable { .. }),
            "expected ViewUnavailable, got {error:?}"
        );
    }
}
```

If `CceEngine` methods return a different error wrapper, match the actual
type — the assertion that matters is "explicit error, no index built". To
prove no index ran, verify no snapshot was committed. Note that
`CceEngine::open` eagerly creates `.cce/metadata.sqlite` (store open), so
asserting on the file's existence is wrong — assert on committed state
instead:

```rust
    let store = cce_store::MetadataStore::open(repo.path().join(".cce"))?;
    assert!(
        store
            .current_snapshot(&<repo identity id>)
            .expect("current_snapshot query")
            .is_none(),
        "atlas calls must not commit a snapshot"
    );
```

where the identity id comes from
`RepositoryScanner::new(engine_config).identify()?.identity.id` — or, if
wiring a second store handle is awkward, simply drop that assertion: the
`ViewUnavailable` error already proves no index ran, since the old path
would have indexed successfully and returned a map.

**Verify**: `cargo test -p cce-engine --test integration atlas_on_unindexed` → 1 passed.

### Step 4: Confirm indexed-repo atlas behavior unchanged

`package_graph_comes_from_build_manifests` and
`typed_relations_carry_provenance_and_confidence` already cover the indexed
path.

**Verify**: `cargo test --workspace --all-features` → all pass.

## Test plan

- New: `atlas_on_unindexed_repository_is_an_explicit_error` (Step 3).
- Existing: the two atlas tests listed above are the indexed-path coverage;
  they must pass unchanged.

## Done criteria

- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo test --workspace --all-features` exits 0
- [ ] `grep -n "architecture orientation" crates/cce-engine/src/atlas.rs` → no match
- [ ] `cce map` on a repo with no committed snapshot exits nonzero with a
      "not been indexed" message and commits no snapshot (note: the
      `.cce/metadata.sqlite` file itself is created eagerly by
      `MetadataStore::open` — check committed snapshots, not the file)
- [ ] No files outside the in-scope list are modified
- [ ] `plans/README.md` status row updated

## STOP conditions

- `current_snapshot_id` no longer resembles the excerpt (drift).
- `MetadataStore` lacks `current_snapshot`/`snapshot_is_complete` with the
  signatures used in `retrieval.rs`/`engine.rs` (API drift).
- Making this work appears to require touching `retrieval.rs` — out of
  scope; report instead.
- Callers (daemon/MCP) are discovered to depend on the index-on-miss side
  effect — report before deciding.

## Maintenance notes

- After this lands, atlas endpoints serve the last **committed** snapshot;
  they do not detect worktree drift. That matches the previous behavior
  (`require_fresh: false` served unverified). If atlas should later report
  staleness, add the `status()`-style scan comparison as a separate change.
- The string `"architecture orientation"` disappears from the codebase; the
  same phrase is used as the union-plan knowledge query nowhere else — safe.
- Reviewer: confirm the fix keeps `Result`/`CceError` conventions and that
  no caller treated the implicit indexing as a feature.
