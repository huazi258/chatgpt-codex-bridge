# MVP implementation backlog

> Historical V1 execution plan. V2 implementation must be planned from [Decision 004 — Conversation relay V2](../decisions/004-conversation-relay-v2.md); it supersedes this backlog where they conflict.

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

Status: **Completed**. On 2026-08-16, the signed-in Chrome extension paired with the desktop bridge and the desktop UI displayed `已验证 ChatGPT 协议状态：PAUSE` / `协议已验证：PAUSE`. The loopback bridge, authenticated pairing, message injection, completion detection, and protocol validator are live-verified.

## Task 5 — Orchestration loop

- Implement the serial state machine from the architecture document.
- Connect valid `NEXT_TASK` messages to Codex turns and return compact results to ChatGPT.
- Persist every state transition and make restart restore a paused state only.

Acceptance: a simulated two-turn loop reaches `MODULE_DONE` without manual copy/paste.

Status: **Completed**. The serial runtime is persisted in SQLite (`module_runtime`) together with an audit event for every transition. Starting a module sends a planning request to the paired ChatGPT tab; a valid `NEXT_TASK` starts one Codex App Server turn; completion sends a compact Review request back to ChatGPT. `MODULE_DONE`, `PAUSE`, and external-adapter failures enter a persisted safe pause/block state. Startup converts any in-progress persisted runtime to `PAUSED_FOR_ACCEPTANCE`. Unit tests simulate two `NEXT_TASK`/Codex cycles followed by `MODULE_DONE`, with no clipboard handoff.

## Task 6 — Outcome verification and user control

- Verify the reported commit SHA and remote branch after Codex completes.
- Implement round, module, and global time budgets; set `pauseAfterCurrentTurn` when a budget is reached.
- Add Windows notifications and the four acceptance actions: approve, continue, stop, replan.

Acceptance: all budget and blocking paths pause correctly and expose the required user actions.

Status: **Completed**. The middleware requires a commit SHA in Codex's final summary, confirms the selected branch, local commit ancestry, and matching `origin` branch head before a Review request is sent. It persists a verified turn, commit SHA, execution start time, and deferred-budget flag. Round/module/global budgets pause before a new turn or after the current turn completes. The desktop UI exposes approve, continue, stop, and replan controls for persisted pause/block states, and native Windows notifications are requested for pause/block events.

## Task 7 — End-to-end pilot

- Run a bounded module in one personal repository.
- Exercise normal completion, malformed ChatGPT protocol, failed Codex turn, and restart recovery.
- Update reliability, security, and product documents from observed behavior.

Acceptance: all MVP PRD acceptance criteria pass with recorded evidence.
