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

/// A→B→A: restoring byte-identical content must reuse the old snapshot AND
/// move `current` back to it — otherwise status, atlas and non-fresh search
/// keep resolving B while `index()` reported A.
#[tokio::test]
async fn reactivates_prior_snapshot() {
    let repo = fixture_repo();
    let engine = engine(repo.path());

    let report_a = engine.index().await.expect("index A");
    let snap_a = report_a.snapshot.id.clone();

    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    !cursor.is_empty() && cursor.len() > 4\n}\n",
    );
    let report_b = engine.index().await.expect("index B");
    assert_ne!(report_b.snapshot.id, snap_a);

    // Restore A byte-for-byte.
    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    cursor.is_empty()\n}\n",
    );
    let report_a2 = engine.index().await.expect("index A again");
    assert!(report_a2.reused_snapshot, "A must be reused, not rebuilt");
    assert_eq!(report_a2.snapshot.id, snap_a);

    // Every snapshot resolution now agrees on A.
    assert_eq!(
        engine
            .store()
            .current_snapshot(&report_a.repository_id)
            .expect("current"),
        Some(snap_a.clone()),
        "store current must move back to A"
    );
    let manifest = engine.status().expect("status");
    assert_eq!(manifest.snapshot_id, snap_a, "status must describe A");
    let map = engine.codebase_map().expect("codebase map");
    assert_eq!(map.snapshot_id, snap_a, "atlas must resolve A");
    let result = engine
        .search(search_request("resume_attempt", false))
        .await
        .expect("non-fresh search");
    assert_eq!(
        result.request.snapshot_id, snap_a,
        "non-fresh search must serve A"
    );
    // Serving a committed-but-unscanned snapshot stays explicitly
    // unverified — activation does not claim freshness.
    assert!(result.hits.iter().all(|hit| !hit.verified_current));
}

/// A checkpoint commits a parse+relations snapshot under a distinct id
/// without moving `current`, and `architecture_diff` can read it.
#[tokio::test]
async fn checkpoint_detaches_and_stays_diffable() {
    let repo = fixture_repo();
    let engine = engine(repo.path());

    let full = engine.index().await.expect("full index");
    let snap_full = full.snapshot.id.clone();

    // Same worktree: a checkpoint mints its own id (distinct profile) —
    // never the full snapshot's.
    let same = engine
        .checkpoint(Some("test".to_owned()))
        .await
        .expect("checkpoint of unchanged tree");
    assert_ne!(same.snapshot.id, snap_full);
    assert_eq!(same.snapshot.origin.as_deref(), Some("test"));

    // Edit, checkpoint again: current stays on the full snapshot while
    // the checkpoint captures the worktree delta.
    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    !cursor.is_empty()\n}\n",
    );
    let moved = engine.checkpoint(None).await.expect("checkpoint of edit");
    assert_ne!(moved.snapshot.id, snap_full);
    assert_ne!(moved.snapshot.id, same.snapshot.id);
    assert_eq!(
        engine
            .store()
            .current_snapshot(&full.repository_id)
            .expect("current"),
        Some(snap_full.clone()),
        "checkpoint must not move current"
    );

    // Diffable: full → checkpoint shows the changed function. Explicit
    // base — a bare `head` would diff against the previous checkpoint.
    let diff = engine
        .architecture_diff(Some(&snap_full), Some(&moved.snapshot.id))
        .expect("architecture diff");
    assert_eq!(diff.base_snapshot_id, snap_full);
    assert_eq!(diff.head_snapshot_id, moved.snapshot.id);

    // Repeating the same checkpoint reuses it instead of duplicating.
    let repeat = engine.checkpoint(None).await.expect("repeat checkpoint");
    assert!(repeat.reused_snapshot);
    assert_eq!(repeat.snapshot.id, moved.snapshot.id);

    // Skipped views report Unavailable, never Ready-by-omission.
    let dense = &repeat.manifest.views[&ViewKind::Dense].state;
    let zoekt = &repeat.manifest.views[&ViewKind::Zoekt].state;
    assert_eq!(*dense, ViewState::Unavailable);
    assert_eq!(*zoekt, ViewState::Unavailable);
    assert_eq!(
        repeat.manifest.views[&ViewKind::Lexical].state,
        ViewState::Ready
    );
}

/// A call edge into a package that never declares the dependency is a
/// boundary violation; declaring it clears the violation.
#[tokio::test]
async fn map_reports_undeclared_cross_package_edges() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "pkg-a/Cargo.toml",
        "[package]\nname = \"pkg-a\"\n",
    );
    write(
        dir.path(),
        "pkg-a/src/lib.rs",
        "pub fn run() { helper() }\n",
    );
    write(
        dir.path(),
        "pkg-b/Cargo.toml",
        "[package]\nname = \"pkg-b\"\n",
    );
    write(dir.path(), "pkg-b/src/lib.rs", "pub fn helper() {}\n");
    let engine = engine(dir.path());
    engine.index().await.expect("index");

    let map = engine.codebase_map().expect("map");
    assert_eq!(map.packages.len(), 2);
    assert!(map.violations.iter().any(|v| {
        v.source_package == "pkg-a" && v.target_package == "pkg-b" && v.kind == "calls"
    }));
    assert_eq!(map.violation_count, map.violations.len());

    // Declaring the dependency clears the violation.
    write(
        dir.path(),
        "pkg-a/Cargo.toml",
        "[package]\nname = \"pkg-a\"\n\n[dependencies]\npkg-b = { path = \"../pkg-b\" }\n",
    );
    engine.index().await.expect("reindex");
    let map = engine.codebase_map().expect("map");
    assert!(map.violations.is_empty(), "{:?}", map.violations);
    assert_eq!(map.violation_count, 0);
}

