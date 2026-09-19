//! Architecture diff: the entity/relation delta between two committed
//! snapshots of one repository.
//!
//! Entity ids hash `(repository, path, byte range, discriminator)`, so an
//! edit that shifts a symbol's byte range renames its id without any
//! semantic change — a naive id diff reports remove+add churn for
//! untouched entities. The comparison therefore runs on the stable
//! pre-hash identity: `(kind, qualified name or name, file path)`.
//! Relations compare on `(kind, source key, target key)`; an edge whose
//! key exists in both snapshots but carries a different `origin` or
//! `confidence` reports `changed`. Everything here is a pure read over
//! committed rows — no scan, no worktree access.

use std::collections::{BTreeMap, HashMap};

use cce_core::{CceError, CodeEntity, EntityKind, Relation, RelationKind, RelationOrigin, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::CceEngine;
use crate::repository::RepositoryScanner;

/// Maximum entries per delta list; `counts` always report full totals.
const DIFF_LIST_CAP: usize = 500;

/// Stable cross-snapshot identity of an entity — the parts `entity_id`
/// hashes minus the byte range that drifts on edits: `(kind, qualified
/// name or display name, file path)`.
type EntityKey = (String, String, String);

/// Stable cross-snapshot identity of a relation: edge kind plus both
/// endpoint entity keys.
type RelationKey = (String, EntityKey, EntityKey);

/// An entity reference inside a diff: the fields the comparison key is
/// built from plus the snapshot-local id it resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffEntityRef {
    /// Entity id within the snapshot this entry was read from — ids embed
    /// byte ranges and are *not* the comparison key.
    pub entity_id: String,
    /// Structural kind — part of the comparison key.
    pub kind: EntityKind,
    /// Display name.
    pub name: String,
    /// Qualified name when recorded — part of the comparison key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    /// Repository-relative path when the entity carries a source address —
    /// part of the comparison key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Definition language when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// One edge of the relation delta, endpoints resolved to stable keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffRelation {
    /// Relation id within the snapshot this entry was read from.
    pub relation_id: String,
    /// Edge kind — part of the comparison key.
    pub kind: RelationKind,
    /// Source endpoint resolved to its stable key + snapshot-local id.
    pub source: DiffEntityRef,
    /// Target endpoint resolved to its stable key + snapshot-local id.
    pub target: DiffEntityRef,
    /// How the edge was derived.
    pub origin: RelationOrigin,
    /// Confidence in [0, 1].
    pub confidence: f32,
    /// Extractor that produced the edge.
    pub extractor: String,
}

/// An edge present in both snapshots whose provenance drifted — same
/// `(kind, source, target)` key, different `origin` or `confidence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangedRelation {
    /// Edge kind — part of the comparison key.
    pub kind: RelationKind,
    /// Source endpoint (head-side reference).
    pub source: DiffEntityRef,
    /// Target endpoint (head-side reference).
    pub target: DiffEntityRef,
    /// Relation id on the base snapshot.
    pub base_relation_id: String,
    /// Relation id on the head snapshot.
    pub head_relation_id: String,
    /// Derivation origin on the base snapshot.
    pub base_origin: RelationOrigin,
    /// Confidence on the base snapshot.
    pub base_confidence: f32,
    /// Derivation origin on the head snapshot.
    pub head_origin: RelationOrigin,
    /// Confidence on the head snapshot.
    pub head_confidence: f32,
}

/// Full delta totals; the entry lists are capped at 500 each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffCounts {
    /// Entities only in head.
    pub added_entities: usize,
    /// Entities only in base.
    pub removed_entities: usize,
    /// Relations only in head.
    pub added_relations: usize,
    /// Relations only in base.
    pub removed_relations: usize,
    /// Relations present in both with drifted origin/confidence.
    pub changed_relations: usize,
}

