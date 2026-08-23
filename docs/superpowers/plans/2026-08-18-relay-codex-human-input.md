# Codex Human Input Implementation Plan

> Status: Superseded / Cancelled. 本计划对应的 middleware Human Input 功能已撤销，不再是当前执行计划；保留仅作历史记录。未来 agent-level 普通文本替代规则另行设计。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 当运行中的 Codex turn 发出 App Server 用户输入请求时，由 middleware 直接向用户展示并回答原 request；保持相同 cycle/thread/turn，不经过 ChatGPT。

**Spec:** [docs/superpowers/specs/2026-08-18-relay-codex-human-input-design.md](../specs/2026-08-18-relay-codex-human-input-design.md)

## 已核对的 App Server 协议基线

本计划以本机 `codex-cli 0.147.0` 的 `codex app-server generate-json-schema --experimental` 输出为准。该 schema 与设计的核心 wire shape 一致：

```json
{
  "id": "原始 string 或 integer JSON-RPC request id",
  "method": "item/tool/requestUserInput",
  "params": {
    "threadId": "...",
    "turnId": "...",
    "itemId": "...",
    "isBlocking": true,
    "autoResolutionMs": 30000,
    "questions": [
      {
        "id": "question-id",
        "header": "...",
        "question": "...",
        "options": [{ "label": "...", "description": "..." }],
        "isOther": false,
        "isSecret": false
      }
    ]
  }
}
```

`id` 的类型为 string 或 int64，必须以 JSON 值无损保存。`isBlocking` 是当前 schema 的必填字段，`autoResolutionMs` 仍存在但已标注 deprecated；实施时二者均作为兼容元数据保留。response 和最终确认分别为：

```json
{
  "id": "同一个原始 request id",
  "result": {
    "answers": {
      "question-id": { "answers": ["自由文本"] },
      "empty-question-id": { "answers": [] }
    }
  }
}
```

```json
{
  "method": "serverRequest/resolved",
  "params": { "requestId": "同一个原始 request id", "threadId": "..." }
}
```

因此未发现需要改变已批准设计的 schema 冲突。计划中将 `isBlocking` 作为已批准的「实现时需要保留的兼容字段」落实；不会把 `autoResolutionMs` 当作本地过期事实。

## 实施不变量

- `item/tool/requestUserInput` 永远不进入 ChatGPT FIFO、`relay_messages`、`UNKNOWN` recovery 或 automation retry。
- `question.id` 是唯一 wire-level 问题身份；UI 顺序、header、question 与 options 都不是 response key。
- `AnswerInput` 只向原 JSON-RPC `id` 写 response，绝不调用 `turn/start`、创建 cycle 或创建/切换 thread。
- worker 写 stdin 成功仅表示 `RESPONSE_SENT`，input record 只有收到同一 JSON request ID 且同一 thread 的 `serverRequest/resolved` 后才进入 `ANSWERED`。
- secret 原值仅存在于 submit 调用和 worker command 的内存；不得写入 SQLite、`relay_events`、event payload、日志、diagnostic 或 UI notice。
- module runtime 包含等待用户输入的时间；terminate/runtime budget 到期只写既有 `stop_after_turn`，允许当前 input 完成并让同一 turn 自然停止，最终结果不回 ChatGPT。

## Task 1 — 移除旧 ChatGPT `CODEX_INPUT` 协议

**Files**

- Modify `src-tauri/src/relay_protocol.rs`
- Modify `src-tauri/src/lib.rs`
- Modify `src/App.tsx` and `src/App.test.tsx`
- Modify `spikes/chatgpt-extension/content.js`, `background.js`, `manifest.json`, `adapter-version.test.mjs`, `content-adapter-completion.test.mjs`, `background-relay.test.mjs`, and `README.md`

**Interfaces**

- `ControlBlock` keeps only `CodexPrompt(String)`, `ModuleDone`, and `Blocked(String)`.
- `parse_terminal_control_block(reply: &str) -> Result<ControlBlock, String>` removes `input_question_count`.
- Default retry text lists only `@@@CODEX_PROMPT@@@`, `@@@MODULE_DONE@@@`, and `@@@BLOCKED@@@`.
- Content adapter version changes `1.3.2` to `1.3.3` in `content.js`, `background.js`, and `manifest.json`.

