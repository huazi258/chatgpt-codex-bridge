# System architecture

> Historical V1 architecture. The active relay architecture is [Decision 004 — Conversation relay V2](../decisions/004-conversation-relay-v2.md); it supersedes this document where they conflict.

## Component view

```text
Dedicated Chrome profile
  └─ ChatGPT browser extension
       └─ loopback authenticated channel
            └─ Desktop middleware
                 ├─ UI: task status and acceptance controls
                 ├─ Orchestrator: state machine and budgets
                 ├─ Protocol validator
                 ├─ Local SQLite store and audit log
                 ├─ Git verifier
                 └─ Codex App Server adapter (stdio JSON-RPC)
                      └─ Codex working in the selected repository
```

## Responsibilities

### Browser extension

The extension binds one dedicated ChatGPT tab, submits middleware-owned messages, observes response completion, and returns response text. It must not infer workflow state from ordinary prose or access sites outside `chatgpt.com`.

### Desktop middleware

The middleware is the system of record for an active module. It validates ChatGPT protocol messages, starts and monitors Codex turns, enforces run budgets, records audit data, and controls user-visible pauses.

Task 2 establishes the local storage boundary with an SQLite database owned by the desktop process. The schema contains `modules`, `budgets`, `turns`, `protocol_messages`, `audit_events`, and `module_runtime`. `module_runtime` stores the current serial orchestration phase and completed-round count; each transition also appends an audit event. On restart, any in-progress runtime is converted to `PAUSED_FOR_ACCEPTANCE` before new external work can begin.

### Codex App Server adapter

The adapter owns the lifecycle of one local `codex app-server` child process per controlled turn and its stdio JSON-RPC session. It completes the initialization handshake, starts a thread and turn, maps streamed `turn/*` and `item/*` notifications into typed `codex-status` events, then ends the child process after `turn/completed`. Server-initiated requests are never auto-answered: they become a blocked UI result. The adapter never relies on desktop-window automation.

### Git verifier

The verifier confirms that the selected repository and branch match the active module and that Codex's reported commit is reachable from the configured remote branch. It may inspect Git; it must not make or amend commits.

## Primary data flow

1. The user selects a repository, branch, ChatGPT tab, and budgets.
2. The middleware sends the protocol bootstrap to ChatGPT through the extension.
3. The protocol validator accepts a `NEXT_TASK` message or pauses the module.
4. The App Server adapter runs the wrapped Codex prompt in the selected repository.
5. After Codex finishes, the middleware requires a reported commit SHA, verifies it is reachable from the selected local branch, and requires that local branch head to equal `origin/<target branch>`. It then records the verified turn and sends only the compact final summary, target branch, and commit SHA to ChatGPT for Review.
6. ChatGPT returns the next protocol state; the persisted orchestrator either starts another turn or pauses.

## Invariants

- Only one active module may own the orchestrator at a time.
- An active module has exactly one repository root, branch and bound ChatGPT tab.
- No Codex turn begins without a valid `NEXT_TASK` payload.
- A budget never interrupts a Codex turn; it blocks starting a subsequent one.
- A pause or block never auto-resumes after application restart.
- A Codex completion cannot reach ChatGPT Review until its reported commit and remote branch verification succeed.
