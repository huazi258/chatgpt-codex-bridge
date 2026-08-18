# 004 — Conversation relay V2

- Status: Accepted
- Date: 2026-08-16
- Decision: Reframe the desktop middleware as a controlled text-relay system between one ChatGPT conversation and one middleware-owned Codex thread. It does not manage engineering work.

## Supersession

This decision supersedes conflicting parts of the MVP PRD, system architecture, ChatGPT orchestration protocol, operating model, local security model, and execution adapter decision. Earlier records remain evidence of the completed V1 implementation and external-boundary validation.

## Product boundary

The middleware owns reliable message delivery, message classification, queues, connection recovery, visibility, and configured runtime budgets. It does **not** decide task decomposition, repository or branch policy, Git validation, test execution, commit verification, or prompt content.

| Owner | Responsibility |
| --- | --- |
| User | Prepares the project, `AGENTS.md`, documentation, and ChatGPT context; controls ordinary ChatGPT conversation and final module acceptance or termination. |
| ChatGPT | Plans and reviews engineering work, writes every Codex prompt, reads repository state, and tells the middleware when an engineering decision needs the user. |
| Codex | Executes work in the selected working directory under the project instructions and reports its final text. Code-changing work follows the project instruction to commit and push. |
| Middleware | Relays messages, maintains one active Codex thread, and applies only the protocol and runtime rules in this document. |

## Conversation lifecycle

### Preparation

Before binding, the user may freely work in the selected ChatGPT browser conversation to establish project context and the control-block convention. Binding does not send a bootstrap message and does not start a module.

When bound, the middleware synchronizes the complete available text and code-block history into its resident ChatGPT window. It must report an incomplete history sync explicitly; it must not present a partial sync as complete. Media and attachments are out of scope for the first V2 slice.

### Resident ChatGPT window

The middleware provides two message classes in one bound ChatGPT conversation:

- **Manual message**: supplied and explicitly sent by the user. Its reply is displayed but never parsed as an automation instruction.
- **Automation request**: supplied and explicitly sent by the user. Only its corresponding completed reply is eligible for control-block parsing.

All outbound messages are serialized in strict first-in, first-out order. A Codex result is an automatic message, but it never overtakes a manual message already in the queue. At most one ChatGPT reply may be in flight.

### Module and Codex lifecycle

A module is a user-visible work phase, not a single engineering task. An engineering task may span many message-relay cycles.

A valid `CODEX_PROMPT` begins one relay cycle and one Codex turn. The first valid prompt for a module starts the module timer, launches a middleware-owned local App Server in the user-selected Codex working directory, and creates a new Codex thread. The middleware sends every later prompt to that same thread unchanged and keeps the App Server process alive until the module ends.

The Codex working directory is an environment selection only. The middleware does not parse, validate, select, switch, or otherwise manage the repository and branch. Project-level instructions are discovered by Codex from the selected working directory.

On module completion or termination, the middleware closes its App Server process and releases, but does not delete, the thread. A released middleware-owned thread may later be listed and resumed through the middleware. The middleware must not promise that an arbitrary Codex Desktop conversation is writable: it may be listed and read, but is usable only if `thread/resume` acquires its writer successfully.

## ChatGPT control-block protocol

The public ChatGPT protocol is plain text, not JSON. An automation reply may contain ordinary explanatory Chinese text, but it must end with exactly one of the following control blocks. A control block is eligible only when it is the terminal non-whitespace content of the reply.

### Send a Codex prompt

```text
@@@CODEX_PROMPT@@@
<verbatim prompt for Codex>
@@@END_CODEX_PROMPT@@@
```

The prompt must be non-empty. The middleware passes the body to Codex verbatim: it adds no Git, test, repository, branch, or completion wrapper.

### Request module acceptance

```text
@@@MODULE_DONE@@@
```

This is not final completion. The middleware enters `WAITING_FOR_ACCEPTANCE`. The user may accept, submit feedback to ChatGPT and continue, or terminate the module.

### Request user intervention

```text
@@@BLOCKED@@@
<Chinese reason and the information or decision needed from the user>
@@@END_BLOCKED@@@
```

The middleware shows a Chinese intervention card. The user enters a reply in the middleware; it is sent to ChatGPT as an automation message and the workflow continues from ChatGPT's next eligible reply.

No reply may contain more than one control block. Codex App Server user-input requests are not ChatGPT control blocks: they follow the direct middleware-to-user flow defined below.