- [ ] **RED — Rust parser and UI expectations.** Replace `input_requires_the_pending_question_count` with tests asserting `@@@CODEX_INPUT@@@...@@@END_CODEX_INPUT@@@` returns the ordinary terminal-control validation error. Add a `handle_relay_chatgpt_reply` test that an eligible automation reply containing that obsolete block performs exactly the existing one `CONTROL_RETRY`, increments `invalid_reply_count` once, creates no Codex cycle/thread, and queues the unchanged configured retry template. Update `App.test.tsx` fixtures so retry UI text excludes `CODEX_INPUT`. Run `cargo test relay_protocol -- --nocapture` and the focused relay reply test; expect compilation failure from the old parser signature or assertion failure because the old block is accepted.
- [ ] **RED — extension completion regression.** Add a real JSDOM/MutationObserver `sendMiddlewareMessageV3` scenario in `content-adapter-completion.test.mjs` for delayed complete `CODEX_PROMPT`, delayed `BLOCKED`, and delayed `MODULE_DONE`; update adapter version assertions to require `1.3.3`. Add a static assertion that `relayWrappedControls` does not contain `CODEX_INPUT`. Run `node spikes/chatgpt-extension/content-adapter-completion.test.mjs` and `node spikes/chatgpt-extension/adapter-version.test.mjs`; expect the version assertion and/or obsolete marker assertion to fail on `1.3.2`.
- [ ] **Implement.** Delete `ControlBlock::CodexInput` and `parse_numbered_answers`; remove pending-input-count lookup and its match arm from `handle_relay_chatgpt_reply`; call the simplified parser everywhere. Change the retry template source and React text fixtures. Remove the input start/end pair from `relayWrappedControls`/`relayControlMarkers`, preserving `CODEX_PROMPT`, `BLOCKED`, `MODULE_DONE`, the fresh-node gate, and delayed-DOM logic. Synchronize `1.3.3` through background dispatch expectation, manifest, README, and all adapter test fixture responses.
- [ ] **GREEN.** Run `cargo test relay_protocol -- --nocapture`, `cargo test handle_relay_chatgpt_reply -- --nocapture`, `node --test spikes/chatgpt-extension/*.test.mjs`, `npm test -- --run`, and `npm run build`. Assert obsolete `CODEX_INPUT` retries as invalid automation protocol while the three supported delayed control blocks still complete normally.
- [ ] **Commit.** `refactor: remove ChatGPT Codex input control block`

## Task 2 — 建立纯 App Server human-input protocol adapter

**Files**

- Create `src-tauri/src/relay_codex_input.rs`
- Modify `src-tauri/src/lib.rs` to declare `mod relay_codex_input;` and import its public types/functions

**Interfaces**

- `RelayCodexInputRequestId` preserves `Value::String` or `Value::Number` and exposes canonical JSON storage without coercing type.
- `RelayCodexInputQuestionOption { label, description }`
- `RelayCodexInputQuestion { id, header, question, options, is_other, is_secret }`
- `RelayCodexInputRequestPayload { request_id, thread_id, turn_id, item_id, is_blocking, auto_resolution_ms, questions, raw_compatibility_fields }`
- `RelayCodexInputSubmission { answers_by_question_id: BTreeMap<String, Vec<String>>, secret_question_ids: BTreeSet<String> }`
- `RelayCodexInputResolved { request_id, thread_id }`
- Pure functions: `parse_request_user_input`, `parse_server_request_resolved`, `build_request_user_input_response`, and `validate_submission`.

- [ ] **RED.** Create unit tests in `relay_codex_input.rs` for: string and integer request IDs serializing back to the exact original JSON; required `threadId`, `turnId`, `itemId`, `isBlocking`, and nonempty unique `questions`; full question metadata preservation; arbitrary free text despite options; duplicate or unknown submitted IDs rejected; every known question ID required in the frontend submission map; `""` mapped to `[]`; and a response containing only the original `id` plus `result.answers[question.id].answers`. Add an assertion that output contains no `method: "turn/start"`. Run `cargo test relay_codex_input -- --nocapture`; expect unresolved imports/module failure.
- [ ] **Implement.** Parse only `method == "item/tool/requestUserInput"`, accepting JSON-RPC `id` as string/int64 exactly. Preserve request-level `isBlocking`, optional `autoResolutionMs`, and a raw object of unrecognized compatibility fields. Reject duplicate question IDs before any DB access. Construct the exact response `{ "id": original_id, "result": { "answers": { id: { "answers": values } } } }`; turn each UI empty string into `Vec::new()`. Parse `serverRequest/resolved` only when both `params.requestId` and `params.threadId` are present and retain their JSON/string identity.
- [ ] **GREEN.** Run `cargo test relay_codex_input -- --nocapture` and `cargo test --lib -- --nocapture`. Verify all tests inspect JSON values structurally, including numeric IDs, rather than stringifying them.
- [ ] **Commit.** `feat: model Codex human input protocol`

