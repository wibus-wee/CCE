# Runtime API

## CLI

`cce index [REPOSITORY]` builds or reuses the exact snapshot. `cce status [REPOSITORY]` is read-only and reports staleness. `cce search REPOSITORY QUERY` returns the plan, manifest, ranked hits, and missing capabilities. `cce context REPOSITORY QUERY` emits the canonical budgeted context pack. Add `--json` for machine-readable output.

Dense retrieval defaults to disabled. `--dense baseline` enables deterministic hash embeddings only for tests and benchmark ablations. Production embeddings require `--dense provider`, an OpenAI-compatible base URL and model, and an API key held in the environment named by `--embedding-api-key-environment`.

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
