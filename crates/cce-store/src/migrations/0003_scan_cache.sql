-- mtime+size keyed content-hash cache used to skip re-reading unchanged
-- files during repository scans. Heuristic only; bytes are still verified
-- against the recorded hash before a snapshot is committed.
CREATE TABLE IF NOT EXISTS scan_cache (
  repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  mtime_ms INTEGER NOT NULL,
  line_count INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  PRIMARY KEY (repository_id, path)
);
PRAGMA user_version = 3;
