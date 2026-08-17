CREATE TABLE IF NOT EXISTS relay_codex_cycles (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES relay_modules(id) ON DELETE CASCADE,
  cycle_number INTEGER NOT NULL CHECK (cycle_number > 0),
  status TEXT NOT NULL CHECK (
    status IN (
      'WAITING_TO_SEND_CODEX',
      'CODEX_RUNNING',
      'CODEX_COMPLETED',
      'WAITING_FOR_CHATGPT',
      'SENDING_TO_CHATGPT',
      'DELIVERED_TO_CHATGPT',
      'FAILED'
    )
  ),
  prompt_text TEXT NOT NULL,
  codex_thread_id TEXT,
  codex_turn_id TEXT,
  result_text TEXT,
  outbound_chatgpt_message_id TEXT UNIQUE REFERENCES relay_messages(id) ON DELETE SET NULL,
  error_text TEXT,
  created_at TEXT NOT NULL,
  codex_started_at TEXT,
  codex_completed_at TEXT,
  relay_queued_at TEXT,
  relay_delivered_at TEXT,
  updated_at TEXT NOT NULL,
  UNIQUE(module_id, cycle_number)
);

CREATE INDEX IF NOT EXISTS idx_relay_codex_cycles_module_cycle
  ON relay_codex_cycles(module_id, cycle_number DESC);

CREATE INDEX IF NOT EXISTS idx_relay_codex_cycles_status
  ON relay_codex_cycles(status);

CREATE UNIQUE INDEX IF NOT EXISTS idx_relay_codex_cycles_single_running
  ON relay_codex_cycles(status)
  WHERE status = 'CODEX_RUNNING';
