# Operations

## Deployment

Run the daemon as an unprivileged user with read-only access to the repository and write access only to its data directory. The supplied Docker image and `deploy/cce.service` follow this boundary. Mount the target repository at `/repository` and persist `.cce` data separately when using containers.

The default loopback listener is the supported trust boundary. If remote access is required, terminate TLS and authenticate at a reverse proxy. Do not expose the daemon directly to an untrusted network.

## Storage and backup

SQLite is canonical metadata. Back up `metadata.sqlite` together with the entire `artifacts/blake3` tree from a consistent filesystem snapshot. WAL and SHM files are runtime state; use SQLite's online backup mechanism or stop the service before a plain filesystem copy.

Artifact objects are immutable and deduplicated. Orphans can exist after a failed build and are safe; `cce gc` removes them — it prunes snapshots beyond `--keep` (the current snapshot is always retained) and deletes artifact objects no committed metadata row references. `cce index` also prunes automatically after each commit (retention: 8 completed snapshots). Do not manually remove objects referenced by SQLite.

Embedding models for `--dense local` and reranker models for `--reranker` are downloaded once into `<data-dir>/models/` and reused offline afterward. If huggingface.co is unreachable, set `HF_ENDPOINT` to a mirror before the first index or reranked search.

## Health and observability

`/healthz` proves that the process can serve requests. `/v1/status` proves that a completed manifest exists and reports stale views without writing. Structured JSON logs include HTTP request IDs. Monitor index duration, 409 contention, failed/unavailable view states, query latency, disk bytes, and the age of the current snapshot.

## Failure recovery

- A build interrupted before the records commit does not advance `current_snapshots`; retry `cce index`. One interrupted after it leaves post-commit views stuck at `building`/`failed` on the committed snapshot — the next `cce index` repairs them in place (each input is recomputed from committed records; a `failed` dense backend retries once per process, so a missing model file reports `failed` once instead of re-running per request).
- A corrupt artifact produces an explicit error and must be restored from backup or rebuilt from source.
- A newer SQLite format is never downgraded in place; deploy a compatible binary.
- If local embedding fails (model download, ONNX session, or inference), the dense view becomes failed while the prior complete snapshot remains available. Re-run `cce index` once the model files are present or the cause is fixed, or disable dense retrieval.
- If the repository changes during a long build, the resulting snapshot still identifies the exact bytes scanned; the next freshness check marks it stale and a subsequent index converges.

## Upgrade policy

Use tagged releases and retain a backup before schema migration. Migrations are monotonic and run when the store opens. Rollback across a schema change requires restoring the pre-upgrade backup.
