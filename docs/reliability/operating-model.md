# Reliability and operating model

## Pause-first policy

The system must prefer a recoverable pause to a speculative action. It pauses when the protocol is invalid, the bound tab is unavailable, Codex fails or asks for clarification, Git verification fails, a configured budget is reached, or the application restarts during active work.

## Budget behavior

- A module has maximum round and duration budgets; the application also has a global duration budget.
- A budget is checked before starting a turn and when an active turn completes.
- Reaching a budget during a turn sets `pauseAfterCurrentTurn`; the turn is allowed to finish and report its Git outcome.
- No budget is bypassed by a `NEXT_TASK` from ChatGPT.

## Recovery

- Persist an append-only audit event before and after every external action.
- On startup, convert in-progress states to `PAUSED_FOR_RECOVERY`; never assume an external action's result.
- Allow the user to inspect the final known event, terminate, or request a replanning message.
- Retrying a failed external action requires an explicit user action in the MVP.

## Observability

The main view shows module, repository, branch, state, current round, elapsed time, configured budgets, and the latest status line. Detailed App Server events and protocol payloads remain available behind a diagnostics view, with secrets redacted.