## Task 3 — 新增 input request 持久化与恢复

**Files**

- Create `src-tauri/migrations/006_codex_human_input.sql`
- Modify `src-tauri/src/lib.rs`

**Interfaces and SQL**

- Add `const CODEX_HUMAN_INPUT_SCHEMA: &str = include_str!("../migrations/006_codex_human_input.sql");` and execute it in `create_connection()` and test `relay_connection()`.
- Add `RelayCodexInputRequestRecord` with all persisted columns and `RelayCodexInputStatus` constants `PENDING | ANSWERING | ANSWERED | INTERRUPTED | EXPIRED`.
- Add helpers `insert_relay_codex_input_request_in`, `list_relay_codex_input_requests_in`, `claim_relay_codex_input_request_in`, `mark_relay_codex_input_response_sent_in`, `resolve_relay_codex_input_request_in`, `expire_relay_codex_input_request_in`, and `interrupt_unfinished_relay_codex_input_requests_in`.

```sql
CREATE TABLE relay_codex_input_requests (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES relay_modules(id) ON DELETE CASCADE,
  cycle_id TEXT NOT NULL REFERENCES relay_codex_cycles(id) ON DELETE CASCADE,
  codex_thread_id TEXT NOT NULL,
  codex_turn_id TEXT NOT NULL,
  app_server_request_id_json TEXT NOT NULL,
  questions_json TEXT NOT NULL,
  answers_json TEXT,
  secret_answer_status_json TEXT NOT NULL,
  is_blocking INTEGER NOT NULL CHECK (is_blocking IN (0, 1)),
  auto_resolution_ms INTEGER,
  request_compatibility_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('PENDING','ANSWERING','ANSWERED','INTERRUPTED','EXPIRED')),
  error_text TEXT,
  created_at TEXT NOT NULL,
  submitted_at TEXT,
  answered_at TEXT,
  interrupted_at TEXT,
  expired_at TEXT,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_relay_codex_input_one_actionable_turn
  ON relay_codex_input_requests(module_id, codex_turn_id)
  WHERE status IN ('PENDING','ANSWERING');
CREATE UNIQUE INDEX idx_relay_codex_input_active_request_id
  ON relay_codex_input_requests(app_server_request_id_json)
  WHERE status IN ('PENDING','ANSWERING');
```

- [ ] **RED.** Add in-memory `relay_connection()` tests that fail before migration/helper implementation: insert/read preserves JSON request-ID type, questions, required `is_blocking`, `auto_resolution_ms`, and request compatibility metadata; a second actionable request for the same module/turn violates the unique index; secret raw text is absent from `answers_json`, `relay_events`, and returned record; startup recovery turns both `PENDING` and `ANSWERING` into `INTERRUPTED` and the module into `RECOVERY_REQUIRED`; terminal records remain unchanged. Run `cargo test relay_codex_input_request -- --nocapture`; expect missing table/helper failures.
- [ ] **Implement.** Create migration 006 with the listed table/indexes and indexes for `(module_id, created_at DESC)` and status. Serialize `app_server_request_id_json` with exact JSON, required `is_blocking`, optional `auto_resolution_ms`, the remaining compatibility object, nonsecret answer maps only, secret `question.id -> provided` flags only, and non-sensitive event details. Call recovery immediately after `mark_uncertain_relay_deliveries` during `.setup`; transition each unfinished record and append `CODEX_INPUT_INTERRUPTED` within one transaction. Ensure all helper transitions append exactly one of `CODEX_INPUT_REQUEST_RECEIVED`, `CODEX_INPUT_ANSWER_SUBMITTED`, `CODEX_INPUT_ANSWERED`, `CODEX_INPUT_EXPIRED`, `CODEX_INPUT_INTERRUPTED`, or `CODEX_INPUT_ANSWER_FAILED` without answer raw values.
- [ ] **GREEN.** Run `cargo test relay_codex_input_request -- --nocapture` and `cargo test --lib -- --nocapture`. Assert raw secret sentinel strings cannot be found in any SQLite column or event detail and recovery creates no `relay_messages`.
- [ ] **Commit.** `feat: persist Codex human input requests`

