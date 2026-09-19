//! Zoekt trigram index integration: Sourcegraph's code-search index run
//! as an external provider. The shard directory lives under the data root
//! (disk-resident, never ingested as artifacts) and serves query-time file
//! candidates through the `zoekt` CLI — no daemon, no port, identical in
//! the `cce` one-shot path and `cce-daemon`. File hits resolve back to
//! snapshot documents through `LineMatch` line numbers, so provenance
//! stays in `SQLite` even though candidate generation is external.
//!
//! Detection order: `CCE_ZOEKT`/`CCE_ZOEKT_INDEX` overrides → `PATH` →
//! the managed cache `<data>/providers/zoekt/bin`. Nothing downloads at
//! index or query time — `cce providers --provision` is the explicit
//! opt-in that installs pinned, checksum-verified binaries into the
//! managed cache (release assets first, `go install` fallback).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cce_core::{CceError, Result};
use serde::Deserialize;

use crate::providers::{ProviderReport, ProviderState, on_path, run_command};

/// The two zoekt binaries one integration needs.
#[derive(Debug, Clone)]
pub(crate) struct ZoektTools {
    /// `zoekt-index` directory indexer.
    pub index: PathBuf,
    /// `zoekt` query CLI (`-jsonl` output).
    pub search: PathBuf,
}

/// Upstream ref the managed toolchain is pinned to. sourcegraph/zoekt
/// publishes no release tags, so CI builds assets from this commit and
/// the `go install` fallback resolves the same revision.
const ZOEKT_REF: &str = "153817f643cde8b229ee388c1dddbcf07f4798af";

/// Managed tool cache: `<data>/providers/zoekt/bin`, absolute —
/// `GOBIN` requires an absolute path and `CCE_DATA_DIR` may be relative.
pub(crate) fn bin_dir(data_root: &Path) -> PathBuf {
    let dir = data_root.join("providers").join("zoekt").join("bin");
    std::path::absolute(&dir).unwrap_or(dir)
}

/// Locate the zoekt toolchain. Env overrides win so a caller can pin a
/// specific build; otherwise PATH is probed, then the managed cache —
/// none of these execute anything.
pub(crate) fn detect(data_root: &Path) -> Option<ZoektTools> {
    let managed = bin_dir(data_root);
    let index = std::env::var_os("CCE_ZOEKT_INDEX")
        .map(PathBuf::from)
        .or_else(|| on_path("zoekt-index"))
        .or_else(|| on_disk(&managed, "zoekt-index"))?;
    let search = std::env::var_os("CCE_ZOEKT")
        .map(PathBuf::from)
        .or_else(|| on_path("zoekt"))
        .or_else(|| on_disk(&managed, "zoekt"))?;
    Some(ZoektTools { index, search })
}

