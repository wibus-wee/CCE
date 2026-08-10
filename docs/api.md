# Runtime API

## CLI

`cce index [REPOSITORY]` builds or reuses the exact snapshot. `cce status [REPOSITORY]` is read-only and reports staleness. `cce search REPOSITORY QUERY` returns the plan, manifest, ranked hits, and missing capabilities. `cce context REPOSITORY QUERY` emits the canonical budgeted context pack. Add `--json` for machine-readable output.

Dense retrieval defaults to disabled. `--dense baseline` enables deterministic hash embeddings only for tests and benchmark ablations. `--dense local` is the production local-inference path: it loads a pinned ONNX model and a caller-supplied local ONNX Runtime shared library, verifies the model artifact set, and never calls an inference API. `--embedding-allow-download` permits only the initial pinned model download; leave it off for offline serving.

The local model argument accepts the coupled presets `quality`, `balanced`, and `fast`. A custom model must include `--embedding-revision` with an immutable 40–64 character hexadecimal commit. Query and document prefixes can be overridden independently, but are part of the index profile and therefore invalidate incompatible dense artifacts. `--embedding-sessions`/`CCE_EMBEDDING_SESSIONS` selects a 1–16 session pool for concurrent daemon or MCP inference; it changes serving capacity and memory, not vector identity.

Remote embedding inference is not part of the runtime. A trained local bundle is selected with `--embedding-model-directory`, its `bundleRevision` with `--embedding-revision`, and the matching built-in runtime family with `--embedding-model`. CCE verifies the bundle manifest and required files before loading ONNX.

`--reranker local` enables a local cross-encoder for broad natural-language, issue, impact, architecture, and history intents; exact symbol, trace, and precise-dataflow routes preserve deterministic ordering. The default is the commit-pinned `jinaai/jina-reranker-v1-turbo-en`; `--reranker-allow-download` is only for initial acquisition. Offline deployments use `--reranker-model-directory` plus the bundle's `--reranker-revision`. `--reranker-max-length`, `--reranker-threads`, and `--reranker-batch-size` bound local compute. `--reranker-sessions` creates 1–16 independently locked ONNX sessions for concurrent daemon/MCP queries and multiplies model memory accordingly; the default is one. CLI, daemon, and MCP expose the same flags and `CCE_RERANKER_*` environment variables. There is no remote reranking backend.

## Compiler graph and dataflow inputs

`--scip-auto` runs local `rust-analyzer scip` and `scip-typescript` providers inside the indexing transaction. JS/TS indexing discovers root and pnpm/Yarn workspace projects, then builds a temporary external TypeScript project for uncovered source files; it does not create or edit repository configuration. Provider indexes are merged by document richness before importing definition/reference/implementation facts. Executable identities are part of the index profile, generated protobufs live in CCE's temporary area, and source is rescanned before commit. `--scip-index PATH` imports a prebuilt SCIP protobuf but reports its snapshot alignment as unattested. Use `--rust-analyzer PATH`, `--scip-typescript PATH`, `--scip-threads`, and `--scip-timeout-seconds` to control the bounded local processes.

`--dataflow-index PATH` imports a local static-analysis artifact conforming to `schemas/dataflow-v1.schema.json`. `--dataflow-joern` instead invokes local `joern-parse` and `joern-export` executables; paths, frontend language, and timeout are configured by `--joern-parse`, `--joern-export`, `--joern-language`, `--joern-timeout-seconds` or the corresponding `CCE_JOERN_*` variables. JS/TS-only repositories infer `JAVASCRIPT` when no language is supplied. CCE records the reported exporter version, falling back to content digests of launchers that do not implement `--version`. The modes conflict. Joern's `REACHING_DEF` and `CDG` edges become data and control dependencies only when both endpoints map to current source.

Both paths pass through the same importer, which accepts only artifacts whose repository ID, base revision, workspace overlay hash, per-file content hashes, byte ranges, and line ranges match the current source. Valid `data_flow`, `taint_flow`, and `control_flow` edges make the dataflow view ready; malformed or stale input is rejected rather than inferred. A Joern run with skipped relevant endpoints is explicitly partial.

Git history is materialized as one retrieval document per reachable commit, including commit ID, timestamp, subject, and up to 64 changed paths computed from Git trees. History results therefore compete as individual evidence rather than one repository-wide summary. Commit messages and tree diffs remain historical evidence, not current-source truth.

## HTTP

The daemon binds to `127.0.0.1:7734` by default.

| Method | Path | Behavior |
|---|---|---|
| GET | `/healthz` | Process liveness and version |
| GET | `/v1/status` | Read-only view manifest and freshness |
| POST | `/v1/index` | Build or refresh the snapshot |
| POST | `/v1/search` | Intent-aware ranked retrieval |
| POST | `/v1/context` | Canonical source-linked context pack |

Search JSON accepts `query`, optional `intent`, and `limit`. Context JSON accepts `query`, optional `intent`, `budgetTokens`, `maxCandidates`, and `requireFresh`. Request IDs are accepted and returned as `x-request-id`. A concurrent index request returns HTTP 409.

CCE does not implement public-network authentication. Non-loopback bind requires the explicit flag and must sit behind an authenticating reverse proxy with TLS, request-size limits, and an origin policy.

## MCP

`cce-mcp REPOSITORY` speaks MCP JSON-RPC over stdio and exposes `cce_index`, `cce_status`, `cce_search`, `cce_context`, and `cce_symbol`. Protocol output is written only to stdout; diagnostics go to stderr. Messages larger than 1 MiB are rejected.

All adapters return the same repository ID, snapshot ID, view manifest, source addresses, provenance, and uncertainty model.
