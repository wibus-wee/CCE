# Deploying CCE

Two deployment shapes, one image:

- **Central** (default `docker-compose.yml`): a `cce-gateway` front door plus
  one or more `cce-daemon` workers. Developers push repositories with
  `cce-push`; the gateway materializes source trees from a content-addressed
  blob store and routes API calls to the owning worker. This is the
  multi-repo / team shape.
- **Standalone** (`--profile standalone`): a single `cce-daemon` serving a
  read-only host worktree mount — the original single-repo local shape.

Neither has an auth layer: host ports publish on `127.0.0.1` only. Put a
reverse proxy with auth in front before exposing either beyond loopback.

## Central: gateway + worker

```bash
docker compose up -d --build
curl -fsS http://127.0.0.1:7735/healthz

# on any machine that can reach the gateway:
cd /path/to/repo
cce-push --gateway http://127.0.0.1:7735 --repo worker
```

Topology:

```
cce-push ──▶ gateway :7735 ──┬─ /v1/repos/*        registry, blob CAS, push
             (stateless API) ├─ /{repo}/v1/*       reverse-proxy → worker
WebUI ────▶ served here      ├─ /{repo}/mcp        MCP-over-HTTP → worker
agents ───▶ MCP endpoint     ├─ /                  static web UI
                             ├─ /openapi.json      generated spec (utoipa)
                             └─ /docs              Swagger UI
                                    │
                              worker :7734 (internal)
                              cce-daemon on the materialized
                              source dir; index + models in its
                              own volume
```

- `cce-push` scans the worktree (`.gitignore` + `.cceignore` + sensitive and
  generated-file policy), hashes blobs with BLAKE3, asks the gateway which
  digests are missing, uploads only those, then commits a manifest — the
  gateway materializes the tree via staging + atomic rename and kicks the
  worker's index. `--watch` re-syncs on worktree or git-ref changes; no
  hooks needed.
- `--repo worker` must match the worker's `CCE_WORKER_REPO` (default
  `worker`): the worker serves exactly one materialized repo, at
  `/sources/<slug>`. The `ensure` endpoint auto-registers the pushed name
  against `--default-worker`, so a pushed repo lands on the single worker
  only when its slug matches.
- Freshness is push-time: queries serve the last pushed tree. The
  gateway's `lastPush` record carries the source git revision.
- Scaling out: add another `worker-b` service pinned to a different
  `CCE_WORKER_REPO`, mount the shared `cce-sources` volume, and register
  the repo explicitly (`POST /v1/repos` with `workerUrl`) instead of
  relying on `--default-worker`.
- The Web UI auto-detects its host: behind a gateway it shows a repository
  selector and proxies calls under `/{slug}/v1/*`; behind a standalone
  daemon it talks to `/v1/*` directly.
- Both services generate their OpenAPI spec from the live axum routes and
  `utoipa::ToSchema` types — `/openapi.json` plus Swagger UI at `/docs`.
  No handwritten spec exists to drift.
- Code browsing is worker-side: `GET /v1/files` lists the indexed files of
  the current snapshot; `GET /v1/file?path=<repo-relative>` returns the
  committed source bytes (1 MiB cap, binary-flagged) — reachable through
  the gateway proxy as `/{slug}/v1/files` etc.
- Agents connect MCP clients to `http://<gateway>:7735/{slug}/mcp`
  (Streamable-HTTP transport, stateless POST — JSON `application/json`
  responses, no SSE fan-out, session ids advisory). 15 `cce_*` tools cover
  index/status/search/context/symbol/map/explain/impact/defs/refs/
  providers/grep/diff/files/file; tool failures arrive as `isError`
  content, protocol failures as JSON-RPC errors. Batches ≤16 supported.
  The stdio `cce-mcp` binary still embeds the engine for local use.

## Standalone: live worktree mount

```bash
CCE_REPO=/path/to/repo docker compose --profile standalone up -d cce
curl -fsS http://127.0.0.1:7734/healthz
```

The daemon indexes the mounted worktree directly; every query re-scans the
live tree. One daemon serves one repository — run a second compose project
(`-p cce-b`, different `CCE_PORT`/`CCE_REPO`) for another.

## Knobs

| Variable | Default | Effect |
|---|---|---|
| `CCE_GATEWAY_PORT` | `7735` | Host loopback port for the gateway |
| `CCE_WORKER_REPO` | `worker` | Repo slug the default worker serves |
| `CCE_PORT` | `7734` | Standalone host port |
| `CCE_REPO` | `.` | Standalone host worktree path |
| `CCE_DENSE` | `local` | `local` = ONNX embeddings (downloads once); `disabled` = fully offline sparse mode |
| `CCE_EMBEDDING_MODEL` | jina-code | Model code, see `cce models` |
| `CCE_NO_PROVIDERS` | unset | `1` skips SCIP toolchain detection |
| `RUST_LOG` | `info` | tracing filter |

## Image & CI

- `Dockerfile` is multi-stage (web UI + Rust workspace → `debian:slim`,
  non-root uid 10001) and ships all four binaries: `cce`, `cce-daemon`,
  `cce-gateway`, `cce-push`.
- CI (`ci.yml`) runs fmt, clippy `-D warnings`, workspace tests, msrv, docs,
  unused-deps, coverage, the web build, the Python research suite, and a
  Docker job that builds the image and health-checks a sparse-mode boot.
- First dense index downloads the embedding model — the only network
  dependency; `CCE_DENSE=disabled` removes it entirely.

## Operational notes

- Gateway state (`repos.json` registry + `blobs/` CAS) lives in the
  `cce-gateway-state` volume; materialized trees in the shared
  `cce-sources` volume; worker index/artifacts/models in `cce-worker-data`.
  Back up state + sources; a worker volume can be dropped and re-derived.
- `POST /v1/index` and `require_fresh` queries serialize on an OS-level
  index lease per worker; concurrent readers never block.
- systemd: `deploy/cce.service` runs a standalone daemon with equivalent
  hardening (`ProtectSystem=strict`, `PrivateTmp`,
  `ReadWritePaths=/var/lib/cce`).
- Memory: ~600 MB resident model + flat-scan workspace per worker; the 6 GB
  compose limit is headroom, not a requirement.
