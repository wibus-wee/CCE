use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    ops::RangeInclusive,
    path::Path,
};

use cce_core::SourceAddress;
use gix::bstr::{BString, ByteSlice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistorySummary {
    pub body: String,
    pub commit_count: usize,
}

pub(crate) fn summarize(
    repository_root: &Path,
    maximum_commits: usize,
) -> Result<Option<HistorySummary>, String> {
    if !repository_root.join(".git").exists() {
        return Ok(None);
    }
    let repository = gix::open(repository_root).map_err(|error| error.to_string())?;
    let head = repository
        .head_commit()
        .map_err(|error| error.to_string())?;
    let walk = head.ancestors().all().map_err(|error| error.to_string())?;
    let mut lines = vec![
        "# Commit history".to_owned(),
        String::new(),
        "Generated from Git objects by gitoxide. Commit messages are historical evidence, not current-source truth."
            .to_owned(),
        String::new(),
    ];
    let mut commit_count = 0_usize;
    for entry in walk.take(maximum_commits) {
        let entry = entry.map_err(|error| error.to_string())?;
        let commit = entry.object().map_err(|error| error.to_string())?;
        let message = commit
            .message_raw_sloppy()
            .lines()
            .next()
            .unwrap_or_default()
            .to_str_lossy();
        let timestamp = commit.time().map_err(|error| error.to_string())?.seconds;
        lines.push(format!("- `{}` ({timestamp}): {message}", entry.id));
        commit_count += 1;
    }
    Ok(Some(HistorySummary {
        body: lines.join("\n"),
        commit_count,
    }))
}

/// Paths touched by each recent commit, diffed against its first parent and
/// newest first. Root commits contribute nothing — their "change" is the
/// whole tree. Every failure degrades to a shorter or empty list: history is
/// evidence for co-change edges, never a hard dependency of indexing.
pub(crate) fn changed_paths_per_commit(
    repository_root: &Path,
    maximum_commits: usize,
) -> Vec<Vec<String>> {
    let mut commits = Vec::new();
    if !repository_root.join(".git").exists() {
        return commits;
    }
    let Ok(repository) = gix::open(repository_root) else {
        return commits;
    };
    let Ok(head) = repository.head_commit() else {
        return commits;
    };
    let Ok(walk) = head.ancestors().all() else {
        return commits;
    };
    for entry in walk.take(maximum_commits) {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(commit) = entry.object() else {
            continue;
        };
        let Some(parent_id) = commit.parent_ids().next() else {
            continue;
        };
        let (Ok(tree), Ok(parent)) = (commit.tree(), repository.find_commit(parent_id)) else {
            continue;
        };
        let Ok(parent_tree) = parent.tree() else {
            continue;
        };
        let mut touched = Vec::new();
        diff_trees(&repository, &parent_tree, &tree, "", &mut touched);
        commits.push(touched);
    }
    commits
}

/// Record paths whose entries differ between two trees, recursing only into
/// subtrees whose ids changed — unchanged subtrees are skipped wholesale.
fn diff_trees(
    repository: &gix::Repository,
    before: &gix::Tree<'_>,
    after: &gix::Tree<'_>,
    prefix: &str,
    touched: &mut Vec<String>,
) {
    let before_entries = tree_entries(before);
    let after_entries = tree_entries(after);
    let names: BTreeSet<&BString> = before_entries.keys().chain(after_entries.keys()).collect();
    for name in names {
        let before_entry = before_entries.get(name).copied();
        let after_entry = after_entries.get(name).copied();
        if before_entry == after_entry {
            continue;
        }
        let path = format!("{prefix}{}", name.to_str_lossy());
        if let (Some((true, before_id)), Some((true, after_id))) = (before_entry, after_entry) {
            if let (Ok(before), Ok(after)) = (
                repository.find_tree(before_id),
                repository.find_tree(after_id),
            ) {
                diff_trees(repository, &before, &after, &format!("{path}/"), touched);
            }
            continue;
        }
        flatten_entry(repository, before_entry, &path, touched);
        flatten_entry(repository, after_entry, &path, touched);
    }
}

/// A changed entry is either a file — record its path — or a tree whose
/// entire contents appeared, vanished, or flipped kind: record every leaf.
fn flatten_entry(
    repository: &gix::Repository,
    entry: Option<(bool, gix::ObjectId)>,
    path: &str,
    touched: &mut Vec<String>,
) {
    match entry {
        Some((true, id)) => {
            if let Ok(tree) = repository.find_tree(id) {
                flatten_tree(repository, &tree, &format!("{path}/"), touched);
            }
        }
        Some((false, _)) => touched.push(path.to_owned()),
        None => {}
    }
}

fn flatten_tree(
    repository: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    touched: &mut Vec<String>,
) {
    for entry in tree.iter().flatten() {
        let path = format!("{prefix}{}", entry.filename().to_str_lossy());
        if entry.mode().is_tree() {
            if let Ok(subtree) = repository.find_tree(entry.object_id()) {
                flatten_tree(repository, &subtree, &format!("{path}/"), touched);
            }
        } else {
            touched.push(path);
        }
    }
}

/// Tree entries as `name → (is_tree, object id)` for cheap side-by-side
/// comparison; unreadable entries are skipped.
fn tree_entries(tree: &gix::Tree<'_>) -> BTreeMap<BString, (bool, gix::ObjectId)> {
    tree.iter()
        .flatten()
        .map(|entry| {
            (
                entry.filename().to_owned(),
                (entry.mode().is_tree(), entry.object_id()),
            )
        })
        .collect()
}

// --- per-commit diff content -------------------------------------------------

/// Blobs larger than this keep path-level history only — diffing them costs
/// more than the lineage evidence is worth.
const MAX_DIFF_BLOB_BYTES: usize = 200 * 1024;
/// Per-file cap on the rendered patch text that reaches the artifact store.
const MAX_FILE_PATCH_BYTES: usize = 8 * 1024;
/// Files past this count still list their path but contribute no patch
/// section — bounds mass commits (bulk renames, reformats).
const MAX_PATCHED_FILES_PER_COMMIT: usize = 512;
/// Identifier-carrying `+`/`-` lines kept per file in the document body.
const MAX_BODY_LINES_PER_FILE: usize = 32;
/// Whole-body cap for one commit document; the artifact keeps the full patch.
const MAX_DOCUMENT_BODY_BYTES: usize = 16 * 1024;
/// Commit-message bytes folded into the document body.
const MAX_MESSAGE_BYTES: usize = 1024;
/// Evidence addresses kept per commit document; each changed file emits at
/// most one covering address per side, so this bound only guards
/// pathological thousands-of-files commits.
const MAX_EVIDENCE_PER_COMMIT: usize = 128;

/// One commit's diff content prepared for indexing: `body` feeds FTS while
/// `patch` carries the complete unified-diff text for the artifact store.
#[derive(Debug, Clone)]
pub(crate) struct CommitDiff {
    /// Full commit id (hex).
    pub id: String,
    /// Committer timestamp, seconds since the epoch.
    pub timestamp: i64,
    /// First line of the commit message.
    pub subject: String,
    /// Document body: message, changed paths, identifier-ish diff lines.
    pub body: String,
    /// Full patch text; per-file sections are individually capped.
    pub patch: String,
    /// Evidence trail: added lines carry post-image line numbers, removed
    /// lines pre-image ones. Byte offsets are `0..0` — the coordinates
    /// describe the commit's blobs, not the indexed worktree.
    pub evidence: Vec<SourceAddress>,
}

/// A leaf-level difference between two commit trees: a blob id on each side
/// (`None` when the path only exists on the other side).
#[derive(Debug)]
struct TreeChange {
    path: String,
    before: Option<gix::ObjectId>,
    after: Option<gix::ObjectId>,
}

/// The extracted content of one file's diff between two blobs.
#[derive(Debug)]
struct FileExtraction {
    /// Rendered unified-diff hunks (`@@` sections, no file header), already
    /// capped at `MAX_FILE_PATCH_BYTES`.
    patch: String,
    /// Identifier-carrying added lines (trimmed).
    added_lines: Vec<String>,
    /// Identifier-carrying removed lines (trimmed).
    removed_lines: Vec<String>,
    /// 1-based inclusive new-file line ranges covering added lines.
    added_ranges: Vec<RangeInclusive<u32>>,
    /// 1-based inclusive old-file line ranges covering removed lines.
    removed_ranges: Vec<RangeInclusive<u32>>,
}

/// Where extracted evidence is anchored: the repository/snapshot the
/// [`SourceAddress`]es point into, plus the sensitive-name and repository
/// ignore policy. Historical payload must obey the same "never index" rules
/// as the worktree scan — a diffed path matching `.cceignore`/`.gitignore`
/// is recorded as a path note but contributes no content or evidence.
struct HistoryScope<'a> {
    repository_id: &'a str,
    snapshot_id: &'a str,
    include_sensitive: bool,
    ignores: &'a ignore::gitignore::Gitignore,
}

