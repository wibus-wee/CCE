# CCE — Codebase Context Engine

CCE is a local-first repository intelligence runtime that turns a repository snapshot into source-linked, capability-aware context for coding agents and humans. It combines deterministic structure, lexical and semantic retrieval, typed graph navigation, budgeted context packing, and an incrementally maintained knowledge plane.

The system follows four implementation boundaries:

- production runtime: Rust
- research and training: Python under `research/`
- Web UI: TypeScript under `apps/web/`
- Agent Skill: Markdown under `integrations/`

SQLite is the canonical metadata store, not a dumping ground. Large vectors, generated pages, snapshots, traces, and model artifacts are content-addressed files referenced transactionally from SQLite.

## Included runtime

CCE ships repository/worktree snapshotting, SQLite FTS5, Tree-sitter symbols for Rust, TypeScript/TSX/JavaScript, Python, Go, Java, and C#, content-addressed parse caches, a typed evidence graph, deterministic hierarchical knowledge pages, pure-Rust Git history ingestion, optional local ONNX embeddings (fastembed), intent-aware routing, reciprocal-rank fusion, bounded context packs, CLI/HTTP/MCP adapters, a Web dashboard, and a stage-separated benchmark harness.

Capabilities are never inferred from the mere presence of an index. `cce status` reports each view as ready, partial, stale, unavailable, or failed. Compiler-resolved references and precise dataflow require evidence-backed analysis artifacts and are explicitly unavailable until supplied.

## Quick start

```bash
cargo run -p cce-cli -- index .
cargo run -p cce-cli -- status .
cargo run -p cce-cli -- context . "where is snapshot freshness decided?" --budget 4096
```

CCE stores repository-local state in `.cce/` by default. Set `CCE_DATA_DIR` to move it elsewhere.

## Safety and privacy

Indexing is local and network-free by default. Dense retrieval is opt-in: `--dense local` runs an in-process ONNX embedding model whose files are downloaded once (the only network operation, and the explicit opt-in point) into the data directory and served offline thereafter. See `SECURITY.md` for the threat model and disclosure process.

For service deployment and recovery, see `docs/operations.md`. HTTP and MCP contracts are documented in `docs/api.md`; benchmark methodology is in `docs/benchmarking.md`.