## Retry, errors, and runtime control

If an eligible ChatGPT reply lacks a valid control block, the middleware sends the user-configurable Chinese retry template once. A second invalid reply stops automatic progress and presents a user-actionable error. Manual-chat replies never enter this retry flow.

The module has only two configured budgets:

- total module runtime, starting when the first Codex turn starts;
- maximum started `CODEX_PROMPT` cycles.

Retries, direct Codex human-input exchanges, and manual messages do not increment the cycle count, but all elapsed time after module start counts toward the runtime budget. When a budget is reached, the active Codex turn is allowed to finish before termination.

If the user requests termination while a Codex turn is running, the middleware allows that turn to finish. If it is waiting for required Codex input, the user may answer the current original App Server request only to finish that same turn. The final result is not sent to ChatGPT and no additional Codex prompt begins.

Connection loss, page refresh, application restart, or a delivery result whose outcome is unknown must never trigger automatic resend. The middleware preserves history, queue records, module/thread references, and the last known state; after reconnection the user chooses to inspect and continue or to resend explicitly.

## App Server permissions and input

The middleware runs Codex with the configured default of full execution access. When an App Server user-input request arrives for the active turn, the middleware persists its full question metadata and directly displays its questions to the user. The user supplies free-text answers in the middleware, which responds to the same original App Server request without involving ChatGPT. UI order is presentation-only: the response maps each App Server `question.id` to that question's answers list, never to its display text, header, or array position. Empty answers are represented by an empty list; options are reference-only. `isSecret` answers are used only in memory for the original response and are never persisted or logged. This does not start another Codex turn, cycle, or thread.

Writing the response only moves the request to an answering state. It is answered only after the matching App Server `serverRequest/resolved` event; a resolved request before submission is expired and cannot accept a late answer. On application or runtime restart, or if a sent response cannot be confirmed, it is marked interrupted and the module enters recovery; the middleware never automatically restores or resends it. `autoResolutionMs`, when supplied, is presentation metadata rather than a local expiration authority.

## User experience and observability

The desktop UI is Chinese by default. Every action immediately enters a visible pending, success, or failure state; it must never require browser refresh as the normal recovery mechanism. The resident view displays ChatGPT connection state, Codex connection state, queue length and current item, active thread, module budget usage, and the latest actionable error. Failures expose explicit retry or termination actions.

## V2 acceptance criteria

1. After the user manually creates an automation request, a terminal valid `CODEX_PROMPT` starts one turn in a middleware-owned Codex thread; a second valid prompt continues the same thread.
2. Manual replies that contain control-looking text never start Codex work.
3. A Codex final reply is delivered to ChatGPT in FIFO order behind all pre-existing manual messages.
4. A malformed eligible reply triggers one configured retry, then a Chinese user-actionable failure.
5. A pending App Server input request is displayed and answered directly by the user through the middleware to the same App Server request, without creating another Codex cycle, thread, or turn and without involving ChatGPT.
6. `MODULE_DONE` waits for explicit user acceptance, and feedback from that screen can resume automation.
7. Restart and uncertain delivery preserve state and require an explicit user decision before any resend or continuation.
8. A completed module releases its middleware-owned thread; a later module can resume that released thread only when `thread/resume` succeeds.

## Implementation status

The first V2 implementation slice is in progress. It replaces the desktop view with a Chinese relay workspace, persists relay modules and full text-message history separately from the historical V1 tables, serializes ChatGPT sends, and distinguishes manual replies from automation replies. A restart changes an unresolved outgoing send to `UNKNOWN` and requires later explicit recovery rather than silently resending it. All global `UNKNOWN` blockers are visible in the relay workspace; for each, the user must explicitly choose either to resend that message or to continue without resending it. The latter records the decision without transmitting the old message, and dispatch resumes only after every `UNKNOWN` has been resolved.

Plain-text terminal control-block parsing, one configured retry, `MODULE_DONE`, and `BLOCKED` are wired into the relay persistence layer. A valid `CODEX_PROMPT` now starts or continues one middleware-owned local App Server process and keeps its created thread alive for later prompts; Codex final text is appended to the same FIFO ChatGPT queue without modification.

The remaining V2 work includes the direct human App Server input-response flow, released-thread resume, browser-history synchronization, and end-to-end browser pilot evidence. None of those incomplete paths may be presented as completed automation.
