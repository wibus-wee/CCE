#![forbid(unsafe_code)]
// Test fixtures panic freely: an unmet test precondition is a test bug, not a
// recoverable error path.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

//! End-to-end coverage for the engine lifecycle: index → status → stale
//! detection → retrieval → context → GC. Every test builds its own
//! temporary repository so nothing outside the tempdir is touched.

use std::fs;
use std::path::Path;

use cce_core::{RetrievalRepresentation, SearchRequest, ViewKind, ViewState};
use cce_engine::{CceEngine, ContextRequest, EngineConfig};

fn write(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(&path, contents).expect("write fixture file");
}

fn fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    cursor.is_empty()\n}\n",
    );
    write(dir.path(), "README.md", "# fixture repository\n");
    dir
}

fn engine(dir: &Path) -> CceEngine {
    CceEngine::open(EngineConfig::for_repository(dir)).expect("open engine")
}

fn search_request(query: &str, require_fresh: bool) -> SearchRequest {
    SearchRequest {
        repository_id: String::new(),
        snapshot_id: String::new(),
        query: query.to_owned(),
        intent: None,
        limit: 10,
        require_fresh,
        routes: Vec::new(),
        filters: cce_core::QueryFilters::default(),
    }
}

#[tokio::test]
async fn index_then_status_then_incremental_reuse() {
    let repo = fixture_repo();
    let engine = engine(repo.path());

    let report = engine.index().await.expect("index");
    assert!(!report.reused_snapshot);
    assert_eq!(report.indexed_files, 2);
    assert!(report.source_units >= 1);
    assert!(report.retrieval_documents >= 2);

    let manifest = engine.status().expect("status");
    assert_eq!(manifest.views[&ViewKind::Lexical].state, ViewState::Ready);
    assert_eq!(manifest.views[&ViewKind::Source].state, ViewState::Ready);

    // Unchanged working tree: the same snapshot is reused without reading or
    // re-parsing file bytes.
    let second = engine.index().await.expect("second index");
    assert!(second.reused_snapshot);
    assert_eq!(second.snapshot.id, report.snapshot.id);
}

#[tokio::test]
async fn working_tree_change_marks_views_stale() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    !cursor.is_empty() && cursor.len() > 4\n}\n",
    );

    let manifest = engine.status().expect("status");
    assert_eq!(manifest.views[&ViewKind::Lexical].state, ViewState::Stale);
}

#[tokio::test]
async fn require_fresh_false_serves_committed_snapshot_unverified() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    let report = engine.index().await.expect("index");

    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    cursor.len() >= 100\n}\n",
    );

    // require_fresh=false: no rescan, old snapshot served, hits unverified.
    let stale_result = engine
        .search(search_request("resume_attempt", false))
        .await
        .expect("stale search");
    assert_eq!(stale_result.request.snapshot_id, report.snapshot.id);
    assert!(
        stale_result
            .missing_capabilities
            .iter()
            .any(|message| message.contains("requireFresh=false"))
    );
    assert!(stale_result.hits.iter().all(|hit| !hit.verified_current));

    // require_fresh=true: the working tree is scanned and reindexed.
    let fresh_result = engine
        .search(search_request("resume_attempt", true))
        .await
        .expect("fresh search");
    assert_ne!(fresh_result.request.snapshot_id, report.snapshot.id);
    assert!(fresh_result.hits.iter().all(|hit| hit.verified_current));
}

#[tokio::test]
async fn sensitive_files_are_excluded_from_index() {
    let repo = fixture_repo();
    write(
        repo.path(),
        ".env",
        "CCE_TEST_SECRET=supersecret-value-not-for-index\n",
    );
    let engine = engine(repo.path());

    let report = engine.index().await.expect("index");
    assert!(
        report
            .skipped_sensitive_files
            .iter()
            .any(|path| path == ".env")
    );

    let result = engine
        .search(search_request("supersecret", true))
        .await
        .expect("search");
    assert!(result.hits.is_empty());

    // Opt-in exists for callers that explicitly want it.
    let mut config = EngineConfig::for_repository(repo.path());
    config.data_root = repo.path().join(".cce-optin");
    config.index.include_sensitive = true;
    let opt_in = CceEngine::open(config).expect("opt-in engine");
    let report = opt_in.index().await.expect("opt-in index");
    assert!(report.skipped_sensitive_files.is_empty());
}