/// `GET /v1/diff/architecture` response: the entity/relation delta between
/// two committed snapshots of one repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectureDiff {
    /// Repository the compared snapshots belong to.
    pub repository_id: String,
    /// Older snapshot of the pair.
    pub base_snapshot_id: String,
    /// Newer snapshot of the pair.
    pub head_snapshot_id: String,
    /// Entities whose stable key exists only in head (capped at 500).
    pub added_entities: Vec<DiffEntityRef>,
    /// Entities whose stable key exists only in base (capped at 500).
    pub removed_entities: Vec<DiffEntityRef>,
    /// Relations whose stable key exists only in head (capped at 500).
    pub added_relations: Vec<DiffRelation>,
    /// Relations whose stable key exists only in base (capped at 500).
    pub removed_relations: Vec<DiffRelation>,
    /// Relations matched by key with drifted origin/confidence (capped at
    /// 500).
    pub changed_relations: Vec<ChangedRelation>,
    /// Totals before capping — always the full delta, even when the lists
    /// above are truncated.
    pub counts: DiffCounts,
    /// `true` when any list hit the 500-entry cap.
    pub truncated: bool,
    /// How the comparison was made.
    pub provenance: String,
}

impl CceEngine {
    /// Entity/relation delta between two committed snapshots of this
    /// repository. `head` defaults to the current committed snapshot,
    /// `base` to the snapshot committed immediately before `head`.
    ///
    /// # Errors
    /// `Configuration` when a supplied id is not a completed snapshot of
    /// this repository; `ViewUnavailable` when there is no snapshot pair to
    /// compare; storage errors on read failure.
    pub fn architecture_diff(
        &self,
        base: Option<&str>,
        head: Option<&str>,
    ) -> Result<ArchitectureDiff> {
        let (repository_id, base_id, head_id) = self.resolve_diff_pair(base, head)?;
        let base_entities = self.store().entities_for_snapshot(&base_id)?;
        let head_entities = self.store().entities_for_snapshot(&head_id)?;
        let base_relations = self.store().relations_for_snapshot(&base_id)?;
        let head_relations = self.store().relations_for_snapshot(&head_id)?;

        let (mut added_entities, mut removed_entities) =
            entity_delta(&base_entities, &head_entities);
        let (mut added_relations, mut removed_relations, mut changed_relations) = relation_delta(
            &base_relations,
            &head_relations,
            &base_entities,
            &head_entities,
        );

        let counts = DiffCounts {
            added_entities: added_entities.len(),
            removed_entities: removed_entities.len(),
            added_relations: added_relations.len(),
            removed_relations: removed_relations.len(),
            changed_relations: changed_relations.len(),
        };
        let truncated = counts.added_entities > DIFF_LIST_CAP
            || counts.removed_entities > DIFF_LIST_CAP
            || counts.added_relations > DIFF_LIST_CAP
            || counts.removed_relations > DIFF_LIST_CAP
            || counts.changed_relations > DIFF_LIST_CAP;
        added_entities.truncate(DIFF_LIST_CAP);
        removed_entities.truncate(DIFF_LIST_CAP);
        added_relations.truncate(DIFF_LIST_CAP);
        removed_relations.truncate(DIFF_LIST_CAP);
        changed_relations.truncate(DIFF_LIST_CAP);

        Ok(ArchitectureDiff {
            repository_id,
            base_snapshot_id: base_id,
            head_snapshot_id: head_id,
            added_entities,
            removed_entities,
            added_relations,
            removed_relations,
            changed_relations,
            counts,
            truncated,
            provenance: "entities compared by (kind, qualified name or name, file path); \
                 relations by (kind, source key, target key); entity and relation ids hash \
                 byte ranges and are never compared"
                .to_owned(),
        })
    }

    /// Resolve the snapshot pair to compare: explicit ids are validated
    /// against this repository's completed snapshots; defaults are current
    /// for `head` and the completed snapshot preceding `head` for `base`.
    fn resolve_diff_pair(
        &self,
        base: Option<&str>,
        head: Option<&str>,
    ) -> Result<(String, String, String)> {
        let anchor = RepositoryScanner::new(self.config().clone()).identify()?;
        let repository_id = anchor.identity.id;
        let committed = self.store().snapshots_for_repository(&repository_id)?;
        let head_id = match head {
            Some(id) if committed.iter().any(|entry| entry.id == id) => id.to_owned(),
            Some(id) => {
                return Err(CceError::Configuration(format!(
                    "`{id}` is not a completed snapshot of this repository"
                )));
            }
            None => self
                .store()
                .current_snapshot(&repository_id)?
                .filter(|id| committed.iter().any(|entry| &entry.id == id))
                .ok_or_else(|| CceError::ViewUnavailable {
                    view: "diff".to_owned(),
                    reason: "repository has not been indexed; run `cce index`".to_owned(),
                })?,
        };
        let base_id = match base {
            Some(id) if committed.iter().any(|entry| entry.id == id) => id.to_owned(),
            Some(id) => {
                return Err(CceError::Configuration(format!(
                    "`{id}` is not a completed snapshot of this repository"
                )));
            }
            None => committed
                .iter()
                .position(|entry| entry.id == head_id)
                .and_then(|index| committed.get(index + 1))
                .map(|entry| entry.id.clone())
                .ok_or_else(|| CceError::ViewUnavailable {
                    view: "diff".to_owned(),
                    reason: "no earlier committed snapshot to diff against".to_owned(),
                })?,
        };
        Ok((repository_id, base_id, head_id))
    }
}

