//! Framework landmarks: deterministic route bindings recognized from web
//! framework call shapes. The axum detector matches literal
//! `.route(<path>, <method>(<handler>))` calls; emitted edges carry
//! `FrameworkRule` provenance at 0.85 — above raw syntax facts, below
//! compiler-resolved truth. New frameworks add a detector fn here with a
//! versioned extractor id (`cce-landmark-<fw>-vN`).

use std::collections::HashMap;

use cce_core::{CodeEntity, EntityKind, Relation, RelationKind, RelationOrigin, SourceAddress};

use crate::engine::{entity_id as engine_entity_id, relation_id as engine_relation_id};
use crate::relations::SymbolCandidate;
use crate::repository::ScannedFile;

const AXUM_EXTRACTOR: &str = "cce-landmark-axum-v1";
const ROUTE_CONFIDENCE: f32 = 0.85;

/// One recognized route binding: the `.route(...)` call-site span plus
/// whatever handler token could be lifted out of the method-router
/// argument. A `None` handler still yields a `Route` entity — the route
/// itself is the deterministic fact.
pub(crate) struct RouteBinding {
    pub route_path: String,
    pub method: String,
    pub handler_name: Option<String>,
    pub file_path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: u32,
    pub end_line: u32,
}

/// Scan Rust sources for axum `.route(path, get(handler))` bindings. Only
/// literal path arguments and the eight method-router functions count;
/// `route_service`, merged routers, and computed paths are not route facts.
pub(crate) fn axum_routes(
    files: &[ScannedFile],
    texts: &HashMap<String, String>,
) -> Vec<RouteBinding> {
    let Ok(pattern) = regex::Regex::new(
        r#"\.route\(\s*"([^"]+)"\s*,\s*(get|post|put|delete|patch|head|options|trace)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)?"#,
    ) else {
        return Vec::new();
    };
    let mut bindings = Vec::new();
    for file in files {
        if file.language.as_deref() != Some("rust") {
            continue;
        }
        let Some(text) = texts.get(&file.relative_path) else {
            continue;
        };
        for capture in pattern.captures_iter(text) {
            let (Some(whole), Some(path), Some(method)) =
                (capture.get(0), capture.get(1), capture.get(2))
            else {
                continue;
            };
            // `post(handlers::search)` resolves by simple name, matching the
            // name_index key shape; a bare identifier is its own last segment.
            let handler_name = capture.get(3).map(|handler| {
                handler
                    .as_str()
                    .rsplit("::")
                    .next()
                    .unwrap_or_else(|| handler.as_str())
                    .to_owned()
            });
            bindings.push(RouteBinding {
                route_path: path.as_str().to_owned(),
                method: method.as_str().to_owned(),
                handler_name,
                file_path: file.relative_path.clone(),
                start_byte: whole.start(),
                end_byte: whole.end(),
                start_line: line_for_offset(text, whole.start()),
                end_line: line_for_offset(text, whole.end()),
            });
        }
    }
    bindings
}

pub(crate) struct LandmarkEmission {
    pub entities: Vec<CodeEntity>,
    pub relations: Vec<Relation>,
}

