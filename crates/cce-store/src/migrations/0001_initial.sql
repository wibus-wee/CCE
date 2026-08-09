PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS repositories (
  id TEXT PRIMARY KEY,
  canonical_root TEXT NOT NULL,
  remote TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
  id TEXT PRIMARY KEY,
  repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
  base_revision TEXT,
  workspace_overlay_hash TEXT NOT NULL,
  index_profile_hash TEXT NOT NULL,
  created_at TEXT NOT NULL,
  file_count INTEGER NOT NULL,
  source_bytes INTEGER NOT NULL,
  complete INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS snapshots_repository_created
  ON snapshots(repository_id, created_at DESC);

CREATE TABLE IF NOT EXISTS current_snapshots (
  repository_id TEXT PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS view_status (
  repository_id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  view_kind TEXT NOT NULL,
  state TEXT NOT NULL,
  profile_hash TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  capabilities_json TEXT NOT NULL,
  artifact_digest TEXT,
  message TEXT,
  PRIMARY KEY (snapshot_id, view_kind)
);

CREATE TABLE IF NOT EXISTS artifacts (
  digest TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  relative_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS source_files (
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  language TEXT,
  content_hash TEXT NOT NULL,
  artifact_digest TEXT NOT NULL REFERENCES artifacts(digest),
  byte_count INTEGER NOT NULL,
  line_count INTEGER NOT NULL,
  PRIMARY KEY (snapshot_id, path)
);

CREATE TABLE IF NOT EXISTS entities (
  id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  qualified_name TEXT,
  signature TEXT,
  language TEXT,
  address_json TEXT,
  capabilities_json TEXT NOT NULL,
  attributes_json TEXT NOT NULL,
  PRIMARY KEY (snapshot_id, id)
);
CREATE INDEX IF NOT EXISTS entities_name ON entities(snapshot_id, name);
CREATE INDEX IF NOT EXISTS entities_qualified_name ON entities(snapshot_id, qualified_name);

CREATE TABLE IF NOT EXISTS relations (
  id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  source_entity_id TEXT NOT NULL,
  target_entity_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  origin TEXT NOT NULL,
  confidence REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
  extractor TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  attributes_json TEXT NOT NULL,
  PRIMARY KEY (snapshot_id, id)
);
CREATE INDEX IF NOT EXISTS relations_source ON relations(snapshot_id, source_entity_id, kind);
CREATE INDEX IF NOT EXISTS relations_target ON relations(snapshot_id, target_entity_id, kind);

CREATE TABLE IF NOT EXISTS retrieval_documents (
  id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  entity_id TEXT NOT NULL,
  representation TEXT NOT NULL,
  body_artifact_digest TEXT NOT NULL REFERENCES artifacts(digest),
  address_json TEXT,
  embedding_profile TEXT,
  generated_by TEXT,
  evidence_json TEXT NOT NULL,
  terms_json TEXT NOT NULL,
  PRIMARY KEY (snapshot_id, id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
  document_id UNINDEXED,
  snapshot_id UNINDEXED,
  entity_id UNINDEXED,
  path,
  name,
  terms,
  body,
  tokenize = 'unicode61 remove_diacritics 2 tokenchars _'
);

CREATE TABLE IF NOT EXISTS invalidations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  repository_id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  created_at TEXT NOT NULL,
  processed_at TEXT
);

CREATE TABLE IF NOT EXISTS trajectories (
  id TEXT PRIMARY KEY,
  repository_id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL,
  query TEXT NOT NULL,
  intent TEXT NOT NULL,
  artifact_digest TEXT NOT NULL REFERENCES artifacts(digest),
  created_at TEXT NOT NULL
);

PRAGMA user_version = 1;

