PRAGMA foreign_keys = ON;

-- Canonical code regions: the single identity that lexical documents, dense
-- vectors, entities, graph edges, and citations all join on. A region is a
-- (snapshot, path, byte range, kind) tuple; its id is content-addressed so
-- every view describes the same object.
CREATE TABLE IF NOT EXISTS regions (
  id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  kind TEXT NOT NULL,
  language TEXT,
  symbol_name TEXT,
  symbol_kind TEXT,
  qualified_name TEXT,
  parent_region_id TEXT,
  start_byte INTEGER NOT NULL,
  end_byte INTEGER NOT NULL,
  start_line INTEGER NOT NULL,
  end_line INTEGER NOT NULL,
  PRIMARY KEY (snapshot_id, id)
);
CREATE INDEX IF NOT EXISTS regions_path ON regions(snapshot_id, path);
CREATE INDEX IF NOT EXISTS regions_symbol_name ON regions(snapshot_id, symbol_name);
CREATE INDEX IF NOT EXISTS regions_parent ON regions(snapshot_id, parent_region_id);

ALTER TABLE retrieval_documents ADD COLUMN region_id TEXT;
ALTER TABLE entities ADD COLUMN region_id TEXT;
CREATE INDEX IF NOT EXISTS entities_region ON entities(snapshot_id, region_id);
CREATE INDEX IF NOT EXISTS retrieval_documents_region
  ON retrieval_documents(snapshot_id, region_id);

PRAGMA user_version = 4;