## Task 4 — 把 `requestUserInput` 接入常驻 worker

**Files**

- Modify `src-tauri/src/lib.rs`
- Modify `src-tauri/src/relay_codex_input.rs`

**Interfaces**

- Extend `RelayCodexCommand` with `AnswerInput { input_request_id: String, submission: RelayCodexInputSubmission, acknowledgement: std_mpsc::Sender<RelayCodexInputAnswerResult> }`.
- `RelayCodexInputAnswerResult` is exactly `ResponseSent | Expired(String) | TransportFailure(String)`; it has no `Answered` variant.
- Worker-local `ActiveRelayCodexInputRequest { input_request_id, request_id, thread_id, turn_id, cycle_id, response_sent }`.
- Add `relay_codex_input_request_received`, `relay_codex_input_resolved`, `relay_codex_input_expired`, and `relay_codex_input_interrupted` orchestration helpers in `lib.rs`.

- [ ] **RED.** Build a deterministic worker seam around the existing `events.recv_timeout(100ms)` loop: a fixture feeds initialize/thread/turn events, an `item/tool/requestUserInput` JSON-RPC request, a command, then `serverRequest/resolved`. Test that commands are checked with `try_recv` while stdout events continue flowing; receipt persists `PENDING`, emits the existing `relay-codex` refresh event, retains the same active cycle/thread/turn, and writes neither `turn/start` nor `relay_messages`. Test response write leaves status `ANSWERING`; only a matching `requestId` plus `threadId` resolution makes it `ANSWERED`; a resolved-before-submit input becomes `EXPIRED`; an unmatched resolved event has no effect. Run `cargo test relay_codex_worker_input -- --nocapture`; expect missing command/seam failures.
- [ ] **Implement.** Keep the stdout reader thread and existing 100ms event loop. Before each blocking receive, drain `commands.try_recv()`; after each input event, continue draining commands instead of waiting synchronously. Parse `item/tool/requestUserInput` through Task 2, require equality with current active `cycle_id`, worker `thread_id`, and active turn ID, persist `PENDING`, set worker-local active identity, and emit the existing `relay-codex` refresh event after every input state change (`PENDING`, `ANSWERING`, `ANSWERED`, `EXPIRED`, and `INTERRUPTED`). On `AnswerInput`, require exact middleware input ID, thread, turn, and original request JSON ID; write the Task 2 response through `send_rpc`, mark only response-sent/`ANSWERING`, then acknowledge `ResponseSent`. On matching `serverRequest/resolved`, atomically choose `PENDING -> EXPIRED` or `ANSWERING && response_sent -> ANSWERED`, clear worker-local pending identity, and emit refresh. On child/stdout/transport failure with an unresolved response-sent identity, interrupt it and set module recovery before invoking normal Codex failure cleanup. On turn completed/failure, expire a still-actionable request with a non-sensitive reason and reject late events without changing terminal module phase.
- [ ] **GREEN.** Run `cargo test relay_codex_worker_input -- --nocapture` and `cargo test --lib -- --nocapture`. Assert fixture stdout after a request remains observable, no test output includes the secret sentinel, and the only `turn/start` frame is the one that began the original cycle.
- [ ] **Commit.** `feat: handle Codex input requests in active turns`

## Task 5 — 后端用户提交 API

**Files**

- Modify `src-tauri/src/lib.rs`

**Interfaces**

- `#[derive(Deserialize)] struct RelayCodexInputAnswerInput { question_id: String, answer: String }`
- `#[tauri::command] fn list_relay_codex_input_requests(module_id: String, state: State<'_, AppState>) -> Result<Vec<RelayCodexInputRequestRecord>, String>`
- `#[tauri::command] fn submit_relay_codex_input(input_request_id: String, answers: Vec<RelayCodexInputAnswerInput>, app: AppHandle) -> Result<RelayCodexInputRequestRecord, String>`

