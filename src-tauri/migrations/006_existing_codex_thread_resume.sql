ALTER TABLE relay_modules ADD COLUMN resume_thread_id TEXT;
ALTER TABLE relay_modules ADD COLUMN codex_recovery_reason TEXT;

CREATE TABLE relay_codex_threads (
  thread_id TEXT PRIMARY KEY NOT NULL,
  working_directory TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('RESERVED', 'ACTIVE', 'RELEASED', 'UNAVAILABLE')),
  owner_module_id TEXT REFERENCES relay_modules(id),
  last_module_id TEXT REFERENCES relay_modules(id),
  reservation_previous_state TEXT,
  updated_at TEXT NOT NULL,
  CHECK (
    (state IN ('RESERVED', 'ACTIVE') AND owner_module_id IS NOT NULL)
    OR (state IN ('RELEASED', 'UNAVAILABLE') AND owner_module_id IS NULL)
  ),
  CHECK (
    (state = 'RESERVED' AND reservation_previous_state IN ('NONE', 'RELEASED'))
    OR (state <> 'RESERVED' AND reservation_previous_state IS NULL)
  )
);

CREATE INDEX idx_relay_codex_threads_owner
  ON relay_codex_threads(owner_module_id);
CREATE INDEX idx_relay_codex_threads_working_directory_state
  ON relay_codex_threads(working_directory, state);
CREATE UNIQUE INDEX idx_relay_codex_threads_single_owned
  ON relay_codex_threads(owner_module_id)
  WHERE state IN ('RESERVED', 'ACTIVE');

INSERT INTO relay_codex_threads (
  thread_id,
  working_directory,
  state,
  owner_module_id,
  last_module_id,
  reservation_previous_state,
  updated_at
)
SELECT
  legacy.codex_thread_id,
  legacy.working_directory,
  CASE
    WHEN legacy.reference_count = 1
      AND legacy.phase IN ('COMPLETED', 'STOPPED')
      AND COALESCE((
        SELECT MAX(released.created_at)
        FROM relay_events released
        WHERE released.module_id = legacy.id
          AND released.event_type = 'CODEX_THREAD_RELEASED'
      ), '') > COALESCE((
        SELECT MAX(failed.created_at)
        FROM relay_events failed
        WHERE failed.module_id = legacy.id
          AND failed.event_type = 'CODEX_THREAD_RELEASE_FAILED'
      ), '')
    THEN 'RELEASED'
    ELSE 'UNAVAILABLE'
  END,
  NULL,
  legacy.id,
  NULL,
  legacy.updated_at
FROM (
  SELECT
    modules.id,
    modules.codex_thread_id,
    modules.working_directory,
    modules.phase,
    modules.updated_at,
    (
      SELECT COUNT(*)
      FROM relay_modules same_thread
      WHERE same_thread.codex_thread_id = modules.codex_thread_id
    ) AS reference_count
  FROM relay_modules modules
  WHERE modules.codex_thread_id IS NOT NULL
    AND modules.id = (
      SELECT newest.id
      FROM relay_modules newest
      WHERE newest.codex_thread_id = modules.codex_thread_id
      ORDER BY newest.updated_at DESC, newest.id DESC
      LIMIT 1
    )
) legacy;