/// Per-commit diff content for lineage queries ("when did this line
/// change"). Each non-root commit is diffed against its first parent, newest
/// first. Sensitive and generated file names are excluded up front —
/// credential-shaped content must never reach a document or artifact.
/// Every failure degrades to a missing patch section or commit: history is
/// evidence, never a hard dependency of indexing.
pub(crate) fn commit_diffs(
    repository_root: &Path,
    maximum_commits: usize,
    repository_id: &str,
    snapshot_id: &str,
    include_sensitive: bool,
    respect_gitignore: bool,
) -> Vec<CommitDiff> {
    let mut commits = Vec::new();
    if !repository_root.join(".git").exists() {
        return commits;
    }
    let Ok(repository) = gix::open(repository_root) else {
        return commits;
    };
    let Ok(head) = repository.head_commit() else {
        return commits;
    };
    let Ok(walk) = head.ancestors().all() else {
        return commits;
    };
    let ignores = history_ignores(repository_root, respect_gitignore);
    let scope = HistoryScope {
        repository_id,
        snapshot_id,
        include_sensitive,
        ignores: &ignores,
    };
    for entry in walk.take(maximum_commits) {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(commit) = entry.object() else {
            continue;
        };
        let Some(parent_id) = commit.parent_ids().next() else {
            continue;
        };
        let (Ok(tree), Ok(parent)) = (commit.tree(), repository.find_commit(parent_id)) else {
            continue;
        };
        let Ok(parent_tree) = parent.tree() else {
            continue;
        };
        let mut changes = Vec::new();
        collect_tree_changes(&repository, &parent_tree, &tree, "", &mut changes);
        if changes.is_empty() {
            continue;
        }
        let message = commit.message_raw_sloppy().to_str_lossy().into_owned();
        let timestamp = commit.time().map_or(0, |time| time.seconds);
        commits.push(build_commit_diff(
            &repository,
            &entry.id.to_string(),
            &message,
            timestamp,
            &changes,
            &scope,
        ));
    }
    commits
}