/// The stable comparison key of one entity.
fn entity_key(entity: &CodeEntity) -> EntityKey {
    (
        format!("{:?}", entity.kind),
        entity
            .qualified_name
            .clone()
            .unwrap_or_else(|| entity.name.clone()),
        entity
            .address
            .as_ref()
            .map_or_else(String::new, |address| address.path.clone()),
    )
}

/// Fallback key for a relation endpoint that has no committed entity row —
/// external/unresolved ids compare on their raw spelling, which for
/// range-free ids (repositories, commits, packages) is already stable.
fn unresolved_key(entity_id: &str) -> EntityKey {
    (
        format!("{:?}", EntityKind::Unknown),
        entity_id.to_owned(),
        String::new(),
    )
}

/// Wire reference of one entity.
fn entity_ref(entity: &CodeEntity) -> DiffEntityRef {
    DiffEntityRef {
        entity_id: entity.id.clone(),
        kind: entity.kind.clone(),
        name: entity.name.clone(),
        qualified_name: entity.qualified_name.clone(),
        path: entity.address.as_ref().map(|address| address.path.clone()),
        language: entity.language.clone(),
    }
}

/// Wire reference for an endpoint that resolved to no committed entity.
fn unresolved_ref(entity_id: &str) -> DiffEntityRef {
    DiffEntityRef {
        entity_id: entity_id.to_owned(),
        kind: EntityKind::Unknown,
        name: entity_id.to_owned(),
        qualified_name: None,
        path: None,
        language: None,
    }
}

/// Bucket entities by stable key, preserving the store's id ordering inside
/// each bucket so multiset pairing is deterministic.
fn key_entities(entities: &[CodeEntity]) -> BTreeMap<EntityKey, Vec<&CodeEntity>> {
    let mut map: BTreeMap<EntityKey, Vec<&CodeEntity>> = BTreeMap::new();
    for entity in entities {
        map.entry(entity_key(entity)).or_default().push(entity);
    }
    map
}

/// Entity delta: keys present only in head are added, only in base removed.
/// Buckets with multiple entities (a key collision) pair off index-wise —
/// extras on either side are the delta.
fn entity_delta(
    base: &[CodeEntity],
    head: &[CodeEntity],
) -> (Vec<DiffEntityRef>, Vec<DiffEntityRef>) {
    let base_map = key_entities(base);
    let head_map = key_entities(head);
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for (key, head_list) in &head_map {
        let matched = base_map.get(key).map_or(0, Vec::len);
        added.extend(
            head_list
                .iter()
                .skip(matched)
                .map(|entity| entity_ref(entity)),
        );
    }
    for (key, base_list) in &base_map {
        let matched = head_map.get(key).map_or(0, Vec::len);
        removed.extend(
            base_list
                .iter()
                .skip(matched)
                .map(|entity| entity_ref(entity)),
        );
    }
    (added, removed)
}

/// The stable comparison key of one relation, endpoint ids resolved
/// through the entity map of its own snapshot.
fn relation_key(relation: &Relation, keys: &HashMap<&str, EntityKey>) -> RelationKey {
    (
        format!("{:?}", relation.kind),
        keys.get(relation.source_entity_id.as_str())
            .cloned()
            .unwrap_or_else(|| unresolved_key(&relation.source_entity_id)),
        keys.get(relation.target_entity_id.as_str())
            .cloned()
            .unwrap_or_else(|| unresolved_key(&relation.target_entity_id)),
    )
}

