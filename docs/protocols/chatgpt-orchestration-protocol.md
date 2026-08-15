# ChatGPT orchestration protocol

## Purpose

ChatGPT writes natural-language planning and review for the user, then finishes every automation reply with one JSON code block. The middleware acts only on that JSON block.

The dedicated ChatGPT extension is paired to the desktop middleware over its local authenticated bridge. The extension is transport-only: it does not infer protocol state or start Codex work.

## Envelope

```json
{
  "state": "NEXT_TASK | MODULE_DONE | PAUSE | BLOCKED",
  "module": "string",
  "reason": "string",
  "codex_prompt": "string | omitted",
  "acceptance_criteria": ["string"],
  "review_scope": "string | omitted",
  "requires_user_input": false
}
```

## Validation rules

- Exactly one JSON code block is allowed at the end of a response.
- `state`, `module`, and `reason` are always required.
- `NEXT_TASK` requires a non-empty `codex_prompt` and at least one acceptance criterion.
- `MODULE_DONE`, `PAUSE`, and `BLOCKED` must omit `codex_prompt`.
- Unknown fields are ignored only after schema validation succeeds; unknown state values fail validation.
- Any invalid payload changes the module state to `BLOCKED` with reason `Protocol validation failed`.

## Local transport

- The desktop bridge listens only on `ws://127.0.0.1:8765`.
- The desktop app generates one in-memory pairing secret on launch; the extension must present it with the bound ChatGPT tab ID.
- A successful pair receives a session ID. Replies without that current session ID are rejected.
- The desktop adapter sends a message to the bound tab and accepts only a completed assistant reply returned by the extension.

## Turn hand-off

For a Codex completion, the middleware sends ChatGPT a compact message containing the module name, task number, branch, commit SHA, Codex final summary, test result, and push result. It does not send raw terminal output or a full diff.

ChatGPT reviews the remote repository and responds with a new valid envelope. `MODULE_DONE` always causes a human acceptance pause; it does not mark the module accepted by itself.
