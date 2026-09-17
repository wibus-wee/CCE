# Plan 001: Benchmark harness can exercise dense retrieval and records engine latency

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git rev-parse --short HEAD` and record it in
> the "Planned at" field below (the advisor's shell was unavailable; the
> field was left unfilled). Then
> `git diff --stat e25751c..HEAD -- research/cce_research/adapters.py benchmarks/adapters/ crates/cce-core/src/context.rs crates/cce-engine/src/context.rs docs/benchmarking.md`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tests (benchmark harness)
- **Planned at**: commit `e25751c`, 2026-09-17

## Why this matters

The checked-in benchmark adapters can only run retrieval with dense disabled:
`--dense` is a CLI flag with no environment variable, and the adapter command
template has no way to emit it. Consequences:

1. The dense route contributes **zero** candidates in every benchmark run, so
   any claim about embedding model quality ("e5-small has topped out") is not
   reproducible from repository state.
2. `ResultBundleManifest.model_identity` records `none` even when a run did
   use a local model, so result bundles cannot be traced back to the model
   that produced them.
3. `CaseResult.query_ms` is the wall-clock time of the whole `cce` subprocess
   (process spawn + require-fresh scan + lazy ONNX session init), while the
   engine's own `latencyMs` field is parsed and then thrown away. Latency
   comparisons conflate startup noise with engine compute.

After this plan: an adapter YAML can declare `dense: local` and
`embedding_model: <code>`; the manifest records the model identity
automatically; and every result carries the engine-reported latency alongside
the subprocess wall-clock.

## Current state

Files and the exact code to modify:

- `research/cce_research/adapters.py` — adapter loading, command expansion,
  subprocess execution. Relevant excerpts (verify before editing):

  ```python
  # adapters.py:17-24
  @dataclass(frozen=True)
  class Adapter:
      name: str
      command: list[str]
      timeout_seconds: int
      environment: dict[str, str]
      model_identity: str
      model_revision: str
  ```

  ```python
  # adapters.py:27-38 — Adapter.load reads keys from YAML
      return cls(
          name=str(raw["name"]),
          command=[str(item) for item in raw["command"]],
          timeout_seconds=int(raw.get("timeout_seconds", 120)),
          environment={str(key): str(value) for key, value in raw.get("environment", {}).items()},
          model_identity=str(raw.get("model_identity", "none")),
          model_revision=str(raw.get("model_revision", "none")),
      )
  ```

  ```python
  # adapters.py:54-65 — command template expansion (excerpt)
      command: list[str] = []
      for part in self.command:
          if part == "{intent_args}": ...
          if part == "{route_args}": ...
          command.append(part.format_map(scalars))
      return command
  ```

  ```python
  # adapters.py:89-102 — CaseResult construction
      return CaseResult(
          case_id=case.case_id,
          ...
          query_ms=elapsed_ms,
          metadata={"command": shlex.join(command)},
      )
  ```

- `benchmarks/adapters/cce.yaml` and `cce-search.yaml` — existing adapters;
  both invoke `target/release/cce` with `--json`, subcommand `context` or
  `search`, and `{intent_args}`/`{route_args}`/`{budget}`/`{limit}`
  placeholders. `model_identity: none`.

- `apps/cli/src/main.rs` — the flags the adapter will emit:
  - `--dense <disabled|baseline|local>` — `global = true`, `value_enum`
    (main.rs:16-17)
  - `--embedding-model <code>` — `global = true`, env `CCE_EMBEDDING_MODEL`
    (main.rs:18-20)
  - `--embedding-dimensions <n>` — `global = true` (main.rs:21-22)

  Because all three are `global = true`, appending them at the **end** of the
  expanded command is valid clap usage — no template changes needed.

- `crates/cce-engine/src/retrieval.rs` — `SearchResult` serializes
  `latency_ms` as `latencyMs` (`#[serde(rename_all = "camelCase")]` on the
  struct, field at line 24).

- `crates/cce-core/src/context.rs` — `ContextPack` (lines 57-76) has **no**
  latency field today; the `context` subcommand's JSON therefore carries no
  engine-side timing.

- `crates/cce-engine/src/context.rs` — `CceEngine::context` (lines 161-174)
  runs `self.search(...)` then `ContextPacker::new().pack(&search, ...)`.

- `research/cce_research/schema.py` — `CaseResult.metadata` is
  `dict[str, str | int | float | bool | None]` (line 146); a nullable
  `engine_latency_ms` fits without a schema change.

- `docs/benchmarking.md` — documents the harness contract; the "Result
  schema" section (line 16) should mention the new metadata key.

Conventions to match:

