use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
};

use cce_core::{CceError, RepositoryIdentity, Result, SnapshotIdentity};
use cce_store::{MetadataStore, ScanCacheEntry};
use chrono::Utc;
use ignore::WalkBuilder;

use crate::{
    EngineConfig,
    ignore::{builtin_skip_reason, is_internal_or_generated, is_sensitive_name},
};

/// One file discovered by the repository scan, with content-hash identity.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Repository-relative path.
    pub relative_path: String,
    /// Absolute filesystem path.
    pub absolute_path: PathBuf,
    /// Detected language, when known.
    pub language: Option<String>,
    /// BLAKE3 content hash.
    pub content_hash: String,
    /// File size.
    pub size_bytes: u64,
    /// Modification time (epoch ms).
    pub mtime_ms: i64,
    /// Line count.
    pub line_count: u64,
    /// Bytes read during the scan when the mtime/size cache missed. `None`
    /// means the hash came from the cache and the content has not been read.
    pub cached_bytes: Option<Vec<u8>>,
}

impl ScannedFile {
    /// File bytes. Reads and hash-verifies from disk when the scan did not
    /// already hold the content; a mismatch means the file changed between
    /// scanning and indexing, so the snapshot would not identify these bytes.
    pub fn bytes(&self) -> Result<Cow<'_, [u8]>> {
        if let Some(bytes) = &self.cached_bytes {
            return Ok(Cow::Borrowed(bytes));
        }
        let bytes = fs::read(&self.absolute_path)
            .map_err(|error| CceError::io(&self.absolute_path, error))?;
        if blake3::hash(&bytes).to_hex().as_str() != self.content_hash {
            return Err(CceError::Configuration(format!(
                "{} changed during indexing; rerun `cce index`",
                self.relative_path
            )));
        }
        Ok(Cow::Owned(bytes))
    }

    /// UTF-8 view of `bytes()`.
    ///
    /// # Errors
    /// `Configuration` when the file is not UTF-8 or changed mid-index.
    pub fn text(&self) -> Result<Cow<'_, str>> {
        match self.bytes()? {
            Cow::Borrowed(bytes) => {
                std::str::from_utf8(bytes)
                    .map(Cow::Borrowed)
                    .map_err(|error| {
                        CceError::Configuration(format!(
                            "{} is not UTF-8: {error}",
                            self.relative_path
                        ))
                    })
            }
            Cow::Owned(bytes) => String::from_utf8(bytes).map(Cow::Owned).map_err(|error| {
                CceError::Configuration(format!("{} is not UTF-8: {error}", self.relative_path))
            }),
        }
    }

    fn cache_entry(&self) -> ScanCacheEntry {
        ScanCacheEntry {
            path: self.relative_path.clone(),
            size_bytes: self.size_bytes,
            mtime_ms: self.mtime_ms,
            line_count: self.line_count,
            content_hash: self.content_hash.clone(),
        }
    }
}

/// Repository identity resolved without walking the file tree.
#[derive(Debug, Clone)]
pub struct RepositoryAnchor {
    pub root: PathBuf,
    pub identity: RepositoryIdentity,
    pub base_revision: Option<String>,
}

/// Result of a full repository scan: identity, snapshot key, files, and
/// per-reason skip lists.
#[derive(Debug, Clone)]
pub struct ScannedRepository {
    /// Repository identity derived from the root path.
    pub identity: RepositoryIdentity,
    /// Snapshot identity the scan implies (content hash + profile).
    pub snapshot: SnapshotIdentity,
    /// Files eligible for indexing.
    pub files: Vec<ScannedFile>,
    /// Files skipped for exceeding `max_file_bytes`.
    pub skipped_large_files: Vec<String>,
    /// Binary files skipped.
    pub skipped_binary_files: Vec<String>,
    /// Sensitive files skipped (`.env`, keys, …).
    pub skipped_sensitive_files: Vec<String>,
    /// Files dropped by the unconditional built-in policy (lockfiles,
    /// minified assets), each paired with its skip reason.
    pub skipped_builtin_files: Vec<(String, &'static str)>,
}

/// Walks the repository honoring ignore rules, size limits, and sensitive-
/// file policy; computes content hashes and the snapshot identity.
#[derive(Debug, Clone)]
pub struct RepositoryScanner {
    config: EngineConfig,
}

impl RepositoryScanner {
    /// Create a scanner bound to `config`.
    #[must_use]
    pub const fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    /// Resolve repository identity (root, remote, HEAD) without reading any
    /// file contents.
    pub fn identify(&self) -> Result<RepositoryAnchor> {
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
        let canonical_root = root.to_string_lossy().to_string();
        Ok(RepositoryAnchor {
            root,
            identity: RepositoryIdentity {
                id: repository_id,
                canonical_root,
                remote,
            },
            base_revision,
        })
    }

