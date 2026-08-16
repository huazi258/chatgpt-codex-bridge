# Reliability and operating model

## Pause-first policy

The system must prefer a recoverable pause to a speculative action. It pauses when the protocol is invalid, the bound tab is unavailable, Codex fails or asks for clarification, Git verification fails, a configured budget is reached, or the application restarts during active work.

## Budget behavior

- A module has maximum round and duration budgets; the application also has a global duration budget.
- A budget is checked before starting a turn and when an active turn completes.
- Reaching a budget during a turn sets `pauseAfterCurrentTurn`; the turn is allowed to finish and report its Git outcome.
- No budget is bypassed by a `NEXT_TASK` from ChatGPT.
- A Review request is sent only after the reported commit SHA is verified against the selected local branch and its matching `origin` branch head.

## Recovery

- Persist an append-only audit event before and after every external action.
- On startup, convert in-progress persisted runtime states to `PAUSED_FOR_ACCEPTANCE`; never assume an external action's result.
- Allow the user to inspect the final known event, terminate, or request a replanning message.
- Retrying a failed external action requires an explicit user action in the MVP.
- Pause and block events request a native Windows notification; the desktop acceptance card remains the authoritative place to approve, continue, stop, or replan.

## Observability

The main view shows module, repository, branch, state, current round, elapsed time, configured budgets, and the latest status line. Detailed App Server events and protocol payloads remain available behind a diagnostics view, with secrets redacted.

## Browser protocol completion window

The browser adapter normally waits 90 seconds for a machine-readable protocol response. It records the pre-send protocol JSON set and accepts a later parsed JSON object when it differs from that baseline, even if ChatGPT has reused a virtual-list assistant node without a discoverable mutation. If the dispatched request has already produced or changed an assistant node containing a JSON/code-block candidate, it allows one additional bounded 90-second rendering grace period. This prevents a partially rendered or virtualized protocol block from being misclassified as malformed while retaining a finite, user-actionable failure path. Diagnostics record only counts and boolean candidate state, never reply text.
