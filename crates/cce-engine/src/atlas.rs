//! Architecture Atlas APIs: deterministic package-level views over the
//! persisted world model. Everything here is derived from `BuildSystem`
//! provenance facts plus the typed relation graph — no inference is
//! presented as truth. Component-level inference above packages is a
//! later layer and must be marked as candidate.

use std::collections::HashMap;

use cce_core::{EntityKind, RelationKind, Result};
use cce_store::RelationDirection;
use serde::Serialize;

use crate::repository::RepositoryScanner;
use crate::CceEngine;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageNode {
    pub name: String,
    pub ecosystem: String,
    pub manifest_path: String,
    pub root_dir: String,
    pub member_files: usize,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodebaseMap {
    pub snapshot_id: String,
    pub packages: Vec<PackageNode>,
    pub dependency_edges: usize,
    /// Boundary violations detected at the package level — currently empty;
    /// direction rules arrive with the declared-architecture layer.
    pub violations: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentExplanation {
    pub name: String,
    pub kind: String,
    pub qualified_name: Option<String>,
    pub address: Option<cce_core::SourceAddress>,
    pub member_files: Vec<String>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
    pub tests: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactReport {
    pub query: String,
    pub snapshot_id: String,
    pub matched_entities: Vec<String>,
    /// Entities that reach the target through impact edges (callers,
    /// references, tests, implementations) within two hops.
    pub impacted: Vec<ImpactedEntity>,
    pub edge_kinds: Vec<String>,
    pub provenance: String,
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactedEntity {
    pub name: String,
    pub kind: String,
    pub path: Option<String>,
    pub hops: usize,
    pub via: String,
    pub confidence: f32,
}

impl CceEngine {
    /// Package-level map of the repository: hard build boundaries and the
    /// declared dependency direction between them.
    pub async fn codebase_map(&self) -> Result<CodebaseMap> {
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
    pub async fn explain_component(&self, name: &str) -> Result<ComponentExplanation> {
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
    pub async fn impact_analysis(&self, query: &str) -> Result<ImpactReport> {
        let snapshot_id = self.current_snapshot_id()?;
        let seeds = self.store().entity_by_name(&snapshot_id, query, 8)?;
        let mut impacted: HashMap<String, ImpactedEntity> = HashMap::new();
        let impact_kinds = [
            RelationKind::Calls,
            RelationKind::References,
            RelationKind::Tests,
            RelationKind::Implements,
            RelationKind::BuildDependsOn,
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
                    let entry = impacted.entry(neighbor.clone()).or_insert(ImpactedEntity {
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
