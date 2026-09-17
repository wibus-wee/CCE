//! Query-time regex search over stored commit patches (`type:diff`).
//!
//! Sourcegraph's `type:diff` is a scan, not an index: the query regex runs
//! over the full unified diff captured for each commit. CCE already stores
//! that payload — every `CommitDiff` document's `body_artifact_digest`
//! points at a `CommitPatch` artifact — so this module needs no new
//! storage, only a patch-section scanner.
//!
//! Matching follows the same line-number convention as history evidence:
//! `+` lines address the post-image (new-file) line number, `-` lines the
//! pre-image (old-file) one. Results are historical evidence —
//! `verified_current` is always `false`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use cce_core::{CceError, Result, SearchHit, SearchRoute, SourceAddress};
use cce_store::MetadataStore;

/// Default maximum number of `(commit, file)` hits a diff search returns.
pub const DEFAULT_DIFF_LIMIT: usize = 20;

/// Matched `+`/`-` lines kept per file before truncation; a file with more
/// matches still surfaces once, with the count recorded in the hit's
/// explanation.
const MAX_MATCHED_LINES_PER_FILE: usize = 32;

/// One regex match inside a patch hunk.
#[derive(Debug)]
struct DiffMatch {
    /// Line number in the matching image: post-image for `+`, pre-image
    /// for `-`.
    line: u32,
    /// `true` for added (`+`) lines, `false` for removed (`-`) lines.
    added: bool,
    /// The matched line text without its `+`/`-` prefix.
    text: String,
}

/// Accumulator for the matches found in one file section of one commit.
#[derive(Debug, Default)]
struct FileMatches {
    matches: Vec<DiffMatch>,
    /// Matches dropped by `MAX_MATCHED_LINES_PER_FILE`.
    truncated: usize,
}

/// Scan every `CommitDiff` patch in `snapshot_id` for `pattern`, returning
/// one [`SearchHit`] per `(commit, file)` pair ordered by commit recency.
///
/// # Errors
/// `CceError::Configuration` when `pattern` is not a valid regex.
/// `CceError::Storage` on metadata/artifact read failures.
pub(crate) fn diff_grep(
    store: &MetadataStore,
    snapshot_id: &str,
    repository_id: &str,
    pattern: &str,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let regex = regex::Regex::new(pattern).map_err(|error| {
        CceError::Configuration(format!("invalid diff pattern `{pattern}`: {error}"))
    })?;
    let mut sections: Vec<(i64, String, String, String, Vec<DiffMatch>, usize)> = Vec::new();
    for document in store
        .documents_for_snapshot(snapshot_id)?
        .into_iter()
        .filter(|document| document.representation == cce_core::RetrievalRepresentation::CommitDiff)
    {
        let (commit, timestamp) = store
            .entity_by_id(snapshot_id, &document.entity_id)?
            .map_or_else(
                || (document.entity_id.clone(), 0),
                |entity| {
                    let timestamp = entity
                        .attributes
                        .get("committerTime")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0);
                    (entity.name, timestamp)
                },
            );
        for (path, file) in scan_patch(&document.text, &regex) {
            if file.matches.is_empty() {
                continue;
            }
            sections.push((
                timestamp,
                commit.clone(),
                path,
                document.entity_id.clone(),
                file.matches,
                file.truncated,
            ));
        }
    }
    // Most recent commits first; within one commit keep patch order by
    // path so a commit reads as a unit.
    sections.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    let mut hits = Vec::new();
    for (timestamp, commit, path, entity_id, matches, truncated) in sections {
        if hits.len() >= limit.max(1) {
            break;
        }
        hits.push(render_hit(
            snapshot_id,
            repository_id,
            &commit,
            timestamp,
            &path,
            &entity_id,
            matches,
            truncated,
        ));
    }
    Ok(hits)
}

