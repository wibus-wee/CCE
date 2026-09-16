use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use gix::bstr::{BString, ByteSlice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistorySummary {
    pub(crate) body: String,
    pub(crate) commit_count: usize,
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