#[tokio::test]
async fn gc_prunes_old_snapshots_and_keeps_current_usable() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("first index");
    write(
        repo.path(),
        "src/extra.rs",
        "pub fn first_revision() -> u32 { 1 }\n",
    );
    engine.index().await.expect("second index");
    write(
        repo.path(),
        "src/more.rs",
        "pub fn second_revision() -> u32 { 2 }\n",
    );
    engine.index().await.expect("third index");

    let report = engine.gc(0).expect("gc");
    assert!(report.pruned_snapshots >= 2);

    let manifest = engine.status().expect("status after gc");
    assert_eq!(manifest.views[&ViewKind::Lexical].state, ViewState::Ready);
}

/// Workspace fixture: two crates with a declared dependency and a call
/// across the boundary, plus a test file exercising it.
fn workspace_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/alpha\", \"crates/beta\"]\n",
    );
    write(
        dir.path(),
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbeta = { path = \"../beta\" }\n",
    );
    write(
        dir.path(),
        "crates/alpha/src/lib.rs",
        "pub fn helper() -> u32 {\n    beta_fn()\n}\n\npub fn verify(token: &beta::Token) -> bool {\n    token.id > 0\n}\n",
    );
    write(
        dir.path(),
        "crates/alpha/tests/test_alpha.rs",
        "#[test]\nfn test_helper_works() {\n    assert_eq!(helper(), 7);\n}\n",
    );
    write(
        dir.path(),
        "crates/beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        dir.path(),
        "crates/beta/src/lib.rs",
        "pub struct Token {\n    pub id: u32,\n}\n\npub fn beta_fn() -> u32 {\n    7\n}\n",
    );
    dir
}

#[tokio::test]
async fn regions_are_canonical_and_persisted() {
    let repo = workspace_fixture();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    let result = engine
        .search(search_request("beta_fn", true))
        .await
        .expect("search");
    assert!(!result.hits.is_empty());
    // Every source-linked hit resolves to a persisted canonical region.
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.address.is_none() || hit.region_id.is_some())
    );
    // Symbol-level retrieval: the winning hit covers `beta_fn`, not the
    // whole file.
    let symbol_hit = result
        .hits
        .iter()
        .find(|hit| hit.symbol_name.as_deref() == Some("beta_fn"))
        .expect("a beta_fn symbol hit");
    let address = symbol_hit.address.as_ref().expect("address");
    assert!(address.end_byte - address.start_byte < 200);
}

