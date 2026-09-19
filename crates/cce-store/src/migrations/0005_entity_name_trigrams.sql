PRAGMA foreign_keys = ON;

-- Entity-name trigram index: the local Zoekt-style inverted index for
-- folded identifier lookup. One row per (snapshot, trigram, entity);
-- `entity_name_candidates` intersects the query term's trigrams here to
-- narrow the candidate set, then verifies with the folded LIKE so the
-- index only ever pays the necessary-condition cost.
CREATE TABLE IF NOT EXISTS entity_name_trigrams (
  snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  trigram TEXT NOT NULL,
  entity_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS entity_name_trigrams_lookup
  ON entity_name_trigrams(snapshot_id, trigram, entity_id);
CREATE INDEX IF NOT EXISTS entity_name_trigrams_entity
  ON entity_name_trigrams(snapshot_id, entity_id);

PRAGMA user_version = 5;
