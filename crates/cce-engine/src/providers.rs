//! External code-intelligence providers. A provider is a local toolchain
//! (rust-analyzer, scip-typescript) or a committed artifact (`index.scip`)
//! that produces evidence the engine ingests itself — providers never write
//! `SQLite` and never see query traffic. Provisioning/downloading tools is a
//! separate opt-in layer; detection only ever looks at what already exists
//! on the machine or in the repository.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cce_core::{CceError, Result};
use serde::{Deserialize, Serialize};

/// What a provider probe found. Detection never executes the tool's real
/// work and never touches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetectState {
    /// Resolved executable or artifact path, plus a display string.
    Ready { tool: PathBuf, detail: String },
    /// The repo needs this provider but the tool is absent.
    Missing { remediation: String },
    /// The provider does not apply to this repository.
    NotApplicable { reason: String },
}

/// Artifact bytes a provider produced. The engine stores them
/// content-addressed and ingests them through the matching parser.
#[derive(Debug)]
pub(crate) enum ProviderArtifact {
    ScipIndex { bytes: Vec<u8> },
}

#[derive(Debug)]
pub(crate) struct ProviderOutput {
    pub artifact: ProviderArtifact,
    pub duration: Duration,
}

/// Lifecycle state reported for one provider after an index pass (or a
/// bare `detect` probe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    /// Toolchain found / artifact produced; ran successfully.
    Ready,
    /// Applicable to the repo but the tool is not installed.
    Missing,
    /// Provider does not apply to this repository.
    NotApplicable,
    /// Execution or ingestion failed; `message` carries diagnostics.
    Failed,
}

/// Per-provider outcome, recorded in `IndexReport` and surfaced through
/// `GET /v1/providers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReport {
    /// Provider identifier (`scip:rust-analyzer`, …).
    pub provider_id: String,
    /// Final lifecycle state.
    pub state: ProviderState,
    /// Resolved tool path/command when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Diagnostic or remediation message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Content digest of the stored provider artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
    /// Wall-clock execution time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// SCIP documents seen during ingest.
    pub scip_documents: usize,
    /// SCIP definitions resolved to entities.
    pub scip_definitions: usize,
    /// SCIP reference edges emitted.
    pub scip_reference_edges: usize,
}

impl ProviderReport {
    fn new(provider_id: &str) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            state: ProviderState::NotApplicable,
            tool: None,
            message: None,
            artifact_digest: None,
            duration_ms: None,
            scip_documents: 0,
            scip_definitions: 0,
            scip_reference_edges: 0,
        }
    }
}

pub(crate) trait Provider {
    fn id(&self) -> &'static str;
    fn detect(&self, repo_root: &Path) -> DetectState;
    /// Produce the provider artifact. `tool` is the path resolved by a
    /// `Ready` detection; `work_dir` is scratch space under the data root.
    fn run(
        &self,
        repo_root: &Path,
        work_dir: &Path,
        timeout: Duration,
        tool: &Path,
    ) -> Result<ProviderOutput>;
}

/// Every registered provider. Order is ingestion order; merges make
/// duplicate edges idempotent.
pub(crate) fn registry() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(ScipFile),
        Box::new(ScipRustAnalyzer),
        Box::new(ScipTypeScript),
    ]
}

/// Detect every provider against a repository without running anything.
/// Powers `cce providers` and `GET /v1/providers`.
pub(crate) fn detect_all(repo_root: &Path) -> Vec<ProviderReport> {
    registry()
        .iter()
        .map(|provider| {
            let mut report = ProviderReport::new(provider.id());
            match provider.detect(repo_root) {
                DetectState::Ready { detail, .. } => {
                    report.state = ProviderState::Ready;
                    report.tool = Some(detail);
                }
                DetectState::Missing { remediation } => {
                    report.state = ProviderState::Missing;
                    report.message = Some(remediation);
                }
                DetectState::NotApplicable { reason } => {
                    report.state = ProviderState::NotApplicable;
                    report.message = Some(reason);
                }
            }
            report
        })
        .collect()
}