fn on_disk(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(exe_name(name));
    path.is_file().then_some(path)
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Shard directory for a data root: `<data>/providers/zoekt/index`,
/// absolute — `CCE_DATA_DIR` may be relative and subprocesses must not
/// re-resolve it against a different working directory.
pub(crate) fn index_dir(data_root: &Path) -> PathBuf {
    let dir = data_root.join("providers").join("zoekt").join("index");
    std::path::absolute(&dir).unwrap_or(dir)
}

/// Marker recording which snapshot the shard set was built for. A query
/// against any other snapshot id must not read the index — the working
/// tree may have moved.
const SNAPSHOT_MARKER: &str = ".cce-snapshot";

/// The snapshot id recorded by the last successful `ensure_index`.
pub(crate) fn indexed_snapshot(index_dir: &Path) -> Option<String> {
    std::fs::read_to_string(index_dir.join(SNAPSHOT_MARKER))
        .ok()
        .map(|marker| marker.trim().to_owned())
        .filter(|marker| !marker.is_empty())
}

/// Detect the toolchain and (re)build the shard set, reporting either
/// outcome — the composition both index paths share. A missing toolchain
/// degrades to a `Missing` report, never an index failure.
pub(crate) fn ensure_report(
    repo_root: &Path,
    data_root: &Path,
    snapshot_id: &str,
    timeout: Duration,
) -> ProviderReport {
    detect(data_root).map_or_else(missing_report, |tools| {
        ensure_index(&tools, repo_root, data_root, snapshot_id, timeout)
            .unwrap_or_else(|error| failed_report(&error))
    })
}

fn report_shell(state: ProviderState, message: Option<String>) -> ProviderReport {
    ProviderReport {
        provider_id: "zoekt:index".to_owned(),
        state,
        tool: None,
        message,
        artifact_digest: None,
        duration_ms: None,
        scip_documents: 0,
        scip_definitions: 0,
        scip_reference_edges: 0,
    }
}

/// Detect-state report for the providers surface — probes the toolchain
/// without executing anything.
pub(crate) fn detect_report(data_root: &Path) -> ProviderReport {
    match detect(data_root) {
        Some(tools) => ProviderReport {
            tool: Some(format!(
                "{} + {}",
                tools.index.display(),
                tools.search.display()
            )),
            ..report_shell(ProviderState::Ready, None)
        },
        None => missing_report(),
    }
}

fn missing_report() -> ProviderReport {
    report_shell(
        ProviderState::Missing,
        Some(
            "zoekt/zoekt-index not found; run `cce providers --provision` \
             to install the pinned toolchain into the managed cache, \
             install github.com/sourcegraph/zoekt yourself, or set \
             CCE_ZOEKT/CCE_ZOEKT_INDEX"
                .to_owned(),
        ),
    )
}

fn failed_report(error: &CceError) -> ProviderReport {
    report_shell(ProviderState::Failed, Some(error.to_string()))
}

/// (Re)build the shard set for `repo_root` and stamp it with
/// `snapshot_id`. zoekt-index rewrites shards atomically, so a rebuild is
/// safe against concurrent readers; it is cheap (~1-2 s at django scale)
/// so freshness is kept by rebuilding on every index pass rather than
/// tracking incremental state.
///
/// `data_root` is excluded by name: the engine's own artifact/blob output
/// must never enter the candidate index (the same self-indexing bug the
/// scanner fixes in `ignore.rs`, enforced here for a walker that does not
/// read `.cceignore`).
pub(crate) fn ensure_index(
    tools: &ZoektTools,
    repo_root: &Path,
    data_root: &Path,
    snapshot_id: &str,
    timeout: Duration,
) -> Result<ProviderReport> {
    let started = Instant::now();
    let mut report = ProviderReport {
        provider_id: "zoekt:index".to_owned(),
        state: ProviderState::Ready,
        tool: Some(format!(
            "{} + {}",
            tools.index.display(),
            tools.search.display()
        )),
        message: None,
        artifact_digest: None,
        duration_ms: None,
        scip_documents: 0,
        scip_definitions: 0,
        scip_reference_edges: 0,
    };
    let dir = index_dir(data_root);
    std::fs::create_dir_all(&dir).map_err(|error| CceError::io(&dir, error))?;
    let mut ignore = String::from(".git,.hg,.svn");
    if let Some(name) = data_root.file_name().and_then(|name| name.to_str()) {
        ignore.push(',');
        ignore.push_str(name);
    }
    if !ignore.contains(".cce") {
        ignore.push_str(",.cce");
    }
    let output = run_command(
        &tools.index,
        &[
            "-index",
            &dir.to_string_lossy(),
            "-ignore_dirs",
            &ignore,
            // Stock BSD ctags (macOS) cannot feed the symbol index;
            // universal-ctags presence makes it opt back in automatically.
            "-disable_ctags",
            &repo_root.to_string_lossy(),
        ],
        repo_root,
        timeout,
    )?;
    report.duration_ms = Some(started.elapsed().as_millis() as u64);
    if output.timed_out {
        report.state = ProviderState::Failed;
        report.message = Some(format!(
            "zoekt-index timed out after {}s",
            timeout.as_secs()
        ));
        return Ok(report);
    }
    if output.code.is_none_or(|code| code != 0) {
        report.state = ProviderState::Failed;
        report.message = Some(format!(
            "zoekt-index exited {:?}: {}",
            output.code,
            output.stderr_tail()
        ));
        return Ok(report);
    }
    // Stamp after a successful build only — a failed run leaves the
    // previous marker (and shard set) untouched so the last good index
    // keeps serving its own snapshot.
    std::fs::write(dir.join(SNAPSHOT_MARKER), snapshot_id)
        .map_err(|error| CceError::io(&dir, error))?;
    Ok(report)
}

/// One ranked file hit from `zoekt -jsonl`.
#[derive(Debug)]
pub(crate) struct ZoektFileHit {
    /// Repository-relative file path (`FileName`).
    pub path: String,
    /// Zoekt's own score (relative ordering only; the route ranks by
    /// position and fuses engine-side, so this is diagnostic for now).
    #[allow(dead_code)]
    pub score: f64,
    /// Matched 1-based line numbers — resolved to regions engine-side.
    pub lines: Vec<u32>,
    /// Decoded matched lines, capped — the snippet surface.
    pub matched_text: String,
    /// Language zoekt detected (informational).
    #[allow(dead_code)]
    pub language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonlFileMatch {
    #[serde(rename = "FileName")]
    file_name: String,
    #[serde(rename = "Score")]
    score: f64,
    #[serde(rename = "Language")]
    language: Option<String>,
    #[serde(rename = "LineMatches")]
    line_matches: Option<Vec<JsonlLineMatch>>,
}

#[derive(Debug, Deserialize)]
struct JsonlLineMatch {
    #[serde(rename = "Line")]
    line: Option<String>,
    #[serde(rename = "LineNumber")]
    line_number: Option<u32>,
    /// `true` when the "match" is the file name itself.
    #[serde(rename = "FileName")]
    file_name: Option<bool>,
}

/// Run the query CLI and return ranked file hits (max `limit`).
/// `index_dir` must already be freshness-checked by the caller.
pub(crate) fn search(
    tools: &ZoektTools,
    index_dir: &Path,
    query: &str,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<ZoektFileHit>> {
    let output = run_command(
        &tools.search,
        &["-index_dir", &index_dir.to_string_lossy(), "-jsonl", query],
        index_dir,
        timeout,
    )?;
    if output.timed_out {
        return Err(CceError::Configuration(format!(
            "zoekt query timed out after {}s",
            timeout.as_secs()
        )));
    }
    if output.code.is_none_or(|code| code != 0) {
        return Err(CceError::Configuration(format!(
            "zoekt exited {:?}: {}",
            output.code,
            output.stderr_tail()
        )));
    }
    let mut hits = Vec::new();
    for line in output.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: JsonlFileMatch = serde_json::from_str(line).map_err(|error| {
            CceError::Configuration(format!("zoekt -jsonl emitted an unparsable row: {error}"))
        })?;
        let mut lines = Vec::new();
        let mut matched_text = String::new();
        for matched in parsed.line_matches.unwrap_or_default() {
            if matched.file_name == Some(true) {
                continue;
            }
            if let Some(number) = matched.line_number {
                lines.push(number);
            }
            if matched_text.len() < 2_000 {
                if let Some(encoded) = matched.line {
                    if let Some(text) = decode_base64_line(&encoded) {
                        if !matched_text.is_empty() {
                            matched_text.push('\n');
                        }
                        matched_text.push_str(text.trim_end());
                    }
                }
            }
        }
        hits.push(ZoektFileHit {
            path: parsed.file_name,
            score: parsed.score,
            lines,
            matched_text,
            language: parsed.language,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

/// Build a zoekt query from raw natural-language/issue text. zoekt is a
/// trigram index — verbatim prose matches nothing — so the query becomes
/// an OR of high-signal atoms: `file:` atoms for path-shaped tokens,
/// literal atoms for identifier-ish or long words. Deliberately shallow:
/// candidate generation favors recall, ranking stays engine-side.
pub(crate) fn build_query(raw_query: &str) -> Option<String> {
    const MAX_FILE_ATOMS: usize = 6;
    const MAX_LITERAL_ATOMS: usize = 10;
    let mut file_atoms = Vec::new();
    let mut literal_atoms = Vec::new();
    for token in raw_query.split(|character: char| {
        !(character.is_alphanumeric()
            || character == '_'
            || character == '/'
            || character == '.'
            || character == '-')
    }) {
        if token.len() < 3 {
            continue;
        }
        if looks_like_path(token) {
            if file_atoms.len() < MAX_FILE_ATOMS && !file_atoms.iter().any(|atom| atom == token) {
                file_atoms.push(token.to_owned());
            }
        } else if looks_like_literal(token)
            && literal_atoms.len() < MAX_LITERAL_ATOMS
            && !literal_atoms.iter().any(|atom| atom == token)
        {
            literal_atoms.push(token.to_owned());
        }
    }
    let mut atoms: Vec<String> = Vec::new();
    for path in &file_atoms {
        atoms.push(format!("file:{}", escape_regexish(path)));
    }
    atoms.extend(literal_atoms.iter().map(|atom| escape_regexish(atom)));
    if atoms.is_empty() {
        return None;
    }
    Some(format!("case:no ({})", atoms.join(" or ")))
}

/// `dir/name.ext`-shaped tokens (need both a slash and a dotted suffix to
/// survive) — `file:` atoms name paths, not content.
fn looks_like_path(token: &str) -> bool {
    token.contains('/')
        && token.rsplit_once('.').is_some_and(|(stem, extension)| {
            stem.contains('/') && !extension.is_empty() && extension.len() <= 6
        })
}

/// Identifier-ish (`snake_case`, `camelCase`, `a.b` chains, `ALLCAPS`) or
/// long lowercase words — the same signal the planner's `looks_like_code`
/// reads, plus the prose tail that carries topical vocabulary.
fn looks_like_literal(token: &str) -> bool {
    if token.contains('_')
        || token.contains("::")
        || token.chars().any(char::is_uppercase) && token.chars().any(char::is_lowercase)
    {
        return true;
    }
    if token
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && token.len() >= 4
    {
        return true;
    }
    // Dotted receiver chains (`self.x`, `os.path`) keep their leaf shape.
    if token.contains('.')
        && token
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return true;
    }
    token.len() >= 5 && token.chars().all(|c| c.is_ascii_lowercase()) && !is_common_word(token)
}

/// Small stoplist for the lowercase-word pool — identifier atoms carry
/// most of the signal, so the bar for prose is "uncommon enough to
/// discriminate", not a full English stoplist.
fn is_common_word(token: &str) -> bool {
    matches!(
        token,
        "actually"
            | "anything"
            | "because"
            | "behavior"
            | "between"
            | "callback"
            | "currently"
            | "different"
            | "document"
            | "everything"
            | "expected"
            | "failure"
            | "function"
            | "happened"
            | "however"
            | "instead"
            | "internal"
            | "manager"
            | "message"
            | "nothing"
            | "problem"
            | "request"
            | "response"
            | "returns"
            | "running"
            | "should"
            | "something"
            | "sometimes"
            | "through"
            | "version"
            | "without"
            | "working"
    )
}

/// zoekt literal atoms are regex-flavored; quote the characters that
/// would change meaning (`a.b` should match the literal chain).
fn escape_regexish(atom: &str) -> String {
    let mut escaped = String::with_capacity(atom.len() + 4);
    for character in atom.chars() {
        if matches!(
            character,
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// zoekt `-jsonl` base64-encodes matched line content.
fn decode_base64_line(encoded: &str) -> Option<String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut decoded = Vec::with_capacity(encoded.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits = 0_u32;
    for byte in encoded.bytes() {
        if byte == b'=' {
            break;
        }
        let value = TABLE.iter().position(|entry| *entry == byte)? as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push((accumulator >> bits) as u8);
        }
    }
    String::from_utf8(decoded).ok()
}

/// Regions covering a matched line, most specific first (a method beats
/// the file region that contains it). `regions` arrive ordered by byte
/// range; sort the survivors by span so entity-level evidence outranks
/// containers.
pub(crate) fn smallest_regions_for_lines(
    regions: &[cce_core::CodeRegion],
    lines: &[u32],
    cap: usize,
) -> Vec<String> {
    let mut containing: Vec<&cce_core::CodeRegion> = regions
        .iter()
        .filter(|region| {
            lines
                .iter()
                .any(|line| region.start_line <= *line && *line <= region.end_line)
        })
        .collect();
    containing.sort_by_key(|region| region.end_line.saturating_sub(region.start_line));
    containing
        .into_iter()
        .take(cap)
        .map(|region| region.id.clone())
        .collect()
}

/// Install the pinned toolchain into the managed cache — the explicit
/// network opt-in behind `cce providers --provision`.
///
/// Assets built by CCE's release CI are preferred (checksum-verified);
/// when this build's version has no release assets the fallback installs
/// the same pinned revision through `go install`. Either way the
/// binaries land in `bin_dir`, where `detect` picks them up.
///
/// Blocking: performs network I/O and subprocesses. Callers on an async
/// executor must use `spawn_blocking` — the blocking HTTP client's
/// runtime cannot drop inside async context.
pub fn provision(data_root: &Path) -> ProviderReport {
    if let Some(tools) = detect(data_root) {
        return ProviderReport {
            tool: Some(format!(
                "{} + {}",
                tools.index.display(),
                tools.search.display()
            )),
            ..report_shell(ProviderState::Ready, None)
        };
    }
    let dir = bin_dir(data_root);
    match provision_into(&dir) {
        Ok(source) => {
            write_manifest(&dir, source);
            match detect(data_root) {
                Some(tools) => ProviderReport {
                    tool: Some(format!(
                        "{} + {}",
                        tools.index.display(),
                        tools.search.display()
                    )),
                    ..report_shell(
                        ProviderState::Ready,
                        Some(format!("provisioned zoekt@{ZOEKT_REF} ({source})")),
                    )
                },
                None => report_shell(
                    ProviderState::Failed,
                    Some(format!(
                        "binaries installed but not found under {}",
                        dir.display()
                    )),
                ),
            }
        }
        Err(error) => report_shell(
            ProviderState::Missing,
            Some(format!(
                "{error}; install github.com/sourcegraph/zoekt manually or set \
                 CCE_ZOEKT/CCE_ZOEKT_INDEX"
            )),
        ),
    }
}

fn provision_into(dir: &Path) -> Result<&'static str, String> {
    download_release(dir).or_else(|release_err| {
        go_install(dir)
            .map(|()| "go-install")
            .map_err(|go_err| format!("release download: {release_err}; go install: {go_err}"))
    })
}

/// Download `zoekt`/`zoekt-index` assets for the host triple from the
/// `v{CARGO_PKG_VERSION}` GitHub release, verifying each against the
/// release's `zoekt-SHA256SUMS.txt` before atomically renaming into
/// place.
fn download_release(dir: &Path) -> Result<&'static str, String> {
    let triple = host_triple().ok_or_else(|| {
        format!(
            "no release asset for host {}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )
    })?;
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let base = format!("https://github.com/wibus-wee/CCE/releases/download/{tag}");
    let client = reqwest::blocking::Client::new();
    let sums = fetch_text(&client, &format!("{base}/zoekt-SHA256SUMS.txt"))
        .map_err(|e| format!("release {tag} zoekt sums unavailable: {e}"))?;
    let expected = parse_sha256sums(&sums);
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    for tool in ["zoekt", "zoekt-index"] {
        let asset = format!("{tool}-{triple}{}", exe_suffix());
        let want = expected
            .get(asset.as_str())
            .ok_or_else(|| format!("{asset} missing from zoekt-SHA256SUMS.txt"))?;
        let bytes = fetch_bytes(&client, &format!("{base}/{asset}"))?;
        let got = hex_sha256(&bytes);
        if got != *want {
            return Err(format!(
                "{asset} checksum mismatch: expected {want}, got {got}"
            ));
        }
        install_binary(dir, &exe_name(tool), &bytes)
            .map_err(|e| format!("install {asset}: {e}"))?;
    }
    Ok("release")
}

/// `go install` both binaries at the pinned revision into a staging dir,
/// then rename into `dir`. The Go checksum database already attests the
/// module bytes; the staged rename keeps an interrupted install from
/// leaving a half-written binary in the detected location.
fn go_install(dir: &Path) -> Result<(), String> {
    let go = on_path("go").ok_or_else(|| "go toolchain not on PATH".to_owned())?;
    let staging = dir.join(".staging");
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    for tool in ["zoekt", "zoekt-index"] {
        let output = std::process::Command::new(&go)
            .env("GOBIN", &staging)
            .args([
                "install",
                &format!("github.com/sourcegraph/zoekt/cmd/{tool}@{ZOEKT_REF}"),
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "go install {tool}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let staged = staging.join(exe_name(tool));
        std::fs::rename(&staged, dir.join(exe_name(tool))).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir(&staging);
    Ok(())
}

fn fetch_text(client: &reqwest::blocking::Client, url: &str) -> Result<String, String> {
    let body = fetch_bytes(client, url)?;
    String::from_utf8(body).map_err(|e| e.to_string())
}

fn fetch_bytes(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::bytes)
        .map(|bytes| bytes.to_vec())
        .map_err(|e| e.to_string())
}

/// Parse `<hex>  <name>` / `<hex> *<name>` lines from a sha256sum
/// manifest (GNU coreutils text/binary marker included).
fn parse_sha256sums(body: &str) -> std::collections::HashMap<String, String> {
    body.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hex = parts.next()?;
            let name = parts.next()?;
            Some((name.trim_start_matches('*').to_owned(), hex.to_owned()))
        })
        .collect()
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Write bytes to a temp file in `dir`, mark executable, rename into
/// place — partial downloads never occupy the detected path.
fn install_binary(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, dir.join(name))
}

fn write_manifest(dir: &Path, source: &str) {
    let manifest = serde_json::json!({
        "provider": "zoekt",
        "ref": ZOEKT_REF,
        "source": source,
    });
    let _ = std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    );
}

fn host_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

const fn exe_suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_query_extracts_identifiers_and_paths() {
        let query = build_query(
            "SessionStore fails when `signed_cookies` backend reads request.COOKIES \
             in django/contrib/sessions/backends/signed_cookies.py",
        )
        .expect("atoms");
        assert!(query.contains("file:django/contrib/sessions/backends/signed_cookies\\.py"));
        assert!(query.contains("SessionStore"));
        assert!(query.starts_with("case:no ("));
    }

    #[test]
    fn build_query_or_structure() {
        let query = build_query("csrf middleware validation").expect("atoms");
        assert!(query.contains(" or "));
        assert!(query.contains("middleware"));
    }

    #[test]
    fn build_query_empty_on_prose() {
        assert!(build_query("the a an of to in on").is_none());
    }

    #[test]
    fn decode_base64_roundtrip() {
        let encoded = "ZnJvbSBkamFuZ28uaW8gaW1wb3J0IHgK";
        assert_eq!(
            decode_base64_line(encoded).as_deref(),
            Some("from django.io import x\n")
        );
    }

    #[test]
    fn parse_sha256sums_handles_text_and_binary_markers() {
        let sums = parse_sha256sums(
            "abc123  zoekt-x86_64-unknown-linux-gnu\n\
             def456 *zoekt-index-x86_64-pc-windows-msvc.exe\n",
        );
        assert_eq!(
            sums.get("zoekt-x86_64-unknown-linux-gnu")
                .map(String::as_str),
            Some("abc123")
        );
        assert_eq!(
            sums.get("zoekt-index-x86_64-pc-windows-msvc.exe")
                .map(String::as_str),
            Some("def456")
        );
    }

    #[test]
    fn detect_finds_managed_cache_binaries() {
        let data = tempfile::tempdir().expect("tempdir");
        let bin = bin_dir(data.path());
        std::fs::create_dir_all(&bin).expect("mkdir");
        for tool in ["zoekt", "zoekt-index"] {
            std::fs::write(bin.join(exe_name(tool)), b"#!/bin/sh\n").expect("stub");
        }
        // PATH/env may already resolve zoekt on a dev machine; the
        // managed lane must at least be consulted, so probe each lane's
        // fallback directly rather than the whole chain.
        let tools = on_disk(&bin, "zoekt").zip(on_disk(&bin, "zoekt-index"));
        assert!(tools.is_some());
    }

    #[test]
    fn detect_reports_missing_without_tools() {
        // With no env override, no PATH match, and an empty managed cache
        // the report must surface Missing — never silent coverage.
        let data = tempfile::tempdir().expect("tempdir");
        if detect(data.path()).is_none() {
            assert_eq!(detect_report(data.path()).state, ProviderState::Missing);
        }
    }

    #[test]
    fn install_binary_lands_executable_without_temp_leftover() {
        let dir = tempfile::tempdir().expect("tempdir");
        install_binary(dir.path(), "zoekt", b"binary-bytes").expect("install");
        let installed = dir.path().join("zoekt");
        assert_eq!(std::fs::read(&installed).expect("read"), b"binary-bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&installed)
                    .expect("meta")
                    .permissions()
                    .mode()
                    & 0o111,
                0o111
            );
        }
        assert!(!dir.path().join(".zoekt.tmp").exists());
    }

    #[test]
    fn smallest_regions_prefers_narrowest_span() {
        let regions = vec![
            region("file", 1, 500),
            region("class", 10, 200),
            region("method", 42, 60),
        ];
        let ids = smallest_regions_for_lines(&regions, &[50], 2);
        assert_eq!(ids, ["method", "class"]);
    }

    fn region(id: &str, start_line: u32, end_line: u32) -> cce_core::CodeRegion {
        cce_core::CodeRegion {
            id: id.to_owned(),
            snapshot_id: "snap".to_owned(),
            path: "f.py".to_owned(),
            kind: cce_core::RegionKind::Symbol,
            language: None,
            symbol_name: None,
            symbol_kind: None,
            qualified_name: None,
            parent_region_id: None,
            start_byte: 0,
            end_byte: 0,
            start_line,
            end_line,
        }
    }
}
