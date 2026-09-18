# Plan 010 — Gateway infra: reliability, ingest performance, fan-out

**Owner:** infra track (parallel to the 007–009 algorithm track — the
algorithm session owns `crates/`, `apps/daemon/`, `research/`; this plan
owns `apps/gateway/` and `apps/push/` exclusively).

**Context.** cce-gateway is the Sourcegraph-model front door: repo
registry, CAS blob ingest, wildcard proxy `/{repo}/v1/*` to per-repo
workers. Audit findings it must fix before "service 能扩容" is real:

- `reqwest::Client::new()` — zero timeouts; a hung worker hangs the door.
- Push missing-check: up to 500k sequential `exists()` syscalls on the
  async executor.
- Materialization: per-file `std::fs::copy` when CAS→sources sits on the
  same filesystem — hardlinks make it O(1) metadata per file.
- No `/v1/search/all` fan-out (Plan 009 Track C's service half).
- cce-push uploads blobs over HTTP roundtrips, one at a time.
- No latency/error observability anywhere.

**Boundary (hard).** Agents touching this plan may edit only:

- `apps/gateway/src/main.rs` + new `apps/gateway/src/*.rs` modules
- `apps/push/src/main.rs`
- `plans/` (status updates by the orchestrator)

Forbidden: `apps/daemon/**`, `crates/**`, `apps/web/**`, `research/**`,
root/app `Cargo.toml` (no new dependencies — check
`[workspace.dependencies]` first and defer instead).

## Track A — reliability

- reqwest client: connect timeout (5s), request timeout for proxy
  traffic (~120s or per-request `.timeout()`).
- Proxy upstream-error taxonomy: timeout → 504, connect/refused → 502,
  worker's own status passes through (current behavior, keep).
- Worker health: `GET /v1/repos/{id}` probes `{worker}/v1/health`
  (2s timeout) → additive `workerStatus` field. No probing on list.
- tracing::info/warn on push completion and proxy upstream errors
  (repo id, elapsed ms).

## Track B — ingest performance

- Missing-blob check → one `spawn_blocking`, parallel stat across
  ~16 scoped threads.
- Materialize via `std::fs::hard_link` (CAS immutable ⇒ OSTree pattern),
  `std::fs::copy` fallback for cross-device.
- Whole staging loop inside one `spawn_blocking`; `put_blob` write too.
- Wire contract unchanged: paths, request/response shapes, error codes.

## Track C — `/v1/search/all` fan-out (Plan 009 Track C, service half)

New `apps/gateway/src/fanout.rs`:

- `POST /v1/search/all` `{query, limit=20, perRepoLimit=limit, repos?}`
  — `repos` by id or slug, absent = all.
- Concurrent scatter via `tokio::task::JoinSet`; per-request 10s timeout.
- Per-repo failure → `degraded` entry, never fatal.
- Merge = round-robin by rank position across repos (scores are
  corpus-local, cross-repo comparison is dishonest — documented in the
  endpoint description). Inject `"repo": slug` per hit.
- Response: `{query, limit, searched, degraded[], perRepo[{repoId,
  repoSlug, hitCount, latencyMs}], hits[], missingCapabilities[],
  verdicts{slug: value}}` — per-repo verdicts pass through, never merged.

## Track D — cce-push upload parallelism

- `--concurrency N` (default 8, clamp 1..=64).
- `std::thread::scope` workers over `check.missing` chunks, shared
  blocking client; progress every 10%/500 blobs; any PUT failure aborts
  (CAS dedups make retry safe).

## Verification

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`.
- `cce-push --help` parses the new flag.
- Fanout merge unit tests live in `fanout.rs` (`merge_hits` pure fn:
  uneven lists, limit truncation, repo injection, empties).
- Live smoke deferred: needs two running workers — docker-compose
  multi-worker exercise is follow-up, not gate.

## Deferred (explicit)

- `MetadataStore` single-connection mutex → read pool: real bottleneck,
  but `metadata.rs` belongs to the algorithm session — coordinate later.
- K8s/K3s manifests, auth, multi-tenancy: deferred since 009 planning
  (service layer already ahead of kernel maturity).
- docker-compose multi-worker integration test: after tracks land.