/// Run every applicable provider. Blocking (spawns subprocesses); callers
/// on an async executor should use `spawn_blocking`. Artifact bytes come
/// back with the report so the caller can store and ingest them.
pub(crate) fn produce(
    repo_root: &Path,
    work_root: &Path,
    timeout: Duration,
) -> Vec<(ProviderReport, Option<ProviderArtifact>)> {
    let mut results = Vec::new();
    for provider in registry() {
        let mut report = ProviderReport::new(provider.id());
        let tool = match provider.detect(repo_root) {
            DetectState::Ready { tool, detail } => {
                report.state = ProviderState::Ready;
                report.tool = Some(detail);
                tool
            }
            DetectState::Missing { remediation } => {
                report.state = ProviderState::Missing;
                report.message = Some(remediation);
                results.push((report, None));
                continue;
            }
            DetectState::NotApplicable { reason } => {
                report.state = ProviderState::NotApplicable;
                report.message = Some(reason);
                results.push((report, None));
                continue;
            }
        };
        let work_dir = work_root.join(provider.id().replace(':', "_"));
        match provider.run(repo_root, &work_dir, timeout, &tool) {
            Ok(output) => {
                report.duration_ms = Some(output.duration.as_millis() as u64);
                results.push((report, Some(output.artifact)));
            }
            Err(error) => {
                report.state = ProviderState::Failed;
                report.message = Some(error.to_string());
                results.push((report, None));
            }
        }
    }
    results
}

// --- providers --------------------------------------------------------------

/// A `index.scip` checked into the repository root — e.g. produced by CI.
/// Always offline; the cheapest way to get compiler-grade truth into CCE.
struct ScipFile;

impl Provider for ScipFile {
    fn id(&self) -> &'static str {
        "scip:file"
    }

    fn detect(&self, repo_root: &Path) -> DetectState {
        let candidate = repo_root.join("index.scip");
        if candidate.is_file() {
            DetectState::Ready {
                tool: candidate,
                detail: "index.scip (repo)".to_owned(),
            }
        } else {
            DetectState::NotApplicable {
                reason: "no index.scip at repository root".to_owned(),
            }
        }
    }

    fn run(
        &self,
        _repo_root: &Path,
        _work_dir: &Path,
        _timeout: Duration,
        tool: &Path,
    ) -> Result<ProviderOutput> {
        let started = Instant::now();
        let bytes = std::fs::read(tool).map_err(|error| CceError::io(tool, error))?;
        Ok(ProviderOutput {
            artifact: ProviderArtifact::ScipIndex { bytes },
            duration: started.elapsed(),
        })
    }
}

/// `rust-analyzer scip` for Cargo workspaces.
struct ScipRustAnalyzer;

impl Provider for ScipRustAnalyzer {
    fn id(&self) -> &'static str {
        "scip:rust-analyzer"
    }

    fn detect(&self, repo_root: &Path) -> DetectState {
        if !repo_root.join("Cargo.toml").is_file() {
            return DetectState::NotApplicable {
                reason: "no Cargo.toml at repository root".to_owned(),
            };
        }
        on_path("rust-analyzer").map_or_else(
            || DetectState::Missing {
                remediation: "rust-analyzer is not on PATH; run `rustup component add \
                              rust-analyzer`"
                    .to_owned(),
            },
            |tool| DetectState::Ready {
                detail: tool.display().to_string(),
                tool,
            },
        )
    }

    fn run(
        &self,
        repo_root: &Path,
        work_dir: &Path,
        timeout: Duration,
        tool: &Path,
    ) -> Result<ProviderOutput> {
        let started = Instant::now();
        std::fs::create_dir_all(work_dir).map_err(|error| CceError::io(work_dir, error))?;
        // cwd = repo root: rustup proxies resolve the toolchain by directory,
        // so running elsewhere can select a toolchain without the component.
        // `--output` keeps index.scip out of the worktree.
        let produced = work_dir.join("index.scip");
        let output = run_command(
            tool,
            &["scip", ".", "--output", &produced.to_string_lossy()],
            repo_root,
            timeout,
        )?;
        if output.timed_out {
            return Err(CceError::Configuration(format!(
                "rust-analyzer scip timed out after {}s",
                timeout.as_secs()
            )));
        }
        if !produced.is_file() {
            // Older builds lack `--output` and write ./index.scip; retry
            // without it and relocate the file rather than leave pollution.
            let fallback = run_command(tool, &["scip", "."], repo_root, timeout)?;
            let strays = repo_root.join("index.scip");
            if strays.is_file() {
                std::fs::rename(&strays, &produced)
                    .map_err(|error| CceError::io(&strays, error))?;
            }
            if fallback.timed_out {
                return Err(CceError::Configuration(format!(
                    "rust-analyzer scip timed out after {}s",
                    timeout.as_secs()
                )));
            }
            if !produced.is_file() {
                return Err(CceError::Configuration(format!(
                    "rust-analyzer scip produced no index.scip (exit {:?}): {}",
                    fallback.code,
                    fallback.stderr_tail()
                )));
            }
        }
        if output.code.is_some_and(|code| code != 0) {
            tracing::warn!(
                code = ?output.code,
                stderr = %output.stderr_tail(),
                "rust-analyzer scip exited non-zero but produced an index"
            );
        }
        let bytes = std::fs::read(&produced).map_err(|error| CceError::io(&produced, error))?;
        let _ = std::fs::remove_file(&produced);
        Ok(ProviderOutput {
            artifact: ProviderArtifact::ScipIndex { bytes },
            duration: started.elapsed(),
        })
    }
}