/// Build the path matcher for historical payload: `.cceignore` always
/// applies (it is CCE's own "never index" contract), `.gitignore` and
/// `.git/info/exclude` follow the scan-time `respect_gitignore` flag.
fn history_ignores(
    repository_root: &Path,
    respect_gitignore: bool,
) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(repository_root);
    let _ = builder.add(repository_root.join(".cceignore"));
    if respect_gitignore {
        let _ = builder.add(repository_root.join(".gitignore"));
        let _ = builder.add(repository_root.join(".git/info/exclude"));
    }
    builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

/// Assemble one [`CommitDiff`]: render per-file patch sections into the
/// artifact-bound patch text, then build the bounded FTS body and the
/// evidence trail from the same extractions.
fn build_commit_diff(
    repository: &gix::Repository,
    commit_id: &str,
    message: &str,
    timestamp: i64,
    changes: &[TreeChange],
    scope: &HistoryScope<'_>,
) -> CommitDiff {
    let mut patch = String::new();
    let mut path_notes: Vec<String> = Vec::new();
    let mut line_sections: Vec<String> = Vec::new();
    let mut evidence: Vec<SourceAddress> = Vec::new();
    let mut patched_files = 0_usize;

    for change in changes {
        let file_name = change.path.rsplit('/').next().unwrap_or_default();
        if !scope.include_sensitive && crate::ignore::is_sensitive_name(file_name) {
            // The path itself is withheld too: sensitive names must never
            // enter a history document.
            continue;
        }
        if let Some(reason) = crate::ignore::builtin_skip_reason(file_name) {
            path_notes.push(format!("{} ({reason})", change.path));
            continue;
        }
        if scope
            .ignores
            .matched_path_or_any_parents(&change.path, false)
            .is_ignore()
        {
            path_notes.push(format!("{} (ignored)", change.path));
            continue;
        }
        if patched_files >= MAX_PATCHED_FILES_PER_COMMIT {
            path_notes.push(format!("{} (patch budget exhausted)", change.path));
            continue;
        }
        match extract_file(repository, change) {
            Ok(extraction) => {
                patched_files += 1;
                emit_patch_section(&mut patch, change, &extraction);
                emit_evidence(&mut evidence, scope, &change.path, &extraction);
                line_sections.push(render_line_section(&change.path, &extraction));
                path_notes.push(change.path.clone());
            }
            Err(reason) => path_notes.push(format!("{} ({reason})", change.path)),
        }
    }

    let mut body = String::new();
    let _ = writeln!(body, "commit {commit_id}");
    let _ = writeln!(body, "date: {timestamp}");
    let mut message = message.to_owned();
    truncate_utf8(&mut message, MAX_MESSAGE_BYTES, "");
    let _ = writeln!(body, "\n{message}\n\nchanged paths:");
    for note in &path_notes {
        let _ = writeln!(body, "- {note}");
    }
    if !line_sections.is_empty() {
        body.push_str("\ndiff lines:\n");
        for section in &line_sections {
            body.push_str(section);
        }
    }
    truncate_utf8(
        &mut body,
        MAX_DOCUMENT_BODY_BYTES,
        "\n… [document body truncated]\n",
    );
    let mut subject = message.lines().next().unwrap_or_default().to_owned();
    truncate_utf8(&mut subject, 160, "…");
    CommitDiff {
        id: commit_id.to_owned(),
        timestamp,
        subject,
        body,
        patch,
        evidence,
    }
}