#[tokio::test]
async fn typed_relations_carry_provenance_and_confidence() {
    let repo = workspace_fixture();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // Calls edge: alpha::helper calls beta::beta_fn (tree-sitter provenance).
    let impact = engine.impact_analysis("beta_fn").expect("impact analysis");
    assert!(
        impact
            .impacted
            .iter()
            .any(|entity| entity.name == "helper" && entity.via == "Calls" && entity.hops == 1)
    );
    assert!(
        impact
            .caveats
            .iter()
            .any(|message| message.contains("tree-sitter"))
    );

    // Tests edge: test_helper_works targets helper.
    let impact = engine.impact_analysis("helper").expect("impact on helper");
    assert!(
        impact
            .impacted
            .iter()
            .any(|entity| entity.name == "test_helper_works" && entity.via == "Tests")
    );

    // References edge: alpha::verify names beta::Token in a type position.
    let impact = engine.impact_analysis("Token").expect("impact on Token");
    assert!(
        impact.impacted.iter().any(|entity| entity.name == "verify"
            && entity.via == "References"
            && entity.hops == 1),
        "expected verify → Token References edge, got {:?}",
        impact
            .impacted
            .iter()
            .map(|entity| (entity.name.clone(), entity.via.clone()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn package_graph_comes_from_build_manifests() {
    let repo = workspace_fixture();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    let map = engine.codebase_map().expect("codebase map");
    let alpha = map
        .packages
        .iter()
        .find(|package| package.name == "alpha")
        .expect("alpha package");
    assert_eq!(alpha.ecosystem, "cargo");
    assert_eq!(alpha.dependencies, vec!["beta".to_owned()]);
    let beta = map
        .packages
        .iter()
        .find(|package| package.name == "beta")
        .expect("beta package");
    assert_eq!(beta.dependents, vec!["alpha".to_owned()]);
    assert_eq!(map.provenance, "build_system manifests (deterministic)");

    let explanation = engine.explain_component("alpha").expect("explain alpha");
    assert_eq!(explanation.kind, "Package");
    assert!(
        explanation
            .member_files
            .iter()
            .any(|path| path == "crates/alpha/src/lib.rs")
    );
    assert_eq!(explanation.dependencies, vec!["beta".to_owned()]);
}

#[tokio::test]
async fn context_pack_is_bounded_and_source_linked() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    let pack = engine
        .context(ContextRequest::new("resume_attempt", 4_096))
        .await
        .expect("context");
    assert!(!pack.items.is_empty());
    assert!(pack.used_tokens <= 4_096);
    assert!(
        pack.items
            .iter()
            .any(|item| item.provenance.verified_current)
    );
}

/// Honest abstention: when no query terms — or only a single coincidental
/// term — match the index, search must return zero hits rather than noise
/// from the lexical prefix fallback. `abstained` in the benchmark adapter is
/// derived from an empty hit list.
///
/// The nonsense tokens are invented words that appear nowhere in this
/// repository: the cce-self benchmark indexes the working tree, so literal
/// reuse of the dataset's control-case vocabulary here would turn this test
/// file into genuine evidence and defeat the control.
#[tokio::test]
async fn zero_evidence_query_abstains() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // Nonsense control: no content term exists in the index at all.
    let nonsense = engine
        .search(search_request("zorblax quinthar vexmoor kraggle", true))
        .await
        .expect("nonsense search");
    assert!(
        nonsense.hits.is_empty(),
        "zero-evidence query must abstain, got {:?}",
        nonsense
            .hits
            .iter()
            .map(|hit| hit.document_id.clone())
            .collect::<Vec<_>>()
    );

    // One coincidental prefix hit ("cursor") among several unmatched terms is
    // not evidence: the fallback requires at least two distinct terms.
    let single_term = engine
        .search(search_request("zorblax quinthar vexmoor cursor", true))
        .await
        .expect("single-term-overlap search");
    assert!(
        single_term.hits.is_empty(),
        "single-term prefix overlap must not count as evidence, got {:?}",
        single_term
            .hits
            .iter()
            .map(|hit| hit.document_id.clone())
            .collect::<Vec<_>>()
    );

    // Positive control: a real multi-term query still retrieves the fixture.
    let control = engine
        .search(search_request("resume_attempt cursor", true))
        .await
        .expect("control search");
    assert!(
        control
            .hits
            .iter()
            .any(|hit| hit.symbol_name.as_deref() == Some("resume_attempt")),
        "expected a resume_attempt hit, got {:?}",
        control
            .hits
            .iter()
            .map(|hit| hit.symbol_name.clone())
            .collect::<Vec<_>>()
    );
}

/// Code-switched CJK+Latin queries: unicode61 indexes a CJK run as one
/// monolithic token, so CJK terms can never match Latin-script source —
/// requiring them vetoes every real document. The embedded Latin word is
/// the deliberate anchor and must carry the retrieval load on its own.
#[tokio::test]
async fn code_switched_query_reaches_latin_anchor() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // "cursor" is a parameter name — present in source but not an entity —
    // so only the lexical Latin-anchor stages can find it. The CJK phrase
    // matches nothing and must not veto it.
    let result = engine
        .search(search_request("cursor 在哪里被使用", true))
        .await
        .expect("code-switched search");
    assert!(
        result
            .hits
            .iter()
            .any(|hit| hit.symbol_name.as_deref() == Some("resume_attempt")),
        "latin anchor must surface the fixture symbol, got {:?}",
        result
            .hits
            .iter()
            .map(|hit| hit.symbol_name.clone())
            .collect::<Vec<_>>()
    );
}

/// Built-in ignore policy: lock files are unconditional skips regardless
/// of .gitignore state, reported with a reason, and never reach FTS or
/// retrieval documents.
#[tokio::test]
async fn lock_files_are_unconditionally_ignored() {
    let repo = fixture_repo();
    write(
        repo.path(),
        "Cargo.lock",
        "# lockfile\n[[package]]\nname = \"zorblax_lockdep\"\nversion = \"9.9.9\"\n",
    );
    write(
        repo.path(),
        "package-lock.json",
        "{\n  \"name\": \"zorblax_lockdep\",\n  \"lockfileVersion\": 3\n}\n",
    );
    let engine = engine(repo.path());

    let report = engine.index().await.expect("index");
    let skipped: Vec<&str> = report
        .skipped_builtin_files
        .iter()
        .map(|(path, reason)| {
            assert_eq!(reason, "lockfile");
            path.as_str()
        })
        .collect();
    assert!(skipped.contains(&"Cargo.lock"), "got {skipped:?}");
    assert!(skipped.contains(&"package-lock.json"), "got {skipped:?}");

    // A token that only exists inside lock files must not retrieve them.
    let result = engine
        .search(search_request("zorblax_lockdep", true))
        .await
        .expect("search");
    assert!(
        result.hits.is_empty(),
        "lock-file content must not be indexed, got {:?}",
        result
            .hits
            .iter()
            .map(|hit| hit.document_id.clone())
            .collect::<Vec<_>>()
    );
}

/// A configured-but-broken reranker degrades to fused order with an
/// explicit missing capability — never a silent drop or a hard error.
#[tokio::test]
async fn broken_reranker_falls_back_with_missing_capability() {
    let repo = fixture_repo();
    let mut config = EngineConfig::for_repository(repo.path());
    config.reranker_model = Some("nonexistent/reranker-model".to_owned());
    let engine = CceEngine::open(config).expect("open engine");
    engine.index().await.expect("index");

    let result = engine
        .search(search_request("resume_attempt cursor", true))
        .await
        .expect("search must not fail on reranker init");
    assert!(
        result
            .hits
            .iter()
            .any(|hit| hit.symbol_name.as_deref() == Some("resume_attempt")),
        "fused ranking must still serve hits"
    );
    assert!(
        result
            .missing_capabilities
            .iter()
            .any(|message| message.contains("reranker")),
        "expected explicit reranker missing-capability, got {:?}",
        result.missing_capabilities
    );
    assert!(
        result.hits.iter().all(|hit| !hit
            .contributing_routes
            .contains(&cce_core::SearchRoute::Reranked)),
        "failed reranker must not stamp the Reranked route"
    );
}

#[tokio::test]
async fn atlas_on_unindexed_repository_is_an_explicit_error() {
    let repo = fixture_repo();
    let engine = engine(repo.path());

    for outcome in [
        engine.codebase_map().map(|_| ()),
        engine.explain_component("anything").map(|_| ()),
        engine.impact_analysis("anything").map(|_| ()),
    ] {
        let error = outcome.expect_err("atlas must fail without an index");
        assert!(
            matches!(error, cce_core::CceError::ViewUnavailable { .. }),
            "expected ViewUnavailable, got {error:?}"
        );
    }
}

/// `lang:`/`path:` tokens become structured filters applied conjunctively
/// across routes, and the expanded FTS name forms let camelCase or folded
/// spellings reach `snake_case` identifiers.
#[tokio::test]
async fn query_filters_and_identifier_forms() {
    let repo = workspace_fixture();
    write(repo.path(), "docs/notes.md", "beta_fn is mentioned here\n");
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // Folded spelling reaches the snake_case entity name.
    let folded = engine
        .search(search_request("betafn", true))
        .await
        .expect("folded search");
    assert!(
        folded
            .hits
            .iter()
            .any(|hit| hit.symbol_name.as_deref() == Some("beta_fn")),
        "folded query must reach beta_fn, got {:?}",
        folded
            .hits
            .iter()
            .map(|hit| hit.symbol_name.clone())
            .collect::<Vec<_>>()
    );

    // camelCase spelling likewise.
    let camel = engine
        .search(search_request("betaFn", true))
        .await
        .expect("camel search");
    assert!(
        camel
            .hits
            .iter()
            .any(|hit| hit.symbol_name.as_deref() == Some("beta_fn")),
        "camelCase query must reach beta_fn"
    );

    // path: confines every hit to the prefix — alpha's lib.rs legitimately
    // matches "beta_fn" because it calls the function.
    let confined = engine
        .search(search_request("beta_fn path:crates/alpha", true))
        .await
        .expect("path-filtered search");
    assert!(
        confined.hits.iter().all(|hit| hit
            .address
            .as_ref()
            .is_some_and(|address| { address.path.starts_with("crates/alpha") })),
        "path:crates/alpha must confine hits, got {:?}",
        confined
            .hits
            .iter()
            .map(|hit| hit.address.as_ref().map(|address| address.path.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        confined
            .hits
            .iter()
            .all(|hit| hit.symbol_name.as_deref() != Some("beta_fn")),
        "the beta_fn definition lives in the beta crate"
    );
    assert_eq!(
        confined.request.filters.path_prefix.as_deref(),
        Some("crates/alpha")
    );
    assert_eq!(confined.request.query, "beta_fn");

    let included = engine
        .search(search_request("beta_fn path:crates/beta", true))
        .await
        .expect("path-filtered search");
    assert!(
        included
            .hits
            .iter()
            .any(|hit| hit.symbol_name.as_deref() == Some("beta_fn"))
    );

    // lang: filters on the entity's detected language.
    let rust_only = engine
        .search(search_request("beta_fn lang:rust", true))
        .await
        .expect("lang-filtered search");
    assert!(!rust_only.hits.is_empty());
    let python_only = engine
        .search(search_request("beta_fn lang:python", true))
        .await
        .expect("lang-filtered search");
    assert!(python_only.hits.is_empty());
}

/// Worktree grep never touches the snapshot or the sensitive-file policy:
/// `.env` content must not come back even when it matches the pattern.
#[tokio::test]
async fn worktree_grep_is_fresh_and_policy_aware() {
    let repo = fixture_repo();
    write(repo.path(), ".env", "CCE_GREP_SECRET=do-not-return\n");
    write(repo.path(), "src/extra.rs", "fn marked_target() {}\n");
    let engine = engine(repo.path());

    let report = engine
        .grep(&cce_engine::GrepRequest {
            pattern: "marked_target|CCE_GREP_SECRET".to_owned(),
            filters: cce_core::QueryFilters::default(),
            limit: 50,
            ignore_case: false,
        })
        .expect("grep");
    assert!(
        report
            .matches
            .iter()
            .any(|hit| hit.path == "src/extra.rs" && hit.line == 1)
    );
    assert!(
        report.matches.iter().all(|hit| hit.path != ".env"),
        "sensitive files must never produce grep hits: {:?}",
        report.matches
    );
    assert_eq!(report.freshness, "worktree");

    // path: filter equivalent.
    let filtered = engine
        .grep(&cce_engine::GrepRequest {
            pattern: "resume_attempt".to_owned(),
            filters: cce_core::QueryFilters {
                path_prefix: Some("src/".to_owned()),
                language: None,
                hit_type: None,
            },
            limit: 50,
            ignore_case: false,
        })
        .expect("filtered grep");
    assert!(
        filtered
            .matches
            .iter()
            .all(|hit| hit.path.starts_with("src/"))
    );
    assert!(!filtered.matches.is_empty());
}

// --- history diff content ----------------------------------------------------

/// Run `git` inside `dir`; the fixture is invalid when the binary or the
/// command fails, so failures panic rather than degrade.
fn git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .arg("-c")
        .arg("user.email=cce@test")
        .arg("-c")
        .arg("user.name=cce test")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repo with two commits; the second changes one line and adds a marker
/// function, giving history one extractable diff hunk.
fn history_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn first_marker() -> u32 {\n    compute_first()\n}\n",
    );
    write(dir.path(), "README.md", "# fixture repository\n");
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-m", "add first_marker"]);
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn first_marker() -> u32 {\n    compute_second()\n}\n\npub fn second_lineage_marker() -> u32 {\n    7\n}\n",
    );
    git(dir.path(), &["add", "-A"]);
    git(
        dir.path(),
        &[
            "commit",
            "-m",
            "switch to compute_second and add second_lineage_marker",
        ],
    );
    dir
}

