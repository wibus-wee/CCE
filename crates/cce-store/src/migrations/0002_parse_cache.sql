ALTER TABLE source_files ADD COLUMN analysis_artifact_digest TEXT REFERENCES artifacts(digest);
PRAGMA user_version = 2;

