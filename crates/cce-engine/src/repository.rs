use std::{
    fs,
    path::{Path, PathBuf},
};

use cce_core::{CceError, RepositoryIdentity, Result, SnapshotIdentity};
use chrono::Utc;
use ignore::{DirEntry, WalkBuilder};

use crate::EngineConfig;

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub language: Option<String>,
    pub bytes: Vec<u8>,
    pub content_hash: String,
    pub line_count: u64,
}

impl ScannedFile {
    pub fn text(&self) -> Result<&str> {
        std::str::from_utf8(&self.bytes).map_err(|error| {
            CceError::Configuration(format!("{} is not UTF-8: {error}", self.relative_path))
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScannedRepository {
    pub identity: RepositoryIdentity,
    pub snapshot: SnapshotIdentity,
    pub files: Vec<ScannedFile>,
    pub skipped_large_files: Vec<String>,
    pub skipped_binary_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RepositoryScanner {
    config: EngineConfig,
}

impl RepositoryScanner {
    #[must_use]
    pub const fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    pub fn scan(&self) -> Result<ScannedRepository> {
        let root = fs::canonicalize(&self.config.repository_root)
            .map_err(|error| CceError::io(&self.config.repository_root, error))?;
        if !root.is_dir() {
            return Err(CceError::InvalidRepository(root));
        }

        let git_directory = discover_git_directory(&root);
        let base_revision = git_directory.as_deref().and_then(read_head_revision);
        let remote = git_directory.as_deref().and_then(read_origin_remote);
        let identity_basis = remote
            .as_deref()
            .unwrap_or_else(|| root.to_str().unwrap_or("repository"));
        let repository_id = format!(
            "repo_{}",
            &blake3::hash(identity_basis.as_bytes()).to_hex()[..24]
        );
        let identity = RepositoryIdentity {
            id: repository_id.clone(),
            canonical_root: root.to_string_lossy().to_string(),
            remote,
        };

        let mut builder = WalkBuilder::new(&root);
        builder
            .hidden(!self.config.index.include_hidden)
            .git_ignore(self.config.index.respect_gitignore)
            .git_exclude(self.config.index.respect_gitignore)
            .git_global(false)
            .parents(self.config.index.respect_gitignore)
            .add_custom_ignore_filename(".cceignore")
            .follow_links(false)
            .filter_entry(|entry| !is_internal_or_generated(entry));

        let mut files = Vec::new();
        let mut skipped_large_files = Vec::new();
        let mut skipped_binary_files = Vec::new();
        for entry in builder.build() {
            let entry = entry.map_err(|error| CceError::Configuration(error.to_string()))?;
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() || file_type.is_symlink() {
                continue;
            }
            let path = entry.into_path();
            let metadata = fs::metadata(&path).map_err(|error| CceError::io(&path, error))?;
            let relative_path = normalized_relative(&root, &path)?;
            if metadata.len() > self.config.index.max_file_bytes {
                skipped_large_files.push(relative_path);
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| CceError::io(&path, error))?;
            if is_probably_binary(&bytes) {
                skipped_binary_files.push(relative_path);
                continue;
            }
            let content_hash = blake3::hash(&bytes).to_hex().to_string();
            let line_count = if bytes.is_empty() {
                0
            } else {
                bytes.iter().filter(|byte| **byte == b'\n').count() as u64 + 1
            };
            files.push(ScannedFile {
                language: language_for_path(&path).map(str::to_owned),
                relative_path,
                absolute_path: path,
                bytes,
                content_hash,
                line_count,
            });
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        let mut overlay_hasher = blake3::Hasher::new();
        let mut source_bytes = 0_u64;
        for file in &files {
            overlay_hasher.update(file.relative_path.as_bytes());
            overlay_hasher.update(&[0]);
            overlay_hasher.update(file.content_hash.as_bytes());
            overlay_hasher.update(&[0]);
            source_bytes = source_bytes.saturating_add(file.bytes.len() as u64);
        }
        let workspace_overlay_hash = overlay_hasher.finalize().to_hex().to_string();
        let profile_json = serde_json::to_vec(&self.config.profile())?;
        let index_profile_hash = blake3::hash(&profile_json).to_hex().to_string();
        let mut snapshot_hasher = blake3::Hasher::new();
        snapshot_hasher.update(repository_id.as_bytes());
        snapshot_hasher.update(base_revision.as_deref().unwrap_or("unborn").as_bytes());
        snapshot_hasher.update(workspace_overlay_hash.as_bytes());
        snapshot_hasher.update(index_profile_hash.as_bytes());
        let snapshot = SnapshotIdentity {
            id: format!("snap_{}", snapshot_hasher.finalize().to_hex()),
            repository_id,
            base_revision,
            workspace_overlay_hash,
            index_profile_hash,
            created_at: Utc::now(),
            file_count: files.len() as u64,
            source_bytes,
        };

        Ok(ScannedRepository {
            identity,
            snapshot,
            files,
            skipped_large_files,
            skipped_binary_files,
        })
    }
}

fn is_internal_or_generated(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    matches!(
        entry.file_name().to_str(),
        Some(".git" | ".cce" | "target" | "node_modules" | ".venv" | "__pycache__")
    )
}

fn normalized_relative(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| CceError::PathEscape(path.to_path_buf()))
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8_192).any(|byte| *byte == 0) || std::str::from_utf8(bytes).is_err()
}

fn language_for_path(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some("cpp"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "py" | "pyi" => Some("python"),
        "go" => Some("go"),
        "java" => Some("java"),
        "cs" => Some("csharp"),
        "md" | "mdx" => Some("markdown"),
        "json" | "jsonc" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "sql" => Some("sql"),
        _ => None,
    }
}

fn discover_git_directory(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() {
        return Some(marker);
    }
    let contents = fs::read_to_string(&marker).ok()?;
    let value = contents.trim().strip_prefix("gitdir:")?.trim();
    let candidate = PathBuf::from(value);
    Some(if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    })
}

fn read_head_revision(git: &Path) -> Option<String> {
    let head = fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ") else {
        return valid_oid(head).then(|| head.to_owned());
    };
    let loose = fs::read_to_string(git.join(reference)).ok();
    if let Some(oid) = loose.as_deref().map(str::trim).filter(|oid| valid_oid(oid)) {
        return Some(oid.to_owned());
    }
    fs::read_to_string(git.join("packed-refs"))
        .ok()?
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .find_map(|line| {
            let (oid, name) = line.split_once(' ')?;
            (name == reference && valid_oid(oid)).then(|| oid.to_owned())
        })
}

fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_origin_remote(git: &Path) -> Option<String> {
    let config = fs::read_to_string(git.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(value) = trimmed
                .strip_prefix("url")
                .and_then(|rest| rest.split_once('=').map(|pair| pair.1.trim()))
            {
                return Some(value.to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_binary_nul() {
        assert!(is_probably_binary(b"hello\0world"));
        assert!(!is_probably_binary(b"hello\nworld"));
    }

    #[test]
    fn recognizes_languages() {
        assert_eq!(language_for_path(Path::new("src/lib.rs")), Some("rust"));
        assert_eq!(language_for_path(Path::new("app/page.tsx")), Some("tsx"));
    }
}