- [ ] **RED.** Add command-level tests with a fake session sender: a valid vector covering every stored `question.id` atomically changes only the record to `ANSWERING`, sends one `AnswerInput`, preserves original JSON request ID, and does not call `turn/start`; duplicate, unknown, or missing question IDs are rejected; an empty string maps to empty answer list; front-end metadata/request ID is not accepted as input; `ANSWERING`, `EXPIRED`, and `INTERRUPTED` reject duplicate submit while `ANSWERED` returns the existing record without sending. Test secret submission with a sentinel and assert it is present only in captured in-memory command, never DB/event/returned record/error. Run `cargo test submit_relay_codex_input -- --nocapture`; expect missing command/helper failures.
- [ ] **Implement.** Read questions and request identity solely from `relay_codex_input_requests`; normalize the explicit UI array only after checking each stored `question.id` occurs exactly once. Treat missing IDs as invalid but `answer == ""` as valid. In a DB transaction claim `PENDING -> ANSWERING`, write nonsecret mappings and secret `provided` flags, append `CODEX_INPUT_ANSWER_SUBMITTED`, then send exactly one `AnswerInput` to the matching in-memory session. If command send or acknowledgement is transport failure, persist `INTERRUPTED`, set `RECOVERY_REQUIRED`, and never retry. If it reports expired, transition to `EXPIRED`. Register both commands in `tauri::generate_handler!`; neither command reads/writes `relay_messages`, dispatcher state, `invalid_reply_count`, or recovery blockers.
- [ ] **GREEN.** Run `cargo test submit_relay_codex_input -- --nocapture`, `cargo test list_relay_codex_input_requests -- --nocapture`, and `cargo test --lib -- --nocapture`. Assert command return after stdin write is `ANSWERING`, not `ANSWERED`.
- [ ] **Commit.** `feat: submit human answers to Codex`

## Task 6 — Codex 通道可观测性

**Files**

- Modify `src-tauri/src/lib.rs`
- Modify `src/relay-observability.ts`
- Modify `src/components/GlobalChannelStatus.tsx`
- Modify `src/App.tsx` and `src/App.test.tsx`

**Interfaces**

- `RelayCodexChannelSnapshot.status` becomes `IDLE | RUNNING | WAITING_FOR_USER_INPUT` without changing the seven `RelayCodexCycleStatus` values.
- Add nullable `active_input_request_id` and `input_status` to the Rust snapshot and `activeInputRequestId`, `inputStatus` to `RelayChannelSnapshot` TypeScript.

- [ ] **RED.** Add Rust snapshot tests for `PENDING` and `ANSWERING` showing `WAITING_FOR_USER_INPUT`, correct module/cycle/thread/turn/request ID/status, and cycle status still `CODEX_RUNNING`. Add React tests that render the new Chinese label while the ChatGPT snapshot remains `IDLE`, `IN_FLIGHT`, or `RECOVERY_BLOCKED` exactly as supplied. Run `cargo test relay_channel_snapshot -- --nocapture` and `npm test -- --run`; expect missing TypeScript fields/status handling and Rust assertions to fail.
- [ ] **Implement.** In `get_relay_channel_snapshot`, query actionable input request rows for the active Codex cycle before returning normal `RUNNING`; emit the existing `relay-codex` refresh event after each input state change. Extend `relay-observability.ts`, display `等待用户输入` with request status in `GlobalChannelStatus`, and refresh snapshot/cycle/input state in `App.tsx` for `relay-codex` and module-action events. Do not add a cycle status and do not modify `RelayChatGptChannelSnapshot` calculation.
- [ ] **GREEN.** Run `cargo test relay_channel_snapshot -- --nocapture`, `npm test -- --run`, and `npm run build`. Assert input events do not create a ChatGPT queue item or alter ChatGPT channel state.
- [ ] **Commit.** `feat: expose Codex human input status`

## Task 7 — runtime budget / terminate 与 pending input 收尾

**Files**

- Modify `src-tauri/src/lib.rs`
- Modify focused Rust tests in `src-tauri/src/lib.rs`

**Interfaces**

- Add `mark_relay_runtime_budget_reached_in(connection, module_id, now) -> Result<bool, String>`; it atomically sets `stop_after_turn = 1` only once and appends `RUNTIME_BUDGET_REACHED` only for the first transition.
- Reuse existing `terminate_relay_module_with_active_turn_in` and `StoppedAfterTurn` completion path; do not add a second terminal phase/state machine.

