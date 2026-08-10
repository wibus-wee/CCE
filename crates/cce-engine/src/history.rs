use std::path::Path;

use gix::bstr::ByteSlice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistorySummary {
    pub(crate) body: String,
    pub(crate) commit_count: usize,
    pub(crate) commits: Vec<HistoryCommit>,
    pub(crate) shallow_boundary: bool,
    pub(crate) omitted_parent_diffs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryCommit {
    pub(crate) id: String,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) changed_paths: Vec<String>,
}

pub(crate) fn summarize(
    repository_root: &Path,
    maximum_commits: usize,
) -> Result<Option<HistorySummary>, String> {
    if !repository_root.join(".git").exists() {
        return Ok(None);
    }
    let repository = gix::open(repository_root).map_err(|error| error.to_string())?;
    let is_shallow = repository.is_shallow();
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
    let mut shallow_boundary = false;
    let mut omitted_parent_diffs = 0_usize;
    for entry in walk.take(maximum_commits) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) if is_shallow && commit_count > 0 => {
                shallow_boundary = true;
                break;
            }
            Err(error) => return Err(error.to_string()),
        };
        let commit = match entry.object() {
            Ok(commit) => commit,
            Err(_) if is_shallow && commit_count > 0 => {
                shallow_boundary = true;
                break;
            }
            Err(error) => return Err(error.to_string()),
        };
        let message = commit
            .message_raw_sloppy()
            .lines()
            .next()
            .unwrap_or_default()
            .to_str_lossy()
            .into_owned();
        let timestamp = commit.time().map_err(|error| error.to_string())?.seconds;
        let id = entry.id.to_string();
        let (changed_paths, parent_diff_omitted) = changed_paths(&repository, &commit, is_shallow)?;
        if parent_diff_omitted {
            shallow_boundary = true;
            omitted_parent_diffs += 1;
        }
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
            changed_paths,
        });
        commit_count += 1;
    }
    if is_shallow {
        shallow_boundary = true;
        lines.push(String::new());
        lines.push(
            "History is bounded by the locally available shallow clone; unavailable ancestors were not inferred."
                .to_owned(),
        );
    }
    Ok(Some(HistorySummary {
        body: lines.join("\n"),
        commit_count,
        commits,
        shallow_boundary,
        omitted_parent_diffs,
    }))
}

fn changed_paths(
    repository: &gix::Repository,
    commit: &gix::Commit<'_>,
    is_shallow: bool,
) -> Result<(Vec<String>, bool), String> {
    let current_tree = commit.tree().map_err(|error| error.to_string())?;
    let mut parent_diff_omitted = false;
    let parent_tree = match commit.parent_ids().next() {
        Some(parent) => match parent.object() {
            Ok(parent) => Some(
                parent
                    .peel_to_commit()
                    .map_err(|error| error.to_string())?
                    .tree()
                    .map_err(|error| error.to_string())?,
            ),
            Err(_) if is_shallow => {
                parent_diff_omitted = true;
                None
            }
            Err(error) => return Err(error.to_string()),
        },
        None => None,
    };
    if parent_diff_omitted {
        return Ok((Vec::new(), true));
    }
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
    Ok((paths, false))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn git(current_directory: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(current_directory)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn shallow_clone_retains_reachable_history_without_inventing_parent_diff() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = temporary.path().join("source");
        let clone = temporary.path().join("clone");
        std::fs::create_dir(&source).expect("source directory");
        git(&source, &["init", "--quiet"]);
        std::fs::write(source.join("history.txt"), "first\n").expect("first revision");
        git(&source, &["add", "history.txt"]);
        git(
            &source,
            &[
                "-c",
                "user.name=CCE Test",
                "-c",
                "user.email=cce@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "first",
            ],
        );
        std::fs::write(source.join("history.txt"), "second\n").expect("second revision");
        git(&source, &["add", "history.txt"]);
        git(
            &source,
            &[
                "-c",
                "user.name=CCE Test",
                "-c",
                "user.email=cce@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "second",
            ],
        );
        let output = Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", "--no-local"])
            .arg(&source)
            .arg(&clone)
            .output()
            .expect("clone shallow repository");
        assert!(
            output.status.success(),
            "shallow clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let summary = summarize(&clone, 32)
            .expect("summarize shallow clone")
            .expect("history summary");

        assert_eq!(summary.commit_count, 1);
        assert!(summary.shallow_boundary);
        assert_eq!(summary.omitted_parent_diffs, 1);
        assert!(summary.commits[0].changed_paths.is_empty());
        assert!(
            summary
                .body
                .contains("unavailable ancestors were not inferred")
        );
    }
}