- Python: pydantic models with `extra="forbid"`, `from __future__ import
  annotations`, stdlib + typer/rich/yaml only (see `research/pyproject.toml`).
- Rust: `#![forbid(unsafe_code)]`, `#[serde(rename_all = "camelCase")]` on
  wire types, additive fields use `#[serde(default)]`.
- YAML adapters: `name`, `command` list, `timeout_seconds`,
  `model_identity`, `model_revision`, `environment` — see
  `benchmarks/adapters/cce-search.yaml`.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Python tests | `uv run --project research pytest research/tests` | all pass |
| Dataset validation | `uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl` | "Validated 15 cases" |
| Rust fmt | `cargo fmt --all -- --check` | exit 0 |
| Rust lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| Rust tests | `cargo test --workspace --all-features` | all pass |
| Build CLI | `cargo build --release --locked -p cce-cli` | `target/release/cce` exists |

## Scope

**In scope**:

- `research/cce_research/adapters.py`
- `research/tests/test_adapters.py` (create)
- `benchmarks/adapters/cce-dense-e5-small.yaml` (create)
- `benchmarks/adapters/cce-search-dense-e5-small.yaml` (create)
- `crates/cce-core/src/context.rs` (one additive field)
- `crates/cce-engine/src/context.rs` (one line to populate it)
- `docs/benchmarking.md` (Result schema paragraph)

**Out of scope**:

- `apps/cli/src/main.rs` — the flags already exist and are global; do not add
  a `CCE_DENSE` env var or change flag semantics in this plan.
- `apps/daemon`, `apps/mcp` — daemon already amortizes the session; adapters
  unchanged.
- `research/cce_research/metrics.py` — do not add new metric families here;
  `engine_latency_ms` lands in `metadata`, not in scored observations.
- Any change to `query_ms` semantics — wall-clock stays as-is; the new field
  is additive in `metadata`.

## Git workflow

- Branch: `advisor/001-harness-dense-passthrough`
- One commit is fine; message style: imperative, e.g.
  `benchmark: let adapters pin dense backend and record engine latency`
- Do NOT push or open a PR.

## Steps

### Step 1: Extend `Adapter` with dense passthrough

In `research/cce_research/adapters.py`, add three optional fields to the
`Adapter` dataclass (with defaults so existing YAML keeps working):

```python
    dense: str | None = None  # "baseline" | "local"
    embedding_model: str | None = None
    embedding_dimensions: int | None = None
```

In `Adapter.load`, read them:

```python
        dense = raw.get("dense")
        if dense is not None and dense not in ("baseline", "local"):
            raise ValueError(f"{path}: dense must be 'baseline' or 'local', got {dense!r}")
        embedding_model = raw.get("embedding_model")
        model_identity = str(raw.get("model_identity", "none"))
        if model_identity == "none" and embedding_model:
            model_identity = f"local:{embedding_model}"
        return cls(
            ...,
            dense=str(dense) if dense else None,
            embedding_model=str(embedding_model) if embedding_model else None,
            embedding_dimensions=(
                int(raw["embedding_dimensions"]) if "embedding_dimensions" in raw else None
            ),
            model_identity=model_identity,
            ...
        )
```

In `build_command`, after the template loop, append the flags (all global, so
position at the end is valid):

```python
        if self.dense:
            command.extend(["--dense", self.dense])
        if self.embedding_model:
            command.extend(["--embedding-model", self.embedding_model])
        if self.embedding_dimensions is not None:
            command.extend(["--embedding-dimensions", str(self.embedding_dimensions)])
        return command
```

Guard: `embedding_model`/`embedding_dimensions` without `dense` is a
misconfiguration — raise `ValueError` in `load` when `dense` is absent but
either of the other two is present.

**Verify**: `uv run --project research python -c "from cce_research.adapters import Adapter; from pathlib import Path; a = Adapter.load(Path('benchmarks/adapters/cce.yaml')); print(a.dense)"` → prints `None`.

### Step 2: Record engine-reported latency in result metadata

In `Adapter.run`, extend the metadata dict:

```python
            metadata={
                "command": shlex.join(command),
                "engine_latency_ms": payload.get("latencyMs"),
            },
```

`SearchResult` payloads already carry `latencyMs`; context packs will after
Step 4. When absent the value is `None`, which `CaseResult.metadata` accepts.

**Verify**: `uv run --project research pytest research/tests` → all pass.

### Step 3: Unit tests for the adapter changes

Create `research/tests/test_adapters.py`. Model after
`research/tests/test_metrics.py` (read it first for fixture style). Cover:

- `Adapter.load` on a YAML without dense keys → `dense is None`, identity
  unchanged (`none`).
