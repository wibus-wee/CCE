//! L2 package architecture extracted from build manifests — Cargo workspaces,
//! `package.json` workspaces, `pnpm-workspace.yaml`. These are deterministic
//! boundaries (the build system defines them), so package entities and
//! `BuildDependsOn`/`Contains` edges carry `BuildSystem` provenance at full
//! confidence. Component-level inference above this layer must be marked as
//! candidate, never as fact.

use std::collections::HashMap;

use cce_core::{CodeEntity, EntityKind, Relation, RelationKind, RelationOrigin, SourceAddress};

use crate::engine::{entity_id as engine_entity_id, relation_id as engine_relation_id};
use crate::repository::ScannedFile;

pub(crate) struct PackageInfo {
    pub name: String,
    pub manifest_path: String,
    pub root_dir: String,
    pub ecosystem: &'static str,
    pub dependencies: Vec<String>,
}

/// Discover packages from build manifests. Every manifest that declares a
/// package name is a package; workspace membership files only widen the
/// candidate set for dependency resolution, not the boundary itself.
pub(crate) fn extract(files: &[ScannedFile], texts: &HashMap<String, String>) -> Vec<PackageInfo> {
    let mut packages = Vec::new();
    for file in files {
        let path = file.relative_path.as_str();
        let Some(text) = texts.get(path) else {
            continue;
        };
        if path.ends_with("Cargo.toml") {
            if let Some(package) = cargo_package(path, text) {
                packages.push(package);
            }
        } else if path.ends_with("package.json")
            && !path.contains("/node_modules/")
            && let Some(package) = npm_package(path, text)
        {
            packages.push(package);
        }
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    packages
}

fn cargo_package(manifest_path: &str, text: &str) -> Option<PackageInfo> {
    let parsed = text.parse::<toml::Table>().ok()?;
    let package = parsed.get("package")?.as_table()?;
    let name = package.get("name")?.as_str()?.to_owned();
    let mut dependencies = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = parsed.get(section).and_then(toml::Value::as_table) {
            dependencies.extend(table.keys().cloned());
        }
    }
    // `dep.workspace = true` inside [workspace.dependencies] names deps too,
    // but member manifests re-declare them; workspace-level deps of the root
    // package are already covered above.
    Some(PackageInfo {
        name,
        manifest_path: manifest_path.to_owned(),
        root_dir: manifest_path
            .rsplit_once('/')
            .map_or(String::new(), |(dir, _)| dir.to_owned()),
        ecosystem: "cargo",
        dependencies,
    })
}

fn npm_package(manifest_path: &str, text: &str) -> Option<PackageInfo> {
    let parsed = text.parse::<serde_json::Value>().ok()?;
    let name = parsed.get("name")?.as_str()?.to_owned();
    let mut dependencies = Vec::new();
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(table) = parsed.get(section).and_then(serde_json::Value::as_object) {
            dependencies.extend(table.keys().cloned());
        }
    }
    Some(PackageInfo {
        name,
        manifest_path: manifest_path.to_owned(),
        root_dir: manifest_path
            .rsplit_once('/')
            .map_or(String::new(), |(dir, _)| dir.to_owned()),
        ecosystem: "npm",
        dependencies,
    })
}

pub(crate) struct PackageEmission {
    pub entities: Vec<CodeEntity>,
    pub relations: Vec<Relation>,
}

/// Turn discovered packages into graph entities and edges:
/// - one `Package` entity per manifest, addressed by the manifest file region;
/// - `Contains` package → file for every file under the package root;
/// - `BuildDependsOn` package → package where a declared dependency name
///   matches another workspace package.
pub(crate) fn emit(
    packages: &[PackageInfo],
    repository_id: &str,
    snapshot_id: &str,
    files: &[ScannedFile],
    file_entities: &HashMap<String, String>,
    file_regions: &HashMap<String, String>,
) -> PackageEmission {
    let by_name: HashMap<&str, &PackageInfo> = packages
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect();
    let mut entities = Vec::new();
    let mut relations = Vec::new();
    for package in packages {
        let entity_id = engine_entity_id(
            repository_id,
            &package.manifest_path,
            0,
            0,
            &format!("package:{}", package.name),
        );
        let region_id = file_regions.get(&package.manifest_path).cloned();
        let address = file_entities.get(&package.manifest_path).and_then(|_| {
            SourceAddress::new(
                repository_id,
                snapshot_id,
                &package.manifest_path,
                0..0,
                1..=1,
            )
            .ok()
        });
        let mut attributes = serde_json::Map::new();
        attributes.insert("ecosystem".to_owned(), package.ecosystem.to_owned().into());
        attributes.insert("rootDir".to_owned(), package.root_dir.clone().into());
        entities.push(CodeEntity {
            id: entity_id.clone(),
            kind: EntityKind::Package,
            name: package.name.clone(),
            qualified_name: Some(format!("{}:{}", package.ecosystem, package.name)),
            signature: None,
            language: None,
            region_id,
            address,
            capabilities: vec!["build_boundary".to_owned()],
            attributes,
        });
        for dependency in &package.dependencies {
            let Some(target) = by_name.get(dependency.as_str()) else {
                continue; // external dependency — not a workspace edge
            };
            let target_id = engine_entity_id(
                repository_id,
                &target.manifest_path,
                0,
                0,
                &format!("package:{}", target.name),
            );
            relations.push(Relation {
                id: engine_relation_id(&entity_id, &target_id, "build_depends_on"),
                source_entity_id: entity_id.clone(),
                target_entity_id: target_id,
                kind: RelationKind::BuildDependsOn,
                origin: RelationOrigin::BuildSystem,
                confidence: 1.0,
                snapshot_id: snapshot_id.to_owned(),
                extractor: format!("{}-manifest", package.ecosystem),
                evidence: Vec::new(),
                attributes: serde_json::Map::new(),
            });
        }
        for file in files {
            // Membership goes to the most specific package root so the root
            // package doesn't swallow files owned by member crates.
            let owner = packages
                .iter()
                .filter(|candidate| {
                    file.relative_path
                        .starts_with(&package_root_prefix(&candidate.root_dir))
                })
                .max_by_key(|candidate| candidate.root_dir.len());
            match owner {
                Some(owner) if std::ptr::eq(owner, package) => {}
                _ => continue,
            }
            let Some(file_entity) = file_entities.get(&file.relative_path) else {
                continue;
            };
            relations.push(Relation {
                id: engine_relation_id(&entity_id, file_entity, "contains"),
                source_entity_id: entity_id.clone(),
                target_entity_id: file_entity.clone(),
                kind: RelationKind::Contains,
                origin: RelationOrigin::BuildSystem,
                confidence: 1.0,
                snapshot_id: snapshot_id.to_owned(),
                extractor: format!("{}-manifest", package.ecosystem),
                evidence: Vec::new(),
                attributes: serde_json::Map::new(),
            });
        }
    }
    PackageEmission {
        entities,
        relations,
    }
}

fn package_root_prefix(root_dir: &str) -> String {
    if root_dir.is_empty() {
        // Root package contains every file; other packages still get their
        // own prefix match first in queries, but Contains stays simple here.
        String::new()
    } else {
        format!("{root_dir}/")
    }
}