/// Wire entry of one relation, endpoints resolved to snapshot-local refs.
fn relation_ref(relation: &Relation, entities: &HashMap<&str, &CodeEntity>) -> DiffRelation {
    let endpoint = |entity_id: &str| {
        entities
            .get(entity_id)
            .map_or_else(|| unresolved_ref(entity_id), |entity| entity_ref(entity))
    };
    DiffRelation {
        relation_id: relation.id.clone(),
        kind: relation.kind.clone(),
        source: endpoint(&relation.source_entity_id),
        target: endpoint(&relation.target_entity_id),
        origin: relation.origin,
        confidence: relation.confidence,
        extractor: relation.extractor.clone(),
    }
}

/// Relation delta: keys only in head are added, only in base removed;
/// matched keys whose `origin` or `confidence` drifted report `changed`.
/// Endpoint entity ids are resolved through each snapshot's own entity
/// set — the same snapshot the relation was committed in.
fn relation_delta(
    base: &[Relation],
    head: &[Relation],
    base_entities: &[CodeEntity],
    head_entities: &[CodeEntity],
) -> (Vec<DiffRelation>, Vec<DiffRelation>, Vec<ChangedRelation>) {
    let (base_keys, base_refs) = endpoint_maps(base_entities);
    let (head_keys, head_refs) = endpoint_maps(head_entities);
    let base_map = key_relations(base, &base_keys);
    let head_map = key_relations(head, &head_keys);

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (key, head_list) in &head_map {
        let Some(base_list) = base_map.get(key) else {
            added.extend(
                head_list
                    .iter()
                    .map(|relation| relation_ref(relation, &head_refs)),
            );
            continue;
        };
        // Paired index-wise; leftovers on the head side are additions.
        added.extend(
            head_list
                .iter()
                .skip(base_list.len())
                .map(|relation| relation_ref(relation, &head_refs)),
        );
        for (base_relation, head_relation) in base_list.iter().zip(head_list.iter()) {
            // Bit-exact confidence comparison: any drift at all is a
            // changed edge — these are recorded provenance values, not
            // measurements with an error margin.
            if base_relation.origin != head_relation.origin
                || base_relation.confidence.to_bits() != head_relation.confidence.to_bits()
            {
                let head_ref = relation_ref(head_relation, &head_refs);
                changed.push(ChangedRelation {
                    kind: head_ref.kind,
                    source: head_ref.source,
                    target: head_ref.target,
                    base_relation_id: base_relation.id.clone(),
                    head_relation_id: head_relation.id.clone(),
                    base_origin: base_relation.origin,
                    base_confidence: base_relation.confidence,
                    head_origin: head_relation.origin,
                    head_confidence: head_relation.confidence,
                });
            }
        }
    }
    for (key, base_list) in &base_map {
        let matched = head_map.get(key).map_or(0, Vec::len);
        removed.extend(
            base_list
                .iter()
                .skip(matched)
                .map(|relation| relation_ref(relation, &base_refs)),
        );
    }
    (added, removed, changed)
}

/// Bucket relations by stable key (endpoint ids resolved to entity keys),
/// preserving the store's id ordering inside each bucket.
fn key_relations<'a>(
    relations: &'a [Relation],
    keys: &HashMap<&str, EntityKey>,
) -> BTreeMap<RelationKey, Vec<&'a Relation>> {
    let mut map: BTreeMap<RelationKey, Vec<&Relation>> = BTreeMap::new();
    for relation in relations {
        map.entry(relation_key(relation, keys))
            .or_default()
            .push(relation);
    }
    map
}

/// Entity-id lookups for one snapshot: the stable-key map relation
/// endpoints compare through, and the entity map display refs resolve
/// through.
fn endpoint_maps(
    entities: &[CodeEntity],
) -> (HashMap<&str, EntityKey>, HashMap<&str, &CodeEntity>) {
    let mut keys = HashMap::with_capacity(entities.len());
    let mut refs = HashMap::with_capacity(entities.len());
    for entity in entities {
        keys.insert(entity.id.as_str(), entity_key(entity));
        refs.insert(entity.id.as_str(), entity);
    }
    (keys, refs)
}
