CREATE TABLE IF NOT EXISTS relay_codex_input_requests (
 id TEXT PRIMARY KEY NOT NULL, module_id TEXT NOT NULL REFERENCES relay_modules(id) ON DELETE CASCADE, cycle_id TEXT NOT NULL REFERENCES relay_codex_cycles(id) ON DELETE CASCADE,
 codex_thread_id TEXT NOT NULL, codex_turn_id TEXT NOT NULL, app_server_request_id_json TEXT NOT NULL, questions_json TEXT NOT NULL, answers_json TEXT,
 secret_answer_status_json TEXT NOT NULL, is_blocking INTEGER NOT NULL CHECK (is_blocking IN (0,1)), auto_resolution_ms INTEGER, request_compatibility_json TEXT NOT NULL,
 status TEXT NOT NULL CHECK (status IN ('PENDING','ANSWERING','ANSWERED','INTERRUPTED','EXPIRED')), error_text TEXT,
 created_at TEXT NOT NULL, submitted_at TEXT, answered_at TEXT, interrupted_at TEXT, expired_at TEXT, updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_relay_codex_input_one_actionable_turn ON relay_codex_input_requests(module_id, codex_turn_id) WHERE status IN ('PENDING','ANSWERING');
CREATE UNIQUE INDEX IF NOT EXISTS idx_relay_codex_input_active_request_id ON relay_codex_input_requests(app_server_request_id_json) WHERE status IN ('PENDING','ANSWERING');
CREATE INDEX IF NOT EXISTS idx_relay_codex_input_module_created ON relay_codex_input_requests(module_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_relay_codex_input_status ON relay_codex_input_requests(status);
