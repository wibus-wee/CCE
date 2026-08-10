# Operations

## Deployment

Run the daemon as an unprivileged user with read-only access to the repository and write access only to its data directory. The supplied Docker image and `deploy/cce.service` follow this boundary. Mount the target repository at `/repository` and persist `.cce` data separately when using containers.

The default loopback listener is the supported trust boundary. If remote access is required, terminate TLS and authenticate at a reverse proxy. Do not expose the daemon directly to an untrusted network.

## Storage and backup

SQLite is canonical metadata. Back up `metadata.sqlite` together with the entire `artifacts/blake3` tree from a consistent filesystem snapshot. WAL and SHM files are runtime state; use SQLite's online backup mechanism or stop the service before a plain filesystem copy.

Artifact objects are immutable and deduplicated. Orphans can exist after a failed build and are safe, but deletion requires a future mark-and-sweep command; do not manually remove objects referenced by SQLite.

## Health and observability

`/healthz` proves that the process can serve requests. `/v1/status` proves that a completed manifest exists and reports stale views without writing. Structured JSON logs include HTTP request IDs. Monitor index duration, 409 contention, failed/unavailable view states, query latency, disk bytes, and the age of the current snapshot.

## Failure recovery

- An interrupted build does not advance `current_snapshots`; retry `cce index`.
- A corrupt artifact produces an explicit error and must be restored from backup or rebuilt from source.
- A newer SQLite format is never downgraded in place; deploy a compatible binary.
- If local model loading or embedding fails, the dense view becomes failed while the prior complete snapshot remains available. Verify the pinned model cache, ONNX Runtime path, and memory-budget flags before retrying.
- Keep `--embedding-allow-download` disabled in steady-state service units. Populate the model cache once, record its BLAKE3-bearing serving profile from result provenance, then serve network-free.
- Keep `--reranker-allow-download` disabled in steady state as well. Prefer a promoted `cce-reranker-manifest.json` bundle and record its content-derived revision in deployment configuration.
- Prefer daemon or MCP deployment for local dense retrieval so model weights remain resident. One-shot CLI query latency includes ONNX model initialization.
- Size reranker capacity from measured candidate count, sequence length, p95, and RSS. It is a bounded Top-K stage, but a cross-encoder is materially more expensive than vector scoring; long-lived processes amortize initialization, not pairwise compute. Increase `--reranker-sessions` only after load testing because every additional session trades memory for concurrent throughput.
- Keep the daemon process long-lived to retain both the ONNX session and decoded vector artifact. Query workers share the immutable index; candidate scoring is parallel and only Top-K source bodies are loaded from the artifact store.
- Inspect `reusedEmbeddings` and the `incremental_embedding_reuse` dense capability after each build. An unexpected zero with an unchanged serving profile usually means retrieval-document identities shifted because a file or enclosing source range changed.
- The default memory guard rejects large `batch × sequence²` configurations. Raising it with `--embedding-allow-high-memory` is an operator decision and should be paired with a measured RSS limit.
- `--scip-auto` requires a local `rust-analyzer` with SCIP support. Its resolved executable path, version, and thread count are part of the index profile. A timeout or non-zero exit retains syntax graph facts and reports the compiler graph failure explicitly.
- Treat supplied SCIP files as unattested unless the producing system separately binds them to source hashes. For authoritative dataflow, validate analyzer output against `schemas/dataflow-v1.schema.json`; a source, overlay, repository, or range mismatch rejects the entire view.
- For `--dataflow-joern`, install and pin Joern locally; CCE performs no container pull or remote analysis. Size the temporary artifact volume and JVM heap for the repository, set a finite `--joern-timeout-seconds`, and monitor partial status for source-unmapped relevant endpoints. Analyzer stdout is discarded and failure diagnostics are bounded; temporary CPG/GraphML files are removed after import.
- CCE rescans the repository before committing an index. If source changes during SCIP generation or any other long build stage, the transaction is cancelled and the previous complete snapshot remains current.
- Search rescans source identity but reuses a source-identical current snapshot even when query-only flags differ from its index profile. If source changed, a fresh search fails closed; run `cce index` explicitly with the intended compiler, dataflow, parser, and embedding materialization. Callers that deliberately set `requireFresh=false` may read the last complete snapshot, whose hits are marked unverified. Search never silently downgrades materialized views.

## Upgrade policy

Use tagged releases and retain a backup before schema migration. Migrations are monotonic and run when the store opens. Rollback across a schema change requires restoring the pre-upgrade backup.