    /// Walk the repository and compute a snapshot identity. Unchanged files
    /// reuse content hashes from `store`'s scan cache (keyed by size+mtime)
    /// instead of being read; everything else is read once here.
    pub fn scan(&self, store: Option<&MetadataStore>) -> Result<ScannedRepository> {
        let anchor = self.identify()?;
        let repository_id = anchor.identity.id.clone();

        // The configured data dir is never source, whatever its name:
        // artifacts inside it change on every request, and scanning them
        // back in would make each `require_fresh` mint a new snapshot.
        // Excluded by resolved path so `CCE_DATA_DIR` names the built-in
        // name policy does not know are still pruned. A data dir equal to
        // the root itself is left alone — the filter cannot prune the
        // walker's own root entry.
        let data_root =
            resolve_data_root(&self.config.data_root).filter(|root| *root != anchor.root);

        let mut builder = WalkBuilder::new(&anchor.root);
        builder
            .hidden(!self.config.index.include_hidden)
            .git_ignore(self.config.index.respect_gitignore)
            .git_exclude(self.config.index.respect_gitignore)
            .git_global(false)
            .parents(self.config.index.respect_gitignore)
            .add_custom_ignore_filename(".cceignore")
            .follow_links(false)
            .filter_entry(move |entry| {
                !is_internal_or_generated(entry)
                    && data_root
                        .as_ref()
                        .is_none_or(|root| !entry.path().starts_with(root))
            });

        let mut files = Vec::new();
        let mut skipped_large_files = Vec::new();
        let mut skipped_binary_files = Vec::new();
        let mut skipped_sensitive_files = Vec::new();
        let mut skipped_builtin_files = Vec::new();
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
            let relative_path = normalized_relative(&anchor.root, &path)?;
            if metadata.len() > self.config.index.max_file_bytes {
                skipped_large_files.push(relative_path);
                continue;
            }
            let mtime_ms = metadata
                .modified()
                .ok()
                .and_then(|modified| {
                    modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                })
                .unwrap_or_default();
            let file_name = entry_file_name(&path);
            if let Some(reason) = builtin_skip_reason(file_name) {
                skipped_builtin_files.push((relative_path, reason));
                continue;
            }
            if !self.config.index.include_sensitive && is_sensitive_name(file_name) {
                skipped_sensitive_files.push(relative_path);
                continue;
            }
            let cached = store.and_then(|store| {
                store
                    .scan_cache_lookup(&repository_id, &relative_path, metadata.len(), mtime_ms)
                    .ok()
                    .flatten()
            });
            let (bytes, content_hash, line_count) = if let Some((hash, lines)) = cached {
                (None, hash, lines)
            } else {
                let bytes = fs::read(&path).map_err(|error| CceError::io(&path, error))?;
                if is_probably_binary(&bytes) {
                    skipped_binary_files.push(relative_path);
                    continue;
                }
                let line_count = if bytes.is_empty() {
                    0
                } else {
                    bytes.iter().filter(|byte| **byte == b'\n').count() as u64 + 1
                };
                let content_hash = blake3::hash(&bytes).to_hex().to_string();
                (Some(bytes), content_hash, line_count)
            };
            files.push(ScannedFile {
                language: language_for_path(&path).map(str::to_owned),
                relative_path,
                absolute_path: path,
                content_hash,
                size_bytes: metadata.len(),
                mtime_ms,
                line_count,
                cached_bytes: bytes,
            });
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        if let Some(store) = store {
            // scan_cache.repository_id references repositories(id), so the
            // repository row must exist before the cache can be written.
            if let Err(error) = store.register_repository(&anchor.identity) {
                tracing::warn!(%error, "could not register repository");
            } else {
                let entries = files
                    .iter()
                    .map(ScannedFile::cache_entry)
                    .collect::<Vec<_>>();
                if let Err(error) = store.update_scan_cache(&repository_id, &entries) {
                    tracing::warn!(%error, "could not update scan cache");
                }
            }
        }

        let mut overlay_hasher = blake3::Hasher::new();
        let mut source_bytes = 0_u64;
        for file in &files {
            overlay_hasher.update(file.relative_path.as_bytes());
            overlay_hasher.update(&[0]);
            overlay_hasher.update(file.content_hash.as_bytes());
            overlay_hasher.update(&[0]);
            source_bytes = source_bytes.saturating_add(file.size_bytes);
        }
        let workspace_overlay_hash = overlay_hasher.finalize().to_hex().to_string();
        let profile_json = serde_json::to_vec(&self.config.profile())?;
        let index_profile_hash = blake3::hash(&profile_json).to_hex().to_string();
        let mut snapshot_hasher = blake3::Hasher::new();
        snapshot_hasher.update(repository_id.as_bytes());
        snapshot_hasher.update(
            anchor
                .base_revision
                .as_deref()
                .unwrap_or("unborn")
                .as_bytes(),
        );
        snapshot_hasher.update(workspace_overlay_hash.as_bytes());
        snapshot_hasher.update(index_profile_hash.as_bytes());
        let snapshot = SnapshotIdentity {
            id: format!("snap_{}", snapshot_hasher.finalize().to_hex()),
            repository_id,
            base_revision: anchor.base_revision,
            workspace_overlay_hash,
            index_profile_hash,
            created_at: Utc::now(),
            file_count: files.len() as u64,
            source_bytes,
        };

        Ok(ScannedRepository {
            identity: anchor.identity,
            snapshot,
            files,
            skipped_large_files,
            skipped_binary_files,
            skipped_sensitive_files,
            skipped_builtin_files,
        })
    }
}

fn entry_file_name(path: &Path) -> &str {
    path.file_name()
        .map_or("", |name| name.to_str().unwrap_or(""))
}

/// The configured data dir as a path comparable to walk entries:
/// canonicalized when it exists, else the deepest existing ancestor
/// canonicalized with the missing tail re-attached — a not-yet-created
/// data dir under a symlinked parent still resolves like the walker's
/// canonical anchor root does.
fn resolve_data_root(data_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = data_root.canonicalize() {
        return Some(path);
    }
    let absolute = std::path::absolute(data_root).ok()?;
    let mut tail = Vec::new();
    let mut probe = absolute.as_path();
    loop {
        if let Ok(mut base) = probe.canonicalize() {
            for component in tail.iter().rev() {
                base.push(component);
            }
            return Some(base);
        }
        tail.push(probe.file_name()?.to_os_string());
        probe = probe.parent()?;
    }
}

fn normalized_relative(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| CceError::PathEscape(path.to_path_buf()))
}