/// Turn route bindings into `Route` entities addressed by the call site,
/// plus `RouteHandledBy` edges when the handler token resolves to a
/// Function/Method symbol (same file first, then global — mirroring call
/// resolution). Unresolved handlers still emit the `Route` entity alone.
pub(crate) fn emit(
    bindings: &[RouteBinding],
    repository_id: &str,
    snapshot_id: &str,
    file_regions: &HashMap<String, String>,
    name_index: &HashMap<String, Vec<SymbolCandidate>>,
) -> LandmarkEmission {
    let mut entities = Vec::new();
    let mut relations = Vec::new();
    for binding in bindings {
        let address = SourceAddress::new(
            repository_id,
            snapshot_id,
            &binding.file_path,
            binding.start_byte as u64..binding.end_byte as u64,
            binding.start_line..=binding.end_line,
        )
        .ok();
        let route_name = format!("{} {}", binding.method.to_uppercase(), binding.route_path);
        let entity_id = engine_entity_id(
            repository_id,
            &binding.file_path,
            binding.start_byte,
            binding.end_byte,
            &format!("route:{route_name}"),
        );
        let attributes = serde_json::Map::from_iter([
            ("framework".to_owned(), "axum".into()),
            ("route".to_owned(), binding.route_path.clone().into()),
            ("method".to_owned(), binding.method.clone().into()),
        ]);
        entities.push(CodeEntity {
            id: entity_id.clone(),
            kind: EntityKind::Route,
            name: route_name.clone(),
            qualified_name: Some(format!("axum:{route_name}")),
            signature: None,
            language: Some("rust".to_owned()),
            region_id: file_regions.get(&binding.file_path).cloned(),
            address: address.clone(),
            capabilities: vec!["route_entrypoint".to_owned()],
            attributes: attributes.clone(),
        });
        let Some(handler_name) = binding.handler_name.as_deref() else {
            continue;
        };
        let Some(handler) = resolve_handler(handler_name, &binding.file_path, name_index) else {
            continue;
        };
        relations.push(Relation {
            id: engine_relation_id(&entity_id, &handler.entity_id, "route_handled_by"),
            source_entity_id: entity_id,
            target_entity_id: handler.entity_id.clone(),
            kind: RelationKind::RouteHandledBy,
            origin: RelationOrigin::FrameworkRule,
            confidence: ROUTE_CONFIDENCE,
            snapshot_id: snapshot_id.to_owned(),
            extractor: AXUM_EXTRACTOR.to_owned(),
            evidence: address
                .map(|site| site.with_symbol(handler.entity_id.clone()))
                .into_iter()
                .collect(),
            attributes,
        });
    }
    LandmarkEmission {
        entities,
        relations,
    }
}

/// Handler resolution prefers callable kinds in the same file, then
/// callables anywhere, then any same-file spelling match, then the first
/// candidate — the same shape as call-site resolution in `relations.rs`.
fn resolve_handler<'a>(
    name: &str,
    path: &str,
    name_index: &'a HashMap<String, Vec<SymbolCandidate>>,
) -> Option<&'a SymbolCandidate> {
    let candidates = name_index.get(name)?;
    let callable = |candidate: &&SymbolCandidate| {
        matches!(candidate.kind, EntityKind::Function | EntityKind::Method)
    };
    candidates
        .iter()
        .find(|candidate| candidate.path == path && callable(candidate))
        .or_else(|| candidates.iter().find(callable))
        .or_else(|| candidates.iter().find(|candidate| candidate.path == path))
        .or_else(|| candidates.first())
}

