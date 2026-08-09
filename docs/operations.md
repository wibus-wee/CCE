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
- If an embedding provider fails, the dense view becomes failed while the prior complete snapshot remains available. Disable dense retrieval or repair provider configuration before retrying.
- If the repository changes during a long build, the resulting snapshot still identifies the exact bytes scanned; the next freshness check marks it stale and a subsequent index converges.

## Upgrade policy

Use tagged releases and retain a backup before schema migration. Migrations are monotonic and run when the store opens. Rollback across a schema change requires restoring the pre-upgrade backup.