/// Walk the patch text, tracking the current file and both line counters,
/// and collect regex matches on `+`/`-` lines per file section.
fn scan_patch(patch: &str, regex: &regex::Regex) -> BTreeMap<String, FileMatches> {
    let mut sections: BTreeMap<String, FileMatches> = BTreeMap::new();
    let mut path = String::new();
    // `old_line`/`new_line` are the next line numbers in each image; a file
    // without hunks contributes nothing.
    let mut old_line = 0_u32;
    let mut new_line = 0_u32;
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // `a/<path> b/<path>` — the post-image path is canonical even
            // for renames, and for pure deletions the `a/` side still names
            // the file that carried the removed lines.
            rest.split_whitespace()
                .last()
                .and_then(|token| token.strip_prefix("b/"))
                .unwrap_or_default()
                .clone_into(&mut path);
            continue;
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            // `@@ -<old>,<count> +<new>,<count> @@` — counts are optional.
            if let Some((old, new)) = parse_hunk_header(header) {
                old_line = old;
                new_line = new;
            }
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with('\\') {
            continue;
        }
        let (added, number, text) = match line.split_at_checked(1) {
            Some(("+", text)) => (true, new_line, text),
            Some(("-", text)) => (false, old_line, text),
            Some((" ", text)) => {
                let _ = text;
                old_line += 1;
                new_line += 1;
                continue;
            }
            _ => continue,
        };
        if added {
            new_line += 1;
        } else {
            old_line += 1;
        }
        if !regex.is_match(text) {
            continue;
        }
        let section = sections.entry(path.clone()).or_default();
        if section.matches.len() >= MAX_MATCHED_LINES_PER_FILE {
            section.truncated += 1;
        } else {
            section.matches.push(DiffMatch {
                line: number,
                added,
                text: text.to_owned(),
            });
        }
    }
    sections
}

/// Parse the `-<old>[,<n>] +<new>[,<n>]` portion of a hunk header into the
/// first line number of each image.
fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let mut old = None;
    let mut new = None;
    for token in header.split_whitespace() {
        if let Some(rest) = token.strip_prefix('-') {
            old = rest.split(',').next()?.parse().ok();
        } else if let Some(rest) = token.strip_prefix('+') {
            new = rest.split(',').next()?.parse().ok();
        }
    }
    Some((old?, new?))
}

/// Render one `(commit, file)` match group as a [`SearchHit`]: the covering
/// range becomes the primary address and every matched line an evidence
/// address under the post-/pre-image convention.
#[allow(clippy::too_many_arguments)]
fn render_hit(
    snapshot_id: &str,
    repository_id: &str,
    commit: &str,
    timestamp: i64,
    path: &str,
    entity_id: &str,
    matches: Vec<DiffMatch>,
    truncated: usize,
) -> SearchHit {
    let mut evidence = Vec::with_capacity(matches.len());
    let mut added_range: Option<(u32, u32)> = None;
    let mut removed_range: Option<(u32, u32)> = None;
    let mut snippet = String::new();
    for matched in &matches {
        let target = if matched.added {
            &mut added_range
        } else {
            &mut removed_range
        };
        match target {
            Some((start, end)) => {
                *start = (*start).min(matched.line);
                *end = (*end).max(matched.line);
            }
            None => *target = Some((matched.line, matched.line)),
        }
        if let Ok(address) = SourceAddress::new(
            repository_id,
            snapshot_id,
            path,
            0..0,
            matched.line..=matched.line,
        ) {
            evidence.push(address);
        }
        let sign = if matched.added { '+' } else { '-' };
        let _ = writeln!(snippet, "{sign}{}", matched.text);
    }
    // The primary address uses the added-side range when present (the
    // post-image is what a reader opens), else the removed side.
    let (start, end) = added_range.or(removed_range).unwrap_or((0, 0));
    let address = SourceAddress::new(repository_id, snapshot_id, path, 0..0, start..=end).ok();
    let short = commit.get(..12).unwrap_or(commit);
    let mut explanation = vec![format!(
        "regex matched {} line(s) in the stored patch of commit {short} (committer time {timestamp}); post-/pre-image line numbers",
        matches.len()
    )];
    if truncated > 0 {
        explanation.push(format!("{truncated} further matched line(s) truncated"));
    }
    SearchHit {
        document_id: format!("diff:{entity_id}:{path}"),
        entity_id: entity_id.to_owned(),
        region_id: None,
        symbol_name: Some(format!("commit:{short}")),
        representation: cce_core::RetrievalRepresentation::CommitDiff,
        route: SearchRoute::Diff,
        rank: 0,
        score: 0.0,
        contributing_routes: vec![SearchRoute::Diff],
        address,
        evidence,
        snippet,
        verified_current: false,
        explanation,
    }
}
