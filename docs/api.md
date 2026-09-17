# Runtime API

## CLI

`cce index [REPOSITORY]` builds or reuses the exact snapshot. `cce status [REPOSITORY]` is read-only and reports staleness. `cce search REPOSITORY QUERY` returns the plan, manifest, ranked hits, and missing capabilities. `cce context REPOSITORY QUERY` emits the canonical budgeted context pack. `cce map [REPOSITORY]` prints the package-level architecture map derived from build manifests. `cce explain REPOSITORY NAME` expands one package or symbol into members, dependencies, dependents, and tests. `cce impact REPOSITORY NAME` reports the blast radius through persisted call/reference/test edges within two hops. `cce def REPOSITORY NAME` resolves a name to its definition sites; `cce refs REPOSITORY NAME` lists inbound reference/call/implement edges with per-edge provenance (`scip` compiler truth at confidence 1.0 vs `tree_sitter` syntax at 0.6) and source-address evidence. `cce providers [REPOSITORY]` reports every provider's detect state (`ready`, `missing`, `not_applicable`, `failed`) with remediation messages. `cce gc [REPOSITORY]` prunes snapshots beyond the retention window (`--keep`) and sweeps artifact objects no committed metadata row references. `cce models` lists supported local embedding models. `cce search`/`cce context` accept a repeatable `--route` flag to pin retrieval routes for ablations. Add `--json` for machine-readable output.

Dense retrieval defaults to disabled. `--dense baseline` enables deterministic hash embeddings only for tests and benchmark ablations — it is not semantic retrieval. `--dense local` runs an in-process ONNX model via fastembed; the model is selected with `--embedding-model` (or `CCE_EMBEDDING_MODEL`, default `jinaai/jina-embeddings-v2-base-code`) and downloaded once into `<data-dir>/models/` — selecting `local` is the explicit network opt-in, after which inference is fully offline. Set `HF_ENDPOINT` to a mirror (for example `https://hf-mirror.com`) when huggingface.co is unreachable.

`--reranker [MODEL]` (or `CCE_RERANKER_MODEL`, default `rozgo/bge-reranker-v2-m3`) adds a local cross-encoder pass: the top fused candidates are re-scored as (query, document) pairs and reordered, stamped with the `reranked` route and a `fused → rerank` score explanation. The model downloads once into `<data-dir>/models/` under the same network opt-in. If the reranker is configured but cannot initialize or score, the fused order is served and the failure is reported in `missingCapabilities` — never silently. `cce models` lists both embedding and reranker model codes.

External providers are orchestrated during `cce index`: `scip:rust-analyzer` runs `rust-analyzer scip` when a `Cargo.toml` exists (install the component via `rustup component add rust-analyzer`), `scip:typescript` runs `scip-typescript` when a `tsconfig.json` exists (Node.js + `npm i -g @sourcegraph/scip-typescript` or a project devDependency), and `scip:file` ingests a repository-local `index.scip` when present — always offline. Provider output is stored content-addressed and ingested by CCE itself; providers never touch SQLite. SCIP supplies definitions/references/implementations — not taint edges — so a SCIP-backed graph turns `graph` Ready while `dataflow` reports `partial` until a source→sink analysis exists. `--no-providers` (or `CCE_NO_PROVIDERS=1`) skips provider execution entirely; queries still work.

Files whose names typically hold credentials (`.env*`, `*.pem`, `*.key`, `id_*`, `credentials*`, `secrets.*`, and similar) are never indexed by default; they are listed under `skippedSensitiveFiles` in the index report.

## HTTP

The daemon binds to `127.0.0.1:7734` by default.

| Method | Path | Behavior |
|---|---|---|
| GET | `/healthz` | Process liveness and version |
| GET | `/v1/status` | Read-only view manifest and freshness |
| POST | `/v1/index` | Build or refresh the snapshot |
| POST | `/v1/search` | Intent-aware ranked retrieval |
| POST | `/v1/context` | Canonical source-linked context pack |
| GET | `/v1/map` | Package-level architecture map |
| GET | `/v1/explain/{name}` | Component members, dependencies, dependents, tests |
| GET | `/v1/impact/{name}` | Blast radius through persisted impact edges |
| GET | `/v1/def/{name}` | Definition sites for a symbol name |
| GET | `/v1/refs/{name}` | Inbound reference/call/implement edges with provenance |
| GET | `/v1/providers` | Provider detect states and remediation |

Search JSON accepts `query`, optional `intent`, and `limit`. Context JSON accepts `query`, optional `intent`, `budgetTokens`, `maxCandidates`, `requireFresh`, and `routes`. `requireFresh=false` serves the last committed snapshot without rescanning the working tree and marks hits `verifiedCurrent: false`; the default `true` scans (and reindexes when the tree changed) before answering. Request IDs are accepted and returned as `x-request-id`. A concurrent index request returns HTTP 409.

CCE does not implement public-network authentication. Non-loopback bind requires the explicit flag and must sit behind an authenticating reverse proxy with TLS, request-size limits, and an origin policy.

## MCP

`cce-mcp REPOSITORY` speaks MCP JSON-RPC over stdio and exposes `cce_index`, `cce_status`, `cce_search`, `cce_context`, `cce_symbol`, `cce_map`, `cce_explain`, and `cce_impact`. Protocol output is written only to stdout; diagnostics go to stderr. Messages larger than 1 MiB are rejected.

All adapters return the same repository ID, snapshot ID, view manifest, source addresses, provenance, and uncertainty model.
