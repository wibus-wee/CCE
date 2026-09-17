//! Architecture Atlas APIs: deterministic package-level views over the
//! persisted world model. Everything here is derived from `BuildSystem`
//! provenance facts plus the typed relation graph — no inference is
//! presented as truth. Component-level inference above packages is a
//! later layer and must be marked as candidate.

use std::collections::HashMap;

use cce_core::{EntityKind, RelationKind, Result};
use cce_store::RelationDirection;
use serde::Serialize;

use crate::CceEngine;
use crate::repository::RepositoryScanner;

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Package-level architecture map for a snapshot.
pub struct CodebaseMap {
    /// Snapshot the map was read from.
    pub snapshot_id: String,
    /// All packages found.
    pub packages: Vec<PackageNode>,
    /// Number of `BuildDependsOn` edges between packages.
    pub dependency_edges: usize,
    /// Boundary violations detected at the package level — currently empty;
    /// direction rules arrive with the declared-architecture layer.
    pub violations: Vec<String>,
    /// How the map was produced.
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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
        Ok(CodebaseMap {
            snapshot_id,
            packages: package_list,
            dependency_edges: edges.len(),
            violations: Vec::new(),
            provenance: "build_system manifests (deterministic)".to_owned(),
        })
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
