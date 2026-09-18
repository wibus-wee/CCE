//! cce-push: thin worktree uploader for the central CCE service.
//!
//! Scans the local worktree with the same ignore/sensitive/binary policy as
//! the engine, then syncs content-addressed blobs to a cce-gateway:
//! check (manifest → missing digests) → blobs (upload only what's missing)
//! → push (commit the tree server-side, which materializes it and triggers
//! the worker's index). `--watch` polls for worktree and git-ref changes —
//! no git hooks required.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use clap::Parser;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;

#[derive(Parser)]
#[command(name = "cce-push", version)]
struct Arguments {
    /// Repository directory to upload (defaults to cwd).
    #[arg(default_value = ".")]
    repository: PathBuf,
    /// Gateway base URL, e.g. <http://127.0.0.1:7735>.
    #[arg(long, env = "CCE_GATEWAY")]
    gateway: String,
    /// Repo name registered on the gateway (defaults to the directory name).
    #[arg(long)]
    repo: Option<String>,
    /// Keep running: re-scan every --interval and push when the worktree or
    /// git refs move — covers edits, commits, and `git push` without hooks.
    #[arg(long)]
    watch: bool,
    /// Poll interval for --watch, in seconds.
    #[arg(long, default_value_t = 15)]
    interval: u64,
    /// Parallel blob uploads per sync.
    #[arg(
        long,
        default_value_t = 8,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=64)
    )]
    concurrency: usize,
    /// Bypass the on-disk scan cache entirely (no read, no write) —
    /// escape hatch for debugging and CI.
    #[arg(long)]
    no_scan_cache: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileEntry {
    path: String,
    size: u64,
    digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoEntry {
    id: String,
    name: String,
}

/// `ensure` auto-registers the repo when the gateway has a default worker
/// configured; otherwise the gateway responds that registration is required.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnsureRequest<'a> {
    name: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckResponse {
    missing: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushResponse {
    push_id: String,
    materialized: u64,
    bytes: u64,
}

/// In-memory (repo-relative path → (`mtime_ms`, size, digest)) cache so
/// --watch polls only re-hash files whose mtime/size actually moved.
/// Persisted as JSON across runs so cold starts skip unchanged files too.
#[derive(Debug, Default)]
struct ScanCache {
    entries: HashMap<String, (u64, u64, String)>,
}

/// On-disk shape: `{"version":1,"root":"…","entries":{"path":[ms,size,digest]}}`.
/// Tuples serialize as JSON arrays; mtime is milliseconds since the epoch.
#[derive(Debug, Serialize, Deserialize)]
struct ScanCacheFile {
    version: u32,
    root: String,
    entries: HashMap<String, (u64, u64, String)>,
}

const SCAN_CACHE_VERSION: u32 = 1;

impl ScanCache {
    /// Read a persisted cache. A missing file, corrupt JSON, version bump,
    /// or a different worktree root all yield an empty cache — the cache is
    /// a hint and must never fail a push.
    fn load(path: &Path, root: &Path) -> Self {
        let root = root.to_string_lossy().into_owned();
        let loaded = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ScanCacheFile>(&bytes).ok());
        match loaded {
            Some(file) if file.version == SCAN_CACHE_VERSION && file.root == root => Self {
                entries: file.entries,
            },
            _ => Self::default(),
        }
    }

    /// Persist atomically (write `<path>.tmp`, then rename) so a crash
    /// mid-write can't leave a torn cache. Failures warn, never fail.
    fn save(&self, path: &Path, root: &Path) {
        if let Err(error) = self.try_save(path, root) {
            eprintln!(
                "cce-push: warning: could not write scan cache {}: {error}",
                path.display()
            );
        }
    }

    fn try_save(&self, path: &Path, root: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ScanCacheFile {
            version: SCAN_CACHE_VERSION,
            root: root.to_string_lossy().into_owned(),
            entries: self.entries.clone(),
        };
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, serde_json::to_vec(&file)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// `$XDG_CACHE_HOME/cce/push-scan-<hash>.json`, else `$HOME/.cache/cce/…`,
/// else `<root>/.cce/push-cache.json` on a bare env. `<hash>` is the first
/// 16 hex chars of the canonicalized root's blake3 — stable across runs,
/// distinct across worktrees.
fn scan_cache_path(root: &Path) -> PathBuf {
    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        });
    cache_dir.map_or_else(
        || root.join(".cce").join("push-cache.json"),
        |dir| {
            let digest = blake3::hash(root.to_string_lossy().as_bytes()).to_hex();
            dir.join("cce")
                .join(format!("push-scan-{}.json", &digest.as_str()[..16]))
        },
    )
}

fn is_internal_dir(entry: &ignore::DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    matches!(
        entry.file_name().to_str(),
        Some(".git" | ".cce" | "target" | "node_modules" | ".venv" | "__pycache__")
    )
}

/// Generated artifacts — lockfiles, minified assets — carry no retrievable
/// evidence; the manifest that produced them is what matters.
fn is_generated(name: &str) -> bool {
    let path = Path::new(name);
    let has_extension = |extension: &str| {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
    };
    if has_extension("lock") || has_extension("lockb") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".min.js")
        || lower.ends_with(".min.css")
        || has_extension("map")
        || matches!(
            lower.as_str(),
            "package-lock.json"
                | "npm-shrinkwrap.json"
                | "pnpm-lock.yaml"
                | "go.sum"
                | "go.work.sum"
        )
}

