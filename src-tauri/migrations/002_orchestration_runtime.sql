CREATE TABLE IF NOT EXISTS module_runtime (
  module_id TEXT PRIMARY KEY NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
  phase TEXT NOT NULL,
  completed_rounds INTEGER NOT NULL DEFAULT 0 CHECK (completed_rounds >= 0),
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_module_runtime_phase ON module_runtime(phase);
