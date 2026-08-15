# MVP implementation backlog

Tasks must be completed in order unless their dependencies have already been demonstrated. Each task should produce one focused commit and update its related documentation if its contract changes.

## Task 1 — Integration spike

Validate the two external boundaries before scaffolding the production application.

- Start local Codex App Server, create a thread, start a harmless turn, and capture its final event.
- Build a throwaway Chrome extension proof that binds one `chatgpt.com` tab, sends a test message, and detects a completed reply.
- Record exact prerequisites, permissions, version constraints, and any unstable selectors in a decision record.

Acceptance: both paths work end-to-end on the development machine, or the project is paused with evidence of the blocker and a revised option.

Status: **Completed**. On 2026-08-15, the App Server smoke script received `APP_SERVER_SMOKE_OK`, and the custom ChatGPT-extension live-browser smoke test received `CHATGPT_EXTENSION_SMOKE_OK`. Evidence and re-run steps are recorded in `docs/decisions/001-task-1-integration-spike.md`.

## Task 2 — Desktop application foundation

- Create the Tauri + React desktop application.
- Add SQLite schema and typed models for modules, turns, budgets, protocol messages, and audit events.
- Implement repository, branch, tab, and budget selection with persisted inactive-module state.

Acceptance: an inactive module can be created, saved, reopened, and deleted without changing the selected repository.

Status: **Completed**. The Tauri + React shell persists `INACTIVE` module configuration and budgets in SQLite. A Rust lifecycle test covers create, reopen, update while retaining the selected repository, and delete. The desktop application also launched successfully on 2026-08-15.

## Task 3 — Codex execution adapter

- Implement stdio JSON-RPC lifecycle management for App Server.
- Start a Codex turn from a wrapped task prompt and transform streamed events into internal status events.
- Collect its final summary and handle App Server errors and user-input requests as blocks.

Acceptance: the UI can execute one controlled Codex turn and display its final status without window automation.

Status: **Completed**. The desktop app launches one stdio App Server child process per controlled turn, emits typed status events to the React UI, returns the final agent summary, and blocks server-initiated requests. The App Server smoke test passed again on 2026-08-15 with `APP_SERVER_SMOKE_OK`; adapter unit tests cover task wrapping and notification mapping.

## Task 4 — ChatGPT adapter and protocol validator

- Implement extension-to-desktop pairing using a local authenticated channel.
- Bind one dedicated ChatGPT tab, send bootstrap/review messages, and detect completion.
- Implement strict JSON envelope validation and protocol-error pause behavior.

Acceptance: the desktop application completes a protocol exchange and rejects malformed payloads safely.

Status: **Awaiting live-browser verification**. The loopback bridge, authenticated extension pairing, message injection, response detection, and strict protocol validator are implemented. Automated checks confirm malformed pairing rejection and protocol-validator behavior; the single signed-in Chrome pairing and protocol-bootstrap exchange is documented in `spikes/chatgpt-extension/README.md`.

## Task 5 — Orchestration loop

- Implement the serial state machine from the architecture document.
- Connect valid `NEXT_TASK` messages to Codex turns and return compact results to ChatGPT.
- Persist every state transition and make restart restore a paused state only.

Acceptance: a simulated two-turn loop reaches `MODULE_DONE` without manual copy/paste.

## Task 6 — Outcome verification and user control

- Verify the reported commit SHA and remote branch after Codex completes.
- Implement round, module, and global time budgets; set `pauseAfterCurrentTurn` when a budget is reached.
- Add Windows notifications and the four acceptance actions: approve, continue, stop, replan.

Acceptance: all budget and blocking paths pause correctly and expose the required user actions.

## Task 7 — End-to-end pilot

- Run a bounded module in one personal repository.
- Exercise normal completion, malformed ChatGPT protocol, failed Codex turn, and restart recovery.
- Update reliability, security, and product documents from observed behavior.

Acceptance: all MVP PRD acceptance criteria pass with recorded evidence.
