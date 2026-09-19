-- Snapshot attribution: who produced this snapshot (cli, watch, an MCP
-- client name, a checkpoint pass). Metadata, not identity — identical
-- content indexed by two different origins still shares one snapshot id.
ALTER TABLE snapshots ADD COLUMN origin TEXT;

PRAGMA user_version = 6;
