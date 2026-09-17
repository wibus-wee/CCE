#![forbid(unsafe_code)]

//! End-to-end coverage for the engine lifecycle: index → status → stale
//! detection → retrieval → context → GC. Every test builds its own
//! temporary repository so nothing outside the tempdir is touched.

use std::fs;
use std::path::Path;

use cce_core::{SearchRequest, ViewKind, ViewState};
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
    let impact = engine
        .impact_analysis("beta_fn")
        .await
        .expect("impact analysis");
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
    let impact = engine
        .impact_analysis("helper")
        .await
        .expect("impact on helper");
    assert!(
        impact
            .impacted
            .iter()
            .any(|entity| entity.name == "test_helper_works" && entity.via == "Tests")
    );

    // References edge: alpha::verify names beta::Token in a type position.
    let impact = engine
        .impact_analysis("Token")
        .await
        .expect("impact on Token");
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

    let map = engine.codebase_map().await.expect("codebase map");
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

    let explanation = engine
        .explain_component("alpha")
        .await
        .expect("explain alpha");
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

#[tokio::test]
async fn atlas_on_unindexed_repository_is_an_explicit_error() {
    let repo = fixture_repo();
    let engine = engine(repo.path());

    for outcome in [
        engine.codebase_map().await.map(|_| ()),
        engine.explain_component("anything").await.map(|_| ()),
        engine.impact_analysis("anything").await.map(|_| ()),
    ] {
        let error = outcome.expect_err("atlas must fail without an index");
        assert!(
            matches!(error, cce_core::CceError::ViewUnavailable { .. }),
            "expected ViewUnavailable, got {error:?}"
        );
    }
}
