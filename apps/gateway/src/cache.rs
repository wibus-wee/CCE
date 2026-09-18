//! Search-response cache for the proxy path: `(repo_id, body_digest)` →
//! the worker's response bytes.
//!
//! Invalidation semantics are exact, not heuristic: in the gateway model
//! a worker's indexed content changes only when a push materializes a
//! new source dir and kicks `index` — the materialized tree cannot drift
//! on its own. `invalidate_repo` on push is therefore the correctness
//! mechanism; the TTL is only a safety bound for changes that bypass the
//! gateway entirely (a direct `/v1/index` call on the worker, manual
//! reindex, operator edits).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bytes::Bytes;

/// One cached upstream response: body plus the content-type needed to
/// replay it verbatim.
#[derive(Debug)]
struct Cached {
    body: Bytes,
    content_type: Option<String>,
    inserted: Instant,
}

#[derive(Debug)]
pub(crate) struct SearchCache {
    entries: parking_lot::Mutex<HashMap<(String, String), Cached>>,
    ttl: Duration,
    max_entries: usize,
}

impl SearchCache {
    pub(crate) fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: parking_lot::Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    /// Replay a cached response when fresh. Key = repo + exact request
    /// body digest — identical semantic queries with different JSON
    /// framing miss rather than risk a wrong-body replay.
    pub(crate) fn get(&self, repo_id: &str, body_digest: &str) -> Option<(Bytes, Option<String>)> {
        let entries = self.entries.lock();
        let cached = entries.get(&(repo_id.to_owned(), body_digest.to_owned()))?;
        if cached.inserted.elapsed() > self.ttl {
            return None;
        }
        Some((cached.body.clone(), cached.content_type.clone()))
    }

    /// Store a successful upstream response. On overflow, expired
    /// entries are swept first; a still-full cache clears rather than
    /// growing unbounded — the next request repopulates from upstream.
    pub(crate) fn put(
        &self,
        repo_id: &str,
        body_digest: &str,
        body: Bytes,
        content_type: Option<String>,
    ) {
        let mut entries = self.entries.lock();
        if entries.len() >= self.max_entries {
            let ttl = self.ttl;
            entries.retain(|_, cached| cached.inserted.elapsed() <= ttl);
            if entries.len() >= self.max_entries {
                entries.clear();
            }
        }
        entries.insert(
            (repo_id.to_owned(), body_digest.to_owned()),
            Cached {
                body,
                content_type,
                inserted: Instant::now(),
            },
        );
    }

    /// Drop every cached response for a repo — called when a push
    /// commits new content, the only in-model mutation channel.
    pub(crate) fn invalidate_repo(&self, repo_id: &str) {
        self.entries.lock().retain(|(repo, _), _| repo != repo_id);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> SearchCache {
        SearchCache::new(Duration::from_secs(60), 4)
    }

    #[test]
    fn get_replays_what_put_stored() {
        let cache = cache();
        cache.put(
            "r1",
            "d1",
            Bytes::from_static(b"{\"hits\":[]}"),
            Some("application/json".to_owned()),
        );
        let (body, content_type) = cache.get("r1", "d1").expect("cache hit");
        assert_eq!(&body[..], b"{\"hits\":[]}");
        assert_eq!(content_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn miss_on_wrong_repo_or_digest() {
        let cache = cache();
        cache.put("r1", "d1", Bytes::from_static(b"x"), None);
        assert!(cache.get("r2", "d1").is_none());
        assert!(cache.get("r1", "d2").is_none());
    }

    #[test]
    fn ttl_expiry_misses() {
        let cache = SearchCache::new(Duration::ZERO, 4);
        cache.put("r1", "d1", Bytes::from_static(b"x"), None);
        std::thread::sleep(Duration::from_millis(1));
        assert!(cache.get("r1", "d1").is_none());
    }

    #[test]
    fn invalidate_repo_drops_only_that_repo() {
        let cache = cache();
        cache.put("r1", "d1", Bytes::from_static(b"1"), None);
        cache.put("r2", "d1", Bytes::from_static(b"2"), None);
        cache.invalidate_repo("r1");
        assert!(cache.get("r1", "d1").is_none());
        assert!(cache.get("r2", "d1").is_some());
    }

    #[test]
    fn overflow_stays_bounded() {
        let cache = SearchCache::new(Duration::from_secs(60), 3);
        for index in 0..6 {
            cache.put("r1", &format!("d{index}"), Bytes::from_static(b"x"), None);
        }
        assert!(cache.len() <= 3);
    }
}
