//! `cce grep`: brute-force regular-expression search over the live
//! worktree. It does not touch the index — every match is inherently
//! current, which makes it the complement to snapshot-scoped search when
//! views are stale. The repository scanner supplies the file list, so
//! ignore rules, sensitive-file policy, and size caps apply exactly as in
//! indexing.

use std::fmt::Write as _;
use utoipa::ToSchema;

use cce_core::{CceError, QueryFilters, Result};
use serde::Serialize;

use crate::RepositoryScanner;

/// Default maximum matches returned by one grep call.
pub const DEFAULT_GREP_LIMIT: usize = 200;
/// Per-file match cap so one minified or generated file cannot crowd out
/// the rest of the tree.
const PER_FILE_CAP: usize = 100;
/// Match line display cap in characters.
const LINE_CAP: usize = 240;

/// A worktree grep request.
#[derive(Debug, Clone)]
pub struct GrepRequest {
    /// Regular expression (Rust `regex` syntax).
    pub pattern: String,
    /// Optional `path:`/`lang:` filters with the same semantics as search.
    pub filters: QueryFilters,
    /// Match cap across the whole tree.
    pub limit: usize,
    /// ASCII case-insensitive matching.
    pub ignore_case: bool,
}

/// One regex match in the worktree.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepHit {
    /// Repository-relative path.
    pub path: String,
    /// 1-based line number.
    pub line: u64,
    /// 1-based column (character offset) of the match start.
    pub column: usize,
    /// The matched line, trimmed and capped at `LINE_CAP` characters.
    pub text: String,
}

/// Result of a worktree grep.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepReport {
    /// The pattern that ran.
    pub pattern: String,
    /// Ordered matches, capped at the request limit.
    pub matches: Vec<GrepHit>,
    /// True when `limit` cut the list short.
    pub truncated: bool,
    /// Files actually scanned (after ignore/policy filtering).
    pub files_scanned: usize,
    /// Files skipped because their bytes were not valid UTF-8 text.
    pub skipped_binary: usize,
    /// Always `"worktree"`: grep reads live files, never the snapshot, so
    /// results are fresh by construction rather than verified.
    pub freshness: &'static str,
}

/// Runs `pattern` against every scanned file's live bytes. Files whose
/// bytes are not UTF-8 are counted and skipped; read failures surface as
/// errors rather than silent gaps. The store is passed to the scanner for
/// stat-cache reuse so repeated greps stay cheap.
///
/// # Errors
/// `Configuration` on an invalid regex; I/O errors on unreadable files.
pub(crate) fn grep(
    scanner: &RepositoryScanner,
    store: &cce_store::MetadataStore,
    request: &GrepRequest,
) -> Result<GrepReport> {
    let expression = regex::RegexBuilder::new(&request.pattern)
        .case_insensitive(request.ignore_case)
        .build()
        .map_err(|error| CceError::Configuration(format!("invalid regex: {error}")))?;
    let scanned = scanner.scan(Some(store))?;
    let mut report = GrepReport {
        pattern: request.pattern.clone(),
        matches: Vec::new(),
        truncated: false,
        files_scanned: 0,
        skipped_binary: 0,
        freshness: "worktree",
    };
    'files: for file in &scanned.files {
        if let Some(prefix) = &request.filters.path_prefix {
            if !file.relative_path.starts_with(prefix.as_str()) {
                continue;
            }
        }
        if let Some(language) = &request.filters.language {
            let matches_language = file
                .language
                .as_ref()
                .is_some_and(|value| value.eq_ignore_ascii_case(language));
            if !matches_language {
                continue;
            }
        }
        let Ok(text) = std::fs::read_to_string(&file.absolute_path) else {
            report.skipped_binary += 1;
            continue;
        };
        report.files_scanned += 1;
        let mut per_file = 0_usize;
        for (line_index, line_text) in text.lines().enumerate() {
            for found in expression.find_iter(line_text) {
                if report.matches.len() >= request.limit {
                    report.truncated = true;
                    break 'files;
                }
                if per_file >= PER_FILE_CAP {
                    break;
                }
                per_file += 1;
                let column = line_text[..found.start()].chars().count() + 1;
                let mut text = line_text.trim().to_owned();
                if text.chars().count() > LINE_CAP {
                    text = text.chars().take(LINE_CAP).collect();
                    let _ = write!(text, "…");
                }
                report.matches.push(GrepHit {
                    path: file.relative_path.clone(),
                    line: line_index as u64 + 1,
                    column,
                    text,
                });
            }
            if per_file >= PER_FILE_CAP {
                break;
            }
        }
    }
    Ok(report)
}
