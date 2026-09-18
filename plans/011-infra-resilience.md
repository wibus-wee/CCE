# Plan 011 — Infra resilience: observability, backpressure, failure isolation

## Goal

Bring the gateway's production mechanics up to service-grade: every
request observable, every failure mode bounded, no unbounded resource
growth. All work stays inside `apps/gateway` plus one repo-level
harness; daemon/engine/web remain the algorithm session's lane.

## Tracks

| Track | File(s) | Content |
|---|---|---|
| Metrics | `apps/gateway/src/metrics.rs` (new) | Lock-free counter registry: proxy requests by outcome (hit/miss/upstream-ok/timeout/connect/other/shedded), search-cache hit/miss/singleflight-follower, fanout requests + degraded repos, breaker rejections; fixed-boundary latency histograms for upstream calls; Prometheus text exposition at `GET /metrics`, JSON snapshot at `GET /v1/metrics`. |
| Circuit breaker | `apps/gateway/src/breaker.rs` (new) | Per-worker state machine: closed → open after N consecutive reachability failures → cooldown → half-open single probe. `admit(key)` gates the call, `on_success`/`on_failure` record outcomes. Timeouts, connect errors, and worker 5xx count as failures; any reached HTTP response <500 closes. |
| Singleflight | `apps/gateway/src/cache.rs` | `begin_flight(repo, digest)` returns Leader or Follower(receiver); leader fetches upstream, `put`s, then `finish_flight` wakes followers who re-read the cache. A failed leader wakes followers into a fresh miss instead of a hang. |
| Wiring | `apps/gateway/src/main.rs`, `fanout.rs` | Metrics instrumented at proxy/fanout boundaries; breaker `admit` before each upstream call (cache hits still serve while a worker is broken); proxy concurrency capped by `Semaphore` with bounded wait → 503 `Retry-After`; fanout scatter bounded by a shared semaphore; reqwest pool `tcp_keepalive` + `pool_max_idle_per_host`. |
| Harness | `research/loadtest.py` (new) | Stdlib-only load driver: concurrency sweep, QPS, p50/p95/p99, error counts, `x-cce-cache` hit ratio. `research/` per the Python boundary. |

## Already in place (Plan 010)

Graceful shutdown (ctrl_c), per-repo push locks, parallel blob
check/upload, hardlink materialization, response gzip, response cache
with exact push invalidation, fan-out merge, WAL reader pool (store),
`--watch` freshness delegation (daemon, pending the other session's
commit).

## Boundaries

- No `unsafe`; no new dependencies (all of the above builds on
  std/tokio/parking_lot already present).
- `crates/` and `apps/daemon`, `apps/web` remain the algorithm session's
  lane; store `prepare_cached` and scan-upsert batching are noted as
  follow-ups pending that lane settling.
- Cache invalidation stays push-exact; metrics expose state, never
  mutate it.

## Status

IN PROGRESS — modules spawned; wiring lands in `main.rs`/`fanout.rs`
after module review.