/// Append `diff --git`/`---`/`+++` headers plus the rendered hunks for one
/// file to the commit's patch text.
fn emit_patch_section(patch: &mut String, change: &TreeChange, extraction: &FileExtraction) {
    let _ = writeln!(patch, "diff --git a/{0} b/{0}", change.path);
    match (change.before.is_some(), change.after.is_some()) {
        (false, true) => {
            let _ = writeln!(patch, "--- /dev/null\n+++ b/{}", change.path);
        }
        (true, false) => {
            let _ = writeln!(patch, "--- a/{}\n+++ /dev/null", change.path);
        }
        _ => {
            let _ = writeln!(patch, "--- a/{0}\n+++ b/{0}", change.path);
        }
    }
    patch.push_str(&extraction.patch);
}

/// Record one covering address per file side as evidence: additions merge
/// into a post-image range, removals into a pre-image range. A commit hit
/// surfaces each touched file once instead of once per hunk — evidence is
/// drill-down, not a per-hunk result list.
fn emit_evidence(
    evidence: &mut Vec<SourceAddress>,
    scope: &HistoryScope<'_>,
    path: &str,
    extraction: &FileExtraction,
) {
    for ranges in [&extraction.added_ranges, &extraction.removed_ranges] {
        if ranges.is_empty() || evidence.len() >= MAX_EVIDENCE_PER_COMMIT {
            continue;
        }
        let start = ranges.iter().map(RangeInclusive::start).min().copied();
        let end = ranges.iter().map(RangeInclusive::end).max().copied();
        if let (Some(start), Some(end)) = (start, end) {
            if let Ok(address) = SourceAddress::new(
                scope.repository_id,
                scope.snapshot_id,
                path,
                0..0,
                start..=end,
            ) {
                evidence.push(address);
            }
        }
    }
}

/// The document-body section for one file: identifier-carrying `+`/`-`
/// lines only, so punctuation-only churn stays out of FTS.
fn render_line_section(path: &str, extraction: &FileExtraction) -> String {
    let mut section = format!("### {path}\n");
    for line in &extraction.added_lines {
        let _ = writeln!(section, "+{line}");
    }
    for line in &extraction.removed_lines {
        let _ = writeln!(section, "-{line}");
    }
    section
}

/// Load both sides of a [`TreeChange`] as text and diff them. `Err` carries
/// a short human-readable reason recorded next to the path in the body.
fn extract_file(
    repository: &gix::Repository,
    change: &TreeChange,
) -> Result<FileExtraction, &'static str> {
    let before = load_text(repository, change.before)?;
    let after = load_text(repository, change.after)?;
    Ok(diff_texts(&before, &after))
}

/// A blob's UTF-8 text, or `""` when the side does not exist (add/delete).
fn load_text(
    repository: &gix::Repository,
    id: Option<gix::ObjectId>,
) -> Result<String, &'static str> {
    let Some(id) = id else {
        return Ok(String::new());
    };
    let blob = repository.find_blob(id).map_err(|_| "unreadable")?;
    if blob.data.len() > MAX_DIFF_BLOB_BYTES {
        return Err("too large");
    }
    if crate::repository::is_probably_binary(&blob.data) {
        return Err("binary");
    }
    String::from_utf8(blob.data.clone()).map_err(|_| "binary")
}

