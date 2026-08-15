# 002 — Codex execution adapter

- Status: Accepted
- Date: 2026-08-15
- Decision: Use one local `codex app-server` child process per controlled turn over stdio JSONL.

## Contract

The desktop process launches `codex app-server` with the selected repository as its current working directory. It sends `initialize`, `initialized`, `thread/start`, and `turn/start` in that order, then consumes stdout notifications until `turn/completed`.

The adapter converts `turn/started`, `item/started`, `item/completed`, and `turn/completed` into typed `codex-status` events for the React UI. It returns the final agent message as the turn summary. Any App Server error, early close, non-completed turn, or server-initiated request results in a visible failed or blocked state. The adapter never answers a server-initiated user-input or approval request.

`CODEX_APP_SERVER_COMMAND` can override the executable name. The default is `codex`, which must be discoverable through `PATH` in the desktop application's environment.

## Rationale

Stdio is the default App Server transport and uses newline-delimited JSON. The official lifecycle requires the initialization handshake before thread and turn creation, and identifies `turn/completed` as the terminal turn notification. [Official Codex App Server documentation](https://learn.chatgpt.com/docs/app-server)