- [ ] **RED.** Add clock-controlled tests: an active `CODEX_RUNNING` cycle with a `PENDING` request crosses `module_started_at + max_runtime_minutes`, remains answerable, gets one `RUNTIME_BUDGET_REACHED`, and does not emit a `TO_CHATGPT` result after the turn completes; a second worker loop tick adds no duplicate event. Add a manual terminate test showing `terminate_relay_module` sets `stop_after_turn` yet `submit_relay_codex_input` remains allowed for the matching current request. Test final completion stores `FROM_CODEX`, sets `STOPPED`, clears `stop_after_turn`, preserves thread ID, and releases runtime. Run `cargo test pending_input_stop -- --nocapture`; expect the missing deadline gate or current submit rejection to fail.
- [ ] **Implement.** At the worker loop's 100ms cadence, calculate deadline only for its active module/turn and call the atomic helper. Gate new `CODEX_PROMPT` starts with existing `stop_after_turn` checks; do not gate `AnswerInput` for the active stored request. Leave pending input record actionable until resolved/expired/interrupted. Route completed turn through existing stopped-after-turn branch so final text remains history/cycle data but no outbound ChatGPT result is created; schedule the existing release logic. Make the event detail non-sensitive and emit it once.
- [ ] **GREEN.** Run `cargo test pending_input_stop -- --nocapture`, `cargo test terminate_running_relay_codex -- --nocapture`, and `cargo test --lib -- --nocapture`. Assert no `turn/interrupt`, `child.kill`, new cycle, or ChatGPT outbound is produced by budget expiry/terminate while input is pending.
- [ ] **Commit.** `feat: finish pending Codex input before stopping`

## Task 8 — React 人工输入 UI

**Files**

- Create `src/components/CodexHumanInputPanel.tsx`
- Create `src/relay-codex-input.ts`
- Modify `src/App.tsx`, `src/App.test.tsx`, and `src/styles.css`

**Interfaces**

- TypeScript `RelayCodexInputQuestion`, `RelayCodexInputRequest`, `RelayCodexInputAnswerInput`, and status union mirror Task 3 DTOs with `answersJson` never containing secret raw values.
- `CodexHumanInputPanel` props: `{ request, stopAfterTurn, onSubmit }`; `onSubmit` receives `Array<{ questionId: string; answer: string }>`.

- [ ] **RED.** Add component and App integration tests with a mocked Tauri `invoke`: selecting a module calls `list_relay_codex_input_requests`; the existing `relay-codex` listener refreshes Codex input requests, Codex cycles, channel snapshot, and necessary module state; questions render in stored order with header, text, reference options, and one textarea each; no radio/select requirement exists. Test empty normal answer submits `""`; test secret uses a password-style control, clears its value immediately when submission starts, and its sentinel never appears in rendered notices/errors after refresh. Test `ANSWERING` disables submit with `答案已发送，正在等待 Codex 确认`; `ANSWERED` history omits secret text; `EXPIRED`/`INTERRUPTED` have no submit button; `stopAfterTurn` shows the exact stop message. Run `npm test -- --run`; expect module/component import and invocation assertions to fail.
- [ ] **Implement.** Add serializable UI DTOs and the focused panel. Fetch input requests inside the existing selected-module refresh path, and after submit invoke `submit_relay_codex_input`, then refresh input/cycle/channel/module. Use standard textareas for normal questions and a non-rehydrating password input for secret questions; keep secret state component-local, clear it before awaiting invoke, and never place it in `notice`/`error`. Render `options` as reference text only. Style pending, answering, terminal, and stop-after-turn states in `styles.css` with current Chinese workspace conventions.
- [ ] **GREEN.** Run `npm test -- --run` and `npm run build`. Assert no mocked call uses `queue_relay_message`, no input value is supplied to ChatGPT, and secret sentinel is absent from DOM after submit.
- [ ] **Commit.** `feat: add Codex human input panel`

## Task 9 — 回归、协议 fixture 与最终验证

**Files**

- Modify `src-tauri/src/relay_codex_input.rs` tests and `src-tauri/src/lib.rs` worker/DB tests
- Modify `src/App.test.tsx`
- Modify extension tests under `spikes/chatgpt-extension/` only where Task 1 fixture coverage requires it

