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
use utoipa::ToSchema;

use cce_core::{CceError, Result};
use serde::{Deserialize, Serialize};

/// What a provider probe found. Detection never executes the tool's real
/// work and never touches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetectState {
    /// Resolved executable or artifact path, plus a display string. `note`
    /// carries detection context worth surfacing as `ProviderReport.message`
    /// (e.g. which project root a monorepo index run will use).
    Ready {
        tool: PathBuf,
        detail: String,
        note: Option<String>,
    },
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
/// `data_root` locates the managed tool cache. Powers `cce providers`
/// and `GET /v1/providers`.
pub(crate) fn detect_all(repo_root: &Path, data_root: &Path) -> Vec<ProviderReport> {
    let mut reports: Vec<ProviderReport> = registry()
        .iter()
        .map(|provider| {
            let mut report = ProviderReport::new(provider.id());
            match provider.detect(repo_root) {
                DetectState::Ready { detail, note, .. } => {
                    report.state = ProviderState::Ready;
                    report.tool = Some(detail);
                    report.message = note;
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
        .collect();
    // Zoekt lives outside the SCIP registry — it produces no artifact —
    // but its toolchain presence is still provider-surface information.
    reports.push(crate::zoekt::detect_report(data_root));
    reports
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
            DetectState::Ready { tool, detail, note } => {
                report.state = ProviderState::Ready;
                report.tool = Some(detail);
                report.message = note;
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
                note: None,
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
                note: None,
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
///
/// Monorepos carry many `tsconfig.json` files; v1 indexes a single project
/// root. The repository root wins when its tsconfig names inputs; a
/// solution-style tsconfig (`"files": []` with no `include`) declares no
/// sources of its own, so detection prefers the largest workspace candidate
/// (npm/yarn `workspaces` globs, `pnpm-workspace.yaml` `packages:`, plus the
/// conventional `apps/*`, `packages/*`, `plugins/*`, `libs/*`). A root
/// solution file remains the last resort when no candidate exists —
/// scip-typescript still recurses into its `references`.
struct ScipTypeScript;

/// The TypeScript project root a detect/run pair agreed on.
#[derive(Debug)]
struct TsProject {
    /// Directory passed as the `index <dir>` argument (absolute when the
    /// repository root is absolute).
    dir: PathBuf,
    /// Repo-relative display form (`.` for the root itself).
    display: String,
    /// Why this root won — surfaced verbatim as `ProviderReport.message`.
    note: String,
}

impl Provider for ScipTypeScript {
    fn id(&self) -> &'static str {
        "scip:typescript"
    }

    fn detect(&self, repo_root: &Path) -> DetectState {
        let Some(project) = select_ts_project(repo_root) else {
            return DetectState::NotApplicable {
                reason: "no tsconfig.json at the repository root or in workspace \
                         directories"
                    .to_owned(),
            };
        };
        // Prefer the project's own devDependency so the indexer matches the
        // project's TypeScript version. pnpm/npm/yarn place the bin shim next
        // to the manifest that declares it, so probe the selected project
        // root first, then the repository root.
        for base in [project.dir.as_path(), repo_root] {
            let local = base.join("node_modules/.bin/scip-typescript");
            if local.is_file() {
                return DetectState::Ready {
                    detail: local
                        .strip_prefix(repo_root)
                        .unwrap_or(&local)
                        .display()
                        .to_string(),
                    tool: local,
                    note: Some(project.note),
                };
            }
            if base == repo_root {
                break;
            }
        }
        on_path("scip-typescript").map_or_else(
            || {
                let install = if repo_root.join("pnpm-workspace.yaml").is_file()
                    || repo_root.join("pnpm-lock.yaml").is_file()
                {
                    "pnpm add -D @sourcegraph/scip-typescript"
                } else {
                    "npm i -D @sourcegraph/scip-typescript"
                };
                let where_ = if project.display == "." {
                    "in the repository root".to_owned()
                } else {
                    format!("in `{}`", project.display)
                };
                DetectState::Missing {
                    remediation: format!(
                        "scip-typescript is not installed; run `{install}` {where_} \
                         (a repo-local devDependency matches the project's \
                         TypeScript version)"
                    ),
                }
            },
            |tool| DetectState::Ready {
                detail: tool.display().to_string(),
                tool,
                note: Some(project.note),
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
        // The project root is recomputed rather than threaded through
        // `Ready`: selection is deterministic over the same tree, so detect
        // and run cannot disagree unless the repo changed underneath us.
        let project = select_ts_project(repo_root).ok_or_else(|| {
            CceError::Configuration(
                "tsconfig.json project root vanished between detect and run".to_owned(),
            )
        })?;
        // scip-typescript emits `relative_path` relative to `--cwd`. Pinning
        // it (and the process cwd) to the repository root keeps document
        // paths repo-relative (`apps/server/src/index.ts`) so they join the
        // engine's file index even when the project sits in a workspace
        // subdirectory. `--output` is made absolute for the same reason.
        let cwd = std::path::absolute(repo_root).map_err(|error| CceError::io(repo_root, error))?;
        let produced = std::path::absolute(work_dir.join("index.scip"))
            .map_err(|error| CceError::io(work_dir, error))?;
        let project_arg =
            std::path::absolute(&project.dir).map_err(|error| CceError::io(&project.dir, error))?;
        let (cwd, output_path, project_arg) = (
            cwd.to_string_lossy().into_owned(),
            produced.to_string_lossy().into_owned(),
            project_arg.to_string_lossy().into_owned(),
        );
        let output = run_command(
            tool,
            &[
                "index",
                "--cwd",
                &cwd,
                "--output",
                &output_path,
                &project_arg,
            ],
            repo_root,
            timeout,
        )?;
        if !produced.is_file() {
            // Unknown-flag or old-version failure: retry bare. `--cwd`
            // defaults to the process cwd — already the repo root — so
            // document paths stay repo-relative and the artifact lands in
            // `<repo>/index.scip` for relocation into the work dir.
            let fallback = run_command(tool, &["index", &project_arg], repo_root, timeout)?;
            for strays in [repo_root.join("index.scip"), project.dir.join("index.scip")] {
                if strays.is_file() {
                    std::fs::rename(&strays, &produced)
                        .map_err(|error| CceError::io(&strays, error))?;
                }
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

/// Choose the single TypeScript project root v1 will index. `None` means no
/// `tsconfig.json` exists anywhere detection looks — the provider is not
/// applicable.
fn select_ts_project(repo_root: &Path) -> Option<TsProject> {
    let root_tsconfig = repo_root.join("tsconfig.json");
    let candidates = workspace_project_dirs(repo_root);
    if root_tsconfig.is_file() && !tsconfig_indexes_nothing(&root_tsconfig) {
        let note = if candidates.is_empty() {
            "project root: repository root".to_owned()
        } else {
            format!(
                "project root: repository root ({} workspace tsconfig.json \
                 project(s) not indexed — v1 runs a single project)",
                candidates.len()
            )
        };
        return Some(TsProject {
            dir: repo_root.to_path_buf(),
            display: ".".to_owned(),
            note,
        });
    }
    let root_gap = if root_tsconfig.is_file() {
        "repository tsconfig.json is a solution file with no inputs"
    } else {
        "no tsconfig.json at the repository root"
    };
    let mut ranked: Vec<(usize, PathBuf)> = candidates
        .iter()
        .map(|dir| (count_ts_sources(dir), dir.clone()))
        .collect();
    // Most sources first; path order breaks ties deterministically.
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    if let Some((sources, dir)) = ranked.into_iter().next() {
        let display = dir
            .strip_prefix(repo_root)
            .unwrap_or(&dir)
            .display()
            .to_string();
        return Some(TsProject {
            dir,
            note: format!(
                "project root `{display}`: largest of {} workspace \
                 tsconfig.json candidate(s) ({sources} TS sources; {root_gap})",
                candidates.len()
            ),
            display,
        });
    }
    // A hollow root tsconfig is still an explicit TypeScript signal — let
    // the tool run and report its own diagnostic rather than guess.
    root_tsconfig.is_file().then(|| TsProject {
        dir: repo_root.to_path_buf(),
        display: ".".to_owned(),
        note: format!("project root: repository root ({root_gap})"),
    })
}

/// Directories holding a `tsconfig.json` under the repository's declared
/// workspace globs (`package.json` `workspaces`, `pnpm-workspace.yaml`
/// `packages:`) plus the conventional `apps/*`, `packages/*`, `plugins/*`,
/// `libs/*` roots. Sorted and deduplicated; the repository root is never
/// included.
fn workspace_project_dirs(repo_root: &Path) -> Vec<PathBuf> {
    let mut globs = npm_workspace_globs(repo_root);
    globs.extend(pnpm_workspace_globs(repo_root));
    globs.extend(
        ["apps/*", "packages/*", "plugins/*", "libs/*"]
            .iter()
            .map(|glob| (*glob).to_owned()),
    );
    let mut dirs = std::collections::BTreeSet::new();
    for pattern in &globs {
        let pattern = pattern.trim().trim_end_matches('/');
        // `foo/*` and `foo/**` both enumerate the immediate subdirectories
        // of `foo`; deeper glob shapes are not expanded in v1.
        if let Some(parent) = pattern
            .strip_suffix("/*")
            .or_else(|| pattern.strip_suffix("/**"))
        {
            if let Ok(entries) = std::fs::read_dir(repo_root.join(parent)) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if dir.is_dir() && dir.join("tsconfig.json").is_file() {
                        dirs.insert(dir);
                    }
                }
            }
        } else if !pattern.contains('*') {
            let dir = repo_root.join(pattern);
            if dir.is_dir() && dir.join("tsconfig.json").is_file() {
                dirs.insert(dir);
            }
        }
    }
    dirs.into_iter().collect()
}

/// `workspaces` globs from `package.json` — either the bare array form or
/// the `{ "packages": [...] }` object form.
fn npm_workspace_globs(repo_root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(repo_root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(workspaces) = value.get("workspaces") else {
        return Vec::new();
    };
    let list = workspaces
        .as_array()
        .or_else(|| workspaces.get("packages").and_then(|v| v.as_array()));
    list.map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter_map(|item| item.as_str())
            .filter(|item| !item.starts_with('!'))
            .map(str::to_owned)
            .collect()
    })
}

/// `packages:` globs from `pnpm-workspace.yaml`, parsed line-wise — pulling
/// in a YAML dependency for one top-level list is not worth it.
fn pnpm_workspace_globs(repo_root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(repo_root.join("pnpm-workspace.yaml")) else {
        return Vec::new();
    };
    let mut globs = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if in_packages {
                if let Some(item) = line.trim().strip_prefix('-') {
                    let pattern = item.trim().trim_matches(['\'', '"']);
                    if !pattern.is_empty() && !pattern.starts_with('!') {
                        globs.push(pattern.to_owned());
                    }
                }
            }
        } else {
            in_packages = line.trim_end() == "packages:";
        }
    }
    globs
}

/// A tsconfig declaring no compilation inputs of its own: `"files": []`
/// with no `include`. `references` are deliberately not counted — they make
/// the file worth a last-resort run (scip-typescript recurses them) but a
/// workspace project's own tsconfig carries the compiler options its
/// sources were written against, so it makes the better primary root.
fn tsconfig_indexes_nothing(tsconfig: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(tsconfig) else {
        return false;
    };
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        let non_empty = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|a| !a.is_empty())
        };
        let files_empty = value
            .get("files")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty);
        return files_empty && !non_empty("include");
    }
    // tsconfig permits comments and trailing commas; when the strict parse
    // fails, a textual check still recognizes the solution-file shape.
    let files_empty =
        regex::Regex::new(r#""files"\s*:\s*\[\s*\]"#).is_ok_and(|re| re.is_match(&text));
    let has_include =
        regex::Regex::new(r#""include"\s*:\s*\[\s*[^\]]"#).is_ok_and(|re| re.is_match(&text));
    files_empty && !has_include
}

/// `.ts`-family source files under `dir`, bounded — used only to rank
/// workspace candidates, never as a gate. JavaScript is not counted:
/// generated bundles (`storybook-static`, vendored dists) would drown the
/// signal. Hidden and vendored trees are skipped; symlinks not followed.
fn count_ts_sources(dir: &Path) -> usize {
    const SKIP_DIRS: [&str; 6] = ["node_modules", "dist", "build", "out", "coverage", "target"];
    const MAX_VISITED: usize = 50_000;
    let mut sources = 0;
    let mut visited = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_VISITED {
                return sources;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(entry.path());
                }
            } else if kind.is_file()
                && matches!(
                    entry.path().extension().and_then(|ext| ext.to_str()),
                    Some("ts" | "tsx" | "mts" | "cts")
                )
            {
                sources += 1;
            }
        }
    }
    sources
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
    pub(crate) fn stderr_tail(&self) -> String {
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
pub(crate) fn run_command(
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
pub(crate) fn on_path(name: &str) -> Option<PathBuf> {
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

    /// Write a `tsconfig.json` and `n` source files under `root/rel`.
    fn ts_project(root: &Path, rel: &str, tsconfig: &str, sources: usize) {
        let dir = root.join(rel);
        std::fs::create_dir_all(dir.join("src")).expect("mkdir");
        std::fs::write(dir.join("tsconfig.json"), tsconfig).expect("write tsconfig");
        for i in 0..sources {
            std::fs::write(dir.join("src").join(format!("f{i}.ts")), "export {}\n")
                .expect("write source");
        }
    }

    const REAL_TSCONFIG: &str = r#"{ "include": ["src/**/*"] }"#;
    const SOLUTION_TSCONFIG: &str =
        r#"{ "references": [{ "path": "./tsconfig.app.json" }], "files": [] }"#;
    const HOLLOW_TSCONFIG: &str = r#"{ "files": [] }"#;

    #[test]
    fn typescript_prefers_real_root_tsconfig() {
        let dir = tempfile::tempdir().expect("tempdir");
        ts_project(dir.path(), ".", REAL_TSCONFIG, 1);
        ts_project(dir.path(), "apps/web", REAL_TSCONFIG, 50);
        let project = select_ts_project(dir.path()).expect("a project root");
        assert_eq!(project.dir, dir.path());
    }

    #[test]
    fn typescript_root_solution_file_yields_to_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        ts_project(dir.path(), ".", SOLUTION_TSCONFIG, 0);
        ts_project(dir.path(), "apps/web", REAL_TSCONFIG, 50);
        let project = select_ts_project(dir.path()).expect("a project root");
        // A solution file declares no inputs; the workspace's own tsconfig
        // carries the compiler options its sources were written against.
        assert_eq!(project.dir, dir.path().join("apps/web"));
        assert!(project.note.contains("solution file"));
    }

    #[test]
    fn typescript_picks_largest_workspace_when_root_hollow() {
        let dir = tempfile::tempdir().expect("tempdir");
        ts_project(dir.path(), ".", HOLLOW_TSCONFIG, 0);
        ts_project(dir.path(), "apps/web", REAL_TSCONFIG, 30);
        ts_project(dir.path(), "apps/server", REAL_TSCONFIG, 60);
        let project = select_ts_project(dir.path()).expect("a project root");
        assert_eq!(project.dir, dir.path().join("apps/server"));
        assert_eq!(project.display, "apps/server");
        assert!(project.note.contains("solution file"));
    }

    #[test]
    fn typescript_picks_largest_workspace_when_root_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        ts_project(dir.path(), "plugins/a", REAL_TSCONFIG, 3);
        ts_project(dir.path(), "plugins/b", REAL_TSCONFIG, 9);
        let project = select_ts_project(dir.path()).expect("a project root");
        assert_eq!(project.dir, dir.path().join("plugins/b"));
    }

    #[test]
    fn typescript_honors_package_json_workspaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "workspaces": ["modules/*"] }"#,
        )
        .expect("write package.json");
        ts_project(dir.path(), "modules/only", REAL_TSCONFIG, 4);
        let project = select_ts_project(dir.path()).expect("a project root");
        assert_eq!(project.dir, dir.path().join("modules/only"));
    }

    #[test]
    fn typescript_honors_pnpm_workspace_yaml() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'modules/*'\n  - '!modules/excluded'\nother: true\n",
        )
        .expect("write pnpm-workspace.yaml");
        ts_project(dir.path(), "modules/only", REAL_TSCONFIG, 4);
        let project = select_ts_project(dir.path()).expect("a project root");
        assert_eq!(project.dir, dir.path().join("modules/only"));
    }

    #[test]
    fn typescript_hollow_root_without_candidates_still_applies() {
        let dir = tempfile::tempdir().expect("tempdir");
        ts_project(dir.path(), ".", HOLLOW_TSCONFIG, 0);
        // The provider stays applicable and lets the tool report the empty
        // project rather than claiming not_applicable.
        let project = select_ts_project(dir.path()).expect("a project root");
        assert_eq!(project.dir, dir.path());
    }

    #[test]
    fn typescript_detect_reports_missing_tool_with_note() {
        let dir = tempfile::tempdir().expect("tempdir");
        ts_project(dir.path(), ".", HOLLOW_TSCONFIG, 0);
        ts_project(dir.path(), "apps/web", REAL_TSCONFIG, 5);
        match ScipTypeScript.detect(dir.path()) {
            // Tool absent on the test machine → remediation names the
            // selected root; present → the note documents the choice.
            DetectState::Missing { remediation } => {
                assert!(remediation.contains("apps/web"));
            }
            DetectState::Ready { note, .. } => {
                assert!(note.is_some_and(|n| n.contains("apps/web")));
            }
            DetectState::NotApplicable { reason } => {
                panic!("unexpected not_applicable: {reason}")
            }
        }
    }

    #[test]
    fn typescript_solution_tsconfig_declares_no_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tsconfig = dir.path().join("tsconfig.json");
        std::fs::write(&tsconfig, HOLLOW_TSCONFIG).expect("write");
        assert!(tsconfig_indexes_nothing(&tsconfig));
        // `references` do not rescue primary-root selection.
        std::fs::write(&tsconfig, SOLUTION_TSCONFIG).expect("write");
        assert!(tsconfig_indexes_nothing(&tsconfig));
        std::fs::write(&tsconfig, REAL_TSCONFIG).expect("write");
        assert!(!tsconfig_indexes_nothing(&tsconfig));
        std::fs::write(&tsconfig, r#"{ "files": ["src/a.ts"] }"#).expect("write");
        assert!(!tsconfig_indexes_nothing(&tsconfig));
        // JSONC flavor: comments and trailing commas defeat serde_json but
        // not the textual fallback.
        std::fs::write(
            &tsconfig,
            "{\n  // nothing to compile\n  \"files\": [],\n}\n",
        )
        .expect("write");
        assert!(tsconfig_indexes_nothing(&tsconfig));
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