/// When activation requires the index lease and another writer holds it,
/// `index()` must fail with `IndexBusy` — never report a successful reuse while
/// leaving `current` on the stale snapshot.
#[tokio::test]
async fn activation_conflict_reports_busy_not_success() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index A");

    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    !cursor.is_empty()\n}\n",
    );
    let report_b = engine.index().await.expect("index B");

    // Restore A — the next index() would need to reactivate it.
    write(
        repo.path(),
        "src/lib.rs",
        "pub fn resume_attempt(cursor: &str) -> bool {\n    cursor.is_empty()\n}\n",
    );

    // A competing writer holds the lease file the engine uses.
    let lock_path = engine.config().data_root.join("index.lock");
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open index.lock");
    fs4::FileExt::try_lock(&lock_file).expect("hold lease");

    let error = engine
        .index()
        .await
        .expect_err("index must fail while busy");
    assert!(
        matches!(error, cce_core::CceError::IndexBusy(_)),
        "expected IndexBusy, got {error:?}"
    );
    drop(lock_file);

    // The failed activation did not move current off B.
    assert_eq!(
        engine
            .store()
            .current_snapshot(&report_b.repository_id)
            .expect("current"),
        Some(report_b.snapshot.id.clone())
    );
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

/// The dense path must embed the restored descriptor text — the document's
/// `address` is provenance pointing at source, never the body location.
/// A recording embedder asserts on the actual strings handed in, not on
/// representation labels.
#[tokio::test]
async fn dense_embeds_descriptor_text_not_source() {
    use cce_engine::{EmbedRole, Embedder};
    use std::sync::Mutex;

    struct Recording {
        inputs: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Embedder for Recording {
        fn profile(&self) -> &'static str {
            "recording-test-embedder"
        }
        fn production_ready(&self) -> bool {
            false
        }
        async fn embed(
            &self,
            inputs: &[String],
            _role: EmbedRole,
        ) -> cce_core::Result<Vec<Vec<f32>>> {
            self.inputs
                .lock()
                .expect("inputs lock")
                .extend(inputs.iter().cloned());
            Ok(inputs.iter().map(|_| vec![1.0_f32; 32]).collect())
        }
    }

    let repo = fixture_repo();
    let engine = engine(repo.path());
    let report = engine.index().await.expect("index");

    let documents = engine
        .store()
        .documents_for_snapshot(&report.snapshot.id)
        .expect("documents");
    let recorder = Recording {
        inputs: Mutex::new(Vec::new()),
    };
    cce_engine::DenseIndex::build(&documents, &recorder, 64, None)
        .await
        .expect("dense build");
    let embedded = recorder.inputs.lock().expect("inputs lock").clone();

    let summary = documents
        .iter()
        .find(|doc| doc.representation == RetrievalRepresentation::SymbolSummary)
        .expect("symbol summary document");
    // Restored text is the descriptor: signature surfaced, body absent.
    assert!(
        summary.text.contains("function ") && summary.text.contains(" in src/lib.rs"),
        "descriptor-shaped text expected, got {:?}",
        summary.text
    );
    assert!(
        !summary.text.contains("cursor.is_empty()"),
        "descriptor must not restore the source body: {:?}",
        summary.text
    );
    // And the embedder received that exact string.
    assert!(
        embedded.iter().any(|input| input == &summary.text),
        "embedder must receive the descriptor verbatim"
    );

    let file_descriptor = documents
        .iter()
        .find(|doc| doc.representation == RetrievalRepresentation::FileDescriptor)
        .expect("file descriptor document");
    assert!(
        !file_descriptor.text.contains("cursor.is_empty()"),
        "file descriptor must be bounded, got the whole file: {:?}",
        file_descriptor.text
    );
    assert!(
        embedded.iter().any(|input| input == &file_descriptor.text),
        "embedder must receive the file descriptor verbatim"
    );
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

/// Ambiguity-aware confidence: `shared_name` is defined in two files, so a
/// call spelling it resolves by guessing among candidates — the edge must
/// be demoted below a uniquely-resolved call and carry the marker; the
/// uniquely-named `unique_helper` call keeps full confidence.
#[tokio::test]
async fn ambiguous_call_targets_are_demoted() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/left.rs",
        "pub fn shared_name() -> u32 {\n    1\n}\n",
    );
    write(
        dir.path(),
        "src/right.rs",
        "pub fn shared_name() -> u32 {\n    2\n}\n",
    );
    write(
        dir.path(),
        "src/helper.rs",
        "pub fn unique_helper() -> u32 {\n    3\n}\n",
    );
    write(
        dir.path(),
        "src/caller.rs",
        "pub fn invoke() -> u32 {\n    shared_name() + unique_helper()\n}\n",
    );
    let engine = engine(dir.path());
    let report = engine.index().await.expect("index");

    let calls = engine
        .store()
        .relations_by_kind(&report.snapshot.id, &cce_core::RelationKind::Calls, 64)
        .expect("calls relations");
    let edge_to = |name: &str| {
        calls
            .iter()
            .find(|relation| {
                relation
                    .attributes
                    .get("calleeName")
                    .and_then(|value| value.as_str())
                    == Some(name)
            })
            .expect("Calls edge for fixture callee")
    };
    let ambiguous = edge_to("shared_name");
    let unique = edge_to("unique_helper");

    assert!(
        ambiguous.confidence < unique.confidence,
        "ambiguous call must be demoted below the unique call: {} vs {}",
        ambiguous.confidence,
        unique.confidence
    );
    assert_eq!(
        ambiguous
            .attributes
            .get("ambiguous")
            .and_then(serde_json::value::Value::as_bool),
        Some(true),
        "the guessed edge must carry the ambiguity marker"
    );
    assert!(
        unique
            .attributes
            .get("ambiguous")
            .and_then(serde_json::value::Value::as_bool)
            .is_none_or(|flag| !flag),
        "the uniquely-resolved edge must not be marked ambiguous"
    );
}

