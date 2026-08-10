PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS learning_events (
  id TEXT PRIMARY KEY,
  trajectory_id TEXT NOT NULL REFERENCES trajectories(id) ON DELETE CASCADE,
  stage TEXT NOT NULL,
  document_id TEXT,
  dwell_ms INTEGER,
  metadata_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS learning_events_trajectory_created
  ON learning_events(trajectory_id, created_at);
CREATE INDEX IF NOT EXISTS learning_events_stage_created
  ON learning_events(stage, created_at);

PRAGMA user_version = 3;
