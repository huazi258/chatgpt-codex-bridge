CREATE TABLE IF NOT EXISTS module_execution_state (
  module_id TEXT PRIMARY KEY NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
  started_at TEXT NOT NULL,
  pause_after_current_turn INTEGER NOT NULL DEFAULT 0 CHECK (pause_after_current_turn IN (0, 1)),
  last_commit_sha TEXT
);