- YAML with `dense: local` + `embedding_model: intfloat/multilingual-e5-base`
  → `build_command` output ends with
  `["--dense", "local", "--embedding-model", "intfloat/multilingual-e5-base"]`,
  and `model_identity == "local:intfloat/multilingual-e5-base"`.
- YAML with `embedding_model` but no `dense` → `Adapter.load` raises
  `ValueError`.
- `build_command` still expands `{intent_args}`/`{route_args}` correctly with
  dense flags appended after them.

**Verify**: `uv run --project research pytest research/tests/test_adapters.py -v` → all pass.

### Step 4: Add `latencyMs` to `ContextPack`

In `crates/cce-core/src/context.rs`, add to `ContextPack`:

```rust
    /// Engine-side wall time for the underlying search plus packing, in ms.
    #[serde(default)]
    pub latency_ms: u64,
```

In `crates/cce-engine/src/context.rs`, in `CceEngine::context`, after
`pack(...)`:

```rust
        let mut pack = ContextPacker::new().pack(&search, request.budget_tokens);
        pack.latency_ms = search.latency_ms;
        Ok(pack)
```

(adjust to the exact surrounding code; the method currently returns
`Ok(ContextPacker::new().pack(...))` directly).

In `docs/benchmarking.md`, in the "Result schema" paragraph, add one
sentence: result `metadata` may carry `engine_latency_ms`, the engine's own
timing, distinct from subprocess wall-clock `query_ms`.

**Verify**: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings` → exit 0.

### Step 5: Dense adapter YAMLs

Create `benchmarks/adapters/cce-dense-e5-small.yaml` — copy of `cce.yaml`
plus:

```yaml
dense: local
embedding_model: intfloat/multilingual-e5-small
```

Create `benchmarks/adapters/cce-search-dense-e5-small.yaml` — copy of
`cce-search.yaml` plus the same two lines. Do not set `model_identity` in
either; Step 1 derives it. Leave `environment.CCE_DATA_DIR: .cce-benchmark`
unchanged — the embedding model is part of `index_profile_hash`
(`crates/cce-engine/src/config.rs`, `EngineConfig::profile`), so dense and
non-dense runs produce different snapshots and can share the data dir
safely.

**Verify**: `uv run --project research python -c "from cce_research.adapters import Adapter; from pathlib import Path; a = Adapter.load(Path('benchmarks/adapters/cce-search-dense-e5-small.yaml')); print(a.dense, a.model_identity)"` → `local local:intfloat/multilingual-e5-small`.

### Step 6: Smoke-run one case through the dense adapter

```bash
cargo build --release --locked -p cce-cli
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-dense-e5-small.yaml . WORKTREE research/output/smoke-dense.jsonl
```

Expected: every case prints `✓`; `research/output/smoke-dense.jsonl.manifest.json`
records `model_identity: "local:intfloat/multilingual-e5-small"`. First run
downloads the model into `.cce-benchmark/models/` (the documented one-time
network opt-in); if huggingface.co is unreachable, set `HF_ENDPOINT` per
`docs/api.md`.

## Test plan

- `research/tests/test_adapters.py` (new) — see Step 3.
- Existing `research/tests/test_metrics.py` must keep passing unchanged.
- Rust: existing tests only; no new engine behavior beyond the additive
  field. `context_pack_is_bounded_and_source_linked` in
  `crates/cce-engine/tests/integration.rs` must still pass.

## Done criteria

- [ ] `uv run --project research pytest research/tests` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo test --workspace --all-features` exits 0
- [ ] `cce context --json` output contains `latencyMs`
- [ ] The two new adapter YAMLs load; dense flags appear in expanded commands
- [ ] A run bundle's manifest shows the derived `model_identity`
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The excerpts above don't match the live files (drift).
- `target/release/cce --dense local` rejects the flag (clap definition
  changed) — the plan assumes the current CLI surface.
- fastembed reports `intfloat/multilingual-e5-small` as an unknown model code
  (run `target/release/cce models` to list valid codes).
- Making `latencyMs` work on packs appears to require more than the additive
  field shown (e.g. a second serialization site).

## Maintenance notes

- Reviewers should check that dense flags are appended **after** template
  expansion so `{route_args}`-derived flags are unaffected.
- `query_ms` remains subprocess wall-clock by design; consumers wanting
  engine compute time read `metadata.engine_latency_ms`. A future plan may
  populate `CaseResult.index_ms` by timing `cce index` separately — deferred.
- When `--dense local` runs, each benchmark case still pays one ONNX session
  init (per-process `OnceCell`). If that dominates runtimes, the follow-up is
  a CLI batch mode or daemon adapter — deliberately out of scope here.