fn line_for_offset(text: &str, offset: usize) -> u32 {
    text.as_bytes()
        .get(..offset.min(text.len()))
        .unwrap_or(text.as_bytes())
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn rust_file(path: &str) -> ScannedFile {
        ScannedFile {
            relative_path: path.to_owned(),
            absolute_path: PathBuf::from(format!("/repo/{path}")),
            language: Some("rust".to_owned()),
            content_hash: String::new(),
            size_bytes: 0,
            mtime_ms: 0,
            line_count: 0,
            cached_bytes: None,
        }
    }

    fn texts(source: &str) -> HashMap<String, String> {
        HashMap::from_iter([("src/main.rs".to_owned(), source.to_owned())])
    }

    fn file_regions() -> HashMap<String, String> {
        HashMap::from_iter([("src/main.rs".to_owned(), "region_file".to_owned())])
    }

    fn candidate(id: &str, path: &str, kind: EntityKind) -> SymbolCandidate {
        SymbolCandidate {
            entity_id: id.to_owned(),
            path: path.to_owned(),
            kind,
        }
    }

    #[test]
    fn emits_route_entity_and_handler_edge() {
        let source = r#"use axum::{routing::post, Router};

async fn search() -> &'static str {
    "ok"
}

fn router() -> Router {
    Router::new().route("/v1/search", post(search))
}
"#;
        let files = vec![rust_file("src/main.rs")];
        let bindings = axum_routes(&files, &texts(source));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].route_path, "/v1/search");
        assert_eq!(bindings[0].method, "post");
        assert_eq!(bindings[0].handler_name.as_deref(), Some("search"));
        assert_eq!(bindings[0].start_line, 8);

        let name_index = HashMap::from_iter([(
            "search".to_owned(),
            vec![candidate("ent_search", "src/main.rs", EntityKind::Function)],
        )]);
        let emission = emit(&bindings, "repo", "snap", &file_regions(), &name_index);

        assert_eq!(emission.entities.len(), 1);
        let route = &emission.entities[0];
        assert_eq!(route.kind, EntityKind::Route);
        assert_eq!(route.name, "POST /v1/search");
        assert_eq!(route.region_id.as_deref(), Some("region_file"));
        assert!(route.address.is_some());

        assert_eq!(emission.relations.len(), 1);
        let edge = &emission.relations[0];
        assert_eq!(edge.kind, RelationKind::RouteHandledBy);
        assert_eq!(edge.origin, RelationOrigin::FrameworkRule);
        assert!((edge.confidence - ROUTE_CONFIDENCE).abs() < f32::EPSILON);
        assert_eq!(edge.extractor, AXUM_EXTRACTOR);
        assert_eq!(edge.source_entity_id, route.id);
        assert_eq!(edge.target_entity_id, "ent_search");
        assert_eq!(edge.evidence.len(), 1);
        assert_eq!(edge.evidence[0].path, "src/main.rs");
        assert_eq!(edge.evidence[0].symbol_id.as_deref(), Some("ent_search"));
        assert_eq!(edge.attributes["framework"], "axum");
        assert_eq!(edge.attributes["route"], "/v1/search");
        assert_eq!(edge.attributes["method"], "post");
    }

    #[test]
    fn non_identifier_handler_still_emits_route() {
        let source = r#"fn router() -> Router {
    Router::new().route("/v1/search", post(|_| async { "ok" }))
}
"#;
        let files = vec![rust_file("src/main.rs")];
        let bindings = axum_routes(&files, &texts(source));
        assert_eq!(bindings.len(), 1);
        assert!(bindings[0].handler_name.is_none());

        let emission = emit(&bindings, "repo", "snap", &file_regions(), &HashMap::new());
        assert_eq!(emission.entities.len(), 1);
        assert_eq!(emission.entities[0].kind, EntityKind::Route);
        assert!(emission.relations.is_empty());
    }

    #[test]
    fn unresolvable_handler_emits_route_without_edge() {
        let source = r#"fn router() -> Router {
    Router::new().route("/v1/search", post(search))
}
"#;
        let files = vec![rust_file("src/main.rs")];
        let bindings = axum_routes(&files, &texts(source));
        assert_eq!(bindings[0].handler_name.as_deref(), Some("search"));

        let emission = emit(&bindings, "repo", "snap", &file_regions(), &HashMap::new());
        assert_eq!(emission.entities.len(), 1);
        assert!(emission.relations.is_empty());
    }

    #[test]
    fn same_file_handler_wins_over_global_candidates() {
        let source = r#"fn router() -> Router {
    Router::new().route("/v1/search", post(search))
}
"#;
        let files = vec![rust_file("src/main.rs")];
        let bindings = axum_routes(&files, &texts(source));
        // Foreign candidate listed first: resolution must still prefer the
        // same-file Function.
        let name_index = HashMap::from_iter([(
            "search".to_owned(),
            vec![
                candidate("ent_other", "src/other.rs", EntityKind::Function),
                candidate("ent_local", "src/main.rs", EntityKind::Function),
            ],
        )]);
        let emission = emit(&bindings, "repo", "snap", &file_regions(), &name_index);
        assert_eq!(emission.relations.len(), 1);
        assert_eq!(emission.relations[0].target_entity_id, "ent_local");

        // A same-file non-callable loses to a callable in another file.
        let name_index = HashMap::from_iter([(
            "search".to_owned(),
            vec![
                candidate("ent_struct", "src/main.rs", EntityKind::Struct),
                candidate("ent_fn", "src/other.rs", EntityKind::Function),
            ],
        )]);
        let emission = emit(&bindings, "repo", "snap", &file_regions(), &name_index);
        assert_eq!(emission.relations[0].target_entity_id, "ent_fn");
    }
}