/// `scip-typescript` for repositories with a `tsconfig.json`.
struct ScipTypeScript;

impl Provider for ScipTypeScript {
    fn id(&self) -> &'static str {
        "scip:typescript"
    }

    fn detect(&self, repo_root: &Path) -> DetectState {
        if !repo_root.join("tsconfig.json").is_file() {
            return DetectState::NotApplicable {
                reason: "no tsconfig.json at repository root".to_owned(),
            };
        }
        // Prefer the project's own devDependency so the indexer matches the
        // project's TypeScript version.
        let local = repo_root.join("node_modules/.bin/scip-typescript");
        if local.is_file() {
            return DetectState::Ready {
                detail: "node_modules/.bin/scip-typescript".to_owned(),
                tool: local,
            };
        }
        on_path("scip-typescript").map_or_else(
            || DetectState::Missing {
                remediation: "scip-typescript is not installed; run `npm i -D \
                              @sourcegraph/scip-typescript` in the repository"
                    .to_owned(),
            },
            |tool| DetectState::Ready {
                detail: tool.display().to_string(),
                tool,
            },
        )
    }

    fn run(
        &self,
        repo_root: &Path,
        work_dir: &Path,
        timeout: Duration,
        tool: &Path,
    ) -> Result<ProviderOutput> {
        let started = Instant::now();
        std::fs::create_dir_all(work_dir).map_err(|error| CceError::io(work_dir, error))?;
        let produced = work_dir.join("index.scip");
        // scip-typescript indexes the project at cwd; `--output` exists on
        // current versions. On an unknown-flag failure retry without it and
        // collect ./index.scip from the repo root.
        let output = run_command(
            tool,
            &["index", "--output", &produced.to_string_lossy()],
            repo_root,
            timeout,
        )?;
        if !produced.is_file() {
            let fallback = run_command(tool, &["index"], repo_root, timeout)?;
            let strays = repo_root.join("index.scip");
            if strays.is_file() {
                std::fs::rename(&strays, &produced)
                    .map_err(|error| CceError::io(&strays, error))?;
            }
            if fallback.timed_out {
                return Err(CceError::Configuration(format!(
                    "scip-typescript timed out after {}s",
                    timeout.as_secs()
                )));
            }
            if !produced.is_file() {
                return Err(CceError::Configuration(format!(
                    "scip-typescript produced no index.scip (exit {:?}): {}",
                    fallback.code,
                    fallback.stderr_tail()
                )));
            }
        }
        if output.timed_out {
            return Err(CceError::Configuration(format!(
                "scip-typescript timed out after {}s",
                timeout.as_secs()
            )));
        }
        let bytes = std::fs::read(&produced).map_err(|error| CceError::io(&produced, error))?;
        let _ = std::fs::remove_file(&produced);
        Ok(ProviderOutput {
            artifact: ProviderArtifact::ScipIndex { bytes },
            duration: started.elapsed(),
        })
    }
}

// --- subprocess plumbing -----------------------------------------------------

