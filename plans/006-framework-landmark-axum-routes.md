# Plan 006: First framework-landmark extractor — axum route bindings

## Priority

P2 — independent; schedule after 001-003 land (it's most valuable once v5 architecture cases exist to measure it). The report explicitly calls framework landmarks "比更大 embedding 更重要的结构补丁" (§5.4) and lists them in P0 scope — this is the cheapest concrete instance.

## Goal

Emit deterministic `Route` entities + `RouteHandledBy` relations for axum-style `.route("path", get(handler))` bindings, so that `impact`, `explain`, and architecture queries can answer "what serves /v1/..." with evidence-carrying facts instead of text matching.

## Background for the executor

- Schema anticipated this: `EntityKind::Route` (`entity.rs:24`), `RelationKind::RouteHandledBy` (`relation.rs:21`), `RelationOrigin::FrameworkRule` (`relation.rs:42`) exist. Nothing new in cce-core. Note `Relation.evidence` is `Vec<SourceAddress>` (no detail field) — extra context goes in `attributes` (`serde_json::Map`); the snapshot field is `snapshot_id`, not `valid_snapshot`.
- Entity type is `CodeEntity` (`cce-core/src/entity.rs:34`), relation type is `Relation` (`relation.rs:48`). IDs come from `engine::entity_id` / `engine::relation_id` (both `pub(crate)`, `engine.rs:1066`/`1097`) — reuse them so route entities dedupe deterministically.
- Route entities must land in `records.entities`, which `add_derived_relations` cannot do (it only pushes into `records.relations`, `engine.rs:640-649`). Follow the `packages::emit` pattern instead (`packages.rs:92-109`): a `landmarks::emit(...) -> LandmarkEmission { entities, relations }` called from `index()` right after the package emission block (`engine.rs:653-663`), sharing `file_entities`/`file_regions`.
- House style precedent: `import_specifiers` (`relations.rs`) uses regex on raw file text — regex is consistent with this codebase for landmark extraction; tree-sitter is not required.
- This repo's own axum surface (dogfoodable): `apps/daemon/src/main.rs` `.route("/v1/index", post(index))` and friends — re-verify line numbers at execution time.
- Trust model: `FrameworkRule` origin sits between syntax facts and model inference (`docs/architecture.md` trust levels). Use **0.85** confidence.

## Steps

1. **New module** `crates/cce-engine/src/landmarks.rs`:
   - `pub(crate) struct RouteBinding { path, method, handler_name, address: SourceAddress }` and `pub(crate) fn axum_routes(files: &[ScannedFile], texts: &HashMap<String, String>) -> Vec<RouteBinding>` scanning `.rs` files for `\.route\(\s*"([^"]+)"\s*,\s*([a-z_]+)\(` plus method-router calls `get|post|put|delete|patch|head|options|trace` — capture route path literal, HTTP method (from the router fn name), handler identifier, and call-site line range (reuse the line-span pattern from `import_specifiers`).
   - `pub(crate) fn emit(bindings, context-pieces…) -> LandmarkEmission { entities: Vec<CodeEntity>, relations: Vec<Relation> }` following `packages::emit` exactly: `engine::entity_id` for deterministic ids, `CodeEntity { kind: EntityKind::Route, name: "{METHOD} {path}", address: call site, region_id: file region or symbol region of the enclosing fn, .. }`.
   - `mod landmarks;` in `lib.rs`.
2. **Wire into `index()`** (`engine.rs`, right after the `packages::emit` block ~line 653-663): `records.entities.extend(emission.entities); records.relations.extend(emission.relations);`. The emission needs `repository_id`, `snapshot_id`, `files`, `texts`, `file_entities`, `file_regions`, and `name_index` (symbol-name → entity lookup, already built for `add_derived_relations`) — assemble whatever subset it needs at the call site; do not stuff it into `add_derived_relations` since that function's contract is relations-only.
   - Handler resolution: `handler_name` → `Function`/`Method` entity via `name_index`, same-file candidates first (match `SymbolCandidate` by path), then global fallback; ambiguous within same file → first match; no candidate → emit the `Route` entity alone, no edge.
   - Emit `Relation { kind: RouteHandledBy, source_entity_id: route, target_entity_id: handler, origin: FrameworkRule, confidence: 0.85, snapshot_id, extractor: "cce-landmark-axum-v1", evidence: vec![call_site_address], attributes: {"framework": "axum", "route": path, "method": method} }`.
3. **Tests** (`landmarks.rs` `#[cfg(test)]` module or integration): fixture `.route("/v1/search", post(search))` + `async fn search(...)` in same file → asserts Route entity + RouteHandledBy edge with correct origin/confidence/evidence; negative: `.route(x, y)` where `y` isn't an identifier → no edge.
4. **Dogfood check**: index this repo (`cargo run -p cce-cli -- index .`), query relations — `sqlite3 .cce/metadata.sqlite "select * from relations where origin='framework_rule'"` (or via `cce explain`/`impact` on a route entity) shows `/v1/search → search` etc.
5. **Caveat accuracy**: `impact.rs:171` caveat mentions only tree-sitter calls/refs — check whether framework edges need their own caveat string (likely fine; they carry higher confidence by design).

## Files

- `crates/cce-engine/src/landmarks.rs` (new, incl. unit tests)
- `crates/cce-engine/src/lib.rs` (mod decl)
- `crates/cce-engine/src/engine.rs` (emit call site, next to `packages::emit`)
- `docs/architecture.md` (one line under extractors, if it lists them)

## Verification

- `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`.
- `cargo run -p cce-cli -- index .` clean on this repo; `/v1/*` routes present as entities/relations with `framework_rule` origin.
- `uv run --project research pytest research/tests -q` (no benchmark regressions).

## Done when

Axum bindings emit `Route` entities + `RouteHandledBy` edges with `framework_rule` origin, evidence addresses, and 0.85 confidence; `impact` on a handler surfaces its route; tests cover positive/negative/ambiguous cases.

## Maintenance notes

- Adding more frameworks (express `.get`, React hooks→`UsesHook`, DI bindings) = new detector fn in `landmarks.rs` + tests; keep each extractor id versioned (`cce-landmark-<fw>-vN`) since relations record provenance.
- These edges are the substrate for report §5.6 Feature Flow and §A `trace_feature` — don't collapse them into generic `calls` edges; the distinct kind is what lets expansion policies weight them.

## Escape hatch

- If handler resolution is too ambiguous in practice (generic routers, `route_service`, merged routers), ship only the literal-method cases with edges; leave non-literal forms as `Route` entities without `RouteHandledBy` — still useful for `map`/`explain`.
- If the pass measurably slows indexing on this repo, gate it behind a config flag rather than optimizing prematurely.
