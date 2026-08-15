PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS modules (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  repository_path TEXT NOT NULL,
  target_branch TEXT NOT NULL,
  chatgpt_tab_id INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('INACTIVE')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS budgets (
  module_id TEXT PRIMARY KEY NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
  max_rounds INTEGER NOT NULL CHECK (max_rounds > 0),
  module_timeout_minutes INTEGER NOT NULL CHECK (module_timeout_minutes > 0),
  global_timeout_minutes INTEGER NOT NULL CHECK (global_timeout_minutes > 0)
);

CREATE TABLE IF NOT EXISTS turns (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
  turn_number INTEGER NOT NULL CHECK (turn_number > 0),
  state TEXT NOT NULL,
  codex_summary TEXT,
  commit_sha TEXT,
  started_at TEXT,
  completed_at TEXT
);

CREATE TABLE IF NOT EXISTS protocol_messages (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
  direction TEXT NOT NULL CHECK (direction IN ('TO_CHATGPT', 'FROM_CHATGPT')),
  protocol_state TEXT,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_events (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT REFERENCES modules(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  message TEXT NOT NULL,
  metadata_json TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_turns_module_id ON turns(module_id);
CREATE INDEX IF NOT EXISTS idx_protocol_messages_module_id ON protocol_messages(module_id);
CREATE INDEX IF NOT EXISTS idx_audit_events_module_id ON audit_events(module_id);