/// Call edges dedup on (caller, callee), not callee alone: two callers in
/// the same file invoking one helper are two edges, while a repeated call
/// from the same caller is still one.
#[tokio::test]
async fn graph_distinct_callers_survive_dedup() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/helper.rs",
        "pub fn helper() -> u32 {\n    1\n}\n",
    );
    write(
        dir.path(),
        "src/callers.rs",
        "pub fn a() -> u32 {\n    helper() + helper()\n}\n\npub fn b() -> u32 {\n    helper()\n}\n",
    );
    let engine = engine(dir.path());
    let report = engine.index().await.expect("index");

    let calls = engine
        .store()
        .relations_by_kind(&report.snapshot.id, &cce_core::RelationKind::Calls, 64)
        .expect("calls relations");
    let to_helper: Vec<_> = calls
        .iter()
        .filter(|relation| {
            relation
                .attributes
                .get("calleeName")
                .and_then(|value| value.as_str())
                == Some("helper")
        })
        .collect();
    let sources: std::collections::BTreeSet<_> = to_helper
        .iter()
        .map(|r| r.source_entity_id.clone())
        .collect();
    assert_eq!(
        sources.len(),
        2,
        "a→helper and b→helper are two edges; repeated a→helper stays one: {:?}",
        calls
            .iter()
            .map(|relation| (
                relation.source_entity_id.clone(),
                relation.target_entity_id.clone()
            ))
            .collect::<Vec<_>>()
    );

    // Both callers surface through impact analysis.
    let impact = engine.impact_analysis("helper").expect("impact on helper");
    for caller in ["a", "b"] {
        assert!(
            impact
                .impacted
                .iter()
                .any(|entity| entity.name == caller && entity.via == "Calls" && entity.hops == 1),
            "expected {caller} among helper's callers: {:?}",
            impact
                .impacted
                .iter()
                .map(|entity| (entity.name.clone(), entity.via.clone()))
                .collect::<Vec<_>>()
        );
    }
}

/// Type-reference edges follow the same (source, target) identity: two
/// units in one file naming the same type produce two edges, not one.
#[tokio::test]
async fn type_reference_edges_dedup_per_source_target_pair() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/types.rs",
        "pub struct Token {\n    pub id: u32,\n}\n",
    );
    write(
        dir.path(),
        "src/users.rs",
        "pub fn make() -> Token {\n    Token { id: 1 }\n}\n\npub fn check(t: &Token) -> bool {\n    t.id > 0\n}\n",
    );
    let engine = engine(dir.path());
    let report = engine.index().await.expect("index");

    let references = engine
        .store()
        .relations_by_kind(&report.snapshot.id, &cce_core::RelationKind::References, 64)
        .expect("references relations");
    let to_token: Vec<_> = references
        .iter()
        .filter(|relation| {
            relation
                .attributes
                .get("typeName")
                .and_then(|value| value.as_str())
                == Some("Token")
                && relation.origin == cce_core::RelationOrigin::TreeSitter
        })
        .collect();
    let sources: std::collections::BTreeSet<_> = to_token
        .iter()
        .map(|r| r.source_entity_id.clone())
        .collect();
    assert_eq!(
        sources.len(),
        2,
        "make→Token and check→Token are two edges: {:?}",
        references
            .iter()
            .map(|relation| {
                (
                    relation.source_entity_id.clone(),
                    relation.target_entity_id.clone(),
                )
            })
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
    assert!(map.provenance.starts_with("build_system manifests"));

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

/// The context pack carries the search verdict verbatim plus a delivery
/// report naming what shipped and what was cut — a consumer must not have
/// to re-derive delivery coverage from the item list.
#[tokio::test]
async fn context_pack_preserves_verdict_and_delivery_gaps() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    let pack = engine
        .context(ContextRequest::new("resume_attempt", 4_096))
        .await
        .expect("context");

    let verdict = pack.search_verdict.as_ref().expect("searchVerdict present");
    assert_ne!(
        verdict.state,
        cce_core::VerdictState::Abstained,
        "a real query on the fixture repo must produce evidence"
    );
    let report = pack
        .delivery_report
        .as_ref()
        .expect("deliveryReport present");
    // Every shipped evidence item is named; orientation is furniture, not
    // an included evidence id.
    assert!(
        report
            .included_item_ids
            .iter()
            .all(|id| id != "orientation"),
        "orientation must not be listed as delivered evidence"
    );
    let shipped: std::collections::HashSet<&str> = pack
        .items
        .iter()
        .map(|item| item.id.as_str())
        .filter(|id| *id != "orientation")
        .collect();
    assert_eq!(
        report.included_item_ids.len(),
        shipped.len(),
        "included ids must equal the shipped evidence items"
    );
    // Wire shape: snake_case omission reasons, camelCase fields.
    let json = serde_json::to_value(&pack).expect("serialize");
    assert!(json.get("searchVerdict").is_some());
    assert!(json.get("deliveryReport").is_some());
    // Total latency covers the whole call — at minimum the search stage.
    let search_ms = pack.search_latency_ms.expect("searchLatencyMs");
    assert!(
        pack.latency_ms >= search_ms,
        "context latency {} must include the {}ms search stage",
        pack.latency_ms,
        search_ms
    );
}

