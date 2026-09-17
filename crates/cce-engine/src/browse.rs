//! Code browsing for the web UI: snapshot-scoped file listing and
//! snapshot-consistent file content.
//!
//! Both reads come from the committed source view — the same artifact
//! digests that back `source_text` snippets — so a browser always sees the
//! exact bytes the index reasoned over, never a moved worktree.

use cce_core::{CceError, Result};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{CceEngine, repository::RepositoryScanner};

/// Files larger than this are served truncated — a browser needs the head
/// of a generated file, not the whole payload.
const FILE_CONTENT_CAP: usize = 1024 * 1024;

/// One indexed file in the file tree.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileListEntry {
    /// Repository-relative path.
    pub path: String,
    /// Detected language, when known.
    pub language: Option<String>,
}

/// `GET /v1/files` response: the indexed file set of the current snapshot.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileListReport {
    /// Snapshot the listing is scoped to — same identity `status()` reports.
    pub snapshot_id: String,
    /// Ordered by path.
    pub files: Vec<FileListEntry>,
}

/// `GET /v1/file/{path}` response: full file content from the source view.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    /// Snapshot the bytes belong to.
    pub snapshot_id: String,
    /// Repository-relative path that was requested.
    pub path: String,
    /// Detected language, when known.
    pub language: Option<String>,
    /// UTF-8 text of the file — empty when `binary` or after `truncated`.
    pub content: String,
    /// True when the file exceeded the 1 MiB content cap.
    pub truncated: bool,
    /// True when the stored bytes are not valid UTF-8 — `content` is empty
    /// rather than lossy-mangled.
    pub binary: bool,
}

impl CceEngine {
    /// Indexed file list for the code browser, scoped to the current
    /// snapshot (the same freshness check `status()` performs).
    ///
    /// # Errors
    /// View-unavailable when the repository has never been indexed.
    pub fn files(&self) -> Result<FileListReport> {
        let (snapshot_id, rows) = self.current_source_files()?;
        Ok(FileListReport {
            snapshot_id,
            files: rows
                .into_iter()
                .map(|row| FileListEntry {
                    path: row.path,
                    language: row.language,
                })
                .collect(),
        })
    }

    /// One file's content from the committed source view.
    ///
    /// # Errors
    /// View-unavailable when never indexed; invalid-repository when the path
    /// is not part of the indexed set (including `..`/absolute attempts —
    /// they simply match nothing).
    pub fn file(&self, path: &str) -> Result<FileContent> {
        let (snapshot_id, rows) = self.current_source_files()?;
        let row = rows
            .into_iter()
            .find(|row| row.path == path)
            .ok_or_else(|| {
                CceError::InvalidRepository(format!("path is not indexed: {path}").into())
            })?;
        let bytes = self.store().source_file_bytes(&snapshot_id, path)?;
        let truncated = bytes.len() > FILE_CONTENT_CAP;
        let (content, binary) =
            String::from_utf8(bytes.get(..FILE_CONTENT_CAP).unwrap_or(&bytes).to_vec())
                .map_or_else(|_| (String::new(), true), |text| (text, false));
        Ok(FileContent {
            snapshot_id,
            path: row.path,
            language: row.language,
            content,
            truncated,
            binary,
        })
    }

    /// Resolve the worktree's current committed snapshot and its indexed
    /// file rows — the shared freshness preamble for browse endpoints.
    fn current_source_files(&self) -> Result<(String, Vec<cce_store::SourceFileRow>)> {
        let scanned = RepositoryScanner::new(self.config().clone()).scan(Some(self.store()))?;
        let current = self
            .store()
            .current_snapshot(&scanned.identity.id)?
            .ok_or_else(|| CceError::ViewUnavailable {
                view: "files".to_owned(),
                reason: "repository has not been indexed".to_owned(),
            })?;
        let rows = self.store().source_files_for_snapshot(&current)?;
        Ok((current, rows))
    }
}
