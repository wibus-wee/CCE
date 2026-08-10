# CCE — Codebase Context Engine

CCE is a local-first repository intelligence runtime that turns a repository snapshot into source-linked, capability-aware context for coding agents and humans. It combines deterministic structure, lexical and semantic retrieval, typed graph navigation, budgeted context packing, and an incrementally maintained knowledge plane.

The system follows four implementation boundaries:

- production runtime: Rust
- research and training: Python under `research/`
- Web UI: TypeScript under `apps/web/`
- Agent Skill: Markdown under `integrations/`

SQLite is the canonical metadata store, not a dumping ground. Large vectors, generated pages, snapshots, traces, and model artifacts are content-addressed files referenced transactionally from SQLite.

## Included runtime

CCE ships repository/worktree snapshotting, SQLite FTS5, Tree-sitter symbols for Rust, C/C++, TypeScript/TSX/JavaScript, Python, Go, Java, and C#, content-addressed parse caches, transaction-aligned SCIP ingestion, a typed multi-hop evidence graph, strict static-analysis dataflow import and local Joern PDG generation, deterministic hierarchical knowledge pages, per-commit Git history with changed paths, pinned local ONNX embeddings and cross-encoder reranking, intent-aware routing, reciprocal-rank fusion, bounded context packs, CLI/HTTP/MCP adapters, a Web dashboard, and a stage-separated benchmark harness.

Capabilities are never inferred from the mere presence of an index. `cce status` reports each view as ready, partial, stale, unavailable, or failed. `--scip-auto` can generate Rust compiler-resolved references locally; other languages remain partial until their SCIP indexes are supplied. Precise dataflow becomes ready only after a source-hash-aligned artifact conforming to `schemas/dataflow-v1.schema.json` is supplied or locally generated with `--dataflow-joern`.

With a pinned local Joern installation on `PATH`, CCE can build program-dependence edges without a remote service:

```bash
cce --dataflow-joern --joern-timeout-seconds 3600 index .
```

CCE runs `joern-parse` and `joern-export --repr all --format graphml` in an isolated local directory, accepts only Joern `REACHING_DEF` and `CDG` edges, maps both endpoints back to current source bytes, and then applies the same repository/revision/file-hash/range validation as a supplied artifact. Set `CCE_JOERN_PARSE` and `CCE_JOERN_EXPORT` for a version-pinned installation. Missing or unaligned endpoints make the view partial rather than guessed.

## Quick start

```bash
cargo run -p cce-cli -- index .
cargo run -p cce-cli -- status .
cargo run -p cce-cli -- context . "where is snapshot freshness decided?" --budget 4096
```

CCE stores repository-local state in `.cce/` by default. Set `CCE_DATA_DIR` to move it elsewhere.

## Local semantic retrieval

Dense retrieval can run without an inference service. Install ONNX Runtime locally, point `CCE_ONNX_RUNTIME_LIBRARY` at its shared library, and allow the first index to download a commit-pinned model snapshot:

```bash
cargo run --release -p cce-cli -- \
  --dense local \
  --embedding-allow-download \
  index .
```

After the model is cached, omit `--embedding-allow-download` for network-free operation. The built-in model presets keep the model, immutable revision, and training prefixes together: `quality` (default, Jina code), `balanced` (multilingual E5 small), and `fast` (quantized BGE small). For example, select the multilingual preset with `--embedding-model balanced`.

Local inference defaults to batch size 4 and 512 tokens. Configurations whose estimated attention allocation exceeds the safe limit are rejected unless `--embedding-allow-high-memory` is explicitly supplied. Long-running daemon and MCP processes keep the model resident; one-shot CLI commands include model-load latency.

Fine-tuned models can be exported as a self-contained, hash-bound bundle and loaded without a registry or service:

```bash
uv run --project research --extra models --extra onnx cce-research export-onnx \
  research/output/models/cce-e5-enhanced \
  research/output/models/cce-e5-enhanced-onnx

cargo run --release -p cce-cli -- \
  --dense local \
  --embedding-model balanced \
  --embedding-revision <bundleRevision-from-cce-model-manifest.json> \
  --embedding-model-directory research/output/models/cce-e5-enhanced-onnx \
  index .
```

CCE verifies the bundle revision, runtime family, path containment, and every required file's SHA-256 before ONNX initialization. See `docs/training.md` for dataset construction, fine-tuning, and held-out comparison.

A learned second-stage reranker is also local and opt-in. One-time registry download is explicit; production can instead use a portable hash-bound bundle:

```bash
cargo run --release -p cce-cli -- \
  --reranker local \
  --reranker-allow-download \
  search . "where is hybrid retrieval fused?" --intent architecture

uv run --project research cce-research promote-reranker-bundle \
  <pinned-cache-snapshot> research/output/models/cce-reranker-onnx \
  jinaai/jina-reranker-v1-turbo-en \
  5bcd26bbe1913aff3cc1983d91f2c3ac9ee91cbf
```

After promotion, pass `--reranker-model-directory` and the manifest's `bundleRevision`, omit `--reranker-allow-download`, and keep the daemon/MCP process resident to amortize model initialization.

## Safety and privacy

Indexing and query inference are local and network-free by default. `--embedding-allow-download` and `--reranker-allow-download` are explicit, one-time model-registry permissions; inference remains local. CCE exposes no remote embedding or reranking mode. See `SECURITY.md` for the threat model and disclosure process.

For service deployment and recovery, see `docs/operations.md`. HTTP and MCP contracts are documented in `docs/api.md`; benchmark methodology is in `docs/benchmarking.md`.
