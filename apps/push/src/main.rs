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

/// In-memory (repo-relative path → (mtime, size, digest)) cache so
/// --watch polls only re-hash files whose mtime/size actually moved.
#[derive(Debug, Default)]
struct ScanCache {
    entries: HashMap<String, (i64, u64, String)>,
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
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or_default())
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
            let by_digest: HashMap<&str, &FileEntry> =
                files.iter().map(|f| (f.digest.as_str(), f)).collect();
            eprintln!("cce-push: uploading {} blobs", check.missing.len());
            for digest in &check.missing {
                let Some(entry) = by_digest.get(digest.as_str()) else {
                    continue;
                };
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
            }
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
    };
    let mut cache = ScanCache::default();
    let files = scan(&root, &mut cache)?;
    uploader.sync_once(&files, head_revision(&root).as_deref())?;
    if !arguments.watch {
        return Ok(());
    }
    let mut last_refs = refs_digest(&root);
    let mut last_seen = cache.entries.clone();
    loop {
        std::thread::sleep(Duration::from_secs(arguments.interval));
        let files = scan(&root, &mut cache)?;
        let refs = refs_digest(&root);
        if cache.entries != last_seen || refs != last_refs {
            eprintln!("cce-push: change detected, syncing…");
            uploader.sync_once(&files, head_revision(&root).as_deref())?;
            last_seen.clone_from(&cache.entries);
            last_refs = refs;
        }
    }
}