/// A tiny budget forces omissions: the report must name the cut hits with
/// reasons rather than silently dropping them.
#[tokio::test]
async fn context_pack_reports_budget_omissions() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // Generous search first to learn what retrieval finds, then a pack
    // budget that cannot fit it all.
    let full = engine
        .context(ContextRequest::new("resume_attempt", 65_536))
        .await
        .expect("full context");
    let tiny = engine
        .context(ContextRequest::new("resume_attempt", 600))
        .await
        .expect("tiny context");
    let report = tiny
        .delivery_report
        .as_ref()
        .expect("deliveryReport present");
    if report.omitted_hits.is_empty() {
        // The fixture repo may genuinely fit under 600 tokens — then the
        // contract is simply that included ids match shipped items.
        assert_eq!(report.included_item_ids.len() + 1, tiny.items.len());
        return;
    }
    assert!(
        report.omitted_hits.iter().all(|omitted| matches!(
            omitted.reason,
            cce_core::OmissionReason::Budget
                | cce_core::OmissionReason::DuplicateRange
                | cce_core::OmissionReason::FileCap
                | cce_core::OmissionReason::DuplicateEntity
        )),
        "every omission carries a typed reason: {:?}",
        report.omitted_hits
    );
    // Nothing omitted also shipped; nothing shipped is reported omitted.
    let shipped: std::collections::HashSet<&str> = report
        .included_item_ids
        .iter()
        .map(String::as_str)
        .collect();
    assert!(
        report
            .omitted_hits
            .iter()
            .all(|omitted| !shipped.contains(omitted.document_id.as_str())),
        "omitted hits must not overlap shipped items"
    );
    // The fuller pack delivered at least as much evidence as the tiny one.
    let full_report = full.delivery_report.as_ref().expect("full report");
    assert!(
        full_report.included_item_ids.len() >= report.included_item_ids.len(),
        "larger budget must not deliver less evidence"
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
                pattern: None,
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

/// Temporal claims ask whether the subject *ever* existed: "never defined
/// now" cannot refute "defined then", so neither the literal veto nor the
/// definition-witness gate may preempt the patch/history routes.
/// `compute_first` is gone from current source — only the first commit's
/// patch remembers it — and `compute_second` is a call target that was
/// never defined anywhere; both must still answer through `type:diff`.
#[tokio::test]
async fn temporal_pins_bypass_current_source_evidence_gates() {
    let repo = history_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // History-only term: gone from current source, alive in patch history.
    let removed = engine
        .search(search_request("type:diff compute_first", true))
        .await
        .expect("type:diff on history-only term");
    assert!(
        removed.hits.iter().any(|hit| {
            hit.route == cce_core::SearchRoute::Diff && hit.snippet.contains("compute_first()")
        }),
        "a term living only in patch history must still answer: {:?}",
        removed.missing_capabilities
    );

    // Never-defined call target: `compute_second` is *called* in current
    // source but no entity bears the name — the definition gate would
    // abstain, yet the diff route answers where it entered history.
    let undefined = engine
        .search(search_request("type:diff compute_second", true))
        .await
        .expect("type:diff on never-defined callee");
    assert!(
        undefined.hits.iter().any(|hit| {
            hit.route == cce_core::SearchRoute::Diff && hit.snippet.contains("compute_second()")
        }),
        "a never-defined callee must still answer through history: {:?}",
        undefined.missing_capabilities
    );

    // The exemption is scoped, not a hole: a `type:diff` term that exists
    // nowhere — not even in patches — still abstains.
    let absent = engine
        .search(search_request("type:diff never_existed_marker_zzz", true))
        .await
        .expect("type:diff on absent term");
    assert!(
        absent.hits.is_empty(),
        "a term absent from all corpora must still abstain: {:?}",
        absent
            .hits
            .iter()
            .map(|hit| hit.document_id.clone())
            .collect::<Vec<_>>()
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

// --- pseudo-relevance feedback ----------------------------------------------

/// PRF fixture: `anchor.rs` answers the query; `harbor.rs` shares its
/// vocabulary ("position", "dock") but never the query term itself, so
/// only the feedback expansion can reach it.
fn prf_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/anchor.rs",
        "pub fn anchor_position(dock: &str) -> bool {\n    dock.is_empty()\n}\n",
    );
    write(
        dir.path(),
        "src/harbor.rs",
        "pub fn harbor_position(dock: &str) -> bool {\n    dock.len() > 1\n}\n",
    );
    write(dir.path(), "README.md", "# fixture repository\n");
    dir
}

fn has_prf_explanation(result: &cce_engine::SearchResult) -> bool {
    result.hits.iter().any(|hit| {
        hit.explanation
            .iter()
            .any(|line| line.contains("PRF expansion"))
    })
}

#[tokio::test]
async fn prf_second_pass_surfaces_expansion_only_hit() {
    let repo = prf_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // "anchor" matches src/anchor.rs lexically; src/harbor.rs shares the
    // head vocabulary (position, dock) without the query term, so the
    // expanded second pass is the only channel that can surface it.
    let result = engine
        .search(search_request("anchor", true))
        .await
        .expect("search");
    let harbor = result
        .hits
        .iter()
        .find(|hit| hit.symbol_name.as_deref() == Some("harbor_position"))
        .expect("harbor_position must surface through the feedback pass");
    assert!(
        harbor
            .explanation
            .iter()
            .any(|line| line.contains("PRF expansion: +")),
        "second-pass hit must carry the expansion explanation: {:?}",
        harbor.explanation
    );
    // Attenuated entry: expansion-only evidence stays below the head of
    // the first-pass ranking rather than leapfrogging it.
    let top = result.hits.first().expect("non-empty hits");
    assert!(
        harbor.score < top.score,
        "PRF-only hit {:?} must not outscore the fused head {:?}",
        harbor.score,
        top.score
    );
}

#[tokio::test]
async fn prf_skips_exact_entity_and_route_overrides() {
    let repo = prf_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // Identifier-shaped query infers ExactEntity: expansion risks
    // precision on a lookup that already names its target.
    let exact = engine
        .search(search_request("anchor_position", true))
        .await
        .expect("exact entity search");
    assert!(!exact.hits.is_empty());
    assert!(
        !has_prf_explanation(&exact),
        "exact-entity query must not run the feedback pass: {:?}",
        exact
            .hits
            .iter()
            .map(|hit| hit.explanation.clone())
            .collect::<Vec<_>>()
    );

    // An explicit route override without Lexical opted out of lexical
    // retrieval; the pass must not smuggle it back in. Intent is supplied
    // so the ExactEntity guard is not the one firing here.
    let mut request = search_request("anchor_position", true);
    request.intent = Some(cce_core::QueryIntent::NaturalLanguageBehavior);
    request.routes = vec![cce_core::SearchRoute::ExactSymbol];
    let routed = engine.search(request).await.expect("route override");
    assert!(!routed.hits.is_empty());
    assert!(
        !has_prf_explanation(&routed),
        "routes without Lexical must not run the feedback pass: {:?}",
        routed
            .hits
            .iter()
            .map(|hit| hit.explanation.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn prf_skips_type_pinned_queries() {
    let repo = history_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // `type:commit` repoints the plan at the history route alone; history
    // hits exist but no feedback pass ran.
    let result = engine
        .search(search_request("type:commit second_lineage_marker", true))
        .await
        .expect("type:commit");
    assert!(!result.hits.is_empty());
    assert!(
        !has_prf_explanation(&result),
        "type:commit must not run the feedback pass"
    );
}

/// Build an engine with the deterministic dense baseline — offline and
/// deterministic, so hit scores are reproducible across runs.
fn dense_engine(dir: &Path) -> CceEngine {
    let mut config = EngineConfig::for_repository(dir);
    config.dense = cce_engine::DenseBackendConfig::DeterministicBaseline { dimensions: 64 };
    CceEngine::open(config).expect("open dense engine")
}

/// A request routed at the dense passes only.
fn dense_request(query: &str, limit: usize) -> SearchRequest {
    let mut request = search_request(query, true);
    request.routes = vec![
        cce_core::SearchRoute::DenseRaw,
        cce_core::SearchRoute::DenseSummary,
    ];
    request.limit = limit;
    request
}

/// Enough documents that `limit * 3` dense hits are a strict subset —
/// the corpus-vs-hitset distinction the materialization fix must prove.
fn dense_fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..14 {
        write(
            dir.path(),
            &format!("src/module_{index}.rs"),
            &format!(
                "pub fn cursor_helper_{index}(cursor: &str) -> bool {{\n    cursor.len() > {index}\n}}\n\npub fn unrelated_{index}() {{}}\n"
            ),
        );
    }
    dir
}

/// Dense search must materialize only the hit documents — never the
/// whole snapshot — and batch entity names through one lookup.
#[tokio::test]
async fn dense_query_materializes_only_hit_documents() {
    let repo = dense_fixture_repo();
    let engine = dense_engine(repo.path());
    let report = engine.index().await.expect("index");
    let total_docs = engine
        .store()
        .documents_for_snapshot(&report.snapshot.id)
        .expect("documents")
        .len();
    assert!(total_docs > 6, "fixture must exceed the hit budget");

    let counters_before = engine.retrieval_counters().snapshot();
    let reads_before = engine.store().artifacts().read_count();
    // Union plan: lexical supplies strict-tier evidence while the dense
    // pass runs in the same query — a dense-only plan abstains by design
    // (dense hits are inferred vicinity, never literal-term evidence).
    let mut request = search_request("cursor helper", true);
    request.limit = 2;
    let result = engine.search(request).await.expect("dense search");
    assert!(!result.hits.is_empty());
    let counters = engine.retrieval_counters().snapshot();
    let restored = counters.dense_documents_restored - counters_before.dense_documents_restored;
    assert!(restored > 0, "dense hits must materialize their documents");
    assert!(
        restored <= 2 * 3,
        "restored documents must not exceed the limit*3 hit set, got {restored}"
    );
    assert!(
        restored < total_docs,
        "restored {restored} == corpus {total_docs}: full-snapshot materialization regressed"
    );
    // Entity names must flow through the batched lookup.
    assert!(
        result.hits.iter().any(|hit| hit.symbol_name.is_some()),
        "batched entity lookup must populate symbol_name"
    );
    // Each distinct body artifact is read at most once; plus one read for
    // the vector index blob on this cold query.
    let reads = engine.store().artifacts().read_count() - reads_before;
    assert!(
        reads <= restored + 1,
        "artifact reads {reads} exceed one per hit document + index blob ({restored} + 1)"
    );
}

/// Warm queries must reuse the decoded index — no second artifact read,
/// no second decode.
#[tokio::test]
async fn dense_index_decodes_once_across_queries() {
    let repo = dense_fixture_repo();
    let engine = dense_engine(repo.path());
    engine.index().await.expect("index");

    engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("cold search");
    let cold = engine.retrieval_counters().snapshot();
    assert_eq!(cold.dense_index_decodes, 1);
    assert_eq!(cold.dense_artifact_reads, 1);

    engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("warm search");
    engine
        .search(dense_request("cursor persistence", 3))
        .await
        .expect("third search");
    let warm = engine.retrieval_counters().snapshot();
    assert_eq!(
        warm.dense_index_decodes, 1,
        "warm queries must not re-decode"
    );
    assert_eq!(
        warm.dense_artifact_reads, 1,
        "warm queries must not re-read the vector artifact"
    );
}

/// Concurrent first queries share a single decode — the per-key
/// `OnceCell` serializes initialization instead of letting N callers
/// each decode.
#[tokio::test]
async fn dense_index_decodes_once_under_concurrency() {
    let repo = dense_fixture_repo();
    let engine = std::sync::Arc::new(dense_engine(repo.path()));
    engine.index().await.expect("index");

    let mut tasks = Vec::new();
    for _ in 0..6 {
        let engine = engine.clone();
        tasks.push(tokio::spawn(async move {
            engine
                .search(dense_request("cursor helper", 3))
                .await
                .expect("concurrent search")
        }));
    }
    for task in tasks {
        task.await.expect("join");
    }
    let counters = engine.retrieval_counters().snapshot();
    assert_eq!(
        counters.dense_index_decodes, 1,
        "six concurrent cold queries must share one decode"
    );
    assert_eq!(counters.dense_artifact_reads, 1);
}

/// A failed decode is never published: after repairing the artifact the
/// next query retries and succeeds.
#[tokio::test]
async fn dense_index_failure_is_not_cached() {
    let repo = dense_fixture_repo();
    let engine = dense_engine(repo.path());
    engine.index().await.expect("index");
    let manifest = engine.status().expect("status");
    let digest = manifest.views[&ViewKind::Dense]
        .artifact_digest
        .clone()
        .expect("dense artifact digest");
    let object = repo
        .path()
        .join(".cce")
        .join("artifacts")
        .join("blake3")
        .join(&digest[..2])
        .join(&digest[2..]);
    let bytes = fs::read(&object).expect("read index blob");
    fs::remove_file(&object).expect("remove index blob");

    let failed = engine.search(dense_request("cursor helper", 3)).await;
    assert!(failed.is_err(), "missing index blob must fail, not degrade");
    let after_failure = engine.retrieval_counters().snapshot();
    assert_eq!(after_failure.dense_index_decodes, 0);
    assert_eq!(after_failure.dense_artifact_reads, 1);

    fs::write(&object, &bytes).expect("restore index blob");
    engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("retry after repair must succeed");
    let recovered = engine.retrieval_counters().snapshot();
    assert_eq!(
        recovered.dense_index_decodes, 1,
        "failure must not be cached — retry decodes once"
    );
    assert_eq!(recovered.dense_artifact_reads, 2);
}

/// A→B→A snapshot switches must not mix indexes: each digest decodes
/// once, and reactivated A reuses its cached decode with identical hits.
#[tokio::test]
async fn dense_cache_isolated_across_snapshots() {
    let repo = dense_fixture_repo();
    let engine = dense_engine(repo.path());
    let first = engine.index().await.expect("index A");
    let a_hits = engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("search A")
        .hits;

    write(
        repo.path(),
        "src/extra.rs",
        "pub fn brand_new_file_marker() {}\n",
    );
    let second = engine.index().await.expect("index B");
    assert_ne!(first.snapshot.id, second.snapshot.id);
    engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("search B");
    let after_ab = engine.retrieval_counters().snapshot();
    assert_eq!(after_ab.dense_index_decodes, 2, "A and B each decode once");

    fs::remove_file(repo.path().join("src/extra.rs")).expect("restore A");
    let reactivated = engine.index().await.expect("reactivate A");
    assert_eq!(reactivated.snapshot.id, first.snapshot.id);
    let a_again = engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("search A again")
        .hits;
    let after_aba = engine.retrieval_counters().snapshot();
    assert_eq!(
        after_aba.dense_index_decodes, 2,
        "reactivated A must hit the cache, not re-decode or mix"
    );
    assert_eq!(
        a_hits
            .iter()
            .map(|hit| (hit.document_id.as_str(), hit.score))
            .collect::<Vec<_>>(),
        a_again
            .iter()
            .map(|hit| (hit.document_id.as_str(), hit.score))
            .collect::<Vec<_>>(),
        "A→B→A must serve A's index — identical hits"
    );
}

/// A search with no dense routes never touches the index cache.
#[tokio::test]
async fn search_without_dense_never_touches_index_cache() {
    let repo = fixture_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");
    engine
        .search(search_request("resume_attempt cursor", true))
        .await
        .expect("sparse search");
    let counters = engine.retrieval_counters().snapshot();
    assert_eq!(counters.dense_index_decodes, 0);
    assert_eq!(counters.dense_artifact_reads, 0);
    assert_eq!(counters.dense_documents_restored, 0);
}

/// Replaying the same query must return identical hits — scores, order,
/// snippets, verdict — modulo wall time.
#[tokio::test]
async fn dense_replay_is_deterministic() {
    let repo = dense_fixture_repo();
    let engine = dense_engine(repo.path());
    engine.index().await.expect("index");

    let first = engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("first");
    let second = engine
        .search(dense_request("cursor helper", 3))
        .await
        .expect("second");
    let shape = |result: &cce_engine::SearchResult| {
        (
            result
                .hits
                .iter()
                .map(|hit| {
                    (
                        hit.document_id.clone(),
                        hit.score,
                        hit.snippet.clone(),
                        hit.address.clone(),
                        hit.symbol_name.clone(),
                    )
                })
                .collect::<Vec<_>>(),
            result.verdict.clone(),
        )
    };
    assert_eq!(shape(&first), shape(&second));
}

/// Plan 018 performance contract: document materialization follows the
/// hit set, not the corpus. Two corpora of very different sizes must
/// restore the same number of documents per query; the retained O(Nd)
/// boundary is the flat dot product, which grows with corpus size while
/// document load stays flat.
#[tokio::test]
async fn dense_query_perf_follows_hits_not_corpus() {
    fn corpus(files: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for index in 0..files {
            write(
                dir.path(),
                &format!("src/module_{index}.rs"),
                &format!(
                    "pub fn cursor_helper_{index}(cursor: &str) -> bool {{\n    cursor.len() > {index}\n}}\n\npub fn padding_{index}() {{}}\n"
                ),
            );
        }
        dir
    }
    fn percentile(sorted: &[u64], p: usize) -> u64 {
        sorted
            .get(sorted.len() * p / 100)
            .copied()
            .unwrap_or_default()
    }

    let mut report = serde_json::json!({ "requests": 30, "hit_k": 3 });
    for (label, files) in [("small", 14), ("large", 42)] {
        let repo = corpus(files);
        let engine = dense_engine(repo.path());
        let index = engine.index().await.expect("index");
        let total_docs = engine
            .store()
            .documents_for_snapshot(&index.snapshot.id)
            .expect("documents")
            .len();

        // Cold query, then 29 warm ones.
        let mut latencies = Vec::new();
        for iteration in 0..30 {
            let mut request = dense_request("cursor helper", 3);
            request.query = format!("cursor helper {}", iteration % 4);
            let result = engine.search(request).await.expect("search");
            latencies.push(result.latency_ms);
        }
        latencies.sort_unstable();
        let counters = engine.retrieval_counters().snapshot();
        let artifact_reads = engine.store().artifacts().read_count();
        report[label] = serde_json::json!({
            "files": files,
            "documents": total_docs,
            "latency_p50_ms": percentile(&latencies, 50),
            "latency_p95_ms": percentile(&latencies, 95),
            "dense_index_decodes": counters.dense_index_decodes,
            "dense_artifact_reads": counters.dense_artifact_reads,
            "dense_documents_restored_total": counters.dense_documents_restored,
            "dense_documents_restored_per_query":
                counters.dense_documents_restored as f64 / 30.0,
            "dense_body_artifact_reads_per_query":
                counters.dense_body_artifact_reads as f64 / 30.0,
            "artifact_reads_total": artifact_reads,
            "dense_index_load_ns_total": counters.dense_index_load_ns,
            "dense_embed_ns_total": counters.dense_embed_ns,
            "dense_score_ns_total": counters.dense_score_ns,
            "dense_document_load_ns_total": counters.dense_document_load_ns,
        });
    }
    let small = &report["small"];
    let large = &report["large"];
    assert!(
        large["documents"].as_u64().unwrap() > small["documents"].as_u64().unwrap() * 2,
        "corpora must differ materially"
    );
    // The key invariant: restored documents per query are identical — the
    // hit K is fixed, so materialization cost cannot scale with N.
    assert_eq!(
        small["dense_documents_restored_per_query"], large["dense_documents_restored_per_query"],
        "hit-scoped materialization must not grow with the corpus"
    );
    assert_eq!(small["dense_index_decodes"], 1);
    assert_eq!(large["dense_index_decodes"], 1);
    // The retained O(Nd) boundary: dot-product work grows with corpus.
    assert!(
        large["dense_score_ns_total"].as_u64().unwrap()
            >= small["dense_score_ns_total"].as_u64().unwrap(),
        "flat dot product is the retained O(Nd) stage"
    );
    println!(
        "p018-perf {}",
        serde_json::to_string_pretty(&report).unwrap()
    );
}

/// Multi-language fixture for `pat:` route tests. `STATE.lock().unwrap()`
/// appears in three Rust files and (as `cache.lock().unwrap()`) in a
/// TypeScript file; the Go file never contains the call so a hit there
/// would mean text-matching leaked into the structural route.
fn pattern_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/lib.rs",
        "use std::sync::Mutex;\n\
         static STATE: Mutex<u64> = Mutex::new(0);\n\
         pub fn alpha_tick() {\n    let mut guard = STATE.lock().unwrap();\n    *guard += 1;\n}\n",
    );
    write(
        dir.path(),
        "src/helper.rs",
        "// zephyr marker\n\
         pub fn helper_touch() {\n    let _value = CACHE.lock().unwrap();\n}\n",
    );
    write(
        dir.path(),
        "tools/util.rs",
        "pub fn util_bump() {\n    let _g = COUNTER.lock().unwrap();\n}\n",
    );
    write(
        dir.path(),
        "web/app.ts",
        "export function touch(): void {\n    const v = cache.lock().unwrap();\n}\n",
    );
    write(
        dir.path(),
        "cmd/main.go",
        "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n",
    );
    dir
}

fn hit_paths(result: &cce_engine::SearchResult) -> Vec<String> {
    let mut paths: Vec<String> = result
        .hits
        .iter()
        .filter_map(|hit| hit.address.as_ref().map(|a| a.path.clone()))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

#[tokio::test]
async fn pattern_route_maps_matches_to_regions() {
    let repo = pattern_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    let result = engine
        .search(search_request("pat:'$S.lock().unwrap()'", true))
        .await
        .expect("search");

    // `pat:` pins the plan to the structural route and the template leaves
    // the query text — FTS never sees `$S`, so a nonempty hit set also
    // proves the template was not fed through as free text.
    assert_eq!(result.plan.routes, vec![cce_core::SearchRoute::Pattern]);
    assert!(
        result
            .plan
            .reasons
            .iter()
            .any(|reason| reason.contains("pat:")),
        "pin reason recorded"
    );
    assert!(result.request.query.is_empty(), "template stripped");
    assert!(
        result
            .missing_capabilities
            .iter()
            .all(|cap| !cap.contains("pat:")),
        "supported languages must not report template failures: {:?}",
        result.missing_capabilities
    );

    let paths = hit_paths(&result);
    assert_eq!(
        paths,
        vec![
            "src/helper.rs".to_owned(),
            "src/lib.rs".to_owned(),
            "tools/util.rs".to_owned(),
            "web/app.ts".to_owned(),
        ],
        "exactly the files containing a structural match"
    );
    for hit in &result.hits {
        assert_eq!(hit.route, cce_core::SearchRoute::Pattern);
        assert_eq!(hit.contributing_routes, vec![cce_core::SearchRoute::Pattern]);
        assert!(hit.verified_current);
        let address = hit
            .address
            .as_ref()
            .unwrap_or_else(|| panic!("hit address: {hit:?}"));
        assert_eq!(address.snapshot_id, result.request.snapshot_id);
        assert!(
            hit.explanation
                .iter()
                .any(|line| line.starts_with("pat: structural match")),
            "provenance explanation present"
        );
    }
    // Matches inside functions must resolve to their enclosing regions —
    // at least one hit carries a canonical region pointer.
    assert!(result.hits.iter().any(|hit| hit.region_id.is_some()));
}

#[tokio::test]
async fn pattern_route_reports_unsupported_language() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn tick() {\n    let _g = STATE.lock().unwrap();\n}\n",
    );
    write(dir.path(), "config.toml", "[pkg]\nkey = \"value\"\n");
    let engine = engine(dir.path());
    engine.index().await.expect("index");

    let result = engine
        .search(search_request("pat:'$S.lock().unwrap()'", true))
        .await
        .expect("search");

    // The toml file is a candidate (indexed source file) but has no
    // grammar — that is an explicit capability gap, not a silent skip.
    assert!(
        result
            .missing_capabilities
            .iter()
            .any(|cap| cap.contains("structural matching unavailable for `toml`")),
        "unsupported language reported: {:?}",
        result.missing_capabilities
    );
    // Supported-language candidates still produce hits.
    assert_eq!(hit_paths(&result), vec!["src/lib.rs".to_owned()]);
}

