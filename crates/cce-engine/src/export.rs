//! Bulk export: id-ordered pages of a committed snapshot's entities,
//! relations, and regions for downstream projection builders.
//!
//! Every row carries everything a consumer needs to rebuild the graph
//! without per-entity lookups — the full `CodeEntity`/`Relation`/
//! `CodeRegion` records, exactly as committed. Ordering is by id (unique
//! and immutable within a snapshot), so `cursor` = the last id of the
//! previous page gives a stable pagination. Pure read over committed
//! rows — no scan, no worktree access.

use cce_core::{CceError, CodeEntity, CodeRegion, Relation, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::CceEngine;
use crate::repository::RepositoryScanner;

/// Largest page an export call returns; larger `limit`s clamp to this.
const MAX_EXPORT_PAGE: usize = 10_000;

/// `GET /v1/export` response: committed row counts of a snapshot — the
/// sizing prelude for paged export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    /// Snapshot the counts were read from.
    pub snapshot_id: String,
    /// Committed entity rows.
    pub entity_count: usize,
    /// Committed relation rows.
    pub relation_count: usize,
    /// Committed region rows.
    pub region_count: usize,
}

/// `GET /v1/export/entities` response: one id-ordered page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntityExportPage {
    /// Snapshot this page was read from.
    pub snapshot_id: String,
    /// Entities ordered by id — the cursor key.
    pub entities: Vec<CodeEntity>,
    /// Cursor for the next page (`id > nextCursor`); `null` at the end of
    /// the snapshot.
    pub next_cursor: Option<String>,
}

/// `GET /v1/export/relations` response: one id-ordered page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelationExportPage {
    /// Snapshot this page was read from.
    pub snapshot_id: String,
    /// Relations ordered by id — the cursor key.
    pub relations: Vec<Relation>,
    /// Cursor for the next page (`id > nextCursor`); `null` at the end of
    /// the snapshot.
    pub next_cursor: Option<String>,
}

/// `GET /v1/export/regions` response: one id-ordered page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionExportPage {
    /// Snapshot this page was read from.
    pub snapshot_id: String,
    /// Canonical regions ordered by id — the cursor key.
    pub regions: Vec<CodeRegion>,
    /// Cursor for the next page (`id > nextCursor`); `null` at the end of
    /// the snapshot.
    pub next_cursor: Option<String>,
}

impl CceEngine {
    /// Committed entity/relation/region counts of a snapshot (`None`
    /// resolves to the current one).
    ///
    /// # Errors
    /// `Configuration` when `snapshot` names no completed snapshot of this
    /// repository; `ViewUnavailable` when never indexed; storage errors.
    pub fn export_summary(&self, snapshot: Option<&str>) -> Result<ExportSummary> {
        let (_, snapshot_id) = self.resolve_export_snapshot(snapshot)?;
        let counts = self.store().snapshot_counts(&snapshot_id)?;
        Ok(ExportSummary {
            snapshot_id,
            entity_count: counts.entities,
            relation_count: counts.relations,
            region_count: counts.regions,
        })
    }

    /// One page of a snapshot's entities, ordered by id. `cursor` is the
    /// last id of the previous page; `limit` clamps to `1..=10_000`.
    ///
    /// # Errors
    /// `Configuration` when `snapshot` names no completed snapshot of this
    /// repository; `ViewUnavailable` when never indexed; storage errors.
    pub fn export_entities(
        &self,
        snapshot: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<EntityExportPage> {
        let (_, snapshot_id) = self.resolve_export_snapshot(snapshot)?;
        let limit = limit.clamp(1, MAX_EXPORT_PAGE);
        // Fetch one row past the page so `next_cursor` only appears when a
        // following page actually has rows.
        let mut entities = self
            .store()
            .entities_page(&snapshot_id, cursor, limit + 1)?;
        let next_cursor = if entities.len() > limit {
            entities.truncate(limit);
            entities.last().map(|entity| entity.id.clone())
        } else {
            None
        };
        Ok(EntityExportPage {
            snapshot_id,
            entities,
            next_cursor,
        })
    }

    /// One page of a snapshot's relations, ordered by id — same cursor
    /// contract as [`CceEngine::export_entities`].
    ///
    /// # Errors
    /// `Configuration` when `snapshot` names no completed snapshot of this
    /// repository; `ViewUnavailable` when never indexed; storage errors.
    pub fn export_relations(
        &self,
        snapshot: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<RelationExportPage> {
        let (_, snapshot_id) = self.resolve_export_snapshot(snapshot)?;
        let limit = limit.clamp(1, MAX_EXPORT_PAGE);
        let mut relations = self
            .store()
            .relations_page(&snapshot_id, cursor, limit + 1)?;
        let next_cursor = if relations.len() > limit {
            relations.truncate(limit);
            relations.last().map(|relation| relation.id.clone())
        } else {
            None
        };
        Ok(RelationExportPage {
            snapshot_id,
            relations,
            next_cursor,
        })
    }

    /// One page of a snapshot's canonical regions, ordered by id — same
    /// cursor contract as [`CceEngine::export_entities`].
    ///
    /// # Errors
    /// `Configuration` when `snapshot` names no completed snapshot of this
    /// repository; `ViewUnavailable` when never indexed; storage errors.
    pub fn export_regions(
        &self,
        snapshot: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<RegionExportPage> {
        let (_, snapshot_id) = self.resolve_export_snapshot(snapshot)?;
        let limit = limit.clamp(1, MAX_EXPORT_PAGE);
        let mut regions = self.store().regions_page(&snapshot_id, cursor, limit + 1)?;
        let next_cursor = if regions.len() > limit {
            regions.truncate(limit);
            regions.last().map(|region| region.id.clone())
        } else {
            None
        };
        Ok(RegionExportPage {
            snapshot_id,
            regions,
            next_cursor,
        })
    }

    /// Resolve a snapshot for read-only export: an explicit id must name a
    /// completed snapshot of this repository; `None` resolves to the
    /// current committed snapshot. Pure metadata — never rescans the
    /// worktree.
    fn resolve_export_snapshot(&self, snapshot: Option<&str>) -> Result<(String, String)> {
        let anchor = RepositoryScanner::new(self.config().clone()).identify()?;
        let repository_id = anchor.identity.id;
        if let Some(id) = snapshot {
            let committed = self.store().snapshots_for_repository(&repository_id)?;
            if committed.iter().all(|entry| entry.id != id) {
                return Err(CceError::Configuration(format!(
                    "`{id}` is not a completed snapshot of this repository"
                )));
            }
            return Ok((repository_id, id.to_owned()));
        }
        let current = self
            .store()
            .current_snapshot(&repository_id)?
            .ok_or_else(|| CceError::ViewUnavailable {
                view: "export".to_owned(),
                reason: "repository has not been indexed; run `cce index`".to_owned(),
            })?;
        if !self.store().snapshot_is_complete(&current)? {
            return Err(CceError::ViewUnavailable {
                view: "export".to_owned(),
                reason: format!("snapshot {current} is still committing"),
            });
        }
        Ok((repository_id, current))
    }
}
