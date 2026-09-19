# Development runbook

Day-to-day commands for working on and with CCE. `cce` is `cargo run -p
cce-cli --` during development or `target/release/cce` after a release
build.

## Build

```bash
cargo build -p cce-cli                      # debug
cargo build --release --locked -p cce-cli   # release (CI builds with --locked)
```

## Index

```bash
cce index .                    # snapshot + FTS + tree-sitter + history + providers
cce index --no-providers .     # skip external toolchains (SCIP, zoekt)
CCE_DATA_DIR=/data cce index . # relocate the .cce/ data root
```

`cce status .` reports every view (lexical/graph/dense/zoekt/history) as
ready / partial / stale / unavailable / failed — never silently inferred.
`cce doctor .` runs store health checks; `cce rebuild .` rewrites derived
state from the canonical store.

## Providers — zoekt toolchain

Zoekt is a derived lexical candidate index living under
`.cce/providers/zoekt/` (`bin/` tools, `index/` shards). Binaries resolve
in order: `CCE_ZOEKT`/`CCE_ZOEKT_INDEX` env → `PATH` → the managed cache.

```bash
cce providers .                # detect report for all toolchains
cce providers --provision .    # install pinned zoekt into .cce/providers/zoekt/bin
                               # (release assets, sha256-verified; `go install`
                               # fallback at the same pinned upstream commit)
```

`--provision` is the explicit network opt-in — nothing downloads during
index or query. A `missing` report includes the remediation; search
degrades to non-zoekt routes rather than claiming coverage.

## Query

```bash
cce search . "where is snapshot freshness decided"   # full route plan
cce search . "query" --route zoekt                   # one route; repeatable
cce search . "query" --no-verify                     # committed snapshot,
                                                     # no worktree rescan
cce context . "query" --budget 4096                  # context pack
cce grep . 'fn resolve_index'                        # always-fresh worktree regex
cce diff . 'pattern'                                 # regex over commit patches
cce diff . --head <snapshot>                         # snapshot delta
cce explain . <symbol> / cce impact . <symbol>       # graph navigation
```

`--no-verify` trades freshness checking for latency — hits report
`verifiedCurrent: false`. Without it every query first rescans the
worktree to decide whether the snapshot is still fresh.

## Daemon

```bash
cce-daemon . --watch             # HTTP + dashboard; poller keeps the
                                 # snapshot fresh, so queries skip the
                                 # per-query worktree rescan
cce-daemon . --watch --watch-interval 30
```

## Quality gates

Per `AGENTS.md` — run all three before committing:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Retrieval, storage, parsing, or packing changes additionally require the
self-index smoke benchmark — invocation and metrics contract are in
`docs/benchmarking.md` (`cce-research run benchmarks/datasets/cce-self.jsonl …`).

## Layout

| Path | Contents |
|---|---|
| `crates/cce-core` | types, errors, config |
| `crates/cce-store` | SQLite metadata + FTS, artifact store |
| `crates/cce-engine` | scanner, providers, planner, retrieval, zoekt |
| `apps/cli` `apps/daemon` `apps/mcp` | adapters over the engine |
| `benchmarks/` | datasets, adapters, result bundles |
| `research/` | Python harness (`cce-research`), datasets, spikes |
