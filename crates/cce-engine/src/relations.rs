//! Derived relation extraction: module imports, call edges, type-level
//! references, git co-change links, and test links.
//! Every edge records where it came from (`origin`) and how much it can be
//! trusted (`confidence`) so compiler-grade sources can later outrank these
//! syntax-level facts.

use std::collections::{HashMap, HashSet};

use cce_core::{EntityKind, Relation, RelationKind, RelationOrigin, SourceAddress};

use crate::{ParsedFile, ScannedFile};

/// Everything the derived-relation pass needs from the indexing pipeline.
pub(crate) struct RelationContext<'a> {
    pub repository_id: &'a str,
    pub snapshot_id: &'a str,
    pub files: &'a [ScannedFile],
    pub texts: &'a HashMap<String, String>,
    pub file_entities: &'a HashMap<String, String>,
    pub parsed: &'a HashMap<String, ParsedFile>,
    pub unit_ids: &'a HashMap<String, Vec<String>>,
    /// Simple-name lookup for every extracted symbol entity.
    pub name_index: &'a HashMap<String, Vec<SymbolCandidate>>,
    /// Newest-first per-commit touched paths, shared with the `lastTouched`
    /// attribute pass so history is walked once per index.
    pub touched: &'a [crate::history::CommitPaths],
}

#[derive(Debug, Clone)]
pub(crate) struct SymbolCandidate {
    pub entity_id: String,
    pub path: String,
    pub kind: EntityKind,
}

/// Demotion applied when name resolution had to choose among several
/// same-name candidates: the picked target is a guess, so the edge must
/// not carry — or propagate — the same trust as a unique resolution.
const AMBIGUOUS_RESOLUTION_FACTOR: f32 = 0.5;

/// Confidence for a name-resolved edge: `base` when the spelling picked
/// out exactly one viable candidate, demoted when the resolver chose
/// among several.
fn resolved_confidence(base: f32, candidates: usize) -> f32 {
    if candidates > 1 {
        base * AMBIGUOUS_RESOLUTION_FACTOR
    } else {
        base
    }
}

/// Record an ambiguous resolution on the edge's attributes so consumers
/// can distinguish a guessed target from a uniquely resolved one without
/// re-running the name index.
fn mark_ambiguity(attributes: &mut serde_json::Map<String, serde_json::Value>, candidates: usize) {
    if candidates > 1 {
        attributes.insert("ambiguous".to_owned(), true.into());
        attributes.insert("candidateCount".to_owned(), candidates.into());
    }
}

pub(crate) fn add_derived_relations(context: &RelationContext<'_>, output: &mut Vec<Relation>) {
    let mut seen = output
        .iter()
        .map(|relation| relation.id.clone())
        .collect::<HashSet<_>>();
    for relation in import_relations(context)
        .into_iter()
        .chain(call_relations(context))
        .chain(type_reference_relations(context))
        .chain(test_relations(context))
        .chain(changed_with_relations(context))
    {
        // `relation_id` is a pure function of (source, target, kind): the set
        // keeps duplicate call sites from producing duplicate primary keys.
        if seen.insert(relation.id.clone()) {
            output.push(relation);
        }
    }
}

// --- imports ---------------------------------------------------------------

