CREATE TABLE IF NOT EXISTS relay_modules (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  working_directory TEXT NOT NULL,
  max_cycles INTEGER NOT NULL CHECK (max_cycles > 0),
  max_runtime_minutes INTEGER NOT NULL CHECK (max_runtime_minutes > 0),
  retry_template TEXT NOT NULL,
  phase TEXT NOT NULL,
  codex_thread_id TEXT,
  module_started_at TEXT,
  stop_after_turn INTEGER NOT NULL DEFAULT 0 CHECK (stop_after_turn IN (0, 1)),
  invalid_reply_count INTEGER NOT NULL DEFAULT 0 CHECK (invalid_reply_count >= 0),
  started_cycles INTEGER NOT NULL DEFAULT 0 CHECK (started_cycles >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS relay_messages (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES relay_modules(id) ON DELETE CASCADE,
  sequence_number INTEGER NOT NULL,
  direction TEXT NOT NULL CHECK (direction IN ('TO_CHATGPT', 'FROM_CHATGPT', 'TO_CODEX', 'FROM_CODEX')),
  kind TEXT NOT NULL CHECK (kind IN ('MANUAL', 'AUTOMATION', 'SYSTEM')),
  text TEXT NOT NULL,
  delivery_state TEXT NOT NULL CHECK (delivery_state IN ('QUEUED', 'SENT', 'DELIVERED', 'UNKNOWN', 'FAILED')),
  created_at TEXT NOT NULL,
  delivered_at TEXT,
  UNIQUE(module_id, sequence_number)
);

CREATE TABLE IF NOT EXISTS relay_events (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES relay_modules(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  detail TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_relay_messages_module_sequence ON relay_messages(module_id, sequence_number);
CREATE INDEX IF NOT EXISTS idx_relay_modules_phase ON relay_modules(phase);