pub(crate) struct CommandOutput {
    pub code: Option<i32>,
    /// Drained to keep the pipe from deadlocking; kept for diagnostics.
    #[allow(dead_code)]
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CommandOutput {
    fn stderr_tail(&self) -> String {
        const TAIL: usize = 2_048;
        let trimmed = self.stderr.trim();
        if trimmed.len() <= TAIL {
            trimmed.to_owned()
        } else {
            let mut start = trimmed.len() - TAIL;
            while !trimmed.is_char_boundary(start) {
                start += 1;
            }
            trimmed.get(start..).unwrap_or(trimmed).to_owned()
        }
    }
}

/// Spawn a subprocess with a scrubbed environment (secret-shaped variables
/// removed), piped output drained on threads, and a hard timeout. Blocking.
fn run_command(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(scrubbed_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            CceError::Configuration(format!("could not spawn {}: {error}", program.display()))
        })?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| CceError::Configuration("missing stdout pipe".to_owned()))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| CceError::Configuration("missing stderr pipe".to_owned()))?;
    let stdout_thread = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buffer);
        buffer
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buffer);
        buffer
    });
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CceError::Configuration(format!(
                    "could not wait on {}: {error}",
                    program.display()
                )));
            }
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_thread.join().unwrap_or_default()).into_owned();
    Ok(CommandOutput {
        code: status.and_then(|status| status.code()),
        stdout,
        stderr,
        timed_out,
    })
}

/// Environment for provider subprocesses: the user's environment minus
/// secret-shaped variables. Indexers are the user's own toolchains running
/// as the user — they need `PATH`/`HOME`/`RUSTUP_HOME` but never API keys.
fn scrubbed_env() -> HashMap<std::ffi::OsString, std::ffi::OsString> {
    std::env::vars_os()
        .filter(|(name, _)| !is_secret_env(name))
        .collect()
}

const SECRET_ENV_MARKERS: [&str; 5] = ["SECRET", "TOKEN", "PASSWORD", "CREDENTIAL", "PRIVATE_KEY"];

fn is_secret_env(name: &OsStr) -> bool {
    let upper = name.to_string_lossy().to_ascii_uppercase();
    SECRET_ENV_MARKERS
        .iter()
        .any(|marker| upper.contains(marker))
        || upper.ends_with("_KEY")
}

/// PATH lookup without executing anything.
fn on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_var) {
        let candidate = directory.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scip_file_detects_repo_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            ScipFile.detect(dir.path()),
            DetectState::NotApplicable { .. }
        ));
        std::fs::write(dir.path().join("index.scip"), b"pb").expect("write");
        assert!(matches!(
            ScipFile.detect(dir.path()),
            DetectState::Ready { .. }
        ));
    }

    #[test]
    fn rust_analyzer_skips_non_cargo_repos() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            ScipRustAnalyzer.detect(dir.path()),
            DetectState::NotApplicable { .. }
        ));
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname='x'\n").expect("write");
        // With a manifest the answer is Ready or Missing, never NotApplicable.
        assert!(!matches!(
            ScipRustAnalyzer.detect(dir.path()),
            DetectState::NotApplicable { .. }
        ));
    }

    #[test]
    fn typescript_requires_tsconfig() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            ScipTypeScript.detect(dir.path()),
            DetectState::NotApplicable { .. }
        ));
    }

    #[test]
    fn secret_env_vars_are_scrubbed() {
        assert!(is_secret_env(OsStr::new("GITHUB_TOKEN")));
        assert!(is_secret_env(OsStr::new("AWS_SECRET_ACCESS_KEY")));
        assert!(is_secret_env(OsStr::new("MY_API_KEY")));
        assert!(!is_secret_env(OsStr::new("PATH")));
        assert!(!is_secret_env(OsStr::new("RUSTUP_HOME")));
    }

    #[cfg(unix)]
    #[test]
    fn run_command_times_out_and_captures_output() {
        let sh = PathBuf::from("/bin/sh");
        let ok = run_command(
            &sh,
            &["-c", "echo out; echo err >&2"],
            Path::new("/"),
            Duration::from_secs(5),
        )
        .expect("run");
        assert_eq!(ok.code, Some(0));
        assert!(!ok.timed_out);
        assert_eq!(ok.stdout.trim(), "out");
        assert_eq!(ok.stderr.trim(), "err");

        let slow = run_command(
            &sh,
            &["-c", "sleep 5"],
            Path::new("/"),
            Duration::from_millis(60),
        )
        .expect("run");
        assert!(slow.timed_out);
    }
}
