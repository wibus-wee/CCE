use std::path::Path;

use gix::bstr::ByteSlice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistorySummary {
    pub(crate) body: String,
    pub(crate) commit_count: usize,
    pub(crate) commits: Vec<HistoryCommit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryCommit {
    pub(crate) id: String,
    pub(crate) subject: String,
    pub(crate) body: String,
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
    let mut commits = Vec::new();
    let mut commit_count = 0_usize;
    for entry in walk.take(maximum_commits) {
        let entry = entry.map_err(|error| error.to_string())?;
        let commit = entry.object().map_err(|error| error.to_string())?;
        let message = commit
            .message_raw_sloppy()
            .lines()
            .next()
            .unwrap_or_default()
            .to_str_lossy()
            .into_owned();
        let timestamp = commit.time().map_err(|error| error.to_string())?.seconds;
        let id = entry.id.to_string();
        let changed_paths = changed_paths(&repository, &commit)?;
        let mut body = vec![
            format!("Commit: {id}"),
            format!("Unix timestamp: {timestamp}"),
            format!("Subject: {message}"),
        ];
        if !changed_paths.is_empty() {
            body.push("Changed paths:".to_owned());
            body.extend(changed_paths.iter().map(|path| format!("- {path}")));
        }
        lines.push(format!("- `{id}` ({timestamp}): {message}"));
        commits.push(HistoryCommit {
            id,
            subject: message,
            body: body.join("\n"),
        });
        commit_count += 1;
    }
    Ok(Some(HistorySummary {
        body: lines.join("\n"),
        commit_count,
        commits,
    }))
}

fn changed_paths(
    repository: &gix::Repository,
    commit: &gix::Commit<'_>,
) -> Result<Vec<String>, String> {
    let current_tree = commit.tree().map_err(|error| error.to_string())?;
    let parent_tree = commit
        .parent_ids()
        .next()
        .map(|parent| {
            parent
                .object()
                .map_err(|error| error.to_string())?
                .peel_to_commit()
                .map_err(|error| error.to_string())?
                .tree()
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let changes = repository
        .diff_tree_to_tree(
            parent_tree.as_ref(),
            Some(&current_tree),
            Some(gix::diff::Options::default()),
        )
        .map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for change in changes {
        use gix::diff::tree_with_rewrites::Change;
        match change {
            Change::Addition { location, .. }
            | Change::Deletion { location, .. }
            | Change::Modification { location, .. } => {
                paths.push(location.to_str_lossy().into_owned());
            }
            Change::Rewrite {
                source_location,
                location,
                ..
            } => {
                paths.push(source_location.to_str_lossy().into_owned());
                paths.push(location.to_str_lossy().into_owned());
            }
        }
    }
    paths.sort();
    paths.dedup();
    if paths.len() > 64 {
        paths.truncate(64);
        paths.push("<additional paths omitted>".to_owned());
    }
    Ok(paths)
}