fn commit_diff_documents(engine: &CceEngine, snapshot_id: &str) -> Vec<cce_store::DocumentContent> {
    engine
        .store()
        .documents_for_snapshot(snapshot_id)
        .expect("documents")
        .into_iter()
        .filter(|document| document.representation == RetrievalRepresentation::CommitDiff)
        .collect()
}

#[tokio::test]
async fn history_diff_content_is_indexed_and_retrievable() {
    let repo = history_repo();
    let engine = engine(repo.path());
    let report = engine.index().await.expect("index");

    let history = report
        .manifest
        .views
        .get(&ViewKind::History)
        .expect("history view status");
    assert_eq!(history.state, ViewState::Ready);
    assert!(
        history
            .capabilities
            .iter()
            .any(|capability| capability.name == "git_diff_hunks")
    );

    // The root commit is its own whole tree; only the second commit
    // produces a diff document.
    let diffs = commit_diff_documents(&engine, &report.snapshot.id);
    assert_eq!(diffs.len(), 1);
    let diff = diffs.first().expect("one commit diff document");
    assert!(diff.text.contains("diff --git a/src/lib.rs"));
    assert!(diff.text.contains("+pub fn second_lineage_marker"));
    assert!(diff.text.contains("-    compute_first()"));
    assert!(diff.text.contains("@@"));
    assert!(
        diff.evidence
            .iter()
            .any(|address| address.path == "src/lib.rs" && address.start_line > 0),
        "expected evidence addresses on src/lib.rs, got {:?}",
        diff.evidence
    );

    // The extracted changed line is reachable through lexical search.
    let hits = engine
        .store()
        .lexical_search(
            &report.snapshot.id,
            "second_lineage_marker",
            10,
            &cce_core::QueryFilters::default(),
        )
        .expect("lexical search");
    assert!(
        hits.iter()
            .any(|hit| hit.representation == RetrievalRepresentation::CommitDiff),
        "commit diff document must match the changed line, got {:?}",
        hits.iter()
            .map(|hit| hit.document_id.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn sensitive_files_stay_out_of_history_documents() {
    let repo = history_repo();
    // Commit a credential-shaped file through git so it exists in history
    // even though the scanner skips the worktree copy.
    write(
        repo.path(),
        ".env",
        "CCE_HISTORY_SECRET=hunter2-not-for-index\n",
    );
    write(repo.path(), "src/extra.rs", "pub fn third_marker() {}\n");
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-m", "add env and extra"]);
    let engine = engine(repo.path());
    let report = engine.index().await.expect("index");

    let diffs = commit_diff_documents(&engine, &report.snapshot.id);
    assert!(!diffs.is_empty());
    for document in &diffs {
        assert!(
            !document.text.contains("hunter2"),
            "secret leaked into patch artifact of {}",
            document.document_id
        );
        assert!(
            !document.text.contains(".env"),
            "sensitive path leaked into patch artifact of {}",
            document.document_id
        );
    }
    let hits = engine
        .store()
        .lexical_search(
            &report.snapshot.id,
            "hunter2 CCE_HISTORY_SECRET",
            10,
            &cce_core::QueryFilters::default(),
        )
        .expect("lexical search");
    assert!(hits.is_empty(), "secret must not be searchable: {hits:?}");
}

#[tokio::test]
async fn commit_documents_stay_off_the_lexical_route() {
    let repo = history_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // The marker exists in current source AND in the commit-diff body.
    // Current-source hits may surface it; commit documents must only ever
    // arrive through the history route — never as `lexical` hits whose
    // per-file evidence crowds out real files.
    let result = engine
        .search(search_request("second_lineage_marker", true))
        .await
        .expect("search");
    assert!(
        result.hits.iter().all(|hit| {
            !(hit.route == cce_core::SearchRoute::Lexical
                && matches!(
                    hit.representation,
                    RetrievalRepresentation::CommitSummary | RetrievalRepresentation::CommitDiff
                ))
        }),
        "commit document reached the lexical route: {:?}",
        result
            .hits
            .iter()
            .map(|hit| (hit.route, hit.representation.clone()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn type_diff_greps_stored_commit_patches() {
    let repo = history_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // `compute_second` exists in current source AND in the stored patch of
    // the second commit; the diff route must return it as a commit hit.
    let mut request = search_request("type:diff compute_second", true);
    let result = engine.search(request.clone()).await.expect("type:diff");
    assert!(
        result.hits.iter().any(|hit| {
            hit.route == cce_core::SearchRoute::Diff
                && hit.representation == RetrievalRepresentation::CommitDiff
                && hit
                    .address
                    .as_ref()
                    .is_some_and(|address| address.path == "src/lib.rs")
                && hit.snippet.contains("+    compute_second()")
        }),
        "type:diff should surface the src/lib.rs hunk, got {:?}",
        result
            .hits
            .iter()
            .map(|hit| (hit.route, hit.snippet.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .missing_capabilities
            .iter()
            .any(|message| message.contains("diff search scans stored commit patches")),
        "coverage bound must be reported"
    );

    // The same scan through the explicit route override.
    request = search_request("compute_first", true);
    request.routes = vec![cce_core::SearchRoute::Diff];
    let routed = engine.search(request).await.expect("route diff");
    assert!(
        routed
            .hits
            .iter()
            .any(|hit| hit.route == cce_core::SearchRoute::Diff),
        "--route diff must reach the patch scanner"
    );

    // `type:commit` restricts retrieval to history documents: the marker
    // function name lives in the second commit's message.
    let commits = engine
        .search(search_request("type:commit second_lineage_marker", true))
        .await
        .expect("type:commit");
    assert!(
        commits
            .hits
            .iter()
            .all(|hit| hit.route == cce_core::SearchRoute::History),
        "type:commit leaked a non-history route: {:?}",
        commits.hits.iter().map(|hit| hit.route).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn ignored_paths_leave_no_history_evidence() {
    let repo = history_repo();
    write(repo.path(), ".cceignore", "ignored.log\n");
    write(repo.path(), "ignored.log", "lineage_secret_marker\n");
    write(
        repo.path(),
        "src/extra.rs",
        "pub fn third_visible_marker() {}\n",
    );
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-m", "add ignored log and extra"]);
    let engine = engine(repo.path());
    let report = engine.index().await.expect("index");

    let diffs = commit_diff_documents(&engine, &report.snapshot.id);
    let latest = diffs
        .iter()
        .find(|document| document.text.contains("third_visible_marker"))
        .expect("commit diff for the third commit");
    // The path may be noted, but an ignored file contributes neither
    // evidence rows nor diff content.
    assert!(
        latest
            .evidence
            .iter()
            .all(|address| address.path != "ignored.log"),
        "ignored path leaked into evidence: {:?}",
        latest.evidence
    );
    assert!(!latest.text.contains("lineage_secret_marker"));
    assert!(!latest.text.contains("diff --git a/ignored.log"));
}

#[tokio::test]
async fn history_indexing_is_idempotent() {
    let repo = history_repo();
    let engine = engine(repo.path());
    let first = engine.index().await.expect("first index");
    let first_ids = commit_diff_documents(&engine, &first.snapshot.id)
        .iter()
        .map(|document| document.document_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(first_ids.len(), 1);

    let second = engine.index().await.expect("second index");
    assert!(second.reused_snapshot);
    assert_eq!(second.snapshot.id, first.snapshot.id);

    // A worktree change forces a fresh snapshot; identical history must
    // reproduce identical document ids rather than duplicating rows.
    write(repo.path(), "README.md", "# changed\n");
    let third = engine.index().await.expect("third index");
    assert_ne!(third.snapshot.id, first.snapshot.id);
    let third_ids = commit_diff_documents(&engine, &third.snapshot.id)
        .iter()
        .map(|document| document.document_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(third_ids, first_ids);
}

#[tokio::test]
async fn interrupted_post_commit_views_are_repaired() {
    let repo = history_repo();
    let engine = engine(repo.path());
    let report = engine.index().await.expect("index");
    let repository_id = report.repository_id.clone();
    let snapshot_id = report.snapshot.id.clone();

    // Simulate a process killed between `commit_snapshot` and the
    // post-commit status writes: records committed, statuses stuck.
    let stuck_kinds = [
        ViewKind::Symbols,
        ViewKind::Graph,
        ViewKind::Knowledge,
        ViewKind::History,
        ViewKind::Dataflow,
    ];
    let manifest = engine
        .store()
        .view_manifest(&repository_id, &snapshot_id)
        .expect("manifest");
    for kind in stuck_kinds {
        let mut view = manifest.views[&kind].clone();
        view.state = ViewState::Building;
        engine
            .store()
            .set_view_status(&repository_id, &snapshot_id, kind, &view)
            .expect("corrupt view");
    }

    let second = engine.index().await.expect("repairing index");
    assert!(second.reused_snapshot);
    let repaired = second.manifest;
    assert_eq!(repaired.views[&ViewKind::Symbols].state, ViewState::Ready);
    assert_eq!(repaired.views[&ViewKind::Graph].state, ViewState::Partial);
    assert_eq!(
        repaired.views[&ViewKind::Knowledge].state,
        ViewState::Partial
    );
    assert_eq!(repaired.views[&ViewKind::History].state, ViewState::Ready);
    assert_eq!(
        repaired.views[&ViewKind::Dataflow].state,
        ViewState::Unavailable
    );
    // Repaired statuses report recomputed evidence, not just a flipped flag.
    let history = &repaired.views[&ViewKind::History];
    assert!(
        history
            .capabilities
            .iter()
            .any(|capability| capability.name == "git_diff_hunks"),
        "repaired history lost its capabilities: {:?}",
        history.capabilities
    );

    // Repair runs once per snapshot per process: a view corrupted after the
    // pass is reported as-is instead of re-triggering the repair pipeline.
    let mut view = repaired.views[&ViewKind::History].clone();
    view.state = ViewState::Building;
    engine
        .store()
        .set_view_status(&repository_id, &snapshot_id, ViewKind::History, &view)
        .expect("corrupt again");
    let third = engine.index().await.expect("third index");
    assert_eq!(
        third.manifest.views[&ViewKind::History].state,
        ViewState::Building,
        "repair must not re-run within the same process"
    );
}