- [ ] **RED.** Add a reusable fixture sequence containing: same-cycle/thread/turn request #1, its response/resolved, request #2, and completion. Add assertions covering every acceptance listed below before final implementation; run the exact targeted command for each failing group and record the first missing behavior rather than treating a harness timeout as a pass.
- [ ] **Implement.** Complete only gaps exposed by the fixture suite. The suite must assert all of the following:
  1. one question request → response → matching resolved remains the same cycle/thread/turn;
  2. multi-question response keys are exact `question.id` values;
  3. two sequential requests in one turn do not increment `started_cycles`;
  4. `AnswerInput` emits zero `turn/start` frames;
  5. input creates zero `relay_messages` rows;
  6. secret sentinel occurs in zero SQLite/event/log/diagnostic values;
  7. empty answer serializes as `[]`;
  8. resolved-before-submit becomes `EXPIRED` with zero response frame;
  9. response-sent-but-unconfirmed child/transport failure becomes `INTERRUPTED` and recovery-required;
  10. restart changes `PENDING`/`ANSWERING` to `INTERRUPTED` plus `RECOVERY_REQUIRED`;
  11. terminate with pending input permits the one answer, then saves result without ChatGPT return and stops;
  12. runtime expiry during pending input has the same safe completion path and one budget event;
  13. obsolete ChatGPT `CODEX_INPUT` follows normal invalid-control retry;
  14. extension delayed `CODEX_PROMPT`, `BLOCKED`, and `MODULE_DONE` completion tests pass on adapter `1.3.3`.
- [ ] **GREEN — complete automatic suite.** Run exactly:

  ```powershell
  node --test spikes/chatgpt-extension/*.test.mjs
  npm test
  npm run build
  cargo test --manifest-path src-tauri/Cargo.toml
  cargo check --manifest-path src-tauri/Cargo.toml
  ```

  Record each exit code. Also run `git diff --check` before committing.
- [ ] **Commit.** `test: cover Codex human input lifecycle`

## REAL_CHROME_TAURI_E2E checklist — NOT_RUN at plan creation

This checklist is manual validation only and must not be reported as passed until performed in an actual paired Chrome/Tauri session:

1. Reload the extension built at adapter `1.3.3`, refresh/rebind the ChatGPT tab, and confirm the adapter check reports `1.3.3`.
2. Cause a single-question `item/tool/requestUserInput`; verify one local panel, submit a free-text answer, observe `ANSWERING`, then only transition after matching `serverRequest/resolved`.
3. Cause a multi-question request; answer in display order and verify server receives keys by `question.id`, including a blank answer as `[]`.
4. Cause two successive input requests in one Codex turn; verify one cycle/thread/turn and no ChatGPT message while either panel is active.
5. Use a secret question; verify its answer is not shown after submit or restart and cannot be resent automatically.
6. Request terminate while input is pending; submit the current answer, then verify `STOPPED`, retained final history, and no result sent to ChatGPT.
7. Let runtime expire while input is pending; verify the same safe completion behavior and exactly one budget notice.
8. Deliver `serverRequest/resolved` before submission and restart with a pending request; verify respectively `EXPIRED` and `INTERRUPTED/RECOVERY_REQUIRED`, with no late or automatic response.

## Plan self-review

- **Spec coverage:** Tasks 1–9 cover removal of public `CODEX_INPUT`, schema parsing, exact `question.id` mapping, persistence/recovery, nonblocking worker operation, submit API, observability, stop semantics, UI, and full regression/E2E evidence.
- **Placeholder scan:** 未发现未决占位词、跨任务省略引用或未具体化的错误处理；每个 task 都列出文件、具体接口、RED 行为、GREEN 命令和提交边界。
- **Type consistency:** App Server request IDs remain JSON string/int64 values; `app_server_request_id_json` and `serverRequest/resolved.params.requestId` compare structurally, while `question.id` remains string. `ANSWERED` is reachable only from matching resolved; `EXPIRED` is pre-submit/cleared; `INTERRUPTED` is restart or unconfirmed transport; secret raw values have no persisted type.
- **Wiring consistency:** all worker input-state transitions reuse the existing `relay-codex` refresh event; React listens only to that event and refreshes input requests, cycles, channel snapshot, and required module state. The plan defines no second Codex refresh event.
- **Product boundary:** no task routes Codex input to ChatGPT, creates a new cycle/turn/thread, changes the seven cycle states, weakens `UNKNOWN`, or changes accepted module termination/acceptance semantics.