/// Credential-shaped names never leave the machine — unlike local indexing,
/// upload makes a copy on another host, so the sensitive policy is
/// unconditional here, not opt-in.
fn is_sensitive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || lower.starts_with(".env.") || lower.starts_with(".env-") {
        return true;
    }
    if lower.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension,
            "env" | "pem" | "key" | "p12" | "pfx" | "keystore" | "jks"
        )
    }) {
        return true;
    }
    matches!(
        lower.as_str(),
        "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
    )
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    let sniff = bytes.get(..BINARY_SNIFF_BYTES).unwrap_or(bytes);
    sniff.contains(&0)
}

fn scan(root: &Path, cache: &mut ScanCache) -> anyhow::Result<Vec<FileEntry>> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(true)
        .add_custom_ignore_filename(".cceignore")
        .follow_links(false)
        .filter_entry(|entry| !is_internal_dir(entry));
    let mut files = Vec::new();
    let mut seen = HashMap::new();
    for entry in builder.build() {
        let entry = entry?;
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_str().unwrap_or_default().to_owned();
        if is_generated(&name) || is_sensitive(&name) {
            continue;
        }
        let path = entry.into_path();
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or_default())
            .unwrap_or_default();
        let size = metadata.len();
        let digest = match cache.entries.get(&relative) {
            Some(&(cached_mtime, cached_size, ref cached_digest))
                if cached_mtime == mtime_ms && cached_size == size =>
            {
                cached_digest.clone()
            }
            _ => {
                let bytes = std::fs::read(&path)?;
                if is_probably_binary(&bytes) {
                    continue;
                }
                blake3::hash(&bytes).to_hex().to_string()
            }
        };
        seen.insert(relative.clone(), (mtime_ms, size, digest.clone()));
        files.push(FileEntry {
            path: relative,
            size,
            digest,
        });
    }
    cache.entries = seen;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Digest of git refs — moves on commit, fetch, and `git push` updates to
/// remote-tracking refs. `None` outside a git worktree.
fn refs_digest(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(blake3::hash(&output.stdout).to_hex().to_string())
}

