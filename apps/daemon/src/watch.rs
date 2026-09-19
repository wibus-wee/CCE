//! `--watch` polling loop: periodically rescan the worktree and reindex
//! when the snapshot identity moves.
//!
//! Polling (not `notify`) is deliberate — it mirrors `cce-push --watch`,
//! adds no filesystem-event dependency, and the store's scan cache makes an
//! unchanged rescan cheap. The scanned snapshot id is the whole change
//! signal: it hashes the repository id, the resolved HEAD revision, the
//! worktree overlay (paths + content hashes), and the index profile, so it
//! moves on any edit, commit, checkout, or config change that could alter
//! index output. Other ref moves (fetch, non-HEAD branch/tag updates)
//! cannot change what `index()` produces — history only walks HEAD
//! ancestors — so they are intentionally not triggers.

use std::{sync::Arc, time::Duration};

use cce_engine::{CceEngine, RepositoryScanner};

/// Snapshot identity the last successful index committed; `None` until the
/// first run so watch mode also indexes a never-indexed repository.
#[derive(Debug, Default)]
struct WatchState {
    committed_snapshot: Option<String>,
}

impl WatchState {
    /// `true` when `snapshot_id` differs from the last committed snapshot —
    /// an index run is due.
    fn is_stale(&self, snapshot_id: &str) -> bool {
        self.committed_snapshot.as_deref() != Some(snapshot_id)
    }

    /// Record the snapshot an index run actually committed. Set from the
    /// `IndexReport`, not the triggering scan, so a tree that moved
    /// mid-index does not retrigger on the next tick; left untouched on
    /// failure so a failed run retries.
    fn commit(&mut self, snapshot_id: String) {
        self.committed_snapshot = Some(snapshot_id);
    }
}

/// Poll forever: sleep `interval`, rescan, and `index()` when the snapshot
/// moved. Every failure logs and retries on the next tick — the loop never
/// exits early so watch survives transient IO errors and lease contention.
/// The task ends only with the process.
pub(crate) async fn run(engine: Arc<CceEngine>, interval: Duration) {
    let mut state = WatchState::default();
    loop {
        tokio::time::sleep(interval).await;
        tick(&engine, &mut state).await;
    }
}

/// One poll cycle: scan → compare → index on change.
async fn tick(engine: &CceEngine, state: &mut WatchState) {
    // The scan is synchronous file IO on the executor — the same pattern
    // `CceEngine::index()` uses when invoked from a request handler.
    let scanned = match RepositoryScanner::new(engine.config().clone()).scan(Some(engine.store())) {
        Ok(scanned) => scanned,
        Err(error) => {
            tracing::warn!(%error, "watch: scan failed");
            return;
        }
    };
    if !state.is_stale(&scanned.snapshot.id) {
        return;
    }
    tracing::info!(
        snapshot = %scanned.snapshot.id,
        files = scanned.files.len(),
        "watch: change detected"
    );
    match engine.index_with_origin(Some("watch".to_owned())).await {
        Ok(report) => {
            tracing::info!(
                snapshot = %report.snapshot.id,
                reused = report.reused_snapshot,
                indexed_files = report.indexed_files,
                source_units = report.source_units,
                "watch: indexed snapshot"
            );
            state.commit(report.snapshot.id);
        }
        // A concurrent index (a manual POST /v1/index, a `cce index` run)
        // holds the lease — not a failure; the next tick reconciles.
        Err(cce_core::CceError::IndexBusy(_)) => {
            tracing::info!("watch: index already in progress, retrying next tick");
        }
        Err(error) => {
            tracing::warn!(%error, "watch: index failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_observation_is_stale() {
        // Nothing committed yet → any snapshot triggers an index, covering
        // a never-indexed repository at daemon startup.
        let state = WatchState::default();
        assert!(state.is_stale("snap_a"));
    }

    #[test]
    fn committed_snapshot_is_fresh() {
        let mut state = WatchState::default();
        state.commit("snap_a".to_owned());
        assert!(!state.is_stale("snap_a"));
        assert!(state.is_stale("snap_b"));
    }

    #[test]
    fn uncommitted_snapshot_retries() {
        // A scan without a following commit (index failed or was skipped)
        // stays stale, so the next tick retries instead of forgetting the
        // change.
        let mut state = WatchState::default();
        assert!(state.is_stale("snap_a"));
        assert!(state.is_stale("snap_a"));
        state.commit("snap_b".to_owned());
        assert!(!state.is_stale("snap_b"));
        assert!(state.is_stale("snap_a"));
    }
}