/// Histogram-diff two file texts, render the unified hunks (3 lines of
/// context, slider postprocessing matching `git diff`), and collect the
/// identifier-carrying changed lines plus their line ranges.
fn diff_texts(before: &str, after: &str) -> FileExtraction {
    let input = imara_diff::InternedInput::new(before, after);
    let mut diff = imara_diff::Diff::compute(imara_diff::Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let mut patch = diff
        .unified_diff(
            &imara_diff::BasicLineDiffPrinter(&input.interner),
            imara_diff::UnifiedDiffConfig::default(),
            &input,
        )
        .to_string();
    truncate_utf8(&mut patch, MAX_FILE_PATCH_BYTES, "… [diff truncated]\n");

    let mut extraction = FileExtraction {
        patch,
        added_lines: Vec::new(),
        removed_lines: Vec::new(),
        added_ranges: Vec::new(),
        removed_ranges: Vec::new(),
    };
    for hunk in diff.hunks() {
        if !hunk.before.is_empty() {
            extraction
                .removed_ranges
                .push((hunk.before.start + 1)..=hunk.before.end);
        }
        if !hunk.after.is_empty() {
            extraction
                .added_ranges
                .push((hunk.after.start + 1)..=hunk.after.end);
        }
        collect_lines(
            &input.before,
            &input.interner,
            hunk.before,
            &mut extraction.removed_lines,
        );
        collect_lines(
            &input.after,
            &input.interner,
            hunk.after,
            &mut extraction.added_lines,
        );
    }
    extraction
}

/// Copy the identifier-carrying lines of a hunk side into `sink`, bounded
/// per file so the document body stays compact.
fn collect_lines(
    tokens: &[imara_diff::Token],
    interner: &imara_diff::Interner<&str>,
    range: std::ops::Range<u32>,
    sink: &mut Vec<String>,
) {
    let Some(tokens) = tokens.get(range.start as usize..range.end as usize) else {
        return;
    };
    for token in tokens {
        if sink.len() >= MAX_BODY_LINES_PER_FILE {
            return;
        }
        let line = interner[*token].trim_end();
        if is_identifierish(line) {
            sink.push(line.to_owned());
        }
    }
}

/// A diff line is worth indexing when it carries an identifier-ish run of
/// at least three characters — punctuation-only churn (`}`, `});`) adds no
/// queryable signal.
fn is_identifierish(line: &str) -> bool {
    let mut run = 0_usize;
    for character in line.chars() {
        if character.is_alphanumeric() || character == '_' {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Cut `text` to at most `maximum` bytes on a `char` boundary, appending
/// `marker` when truncation happened.
fn truncate_utf8(text: &mut String, maximum: usize, marker: &str) {
    if text.len() <= maximum {
        return;
    }
    let mut end = maximum;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(marker);
}

/// Which side of a tree-level change a flattened leaf belongs to.
#[derive(Debug, Clone, Copy)]
enum ChangeSide {
    Before,
    After,
}

/// Record `(path, before id, after id)` for every leaf that differs between
/// two trees, recursing only into subtrees whose ids changed.
fn collect_tree_changes(
    repository: &gix::Repository,
    before: &gix::Tree<'_>,
    after: &gix::Tree<'_>,
    prefix: &str,
    changes: &mut Vec<TreeChange>,
) {
    let before_entries = tree_entries(before);
    let after_entries = tree_entries(after);
    let names: BTreeSet<&BString> = before_entries.keys().chain(after_entries.keys()).collect();
    for name in names {
        let before_entry = before_entries.get(name).copied();
        let after_entry = after_entries.get(name).copied();
        if before_entry == after_entry {
            continue;
        }
        let path = format!("{prefix}{}", name.to_str_lossy());
        if let (Some((true, before_id)), Some((true, after_id))) = (before_entry, after_entry) {
            if let (Ok(before), Ok(after)) = (
                repository.find_tree(before_id),
                repository.find_tree(after_id),
            ) {
                collect_tree_changes(repository, &before, &after, &format!("{path}/"), changes);
            }
            continue;
        }
        flatten_change(repository, before_entry, after_entry, &path, changes);
    }
}

/// Record a non-tree-vs-tree difference: leaf-vs-leaf becomes one paired
/// change; kind flips and add/remove subtrees decompose into one-sided leaf
/// changes so every blob remains diffable.
fn flatten_change(
    repository: &gix::Repository,
    before: Option<(bool, gix::ObjectId)>,
    after: Option<(bool, gix::ObjectId)>,
    path: &str,
    changes: &mut Vec<TreeChange>,
) {
    if let (Some((false, before_id)), Some((false, after_id))) = (before, after) {
        changes.push(TreeChange {
            path: path.to_owned(),
            before: Some(before_id),
            after: Some(after_id),
        });
        return;
    }
    flatten_entry_change(repository, before, path, ChangeSide::Before, changes);
    flatten_entry_change(repository, after, path, ChangeSide::After, changes);
}

/// Record one side of a changed entry: a leaf becomes a one-sided change, a
/// tree expands into one-sided changes for every leaf inside.
fn flatten_entry_change(
    repository: &gix::Repository,
    entry: Option<(bool, gix::ObjectId)>,
    path: &str,
    side: ChangeSide,
    changes: &mut Vec<TreeChange>,
) {
    match entry {
        Some((true, id)) => {
            if let Ok(tree) = repository.find_tree(id) {
                flatten_tree_changes(repository, &tree, &format!("{path}/"), side, changes);
            }
        }
        Some((false, id)) => {
            let (before, after) = match side {
                ChangeSide::Before => (Some(id), None),
                ChangeSide::After => (None, Some(id)),
            };
            changes.push(TreeChange {
                path: path.to_owned(),
                before,
                after,
            });
        }
        None => {}
    }
}

/// Push a one-sided change for every leaf under `tree`.
fn flatten_tree_changes(
    repository: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    side: ChangeSide,
    changes: &mut Vec<TreeChange>,
) {
    for entry in tree.iter().flatten() {
        let path = format!("{prefix}{}", entry.filename().to_str_lossy());
        if entry.mode().is_tree() {
            if let Ok(subtree) = repository.find_tree(entry.object_id()) {
                flatten_tree_changes(repository, &subtree, &format!("{path}/"), side, changes);
            }
        } else {
            let (before, after) = match side {
                ChangeSide::Before => (Some(entry.object_id()), None),
                ChangeSide::After => (None, Some(entry.object_id())),
            };
            changes.push(TreeChange {
                path,
                before,
                after,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_texts_extracts_changed_lines_and_ranges() {
        let before = "fn alpha() -> u32 {\n    compute_old()\n}\n";
        let after = "fn alpha() -> u32 {\n    compute_value()\n}\n\nfn beta_added() {}\n";
        let extraction = diff_texts(before, after);
        assert!(extraction.patch.contains("@@"));
        assert!(extraction.patch.contains("-    compute_old()\n"));
        assert!(extraction.patch.contains("+    compute_value()\n"));
        assert_eq!(extraction.added_ranges.first(), Some(&(2..=2)));
        assert_eq!(extraction.removed_ranges, vec![2..=2]);
        assert!(
            extraction
                .added_lines
                .iter()
                .any(|line| line.contains("compute_value"))
        );
        assert!(
            extraction
                .added_lines
                .iter()
                .any(|line| line.contains("beta_added"))
        );
        assert_eq!(
            extraction.removed_lines,
            vec!["    compute_old()".to_owned()]
        );
    }

    #[test]
    fn diff_texts_renders_pure_addition() {
        let extraction = diff_texts("", "fn created() {}\n");
        assert!(extraction.patch.contains("+fn created() {}"));
        assert_eq!(extraction.added_ranges, vec![1..=1]);
        assert!(extraction.removed_lines.is_empty());
    }

    #[test]
    fn diff_texts_caps_patch_size() {
        let before = "let v = 0;\n".repeat(400);
        let mut after = String::new();
        for index in 0..400 {
            let _ = writeln!(after, "let v{index} = {index};");
        }
        let extraction = diff_texts(&before, &after);
        assert!(extraction.patch.len() <= MAX_FILE_PATCH_BYTES + 64);
        assert!(extraction.patch.contains("[diff truncated]"));
    }

    #[test]
    fn identifierish_filters_punctuation_only_lines() {
        assert!(is_identifierish("let compute_value = 1;"));
        assert!(is_identifierish("fn beta_added() {}"));
        assert!(!is_identifierish("}"));
        assert!(!is_identifierish("});"));
        assert!(!is_identifierish(""));
        assert!(!is_identifierish("//"));
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        let mut text = "abédef".to_owned();
        truncate_utf8(&mut text, 4, "…");
        assert_eq!(text, "abé…");
        let mut short = "abc".to_owned();
        truncate_utf8(&mut short, 10, "…");
        assert_eq!(short, "abc");
    }
}
