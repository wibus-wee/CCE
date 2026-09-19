//! Architecture Atlas APIs: deterministic package-level views over the
//! persisted world model. Everything here is derived from `BuildSystem`
//! provenance facts plus the typed relation graph — no inference is
//! presented as truth. Component-level inference above packages is a
//! later layer and must be marked as candidate.

use std::collections::HashMap;
use utoipa::ToSchema;

use cce_core::{EntityKind, RelationKind, Result};
use cce_store::RelationDirection;
use serde::Serialize;

use crate::CceEngine;
use crate::repository::RepositoryScanner;

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// One build-level package (crate/npm package) in the map.
pub struct PackageNode {
    /// Package name from its manifest.
    pub name: String,
    /// Ecosystem tag (`cargo`, `npm`, …).
    pub ecosystem: String,
    /// Path to the manifest file.
    pub manifest_path: String,
    /// Package root directory.
    pub root_dir: String,
    /// Files contained by this package.
    pub member_files: usize,
    /// Packages this one depends on.
    pub dependencies: Vec<String>,
    /// Packages depending on this one.
    pub dependents: Vec<String>,
}

/// Violation list cap — the count field still reports the full total.
const VIOLATION_LIST_CAP: usize = 256;

/// Serde label for a kind/origin (`calls`, `tree_sitter`) with a Debug
/// fallback — serde naming stays the single owner of the wire format.
fn serde_label(value: &(impl Serialize + std::fmt::Debug)) -> String {
    serde_json::to_value(value).map_or_else(
        |_| format!("{value:?}"),
        |json| {
            json.as_str()
                .map_or_else(|| format!("{value:?}"), str::to_owned)
        },
    )
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// One observed code edge crossing a build boundary without a declared
/// package dependency — deterministic evidence, not a rule judgement.
pub struct BoundaryViolation {
    /// Package the edge starts in.
    pub source_package: String,
    /// Package the edge lands in — not a declared dependency of the source.
    pub target_package: String,
    /// Edge kind (`imports`, `calls`, `references`, …).
    pub kind: String,
    /// Source entity name.
    pub source_entity: String,
    /// Target entity name.
    pub target_entity: String,
    /// Where the edge occurs — first evidence address, else the source
    /// entity's own path.
    pub evidence_path: String,
    /// Derivation origin of the edge (`tree_sitter`, `scip`, …).
    pub origin: String,
    /// Edge confidence — syntax-derived edges are < 1.
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Package-level architecture map for a snapshot.
pub struct CodebaseMap {
    /// Snapshot the map was read from.
    pub snapshot_id: String,
    /// All packages found.
    pub packages: Vec<PackageNode>,
    /// Number of `BuildDependsOn` edges between packages.
    pub dependency_edges: usize,
    /// Code edges crossing into a package that declares no dependency on
    /// it — capped at `VIOLATION_LIST_CAP`; `violation_count` is the total.
    pub violations: Vec<BoundaryViolation>,
    /// Total undeclared-boundary edges observed (>= `violations.len()`).
    pub violation_count: usize,
    /// How the map was produced.
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Explanation of one named component: members, dependencies, dependents,
/// and tests — all from persisted relations.
pub struct ComponentExplanation {
    /// Component name.
    pub name: String,
    /// Entity kind of the component.
    pub kind: String,
    /// Qualified name when known.
    pub qualified_name: Option<String>,
    /// Source address of the component.
    pub address: Option<cce_core::SourceAddress>,
    /// Files contained by the component.
    pub member_files: Vec<String>,
    /// What the component depends on.
    pub dependencies: Vec<String>,
    /// What depends on the component.
    pub dependents: Vec<String>,
    /// Tests covering the component.
    pub tests: Vec<String>,
    /// Provenance note for the explanation.
    pub provenance: String,
}

/// One resolved definition location for `definitions`/`references`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionHit {
    /// Symbol name.
    pub name: String,
    /// Entity kind of the definition.
    pub kind: String,
    /// Qualified name when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    /// Definition language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Definition site address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<cce_core::SourceAddress>,
}

impl From<&cce_core::CodeEntity> for DefinitionHit {
    fn from(entity: &cce_core::CodeEntity) -> Self {
        Self {
            name: entity.name.clone(),
            kind: format!("{:?}", entity.kind),
            qualified_name: entity.qualified_name.clone(),
            language: entity.language.clone(),
            address: entity.address.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Result of a `def` query: all entities matching the name.
pub struct DefinitionsReport {
    /// The queried name.
    pub query: String,
    /// Snapshot the lookup ran against.
    pub snapshot_id: String,
    /// Resolved definition sites.
    pub definitions: Vec<DefinitionHit>,
}

/// One inbound edge to a target entity.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceHit {
    /// Name of the referencing entity.
    pub from_name: String,
    /// Qualified name of the referencing entity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_qualified_name: Option<String>,
    /// Entity kind of the referencing entity.
    pub from_kind: String,
    /// Edge kind (`references`, `calls`, `implements`, `type_uses`).
    pub via: String,
    /// Provenance (`scip`, `tree_sitter`, …) — compiler truth vs syntax guess.
    pub origin: String,
    /// Edge confidence (1.0 for compiler-derived, <1 for syntax).
    pub confidence: f32,
    /// Source addresses where the reference occurs.
    pub evidence: Vec<cce_core::SourceAddress>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Result of a `refs` query: resolved targets plus inbound edges.
pub struct ReferencesReport {
    /// The queried name.
    pub query: String,
    /// Snapshot the lookup ran against.
    pub snapshot_id: String,
    /// Entities the name resolved to.
    pub targets: Vec<DefinitionHit>,
    /// Inbound reference/call/implement edges.
    pub references: Vec<ReferenceHit>,
    /// Whether the edge cap truncated the list.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// Impact-analysis result: entities reaching the seed within two hops.
pub struct ImpactReport {
    /// The queried name.
    pub query: String,
    /// Snapshot the analysis ran against.
    pub snapshot_id: String,
    /// Seed entities the name resolved to.
    pub matched_entities: Vec<String>,
    /// Entities that reach the target through impact edges (callers,
    /// references, tests, implementations) within two hops.
    pub impacted: Vec<ImpactedEntity>,
    /// Edge kinds counted as impact edges.
    pub edge_kinds: Vec<String>,
    /// How the report was produced.
    pub provenance: String,
    /// Limitations the caller should know (heuristic edges, hop cap, …).
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
/// One entity impacted through the relation graph.
pub struct ImpactedEntity {
    /// Entity name.
    pub name: String,
    /// Entity kind.
    pub kind: String,
    /// Source path when known.
    pub path: Option<String>,
    /// Distance in edges from the seed.
    pub hops: usize,
    /// Edge kind connecting it.
    pub via: String,
    /// Confidence of the connecting edge.
    pub confidence: f32,
}

impl CceEngine {
    /// Package-level map of the repository: hard build boundaries and the
    /// declared dependency direction between them.
    pub fn codebase_map(&self) -> Result<CodebaseMap> {
        let snapshot_id = self.current_snapshot_id()?;
        let packages = self
            .store()
            .entities_by_kind(&snapshot_id, &EntityKind::Package)?;
        let edges =
            self.store()
                .relations_by_kind(&snapshot_id, &RelationKind::BuildDependsOn, 4096)?;
        let mut nodes: HashMap<String, PackageNode> = HashMap::new();
        let mut id_to_name: HashMap<String, String> = HashMap::new();
        for entity in &packages {
            id_to_name.insert(entity.id.clone(), entity.name.clone());
            let ecosystem = entity
                .attributes
                .get("ecosystem")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown")
                .to_owned();
            let root_dir = entity
                .attributes
                .get("rootDir")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned();
            let manifest_path = entity
                .address
                .as_ref()
                .map_or_else(|| entity.name.clone(), |address| address.path.clone());
            nodes.insert(
                entity.id.clone(),
                PackageNode {
                    name: entity.name.clone(),
                    ecosystem,
                    manifest_path,
                    root_dir,
                    member_files: 0,
                    dependencies: Vec::new(),
                    dependents: Vec::new(),
                },
            );
        }
        for edge in &edges {
            let Some(target_name) = id_to_name.get(&edge.target_entity_id) else {
                continue;
            };
            if let Some(node) = nodes.get_mut(&edge.source_entity_id) {
                node.dependencies.push(target_name.clone());
            }
            if let Some(source_name) = id_to_name.get(&edge.source_entity_id) {
                if let Some(node) = nodes.get_mut(&edge.target_entity_id) {
                    node.dependents.push(source_name.clone());
                }
            }
        }
        // Member files come from package Contains edges.
        for entity in &packages {
            let contains = self.store().relations_for_entity(
                &snapshot_id,
                &entity.id,
                RelationDirection::Outgoing,
                4096,
            )?;
            if let Some(node) = nodes.get_mut(&entity.id) {
                node.member_files = contains
                    .iter()
                    .filter(|relation| relation.kind == RelationKind::Contains)
                    .count();
            }
        }
        let mut package_list: Vec<PackageNode> = nodes.into_values().collect();
        package_list.sort_by(|left, right| left.name.cmp(&right.name));
        for node in &mut package_list {
            node.dependencies.sort();
            node.dependents.sort();
        }
        let (violations, violation_count) =
            self.boundary_violations(&snapshot_id, &package_list)?;
        Ok(CodebaseMap {
            snapshot_id,
            packages: package_list,
            dependency_edges: edges.len(),
            violations,
            violation_count,
            provenance: "build_system manifests + typed relation graph (deterministic); \
                 violations are code edges crossing undeclared package boundaries"
                .to_owned(),
        })
    }

    /// Cross-package usage edges (`imports`, `calls`, `references`, …)
    /// whose target package is not a declared `BuildDependsOn` dependency
    /// of the source package. Package membership resolves by longest
    /// `rootDir` prefix over entity paths; an edge with an unpackaged
    /// endpoint (external crate, unmapped path) cannot violate a boundary
    /// and is skipped.
    fn boundary_violations(
        &self,
        snapshot_id: &str,
        packages: &[PackageNode],
    ) -> Result<(Vec<BoundaryViolation>, usize)> {
        const KINDS: [RelationKind; 8] = [
            RelationKind::Imports,
            RelationKind::Exports,
            RelationKind::References,
            RelationKind::Implements,
            RelationKind::Extends,
            RelationKind::TypeUses,
            RelationKind::Instantiates,
            RelationKind::Calls,
        ];
        const EDGE_CAP: usize = 100_000;
        if packages.is_empty() {
            return Ok((Vec::new(), 0));
        }
        // Longest prefix first so nested packages win over a workspace-root
        // package (rootDir "" is the catch-all).
        let mut roots: Vec<(&str, &str)> = packages
            .iter()
            .map(|node| (node.root_dir.as_str(), node.name.as_str()))
            .collect();
        roots.sort_by_key(|(root, _)| std::cmp::Reverse(root.len()));
        let package_of = |path: &str| -> Option<&str> {
            roots
                .iter()
                .find(|(root, _)| {
                    root.is_empty()
                        || path == *root
                        || path
                            .strip_prefix(root)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                .map(|(_, name)| *name)
        };
        let mut declared: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
        for node in packages {
            declared.insert(
                node.name.as_str(),
                node.dependencies.iter().map(String::as_str).collect(),
            );
        }
        // One bulk entity load for endpoint resolution — path and name by id.
        let entities: HashMap<String, (String, Option<String>)> = self
            .store()
            .entities_for_snapshot(snapshot_id)?
            .into_iter()
            .map(|entity| {
                let path = entity.address.map(|address| address.path);
                (entity.id, (entity.name, path))
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        let mut violations = Vec::new();
        let mut total = 0_usize;
        for kind in &KINDS {
            for edge in self
                .store()
                .relations_by_kind(snapshot_id, kind, EDGE_CAP)?
            {
                let Some((source_name, source_path)) = entities.get(&edge.source_entity_id) else {
                    continue;
                };
                let Some((target_name, target_path)) = entities.get(&edge.target_entity_id) else {
                    continue;
                };
                let (Some(source_path), Some(target_path)) = (source_path, target_path) else {
                    continue;
                };
                let (Some(source_pkg), Some(target_pkg)) = (
                    package_of(source_path.as_str()),
                    package_of(target_path.as_str()),
                ) else {
                    continue;
                };
                if source_pkg == target_pkg
                    || declared
                        .get(source_pkg)
                        .is_some_and(|deps| deps.contains(target_pkg))
                {
                    continue;
                }
                total += 1;
                if !seen.insert((
                    edge.kind.clone(),
                    edge.source_entity_id.clone(),
                    edge.target_entity_id.clone(),
                )) {
                    continue;
                }
                if violations.len() < VIOLATION_LIST_CAP {
                    violations.push(BoundaryViolation {
                        source_package: source_pkg.to_owned(),
                        target_package: target_pkg.to_owned(),
                        kind: serde_label(&edge.kind),
                        source_entity: source_name.clone(),
                        target_entity: target_name.clone(),
                        evidence_path: edge
                            .evidence
                            .first()
                            .map_or_else(|| source_path.clone(), |address| address.path.clone()),
                        origin: serde_label(&edge.origin),
                        confidence: edge.confidence,
                    });
                }
            }
        }
        violations.sort_by(|left, right| {
            left.source_package
                .cmp(&right.source_package)
                .then_with(|| left.target_package.cmp(&right.target_package))
                .then_with(|| left.kind.cmp(&right.kind))
        });
        Ok((violations, total))
    }

    /// Explain one named component: package or symbol entity, its members,
    /// declared dependencies, dependents, and tests.
    pub fn explain_component(&self, name: &str) -> Result<ComponentExplanation> {
        let snapshot_id = self.current_snapshot_id()?;
        let entity = self
            .store()
            .entity_by_name(&snapshot_id, name, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                cce_core::CceError::Configuration(format!(
                    "no entity named `{name}` in the current snapshot"
                ))
            })?;
        let outgoing = self.store().relations_for_entity(
            &snapshot_id,
            &entity.id,
            RelationDirection::Outgoing,
            4096,
        )?;
        let incoming = self.store().relations_for_entity(
            &snapshot_id,
            &entity.id,
            RelationDirection::Incoming,
            4096,
        )?;
        let mut member_files = Vec::new();
        let mut dependencies = Vec::new();
        let mut dependents = Vec::new();
        let mut tests = Vec::new();
        for relation in &outgoing {
            match relation.kind {
                RelationKind::Contains => {
                    if let Some(target) = self
                        .store()
                        .entity_by_id(&snapshot_id, &relation.target_entity_id)?
                    {
                        member_files.push(target.name);
                    }
                }
                RelationKind::BuildDependsOn | RelationKind::Imports => {
                    if let Some(target) = self
                        .store()
                        .entity_by_id(&snapshot_id, &relation.target_entity_id)?
                    {
                        dependencies.push(target.name);
                    }
                }
                _ => {}
            }
        }
        for relation in &incoming {
            match relation.kind {
                RelationKind::BuildDependsOn | RelationKind::Imports | RelationKind::Calls => {
                    if let Some(source) = self
                        .store()
                        .entity_by_id(&snapshot_id, &relation.source_entity_id)?
                    {
                        dependents.push(source.name);
                    }
                }
                RelationKind::Tests => {
                    if let Some(source) = self
                        .store()
                        .entity_by_id(&snapshot_id, &relation.source_entity_id)?
                    {
                        tests.push(source.name);
                    }
                }
                _ => {}
            }
        }
        member_files.sort();
        member_files.dedup();
        dependencies.sort();
        dependencies.dedup();
        dependents.sort();
        dependents.dedup();
        tests.sort();
        tests.dedup();
        Ok(ComponentExplanation {
            name: entity.name,
            kind: format!("{:?}", entity.kind),
            qualified_name: entity.qualified_name,
            address: entity.address,
            member_files,
            dependencies,
            dependents,
            tests,
            provenance: "entities and typed relations persisted at index time".to_owned(),
        })
    }

    /// Blast radius of a symbol or file: everything reaching it through
    /// impact edges within two hops, cheapest evidence first.
    pub fn impact_analysis(&self, query: &str) -> Result<ImpactReport> {
        let snapshot_id = self.current_snapshot_id()?;
        let seeds = self.store().entity_by_name(&snapshot_id, query, 8)?;
        let mut impacted: HashMap<String, ImpactedEntity> = HashMap::new();
        let impact_kinds = [
            RelationKind::Calls,
            RelationKind::References,
            RelationKind::Tests,
            RelationKind::Implements,
            RelationKind::BuildDependsOn,
            RelationKind::RouteHandledBy,
        ];
        let mut caveats = Vec::new();
        if seeds.is_empty() {
            caveats.push(format!(
                "no entity matched `{query}`; impact analysis needs an indexed symbol or package name"
            ));
        }
        let mut frontier: Vec<(String, usize)> =
            seeds.iter().map(|entity| (entity.id.clone(), 0)).collect();
        let mut visited: HashMap<String, usize> = frontier.iter().cloned().collect();
        while let Some((entity_id, hops)) = frontier.pop() {
            if hops >= 2 {
                continue;
            }
            let edges = self.store().relations_for_entity(
                &snapshot_id,
                &entity_id,
                RelationDirection::Incoming,
                256,
            )?;
            for edge in edges {
                if !impact_kinds.contains(&edge.kind) {
                    continue;
                }
                let neighbor = edge.source_entity_id.clone();
                let entry_hops = visited.entry(neighbor.clone()).or_insert(hops + 1);
                if *entry_hops == hops + 1 {
                    frontier.push((neighbor.clone(), hops + 1));
                }
                if let Some(entity) = self.store().entity_by_id(&snapshot_id, &neighbor)? {
                    let entry =
                        impacted
                            .entry(neighbor.clone())
                            .or_insert_with(|| ImpactedEntity {
                                name: entity.name,
                                kind: format!("{:?}", entity.kind),
                                path: entity.address.map(|address| address.path),
                                hops: hops + 1,
                                via: format!("{:?}", edge.kind),
                                confidence: edge.confidence,
                            });
                    if hops + 1 < entry.hops {
                        entry.hops = hops + 1;
                        entry.via = format!("{:?}", edge.kind);
                        entry.confidence = edge.confidence;
                    }
                }
            }
        }
        let mut impacted_list: Vec<ImpactedEntity> = impacted.into_values().collect();
        impacted_list.sort_by(|left, right| {
            left.hops
                .cmp(&right.hops)
                .then_with(|| right.confidence.total_cmp(&left.confidence))
        });
        caveats.push(
            "call and reference edges are tree-sitter syntax facts (confidence < 1); \
             verify against a compiler or SCIP index for precision"
                .to_owned(),
        );
        Ok(ImpactReport {
            query: query.to_owned(),
            snapshot_id,
            matched_entities: seeds.iter().map(|entity| entity.name.clone()).collect(),
            impacted: impacted_list,
            edge_kinds: impact_kinds
                .iter()
                .map(|kind| format!("{kind:?}"))
                .collect(),
            provenance: "persisted typed relation graph".to_owned(),
            caveats,
        })
    }

    /// Resolve a name to its definition entities — `cce def`.
    pub fn definitions(&self, name: &str) -> Result<DefinitionsReport> {
        let snapshot_id = self.current_snapshot_id()?;
        let entities = self.store().entity_by_name(&snapshot_id, name, 32)?;
        Ok(DefinitionsReport {
            query: name.to_owned(),
            snapshot_id,
            definitions: entities.iter().map(DefinitionHit::from).collect(),
        })
    }

    /// Everything referencing a named symbol — `cce refs`. Incoming
    /// `References`/`Calls`/`Implements`/`TypeUses` edges, origin-tagged so
    /// compiler-derived (SCIP) rows are distinguishable from syntax guesses.
    pub fn references(&self, name: &str) -> Result<ReferencesReport> {
        const MAX_REFERENCES: usize = 256;
        let snapshot_id = self.current_snapshot_id()?;
        let targets = self.store().entity_by_name(&snapshot_id, name, 32)?;
        let mut hits = Vec::new();
        let mut truncated = false;
        'outer: for target in &targets {
            let incoming = self.store().relations_for_entity(
                &snapshot_id,
                &target.id,
                RelationDirection::Incoming,
                4096,
            )?;
            for relation in incoming {
                let covered = matches!(
                    relation.kind,
                    RelationKind::References
                        | RelationKind::Calls
                        | RelationKind::Implements
                        | RelationKind::TypeUses
                );
                if !covered {
                    continue;
                }
                if hits.len() >= MAX_REFERENCES {
                    truncated = true;
                    break 'outer;
                }
                let source = self
                    .store()
                    .entity_by_id(&snapshot_id, &relation.source_entity_id)?;
                hits.push(ReferenceHit {
                    from_name: source.as_ref().map_or_else(
                        || relation.source_entity_id.clone(),
                        |entity| entity.name.clone(),
                    ),
                    from_qualified_name: source
                        .as_ref()
                        .and_then(|entity| entity.qualified_name.clone()),
                    from_kind: source.as_ref().map_or_else(
                        || "unknown".to_owned(),
                        |entity| format!("{:?}", entity.kind),
                    ),
                    via: serde_json::to_value(&relation.kind)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "other".to_owned()),
                    origin: serde_json::to_value(relation.origin)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".to_owned()),
                    confidence: relation.confidence,
                    evidence: relation.evidence,
                });
            }
        }
        Ok(ReferencesReport {
            query: name.to_owned(),
            snapshot_id,
            targets: targets.iter().map(DefinitionHit::from).collect(),
            references: hits,
            truncated,
        })
    }

    /// Snapshot id of the last committed index. Atlas reads committed state;
    /// it never triggers indexing — an unindexed repository is an explicit
    /// error, matching `status` semantics.
    fn current_snapshot_id(&self) -> Result<String> {
        let anchor = RepositoryScanner::new(self.config().clone()).identify()?;
        match self.store().current_snapshot(&anchor.identity.id)? {
            Some(snapshot_id) if self.store().snapshot_is_complete(&snapshot_id)? => {
                Ok(snapshot_id)
            }
            _ => Err(cce_core::CceError::ViewUnavailable {
                view: "atlas".to_owned(),
                reason: "repository has not been indexed; run `cce index`".to_owned(),
            }),
        }
    }
}