pub(crate) fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8_192).any(|byte| *byte == 0) || std::str::from_utf8(bytes).is_err()
}

fn language_for_path(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
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

    #[test]
    fn flags_sensitive_names() {
        assert!(is_sensitive_name(".env"));
        assert!(is_sensitive_name(".env.production"));
        assert!(is_sensitive_name("server.key"));
        assert!(is_sensitive_name("id_ed25519"));
        assert!(!is_sensitive_name("environment.rs"));
        assert!(!is_sensitive_name("keys.ts"));
    }

    #[test]
    fn scan_prunes_cce_data_dirs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(root.join("src/main.rs"), b"fn main() {}\n").expect("source file");
        fs::write(root.join(".cceignore"), b"# rules\n").expect("cceignore");
        for name in [".cce", ".cce-bench-external", ".cce_sandbox"] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).expect("state dir");
            fs::write(dir.join("metadata.sqlite"), b"artifacts").expect("artifact");
        }
        // An arbitrarily-named configured data dir, not matching `.cce*`.
        fs::create_dir_all(root.join("bench-data")).expect("data dir");
        fs::write(root.join("bench-data/store.bin"), b"artifacts").expect("store");

        let mut config = EngineConfig::for_repository(root);
        config.data_root = root.join("bench-data");
        let scanned = RepositoryScanner::new(config).scan(None).expect("scan");
        let paths: Vec<&str> = scanned
            .files
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect();
        assert!(paths.contains(&"src/main.rs"), "{paths:?}");
        assert!(paths.contains(&".cceignore"), "{paths:?}");
        for name in [".cce", ".cce-bench-external", ".cce_sandbox", "bench-data"] {
            assert!(
                !paths
                    .iter()
                    .any(|path| path.starts_with(&format!("{name}/"))),
                "{name} leaked into scan: {paths:?}"
            );
        }
    }

    #[test]
    fn resolves_missing_data_root_through_existing_ancestor() {
        let directory = tempfile::tempdir().expect("tempdir");
        let canonical = directory.path().canonicalize().expect("canonical root");
        let resolved =
            resolve_data_root(&directory.path().join("missing/deep")).expect("data root resolves");
        assert_eq!(resolved, canonical.join("missing/deep"));
    }
}
