use std::path::Path;

use gix::bstr::ByteSlice;

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