fn head_revision(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

struct Uploader {
    client: reqwest::blocking::Client,
    gateway: String,
    repo_id: String,
    root: PathBuf,
    concurrency: usize,
}

impl Uploader {
    fn sync_once(&self, files: &[FileEntry], revision: Option<&str>) -> anyhow::Result<()> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CheckRequest<'a> {
            files: &'a [FileEntry],
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct PushFile<'a> {
            path: &'a str,
            digest: &'a str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct PushRequest<'a> {
            revision: Option<&'a str>,
            files: Vec<PushFile<'a>>,
        }
        let check: CheckResponse = self
            .client
            .post(format!("{}/v1/repos/{}/check", self.gateway, self.repo_id))
            .json(&CheckRequest { files })
            .send()?
            .error_for_status()?
            .json()?;
        if check.missing.is_empty() {
            eprintln!("cce-push: all blobs already on gateway");
        } else {
            eprintln!("cce-push: uploading {} blobs", check.missing.len());
            self.upload_missing(files, &check.missing)?;
        }
        let response: PushResponse = self
            .client
            .post(format!("{}/v1/repos/{}/push", self.gateway, self.repo_id))
            .json(&PushRequest {
                revision,
                files: files
                    .iter()
                    .map(|f| PushFile {
                        path: &f.path,
                        digest: &f.digest,
                    })
                    .collect(),
            })
            .send()?
            .error_for_status()?
            .json()?;
        eprintln!(
            "cce-push: {} — {} files ({} bytes) materialized",
            response.push_id, response.materialized, response.bytes
        );
        Ok(())
    }

    /// One blob upload: read the worktree file, PUT it under its digest.
    fn upload_blob(&self, digest: &str, entry: &FileEntry) -> anyhow::Result<()> {
        // Manifest paths are repo-relative; reads must resolve
        // against the scanned root, not this process's cwd.
        let bytes = std::fs::read(self.root.join(&entry.path))?;
        self.client
            .put(format!(
                "{}/v1/repos/{}/blobs/{digest}",
                self.gateway, self.repo_id
            ))
            .body(bytes)
            .send()?
            .error_for_status()?;
        Ok(())
    }

    /// Fan `missing` across `--concurrency` scoped workers sharing the
    /// blocking client (it is `Send + Sync`). Each worker PUTs its
    /// contiguous chunk sequentially; the first failure flips `abort` so
    /// peers stop at their next blob, and the joined error fails the push.
    /// Partial uploads are safe — the CAS dedups them on retry.
    fn upload_missing(&self, files: &[FileEntry], missing: &[String]) -> anyhow::Result<()> {
        let by_digest: HashMap<&str, &FileEntry> =
            files.iter().map(|f| (f.digest.as_str(), f)).collect();
        let total = missing
            .iter()
            .filter(|digest| by_digest.contains_key(digest.as_str()))
            .count();
        if total == 0 {
            return Ok(());
        }
        let workers = self.concurrency.min(missing.len());
        let chunk_size = missing.len().div_ceil(workers);
        // Progress every ~10% of the batch or every 500 blobs, whichever first.
        let stride = total.div_ceil(10).clamp(1, 500);
        let uploaded = AtomicUsize::new(0);
        let abort = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let by_digest = &by_digest;
            let uploaded = &uploaded;
            let abort = &abort;
            let mut handles = Vec::with_capacity(workers);
            for chunk in missing.chunks(chunk_size) {
                handles.push(scope.spawn(move || -> anyhow::Result<()> {
                    for digest in chunk {
                        if abort.load(Ordering::Relaxed) {
                            break;
                        }
                        let Some(&entry) = by_digest.get(digest.as_str()) else {
                            continue;
                        };
                        self.upload_blob(digest, entry).map_err(|error| {
                            abort.store(true, Ordering::Relaxed);
                            error.context(format!("blob {digest}"))
                        })?;
                        let done = uploaded.fetch_add(1, Ordering::Relaxed) + 1;
                        if done == total || done.is_multiple_of(stride) {
                            eprintln!("cce-push: uploaded {done}/{total}");
                        }
                    }
                    Ok(())
                }));
            }
            let mut first_error = None;
            for handle in handles {
                let failure = match handle.join() {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some(anyhow::anyhow!("blob upload worker panicked")),
                };
                if first_error.is_none() {
                    first_error = failure;
                }
            }
            first_error.map_or(Ok(()), Err)
        })
    }
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let root = arguments.repository.canonicalize()?;
    let gateway = arguments.gateway.trim_end_matches('/').to_owned();
    let name = arguments.repo.unwrap_or_else(|| {
        root.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repo")
            .to_owned()
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_mins(5))
        .build()?;
    let repo: RepoEntry = client
        .post(format!("{gateway}/v1/repos/ensure"))
        .json(&EnsureRequest { name: &name })
        .send()?
        .error_for_status()?
        .json()?;
    eprintln!("cce-push: repo {:?} → id {}", repo.name, repo.id);
    let uploader = Uploader {
        client,
        gateway,
        repo_id: repo.id,
        root: root.clone(),
        concurrency: arguments.concurrency,
    };
    let cache_path = (!arguments.no_scan_cache).then(|| scan_cache_path(&root));
    let mut cache = cache_path
        .as_ref()
        .map_or_else(ScanCache::default, |path| ScanCache::load(path, &root));
    // Snapshot of what's already on disk; only re-save when a scan actually
    // moves the entries (avoids rewriting a multi-MB JSON every watch tick).
    let mut last_saved = cache.entries.clone();
    let files = scan(&root, &mut cache)?;
    if cache.entries != last_saved {
        if let Some(path) = &cache_path {
            cache.save(path, &root);
        }
        last_saved.clone_from(&cache.entries);
    }
    uploader.sync_once(&files, head_revision(&root).as_deref())?;
    if !arguments.watch {
        return Ok(());
    }
    let mut last_refs = refs_digest(&root);
    let mut last_seen = cache.entries.clone();
    loop {
        std::thread::sleep(Duration::from_secs(arguments.interval));
        let files = scan(&root, &mut cache)?;
        if cache.entries != last_saved {
            if let Some(path) = &cache_path {
                cache.save(path, &root);
            }
            last_saved.clone_from(&cache.entries);
        }
        let refs = refs_digest(&root);
        if cache.entries != last_seen || refs != last_refs {
            eprintln!("cce-push: change detected, syncing…");
            uploader.sync_once(&files, head_revision(&root).as_deref())?;
            last_seen.clone_from(&cache.entries);
            last_refs = refs;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cce-push-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("push-scan.json")
    }

    #[test]
    fn scan_cache_round_trip() {
        let path = temp_cache_path("round-trip");
        let root = Path::new("/some/repo");
        let mut cache = ScanCache::default();
        cache.entries.insert(
            "src/main.rs".to_owned(),
            (1_700_000_000_000, 42, "deadbeef".to_owned()),
        );
        cache.save(&path, root);
        let loaded = ScanCache::load(&path, root);
        assert_eq!(loaded.entries, cache.entries);
        // On-disk shape: {"version":1,"root":"…","entries":{"p":[ms,size,digest]}}.
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["root"], "/some/repo");
        assert_eq!(
            raw["entries"]["src/main.rs"],
            serde_json::json!([1_700_000_000_000u64, 42, "deadbeef"])
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scan_cache_rejects_wrong_version() {
        let path = temp_cache_path("version");
        std::fs::write(&path, br#"{"version":2,"root":"/some/repo","entries":{}}"#).unwrap();
        assert!(
            ScanCache::load(&path, Path::new("/some/repo"))
                .entries
                .is_empty()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scan_cache_rejects_wrong_root_and_corrupt() {
        let path = temp_cache_path("root");
        std::fs::write(
            &path,
            br#"{"version":1,"root":"/other","entries":{"a.rs":[1,2,"d"]}}"#,
        )
        .unwrap();
        assert!(
            ScanCache::load(&path, Path::new("/some/repo"))
                .entries
                .is_empty()
        );
        std::fs::write(&path, b"not json at all").unwrap();
        assert!(
            ScanCache::load(&path, Path::new("/some/repo"))
                .entries
                .is_empty()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scan_cache_missing_file_loads_empty() {
        let path = temp_cache_path("missing").with_file_name("does-not-exist.json");
        assert!(
            ScanCache::load(&path, Path::new("/some/repo"))
                .entries
                .is_empty()
        );
    }

    #[test]
    fn scan_cache_path_is_stable_and_named_for_root() {
        let root = Path::new("/tmp/cce-scan-cache-path-test");
        let first = scan_cache_path(root);
        assert_eq!(first, scan_cache_path(root));
        // Either the XDG/HOME form push-scan-<16 hex of blake3(root)>.json,
        // or the bare-env .cce/push-cache.json fallback under the root.
        let name = first.file_name().unwrap().to_str().unwrap();
        let digest = blake3::hash(root.to_string_lossy().as_bytes()).to_hex();
        let hashed = format!("push-scan-{}.json", &digest.as_str()[..16]);
        if first.starts_with(root) {
            assert_eq!(first, root.join(".cce").join("push-cache.json"));
        } else {
            assert_eq!(name, hashed);
        }
    }
}