/// File-to-file `Imports` edges: TypeScript/JavaScript and Python relative
/// specifiers plus Rust `mod`/`use` paths resolved to workspace files.
fn import_relations(context: &RelationContext<'_>) -> Vec<Relation> {
    let mut relations = Vec::new();
    for file in context.files {
        let Some(source_id) = context.file_entities.get(&file.relative_path) else {
            continue;
        };
        let Some(text) = context.texts.get(&file.relative_path) else {
            continue;
        };
        for (specifier, confidence) in import_specifiers(text, file.language.as_deref()) {
            let target = match file.language.as_deref() {
                Some("rust") => resolve_rust_module(&file.relative_path, &specifier, context),
                _ => resolve_relative_import(&file.relative_path, &specifier, context),
            };
            let Some(target_path) = target else {
                continue;
            };
            let Some(target_id) = context.file_entities.get(&target_path) else {
                continue;
            };
            relations.push(Relation {
                id: relation_id(source_id, target_id, "imports"),
                source_entity_id: source_id.clone(),
                target_entity_id: target_id.clone(),
                kind: RelationKind::Imports,
                origin: RelationOrigin::FrameworkRule,
                confidence,
                snapshot_id: context.snapshot_id.to_owned(),
                extractor: "cce-module-import-v2".to_owned(),
                evidence: Vec::new(),
                attributes: serde_json::Map::from_iter([(
                    "specifier".to_owned(),
                    specifier.into(),
                )]),
            });
        }
    }
    relations
}

fn import_specifiers(text: &str, language: Option<&str>) -> Vec<(String, f32)> {
    match language {
        Some("typescript" | "tsx" | "javascript") => capture_specifiers(
            text,
            r#"(?m)(?:from\s+|import\s*\(|require\s*\()\s*[\"'](\.{1,2}/[^\"']+)[\"']"#,
            0.85,
        ),
        Some("python") => {
            capture_specifiers(text, r"(?m)^\s*from\s+(\.+[A-Za-z0-9_\.]*)\s+import", 0.85)
        }
        Some("rust") => rust_module_specifiers(text),
        _ => Vec::new(),
    }
}

