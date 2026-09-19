# CCE Engineering Contract

CCE is a local-first, Agent-agnostic repository intelligence runtime. It is not a prompt-only Skill and it must never present generated knowledge as source truth.

## Hard boundaries

- Production runtime is Rust. Every Rust crate inherits `unsafe_code = "forbid"`; do not add `unsafe` blocks, declarations, or unsafe traits.
- SQLite is the canonical metadata and FTS store. Large or independently versioned payloads belong in the content-addressed artifact store, including vectors, source snapshots, generated knowledge, model files, and benchmark traces.
- Python is confined to `research/` for datasets, model benchmarking/training, SFT, trajectory analysis, plots, and offline experiments.
- TypeScript is confined to the Web UI and generated API clients. Do not reimplement retrieval or indexing logic in TypeScript.
- Agent integrations are thin Markdown/MCP/CLI adapters over the engine.

## Correctness

- Every result maps to a canonical source address and records snapshot, view freshness, route, score, and provenance.
- Deterministic facts, framework-derived relations, and model inference must remain distinguishable.
- A stale or unavailable view is reported explicitly; never silently fall back and claim the requested capability.
- Retrieval, selection, packing, and downstream generation are separate evaluation stages.
- Keep network access minimal and disclosed. Dense local embeddings are the frontend default (`--dense local`); the first index downloads the ONNX model once into `<data>/models/`. `--dense disabled` / `CCE_DENSE=disabled` is the offline switch — indexing and lexical/structural querying must keep working with no network and no cached model (mark the dense view `Failed`/`Unavailable` with a reason, never hard-fail).

## Quality gates

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`. Run the self-index smoke benchmark when retrieval, storage, parsing, or packing changes.

Daemon-mode adapters resolve `cce-daemon` as the sibling binary of `target/release/cce` — rebuild both before benchmarking (`cargo build --release -p cce-cli -p cce-daemon`) or the daemon keeps serving the previous build.


## Web UI design system

- When making UI or using components, always check if components from `@antfu/design` can be reused, before making new components or creating inline DOM elements. The React ports live in `apps/web/src/ui/` (Base UI + StyleX, same prefixed names: `Action*`, `Display*`, `Form*`, `Feedback*`, `Layout*`, `Overlay*`).
- Style with the semantic tokens in `apps/web/src/ui/tokens.stylex.ts` — never hard-code colors; keep light/dark parity and use mono/tabular numerals for technical values.
- Mock/fixture data for surfaces without a backend endpoint lives only in `apps/web/src/mock/` (typed, one export per surface) and every consumer must label the surface `preview` — never present fixtures as live index truth.
