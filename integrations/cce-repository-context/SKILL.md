---
name: cce-repository-context
description: Build source-linked, budgeted repository context with CCE before codebase-wide analysis or implementation. Use only when explicitly invoked for understanding architecture, locating behavior or bugs, tracing dependencies/data flow, estimating impact, investigating history, or preparing a code change against a repository indexed by the CCE CLI or MCP server.
---

# CCE Repository Context

Use CCE as a retrieval and context service. Keep source code authoritative; treat summaries and derived graph edges as navigation aids.

## Workflow

1. Resolve the repository root and read its local instructions.
2. Locate CCE through its MCP tools, the `cce` executable, or this repository's `cargo run -p cce-cli --` command.
3. Inspect per-view status. Index a missing snapshot; refresh stale required views. Report failed or unavailable capabilities.
4. Select an intent and token budget using [contracts.md](references/contracts.md).
5. Prefer `cce_context` for analysis. Use `cce_search` or `cce_symbol` for exact navigation and source verification.
6. Check snapshot, source address, route, rank, score, freshness, trust origin, and uncertainty for every material result.
7. Open cited source before relying on generated summaries or derived relations.
8. After material changes, refresh the index and repeat the same query to detect retrieval or context regressions.

Do not invoke this Skill implicitly. Never infer an unavailable compiler, history, or dataflow fact from semantic similarity.