#[tokio::test]
async fn pattern_route_and_semantics() {
    let repo = pattern_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // `lang:` narrows candidates to one grammar.
    let rust_only = engine
        .search(search_request("pat:'$S.lock().unwrap()' lang:rust", true))
        .await
        .expect("lang:rust search");
    assert_eq!(
        hit_paths(&rust_only),
        vec![
            "src/helper.rs".to_owned(),
            "src/lib.rs".to_owned(),
            "tools/util.rs".to_owned(),
        ]
    );

    // `path:` narrows candidates to a prefix.
    let src_only = engine
        .search(search_request("pat:'$S.lock().unwrap()' path:src/", true))
        .await
        .expect("path: search");
    assert_eq!(
        hit_paths(&src_only),
        vec!["src/helper.rs".to_owned(), "src/lib.rs".to_owned()]
    );

    // Free text ANDs against the pattern: only files whose content
    // FTS-matches the residual term stay candidates.
    let narrowed = engine
        .search(search_request("pat:'$S.lock().unwrap()' zephyr", true))
        .await
        .expect("free-text narrowed search");
    assert_eq!(narrowed.request.query, "zephyr");
    assert_eq!(hit_paths(&narrowed), vec!["src/helper.rs".to_owned()]);
}

#[tokio::test]
async fn pattern_route_deterministic_replay() {
    let repo = pattern_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    let first = engine
        .search(search_request("pat:'fmt.Println($$$X)'", true))
        .await
        .expect("first search");
    let second = engine
        .search(search_request("pat:'fmt.Println($$$X)'", true))
        .await
        .expect("replay search");

    assert_eq!(hit_paths(&first), vec!["cmd/main.go".to_owned()]);
    let key = |result: &cce_engine::SearchResult| {
        result
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.document_id.clone(),
                    hit.region_id.clone(),
                    hit.score.to_bits(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&first), key(&second), "identical hit order on replay");
}

#[tokio::test]
async fn pattern_route_reports_unparseable_template_once() {
    let repo = pattern_repo();
    let engine = engine(repo.path());
    engine.index().await.expect("index");

    // `fn $F(` parses to an ERROR root — an illegal template is an
    // explicit capability failure, reported once per language rather than
    // once per candidate file.
    let result = engine
        .search(search_request("pat:'fn $F('", true))
        .await
        .expect("search");
    let failures: Vec<&String> = result
        .missing_capabilities
        .iter()
        .filter(|cap| cap.contains("does not parse"))
        .collect();
    assert!(!failures.is_empty(), "template failure reported");
    let mut unique = failures.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        failures.len(),
        unique.len(),
        "one message per grammar, not per file: {failures:?}"
    );
    assert!(result.hits.is_empty());
}
