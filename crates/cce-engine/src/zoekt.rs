//! Zoekt trigram index integration: Sourcegraph's code-search index run
//! as an external provider. The shard directory lives under the data root
//! (disk-resident, never ingested as artifacts) and serves query-time file
//! candidates through the `zoekt` CLI — no daemon, no port, identical in
//! the `cce` one-shot path and `cce-daemon`. File hits resolve back to
//! snapshot documents through `LineMatch` line numbers, so provenance
//! stays in `SQLite` even though candidate generation is external.
//!
//! Detection mirrors SCIP providers: binaries must already exist on the
//! machine (`PATH`, or `CCE_ZOEKT`/`CCE_ZOEKT_INDEX` overrides); nothing
//! is downloaded.

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

/// Locate the zoekt toolchain. Env overrides win so a caller can pin a
/// specific build; otherwise PATH is probed without executing anything.
pub(crate) fn detect() -> Option<ZoektTools> {
    let index = std::env::var_os("CCE_ZOEKT_INDEX")
        .map(PathBuf::from)
        .or_else(|| on_path("zoekt-index"))?;
    let search = std::env::var_os("CCE_ZOEKT")
        .map(PathBuf::from)
        .or_else(|| on_path("zoekt"))?;
    Some(ZoektTools { index, search })
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
    detect().map_or_else(missing_report, |tools| {
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
pub(crate) fn detect_report() -> ProviderReport {
    match detect() {
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
            "zoekt/zoekt-index not on PATH; install \
             github.com/sourcegraph/zoekt (`go install \
             github.com/sourcegraph/zoekt/cmd/zoekt@latest \
             github.com/sourcegraph/zoekt/cmd/zoekt-index@latest`) \
             or set CCE_ZOEKT/CCE_ZOEKT_INDEX"
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