fn capture_specifiers(text: &str, pattern: &str, confidence: f32) -> Vec<(String, f32)> {
    regex::Regex::new(pattern)
        .ok()
        .into_iter()
        .flat_map(|regex| {
            regex
                .captures_iter(text)
                .filter_map(|capture| {
                    capture
                        .get(1)
                        .map(|value| (value.as_str().to_owned(), confidence))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Rust module edges: `mod name;` declarations (high confidence — the
/// compiler requires the file) and `use crate::…`/`use self::…`/`use super::…`
/// paths (lower — the final segment may be an item, not a file).
fn rust_module_specifiers(text: &str) -> Vec<(String, f32)> {
    let mut specifiers = Vec::new();
    if let Ok(mods) =
        regex::Regex::new(r"(?m)^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;")
    {
        for capture in mods.captures_iter(text) {
            if let Some(name) = capture.get(1) {
                specifiers.push((format!("mod:{}", name.as_str()), 0.95));
            }
        }
    }
    if let Ok(uses) = regex::Regex::new(
        r"(?m)^\s*use\s+(crate|self|super)(::[a-zA-Z_][a-zA-Z0-9_]*)+\s*(?:\{[^}]*\})?\s*;",
    ) {
        for capture in uses.captures_iter(text) {
            if let Some(path) = capture.get(0) {
                let body = path
                    .as_str()
                    .trim_start()
                    .trim_start_matches("use")
                    .trim_end_matches(';')
                    .trim();
                // `use crate::a::b::{C, D}` — drop the grouped tail, keep path.
                let path = body.split('{').next().unwrap_or(body).trim_end_matches(':');
                specifiers.push((format!("use:{path}"), 0.8));
            }
        }
    }
    specifiers
}

/// Resolve a Rust `mod`/`use` specifier to an indexed file. `mod name;` maps
/// into the file's module directory; `use` paths resolve the longest path
/// prefix that is a real file under the crate source root.
fn resolve_rust_module(
    source_path: &str,
    specifier: &str,
    context: &RelationContext<'_>,
) -> Option<String> {
    if let Some(name) = specifier.strip_prefix("mod:") {
        let module_dir = rust_module_dir(source_path);
        for candidate in [
            format!("{module_dir}/{name}.rs"),
            format!("{module_dir}/{name}/mod.rs"),
        ] {
            if context.file_entities.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        return None;
    }
    let path = specifier.strip_prefix("use:")?;
    let mut segments = path.split("::").collect::<Vec<_>>();
    let anchor = match segments.first().copied() {
        Some("crate") => crate_source_root(source_path),
        Some("self") => rust_module_dir(source_path),
        Some("super") => parent_dir(&rust_module_dir(source_path)),
        _ => return None,
    };
    segments.remove(0);
    // Shrink the segment list until a prefix resolves to a file: the tail may
    // name an item inside the module rather than the module itself.
    while !segments.is_empty() {
        let joined = segments.join("/");
        for candidate in [
            format!("{anchor}/{joined}.rs"),
            format!("{anchor}/{joined}/mod.rs"),
        ] {
            if context.file_entities.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        segments.pop();
    }
    None
}

/// The directory a Rust file's `mod`/`self` declarations resolve against:
/// `a/b.rs` owns module dir `a/b/`; `a/mod.rs`, `a/lib.rs`, `a/main.rs` own `a/`.
fn rust_module_dir(source_path: &str) -> String {
    let stem = source_path.trim_end_matches(".rs");
    if source_path.ends_with("/mod.rs")
        || source_path.ends_with("/lib.rs")
        || source_path.ends_with("/main.rs")
    {
        parent_dir(source_path)
    } else {
        stem.to_owned()
    }
}

fn parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_owned())
        .unwrap_or_default()
}

/// Crate source root for `crate::` resolution: the nearest `src/` directory
/// in the path, falling back to the top-level directory.
fn crate_source_root(source_path: &str) -> String {
    if let Some(index) = source_path.find("/src/") {
        return source_path[..index + 4].to_owned();
    }
    if let Some(rest) = source_path.strip_prefix("src/") {
        let _ = rest;
        return "src".to_owned();
    }
    parent_dir(source_path)
}

fn resolve_relative_import(
    source_path: &str,
    specifier: &str,
    context: &RelationContext<'_>,
) -> Option<String> {
    let parent = std::path::Path::new(source_path).parent()?;
    let normalized_specifier = if specifier.starts_with('.') && !specifier.contains('/') {
        specifier
            .replace('.', "../")
            .trim_end_matches('/')
            .to_owned()
    } else {
        specifier.to_owned()
    };
    let base = normalize_components(parent.join(normalized_specifier))?;
    let extensions = [
        "",
        ".ts",
        ".tsx",
        ".js",
        ".jsx",
        ".py",
        "/index.ts",
        "/index.tsx",
        "/index.js",
    ];
    extensions
        .iter()
        .map(|extension| format!("{base}{extension}"))
        .find(|candidate| context.file_entities.contains_key(candidate))
}

fn normalize_components(path: std::path::PathBuf) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

// --- calls -----------------------------------------------------------------

/// `Calls` edges between symbols. Tree-sitter gives us the callee's
/// *spelling*, not its canonical definition, so resolution prefers a
/// same-file candidate, then callable kinds, and carries confidence 0.6 —
/// these are syntax facts, not compiler truth. A spelling shared by
/// several symbols is a guess: the edge is demoted and marked `ambiguous`.
fn call_relations(context: &RelationContext<'_>) -> Vec<Relation> {
    let mut relations = Vec::new();
    for file in context.files {
        let Some(parsed) = context.parsed.get(&file.relative_path) else {
            continue;
        };
        let Some(unit_ids) = context.unit_ids.get(&file.relative_path) else {
            continue;
        };
        let Some(text) = context.texts.get(&file.relative_path) else {
            continue;
        };
        let Some(file_id) = context.file_entities.get(&file.relative_path) else {
            continue;
        };
        // Dedup on the edge identity — (caller, callee) — not on the
        // callee alone: two different callers invoking one helper are two
        // real edges, while a repeated call from the same caller is one.
        let mut emitted = HashSet::new();
        for call in &parsed.calls {
            let caller_id = call
                .caller
                .and_then(|index| unit_ids.get(index))
                .unwrap_or(file_id);
            let Some((callee, candidates)) =
                resolve_callee(&call.name, &file.relative_path, context)
            else {
                continue;
            };
            if callee.entity_id == *caller_id
                || !emitted.insert((caller_id.clone(), callee.entity_id.clone()))
            {
                continue;
            }
            let evidence = call_site_address(context, file, text, call.start_byte, call.end_byte);
            let mut attributes =
                serde_json::Map::from_iter([("calleeName".to_owned(), call.name.clone().into())]);
            mark_ambiguity(&mut attributes, candidates);
            relations.push(Relation {
                id: relation_id(caller_id, &callee.entity_id, "calls"),
                source_entity_id: caller_id.clone(),
                target_entity_id: callee.entity_id.clone(),
                kind: RelationKind::Calls,
                origin: RelationOrigin::TreeSitter,
                confidence: resolved_confidence(0.6, candidates),
                snapshot_id: context.snapshot_id.to_owned(),
                extractor: format!(
                    "cce-call-extract-v2:{}",
                    parsed.parser.as_deref().unwrap_or("unknown")
                ),
                evidence: evidence.into_iter().collect(),
                attributes,
            });
        }
    }
    relations
}

/// Returns the picked candidate plus the size of the same-name choice
/// set. Every candidate is viable — the final fallback accepts any kind —
/// so a non-unique spelling always means the pick was a guess.
fn resolve_callee<'a>(
    name: &str,
    caller_path: &str,
    context: &'a RelationContext<'_>,
) -> Option<(&'a SymbolCandidate, usize)> {
    let candidates = context.name_index.get(name)?;
    let picked = candidates
        .iter()
        .find(|candidate| candidate.path == caller_path)
        .or_else(|| {
            candidates.iter().find(|candidate| {
                matches!(candidate.kind, EntityKind::Function | EntityKind::Method)
            })
        })
        .or_else(|| candidates.first())?;
    Some((picked, candidates.len()))
}

fn call_site_address(
    context: &RelationContext<'_>,
    file: &ScannedFile,
    text: &str,
    start: usize,
    end: usize,
) -> Option<SourceAddress> {
    let start_line = text
        .as_bytes()
        .get(..start.min(text.len()))
        .unwrap_or(text.as_bytes())
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1;
    let end_line = text
        .as_bytes()
        .get(..end.min(text.len()))
        .unwrap_or(text.as_bytes())
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1;
    SourceAddress::new(
        context.repository_id,
        context.snapshot_id,
        &file.relative_path,
        start as u64..end as u64,
        start_line..=end_line,
    )
    .ok()
}

// --- type references --------------------------------------------------------

/// `References` edges from a symbol to the type-level entities named in its
/// type positions (parameters, returns, fields, annotations). Resolution is
/// spelling-based like calls but restricted to type kinds, so confidence
/// stays at 0.55 — syntax-level, not compiler-resolved. Spellings shared by
/// several type-level candidates are demoted and marked `ambiguous`.
fn type_reference_relations(context: &RelationContext<'_>) -> Vec<Relation> {
    let mut relations = Vec::new();
    for file in context.files {
        let Some(parsed) = context.parsed.get(&file.relative_path) else {
            continue;
        };
        let Some(unit_ids) = context.unit_ids.get(&file.relative_path) else {
            continue;
        };
        let Some(file_id) = context.file_entities.get(&file.relative_path) else {
            continue;
        };
        // Same edge-identity rule as call_relations: (source, target).
        // Two units referencing one type are two edges; a unit spelling the
        // type twice is one.
        let mut emitted = HashSet::new();
        for (index, unit) in parsed.units.iter().enumerate() {
            let source_id = unit_ids.get(index).unwrap_or(file_id);
            for name in &unit.type_references {
                let Some((target, candidates)) =
                    resolve_type_reference(name, &file.relative_path, context)
                else {
                    continue;
                };
                if target.entity_id == *source_id
                    || !emitted.insert((source_id.clone(), target.entity_id.clone()))
                {
                    continue;
                }
                let evidence = SourceAddress::new(
                    context.repository_id,
                    context.snapshot_id,
                    &file.relative_path,
                    unit.start_byte as u64..unit.end_byte as u64,
                    unit.start_line..=unit.end_line,
                )
                .ok();
                let mut attributes =
                    serde_json::Map::from_iter([("typeName".to_owned(), name.clone().into())]);
                mark_ambiguity(&mut attributes, candidates);
                relations.push(Relation {
                    id: relation_id(source_id, &target.entity_id, "references"),
                    source_entity_id: source_id.clone(),
                    target_entity_id: target.entity_id.clone(),
                    kind: RelationKind::References,
                    origin: RelationOrigin::TreeSitter,
                    confidence: resolved_confidence(0.55, candidates),
                    snapshot_id: context.snapshot_id.to_owned(),
                    extractor: format!(
                        "cce-type-ref-v2:{}",
                        parsed.parser.as_deref().unwrap_or("unknown")
                    ),
                    evidence: evidence.into_iter().collect(),
                    attributes,
                });
            }
        }
    }
    relations
}

/// A type spelling only resolves to type-level entities: a function named
/// `Token` must not win over a struct in another file. Same-file candidates
/// win first, mirroring call resolution. Returns the picked candidate plus
/// the number of viable (type-kind) candidates it was chosen among.
fn resolve_type_reference<'a>(
    name: &str,
    path: &str,
    context: &'a RelationContext<'_>,
) -> Option<(&'a SymbolCandidate, usize)> {
    let candidates = context.name_index.get(name)?;
    let viable = candidates
        .iter()
        .filter(|candidate| is_type_kind(&candidate.kind))
        .count();
    let picked = candidates
        .iter()
        .find(|candidate| candidate.path == path && is_type_kind(&candidate.kind))
        .or_else(|| {
            candidates
                .iter()
                .find(|candidate| is_type_kind(&candidate.kind))
        })?;
    Some((picked, viable))
}

const fn is_type_kind(kind: &EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Struct
            | EntityKind::Class
            | EntityKind::Interface
            | EntityKind::Trait
            | EntityKind::Enum
            | EntityKind::Schema
    )
}

// --- git co-change ----------------------------------------------------------

/// `ChangedWith` edges between file entities that keep landing in the same
/// commits. History is evidence about coupling rather than source truth, so
/// confidence is `co_changes / appearances` capped at 0.7 and the top-8
/// partners per file bound hub explosions.
fn changed_with_relations(context: &RelationContext<'_>) -> Vec<Relation> {
    const MAX_COMMITS: usize = 256;
    const MAX_FILES_PER_COMMIT: usize = 512;
    const MAX_PARTNERS: usize = 8;
    let mut appearances: HashMap<String, usize> = HashMap::new();
    let mut co_changes: HashMap<(String, String), usize> = HashMap::new();
    // The shared walk covers more commits than this pass needs; the cap
    // here keeps the co-change window unchanged.
    for commit in context.touched.iter().take(MAX_COMMITS) {
        let mut paths = commit
            .paths
            .iter()
            .filter(|path| context.file_entities.contains_key(path.as_str()))
            .cloned()
            .collect::<Vec<String>>();
        paths.sort_unstable();
        paths.dedup();
        // Mass commits (bulk renames, reformats) are noise for pair counting.
        if paths.len() > MAX_FILES_PER_COMMIT {
            continue;
        }
        for path in &paths {
            *appearances.entry(path.clone()).or_default() += 1;
        }
        for (index, left) in paths.iter().enumerate() {
            for right in paths.iter().skip(index + 1) {
                *co_changes.entry((left.clone(), right.clone())).or_default() += 1;
            }
        }
    }
    let mut partners: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
    for ((left, right), count) in &co_changes {
        partners
            .entry(left.as_str())
            .or_default()
            .push((right.as_str(), *count));
        partners
            .entry(right.as_str())
            .or_default()
            .push((left.as_str(), *count));
    }
    let mut keep = HashSet::new();
    for (path, neighbours) in &mut partners {
        neighbours.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        for (other, _) in neighbours.iter().take(MAX_PARTNERS) {
            keep.insert(if *path < *other {
                (*path, *other)
            } else {
                (*other, *path)
            });
        }
    }
    let mut relations = Vec::new();
    for ((left, right), count) in &co_changes {
        if !keep.contains(&(left.as_str(), right.as_str())) {
            continue;
        }
        let (Some(left_id), Some(right_id)) = (
            context.file_entities.get(left),
            context.file_entities.get(right),
        ) else {
            continue;
        };
        let appearances_of_pair = appearances
            .get(left)
            .copied()
            .unwrap_or(0)
            .max(appearances.get(right).copied().unwrap_or(0))
            .max(1);
        let confidence = (*count as f32 / appearances_of_pair as f32).min(0.7);
        // Symmetric edge stored once, with the smaller id as source.
        let (source, target) = if left_id <= right_id {
            (left_id, right_id)
        } else {
            (right_id, left_id)
        };
        relations.push(Relation {
            id: relation_id(source, target, "changed_with"),
            source_entity_id: source.clone(),
            target_entity_id: target.clone(),
            kind: RelationKind::ChangedWith,
            origin: RelationOrigin::FrameworkRule,
            confidence,
            snapshot_id: context.snapshot_id.to_owned(),
            extractor: "cce-git-cochange-v1".to_owned(),
            evidence: Vec::new(),
            attributes: serde_json::Map::from_iter([
                ("coChangeCount".to_owned(), (*count).into()),
                ("commitAppearances".to_owned(), appearances_of_pair.into()),
            ]),
        });
    }
    relations
}

// --- tests -----------------------------------------------------------------

/// `Tests` edges: file-level pairings (`foo.test.ts` → `foo.ts`,
/// `test_foo.py` → `foo.py`, `tests/x.rs` → `x.rs` by basename) and
/// symbol-level `test_X` → `X` links.
fn test_relations(context: &RelationContext<'_>) -> Vec<Relation> {
    let mut relations = Vec::new();
    let mut emitted = HashSet::new();
    let by_basename = context
        .file_entities
        .keys()
        .map(|path| (basename(path).to_owned(), path.clone()))
        .collect::<HashMap<_, _>>();
    for file in context.files {
        let Some(test_id) = context.file_entities.get(&file.relative_path) else {
            continue;
        };
        for target in test_target_paths(&file.relative_path) {
            if let Some(target_id) = context.file_entities.get(&target) {
                if emitted.insert((test_id.clone(), target_id.clone())) {
                    relations.push(test_relation(
                        context,
                        test_id,
                        target_id,
                        0.7,
                        "cce-test-file-pair-v1",
                    ));
                }
            }
        }
        // `tests/foo.rs` convention: link by basename to a non-test file.
        if is_test_path(&file.relative_path) {
            if let Some(target) = by_basename.get(basename(&file.relative_path)) {
                if let Some(target_id) = context.file_entities.get(target) {
                    if target_id != test_id && emitted.insert((test_id.clone(), target_id.clone()))
                    {
                        relations.push(test_relation(
                            context,
                            test_id,
                            target_id,
                            0.6,
                            "cce-test-dir-v1",
                        ));
                    }
                }
            }
        }
        // Symbol-level: `test_foo` / `foo_test` / `testFoo` inside this file
        // naming a symbol that exists.
        let Some(parsed) = context.parsed.get(&file.relative_path) else {
            continue;
        };
        let Some(unit_ids) = context.unit_ids.get(&file.relative_path) else {
            continue;
        };
        for (index, unit) in parsed.units.iter().enumerate() {
            let Some(stripped) = strip_test_affix(&unit.name) else {
                continue;
            };
            // Exact name match first; then `test_<symbol>_<suffix>`
            // conventions (`test_helper_works` → `helper`) at lower
            // confidence, preferring the longest matching symbol name and
            // same-file candidates.
            let mut best: Option<(&SymbolCandidate, usize, bool, bool)> = None;
            for (name, candidates) in context.name_index {
                let exact = stripped == name.as_str();
                let prefix =
                    !exact && name.len() >= 3 && stripped.starts_with(format!("{name}_").as_str());
                if !exact && !prefix {
                    continue;
                }
                for candidate in candidates {
                    let same_file = candidate.path == file.relative_path;
                    let key = (name.len(), same_file, exact);
                    if best.is_none_or(|(_, len, file_flag, exact_flag)| {
                        (key.0, key.1, key.2) > (len, file_flag, exact_flag)
                    }) {
                        best = Some((candidate, name.len(), same_file, exact));
                    }
                }
            }
            let Some((target, _, _, exact)) = best else {
                continue;
            };
            let Some(test_unit_id) = unit_ids.get(index) else {
                continue;
            };
            if emitted.insert((test_unit_id.clone(), target.entity_id.clone())) {
                relations.push(test_relation(
                    context,
                    test_unit_id,
                    &target.entity_id,
                    if exact { 0.65 } else { 0.55 },
                    "cce-test-name-v1",
                ));
            }
        }
    }
    relations
}

fn test_relation(
    context: &RelationContext<'_>,
    test_id: &str,
    target_id: &str,
    confidence: f32,
    extractor: &str,
) -> Relation {
    Relation {
        id: relation_id(test_id, target_id, "tests"),
        source_entity_id: test_id.to_owned(),
        target_entity_id: target_id.to_owned(),
        kind: RelationKind::Tests,
        origin: RelationOrigin::FrameworkRule,
        confidence,
        snapshot_id: context.snapshot_id.to_owned(),
        extractor: extractor.to_owned(),
        evidence: Vec::new(),
        attributes: serde_json::Map::new(),
    }
}

/// Candidate source paths for a test file's target.
fn test_target_paths(path: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let (dir, name) = path.rsplit_once('/').map_or_else(
        || (String::new(), path),
        |(directory, name)| (format!("{directory}/"), name),
    );
    let stem = name.split('.').next().unwrap_or(name);
    for (suffix, replacements) in [
        (".test", &["ts", "tsx", "js", "jsx"][..]),
        (".spec", &["ts", "tsx", "js", "jsx"][..]),
    ] {
        if let Some(base) = stem.strip_suffix(suffix) {
            for extension in replacements {
                targets.push(format!("{dir}{base}.{extension}"));
            }
        }
    }
    if let Some(base) = stem.strip_prefix("test_") {
        targets.push(format!("{dir}{base}.py"));
        targets.push(format!("{dir}{base}.rs"));
    }
    if let Some(base) = stem.strip_suffix("_test") {
        targets.push(format!("{dir}{base}.go"));
        targets.push(format!("{dir}{base}.rs"));
    }
    targets
}

fn is_test_path(path: &str) -> bool {
    path.split('/')
        .any(|component| matches!(component, "tests" | "test" | "__tests__" | "spec"))
}

fn strip_test_affix(name: &str) -> Option<&str> {
    name.strip_prefix("test_")
        .or_else(|| name.strip_suffix("_test"))
        .or_else(|| name.strip_prefix("test"))
        .filter(|stripped| !stripped.is_empty() && stripped.chars().any(char::is_alphanumeric))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn relation_id(source: &str, target: &str, kind: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for component in [source, target, kind] {
        hasher.update(component.as_bytes());
        hasher.update(&[0]);
    }
    format!("rel_{}", &hasher.finalize().to_hex()[..32])
}
