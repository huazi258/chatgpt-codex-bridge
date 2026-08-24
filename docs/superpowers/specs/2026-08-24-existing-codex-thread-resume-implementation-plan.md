# Existing Codex Thread Resume — Implementation Plan

- Date: 2026-08-24
- Status: Implementation plan for the frozen [Existing Codex Thread Resume design](2026-08-24-existing-codex-thread-resume-design.md)
- Baseline: `554a055b504faaf3af1964746db27e95cb1c24cd`
- Scope: allow a new Conversation Relay V2 module to explicitly reserve and later resume one eligible existing Codex thread, while preserving the current new-thread flow.

## Planning constraints and current-code map

This plan implements only existing-thread resume. It does not change public ChatGPT control blocks, `UNKNOWN` recovery, the browser adapter, App Server `requestUserInput`, or the one-runtime serialisation rule.

The current relay implementation is concentrated in `src-tauri/src/lib.rs`:

- `create_relay_module`, `RelayModuleInput`, `RelayModuleRecord`, `relay_row_to_module`, `get_relay_module`, and `list_relay_modules` define module persistence and the Tauri creation API.
- `handle_relay_chatgpt_reply` creates `TO_CODEX` history and `relay_codex_cycles`; `create_relay_codex_cycle` currently receives `module.started_cycles + 1`.
- `start_or_continue_relay_codex_turn`, `mark_relay_codex_turn_starting_in`, `relay_codex_worker`, `relay_codex_thread_ready`, and `relay_codex_turn_started` create a new App Server thread and currently mark the turn running before a `turn/start` RPC result is confirmed.
- `RelayCodexSession`, `RelayCodexCommand`, `release_relay_codex_session`, `accept_relay_module`, `terminate_relay_module`, `complete_relay_codex_turn_in`, and `relay_codex_failed` own runtime/release/terminal paths.
- Startup calls `pause_unfinished_orchestrations` and `mark_uncertain_relay_deliveries` in `.setup`; no relay-thread ownership restart recovery exists.
- Tests live in the `#[cfg(test)]` module in `src-tauri/src/lib.rs`, using `relay_connection()` and `insert_relay_module()` with in-memory SQLite. There is no relay-worker fake App Server process. `send_rpc(&mut impl Write, Value)` is reusable for a capture writer, but `relay_codex_worker` directly owns `Command`, stdin, and stdout.
- The UI is in `src/App.tsx`, with the create form in the `!selected` branch. `src/App.test.tsx` mocks `invoke` and `listen`; `src/styles.css` owns UI styling. `src/relay-observability.ts` supplies cycle/channel types.

The Chrome extension transports only ChatGPT relay messages. Existing-thread resume begins after a persisted `CODEX_PROMPT`; no `spikes/chatgpt-extension/*` source, protocol, manifest, or adapter-version change is planned.

## Fixed persistence and state rules

### Module target input

Do not expose a free-form `resume_thread_id` field to the browser. Replace creation input with a tagged, mutually exclusive target:

```text
RelayModuleInput {
  name, workingDirectory, maxCycles, maxRuntimeMinutes, retryTemplate,
  codexThreadTarget: { mode: "NEW" }
                   | { mode: "EXISTING", threadId: string }
}
```

Rust derives `relay_modules.resume_thread_id`: `NULL` for `NEW`, backend-validated T1 for `EXISTING`. `codex_thread_id` is `NULL` at creation in both modes and is written only after successful `thread/start` or `thread/resume`.

### Thread registry

Add `relay_codex_threads` as an ownership/risk registry, not a cached thread-list catalog.

| Column | Rule |
| --- | --- |
| `thread_id TEXT PRIMARY KEY` | Codex thread identity; one row means local ownership/risk is global across modules. |
| `working_directory TEXT NOT NULL` | Exact validated Codex cwd. |
| `state TEXT NOT NULL` | `RESERVED`, `ACTIVE`, `RELEASED`, or `UNAVAILABLE`. |
| `owner_module_id TEXT NULL REFERENCES relay_modules(id)` | Required for `RESERVED`/`ACTIVE`; null for `RELEASED`/`UNAVAILABLE`. |
| `last_module_id TEXT NULL REFERENCES relay_modules(id)` | Most recent Bridge owner. |
| `reservation_previous_state TEXT NULL` | `NONE` for a reservation created from no row; `RELEASED` when temporarily replacing a released row; null outside `RESERVED`. |
| `updated_at TEXT NOT NULL` | Last local state change. |

Use a state/owner `CHECK` plus indexes on `owner_module_id` and `(working_directory, state)`. The thread primary key and a single SQLite `IMMEDIATE` transaction prevent concurrent ownership. A partial unique index on `owner_module_id` for `RESERVED`/`ACTIVE` additionally defends the one-runtime/one-target association.

Add to `relay_modules`:

- `resume_thread_id TEXT NULL` for existing-thread intent;
- `codex_recovery_reason TEXT NULL` for a machine-readable `RelayCodexRecoveryReason` while phase is `RECOVERY_REQUIRED`.

Existing `relay_codex_cycles.error_text` remains the non-sensitive cycle block/error summary. Pre-turn recovery leaves P1 in `WAITING_TO_SEND_CODEX` with this text; only an actual started-turn failure uses `FAILED`.

### Recovery types and action matrix

Add serde/Tauri enums in `src-tauri/src/lib.rs`:

- `RelayCodexThreadState`: `RESERVED`, `ACTIVE`, `RELEASED`, `UNAVAILABLE`;
- `RelayCodexRecoveryReason`: `THREAD_RESUME_FAILED`, `THREAD_RESUME_UNKNOWN`, `THREAD_REACQUIRE_REQUIRED`, `TURN_START_FAILED`, `TURN_START_UNKNOWN`, `THREAD_START_FAILED`, `THREAD_START_UNKNOWN`, `THREAD_BECAME_ACTIVE_BEFORE_RESUME`;
- `RelayCodexRecoveryAction`: `RETRY_RESUME`, `REACQUIRE_THREAD`, `START_NEW_THREAD`, `RETRY_TURN_START`, and `SELECT_EXISTING_THREAD { thread_id }`.

The backend validates persisted reason, module/cycle/registry state, and ownership before side effects.

| Recovery reason | Allowed actions |
| --- | --- |
| `THREAD_RESUME_FAILED` | `RETRY_RESUME`, `START_NEW_THREAD`, terminate |
| `THREAD_RESUME_UNKNOWN` | `REACQUIRE_THREAD`, terminate |
| `THREAD_REACQUIRE_REQUIRED` | `REACQUIRE_THREAD`, terminate |
| `TURN_START_FAILED` | `RETRY_TURN_START`, terminate |
| `TURN_START_UNKNOWN` | terminate only; no retry or thread switch |
| `THREAD_START_FAILED` | `START_NEW_THREAD`, terminate |
| `THREAD_START_UNKNOWN` | `SELECT_EXISTING_THREAD(thread_id)`, `START_NEW_THREAD`, terminate |
| `THREAD_BECAME_ACTIVE_BEFORE_RESUME` | recheck/retry same intended thread after idle, `START_NEW_THREAD`, terminate |

`START_NEW_THREAD` is explicit: revert/delete a never-acquired reservation according to `reservation_previous_state`, set `resume_thread_id` null, retain P1/cycle/prompt, and start no thread until normal execution. `REACQUIRE_THREAD` never sends P1 or `turn/start` automatically after an uncertain side effect.

## Ordered implementation tasks

Each task starts with the stated failing test, implements the smallest coherent slice, runs its focused command, and lands as one reviewable commit. Later tasks do not amend earlier commits.

For each task, a dimension not listed is intentionally unchanged: backend-only tasks have no frontend/UI work, frontend-only tasks have no schema/Rust API work, and every state transition named below is persisted transactionally before a UI refresh event is emitted.

### Task 1 — Schema, legacy backfill, and registry persistence

**Goal.** Add durable thread intent/ownership and recovery-reason storage without changing creation or starting an App Server.

**Files.** Add `src-tauri/migrations/006_existing_codex_thread_resume.sql`; update `src-tauri/src/lib.rs` to include `EXISTING_CODEX_THREAD_RESUME_SCHEMA`, add the one-version migration gate in `create_connection`, and extend test fixtures. No frontend files change in this task.

**Database and Rust changes.** Current `create_connection` executes each included schema on every launch, and SQLite does not make `ALTER TABLE ... ADD COLUMN` idempotent. Therefore add a minimal local `schema_migrations` version gate: after the existing repeat-safe schemas run, it starts an `IMMEDIATE` transaction, checks whether version `006_existing_codex_thread_resume` is recorded, executes the new SQL and its legacy backfill only when absent, records the version, and commits. It must not retrofit or rewrite migrations 001–005. Add both `relay_modules` columns and `relay_codex_threads` with the table contract above. Add `RelayCodexThreadState`, `RelayCodexThreadRecord`, `RelayCodexRecoveryReason`, registry row mapping, `get/list/upsert` helpers, and a transactional state-change helper. Keep them private until API tasks expose them.

**Legacy backfill.** For every existing `codex_thread_id`: any nonterminal reference is `UNAVAILABLE`; a terminal-only reference is `RELEASED` only if `CODEX_THREAD_RELEASED` is later than every `CODEX_THREAD_RELEASE_FAILED` and no other nonterminal module references the same ID; all other, duplicate, or conflicting references are `UNAVAILABLE`. Set `last_module_id` from the deterministically newest referencing module. The migration makes no App Server call and does not infer release from module phase alone.

**Tests.** In the current `lib.rs` test module, cover state/owner checks, no-row/released reservation provenance, terminal/nonterminal/conclusive-release/conflict backfill, and migration idempotence. Assert no App Server RPC occurs.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_codex_thread -- --nocapture`.

**Done/dependency.** Old databases upgrade deterministically and one local owner is enforced. Depends only on baseline. **Commit:** `feat: persist relay Codex thread ownership`.

### Task 2 — Target DTOs and confirmed-start accounting helpers

**Goal.** Make creation input unambiguous and move cycle/counter/timer accounting to confirmed turn start.

**Files.** `src-tauri/src/lib.rs` only.

**Rust API/type changes.** Extend `RelayModuleInput`, `RelayModuleRecord`, `relay_row_to_module`, `get_relay_module`, every module `SELECT`, and `list_relay_modules` with `resume_thread_id` and `codex_recovery_reason`. Add a tagged `RelayCodexThreadTargetInput`; validation rejects blank existing IDs and cannot represent `NEW` plus an ID.

Replace `mark_relay_codex_turn_starting_in` with a transaction helper called only after a matching successful `turn/start` response. It verifies P1 is `WAITING_TO_SEND_CODEX`, makes the cycle `CODEX_RUNNING`, writes thread/turn IDs, increments `started_cycles`, sets `module_started_at` only if null, makes phase `CODEX_RUNNING`, and appends `CODEX_TURN_STARTED` atomically. Add helpers to persist/clear recovery reason and P1 `error_text` without changing P1/counter/timer.

**State transition.** Valid `CODEX_PROMPT` still creates one P1 with `cycle_number = started_cycles + 1`; prompt acceptance, `thread/start` success, and `thread/resume` success do not consume a cycle. Only confirmed `turn/start` does. The existing max-cycle/runtime gate stays before any App Server request, takes the normal `WAITING_FOR_ACCEPTANCE` path, and never sets `stop_after_turn`.

**Tests.** Target variants, invalid input, stable P1 with zero started cycles, null module-start time before confirmation, and one increment/timestamp after confirmed success; keep new-thread cycle/result tests green.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_codex_cycle -- --nocapture`.

**Done/dependency.** No code path consumes a cycle on prompt acceptance or RPC write. Depends on Task 1. **Commit:** `refactor: account relay cycles on confirmed turn start`.

### Task 3 — Temporary, read-only thread discovery

**Goal.** Implement one user-triggered discovery command returning metadata-only exact-cwd candidates and exiting its temporary App Server.

**Files.** `src-tauri/src/lib.rs`; add frontend DTO declarations in `src/relay-thread-resume.ts`; register the command in `tauri::generate_handler!`.

**Backend/API changes.** Add `RelayCodexThreadCandidate` with `thread_id`, nullable `name`, returned `source`, status kind, nullable branch, recency timestamp, `selectable`, and nullable Chinese `disabled_reason`. Add `list_relay_codex_threads_for_cwd` and an internal temporary-session helper next to `codex_command`, `ManagedAppServer`, and `send_rpc`.

It validates the existing exact cwd, spawns `codex app-server`, sends `initialize`, then exhausts paginated `thread/list` with `archived: false`, `sourceKinds: ["cli", "vscode", "appServer"]`, `sortKey: "recency_at"`, and `sortDirection: "desc"`. It uses increasing request IDs until `nextCursor` is null. It filters exact `thread.cwd`, allowed sources, and registry state. It never calls `thread/read`, `thread/resume`, `thread/start`, `turn/start`, reserve, or display preview/transcript. It does not enter `AppState`/channel snapshots.

`idle`/`notLoaded` are selectable. `active` is disabled with “当前正在运行，暂不可选择”. `systemError` is disabled with “Codex 对话当前处于系统错误状态，暂不可选择；请在 Codex 中恢复后刷新。”; this is Bridge-generated, not a remote error detail. `RESERVED`, `ACTIVE`, and `UNAVAILABLE` registry rows disable candidates; no row/`RELEASED` defers to current Codex status.

**Test seam.** Current relay code has no fake App Server. Extract only the JSON-lines request/response scheduler needed by discovery and the worker, accepting a `BufRead` source and `Write` sink. Use `Cursor<Vec<u8>>` plus captured `Vec<u8>` in the existing `lib.rs` test module, not a real `codex` process.

**Tests.** Explicit source filter/recency request, multi-page exhaustion, exact-cwd/source filter, metadata-only DTO, every status reason, registry filtering, and zero resume/start/turn methods in captured frames.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_codex_thread_discovery -- --nocapture`.

**Done/dependency.** Discovery is finite/read-only and AppServer-created durable threads are discoverable. Depends on Tasks 1–2. **Commit:** `feat: discover eligible Codex threads`.

### Task 4 — Module creation and atomic reservation

**Goal.** Create lazy `NEW` and `EXISTING(T1)` modules without races or side effects.

**Files.** `src-tauri/src/lib.rs` (`create_relay_module`, validation, mappings, tests); `src/relay-thread-resume.ts` for final invoke payload/record types.

**Backend flow.** Keep `Path::is_dir`. `NEW` inserts null target and null actual thread without spawning an App Server. `EXISTING(T1)` first invokes Task 3 temporary revalidation: exact cwd, allowed source, idle/notLoaded, and no registry/local owner blocker. In a SQLite transaction, reread registry ownership, create module, set `resume_thread_id = T1`, keep `codex_thread_id = NULL`, and insert/update T1 as `RESERVED(owner=current)` with provenance. Commit all or nothing.

Stale validation or a failed conditional reservation returns a Chinese refresh/reselect error, creates no partial module, and never falls back to new thread.

**Tests.** New-mode laziness, valid existing creation, stale/mismatched cwd, active/systemError/unsupported-source rejection, `UNAVAILABLE` rejection, two contenders for T1 with one commit, and rollback/no module on race. Assert zero cycles/timer and no execution request.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml create_relay_module -- --nocapture`.

**Done/dependency.** SQLite—not browser state—is final reservation authority. Depends on Task 3. **Commit:** `feat: reserve existing Codex threads for relay modules`.

### Task 5 — Confirmed-RPC relay worker state machine

**Goal.** Refactor `relay_codex_worker` enough to distinguish explicit RPC errors from unknown transport/process outcomes.

**Files.** `src-tauri/src/lib.rs` (`RelayCodexSession`, `RelayCodexCommand`, `start_or_continue_relay_codex_turn`, `relay_codex_worker`, `relay_codex_thread_ready`, `relay_codex_turn_started`, `relay_codex_failed`, inline tests).

**Changes.** Keep one session/runtime, but add acquisition mode (`StartNew` or `ResumeExisting(T1)`), outstanding request IDs, pending P1, and a written-but-unconfirmed `turn/start` marker. Move the current `relay_codex_turn_started` call from immediately after `send_rpc` to its matching successful RPC response. Matching JSON-RPC errors are explicit; EOF, child exit, malformed output, and post-write transport loss are unknown. Keep `item/agentMessage/delta`, `item/completed`, `turn/completed`, `Release`, stdout priority, and `stop_after_turn` behavior intact. P1 is never replayed by the loop.

**Tests.** With Task 3 scripted transport: RPC write alone does not count/start timer; matching result does; explicit error versus missing response is classified; foreign/duplicate response IDs do not advance state; no extra `turn/start` frame occurs.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_codex_worker -- --nocapture`.

**Done/dependency.** Start success versus uncertain side effect is evidence-based. Depends on Task 2 and Task 3 test seam. **Commit:** `refactor: confirm relay Codex RPC outcomes`.

### Task 6 — Lazy new-thread lifecycle under confirmed outcomes

**Goal.** Preserve current `resume_thread_id = NULL` functionality on the refactored worker.

**Files.** `src-tauri/src/lib.rs`.

**State transition.** `start_or_continue_relay_codex_turn` reads P1 and runs the existing start-time budget gate before spawning. For a new target it initializes, issues `thread/start` with the module exact `working_directory`, waits for its matching response, then allows one `turn/start(P1)`. Explicit start success atomically writes `codex_thread_id`, creates registry `ACTIVE(owner=current,last=current)`, and proceeds. Explicit error leaves P1 pending and records `THREAD_START_FAILED`; unknown records `THREAD_START_UNKNOWN`, invents no ID, and never sends a second thread/start. Neither begins timer nor increments cycles until confirmed turn/start.

**Tests.** Existing new-thread happy path, explicit/unknown thread-start, no automatic second start, confirmed accounting, and budget expiration between cycles. Assert only confirmed acquisition yields `ACTIVE`.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_new_thread -- --nocapture`.

**Done/dependency.** New modules remain lazy/conservative and acceptance/termination regressions remain valid. Depends on Task 5. **Commit:** `feat: make new relay thread start recoverable`.

### Task 7 — Lazy existing-thread resume preflight

**Goal.** Use a reserved `resume_thread_id` only on P1, retain the exact selected context, and never take over external active work.

**Files.** `src-tauri/src/lib.rs`.

**State transition.** Before any execution RPC, rerun Task 3 revalidation against `resume_thread_id`. `active` becomes `THREAD_BECAME_ACTIVE_BEFORE_RESUME`; `systemError`, missing, wrong cwd, unsupported source, or local registry contradiction becomes actionable `RECOVERY_REQUIRED` without `thread/resume`.

For still-eligible T1, start execution App Server, initialize, send exactly one `thread/resume({threadId: T1})`, and wait for its matching result. Explicit success sets `relay_modules.codex_thread_id = T1`, transitions T1 `RESERVED -> ACTIVE`, clears reservation provenance/recovery reason, then allows `turn/start(P1)`. Explicit error keeps T1 `RESERVED`, P1/cycle/counter/timer unchanged, and records `THREAD_RESUME_FAILED`. Unknown resume sets T1 `UNAVAILABLE`, clears owner, retains P1, and records `THREAD_RESUME_UNKNOWN`; it does not select another thread or start one.

After successful acquisition, explicit `turn/start` error keeps T1 `ACTIVE` and records `TURN_START_FAILED`; unknown turn-start makes T1 `UNAVAILABLE` and records `TURN_START_UNKNOWN`. Neither resends P1, increments cycles, or changes target automatically.

**Tests.** Script complete resume success and assert one T1 is used for resume, module fact, registry, cycle, and turn. Cover explicit/unknown resume, became-active preflight, explicit/unknown turn-start, no fallback, and unchanged P1/counter/timer.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_existing_thread -- --nocapture`.

**Done/dependency.** Existing context is resumed only after explicit successful ownership acquisition. Depends on Tasks 4–6. **Commit:** `feat: resume reserved Codex threads for relay`.

### Task 8 — Unified persisted recovery command

**Goal.** Expose the frozen recovery matrix through one backend command; frontend code must not own the state machine.

**Files.** `src-tauri/src/lib.rs` (new `recover_relay_codex` command, validation helpers, command registration); `src/relay-thread-resume.ts` for typed recovery payloads.

**Backend behavior.** Read module, active P1, `codex_recovery_reason`, registry, terminal/`UNKNOWN`/runtime ownership state inside a transaction before deciding whether an action is legal. Reject every unlisted action without mutation.

- `RETRY_RESUME` and `RETRY_TURN_START` retain P1 exactly and initiate only their known-safe request. `TURN_START_UNKNOWN` cannot retry.
- `START_NEW_THREAD` explicitly reverts/deletes a never-acquired reservation according to `reservation_previous_state`, sets `resume_thread_id = NULL`, retains P1, and uses Task 6 only after this deliberate action.
- `SELECT_EXISTING_THREAD(thread_id)` is allowed at least for `THREAD_START_UNKNOWN`. Re-run temporary discovery validation, then atomically set intended target, keep actual ID null, reserve T1, retain the original P1/cycle ID/number/prompt, and do not call `thread/resume` in that transaction. A lost race remains recovery and requires refresh/reselection.
- `REACQUIRE_THREAD` performs only the explicit revalidation/acquisition safety step. After an uncertain side effect it never sends P1 or `turn/start` automatically and keeps clear persisted recovery state until another allowed action exists.

Accepted recovery actions append non-sensitive audit data and emit the existing `relay-codex` refresh event. Do not add a browser protocol or new Tauri event name.

**Tests.** Table-drive every reason/action allow/deny pair; assert denied actions send no RPC. Cover SELECT success, wrong cwd/status/source, registry race, P1/cycle/counter/timer preservation, and START_NEW_THREAD reservation reversion. Assert `UNKNOWN` remains untouched and no prompt is automatically sent.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml recover_relay_codex -- --nocapture`.

**Done/dependency.** Recovery is explicit, persisted-state validated, and cannot silently create a second thread or replay an uncertain side effect. Depends on Task 7. **Commit:** `feat: recover relay Codex thread acquisition`.

### Task 9 — Restart, release, acceptance, and termination registry integration

**Goal.** Keep registry ownership truthful across restart, acceptance, idle/running termination, runtime budget completion, and release failures.

**Files.** `src-tauri/src/lib.rs` (`.setup`, release helpers, `accept_relay_module`, `terminate_relay_module`, `complete_relay_codex_turn_in`, `relay_codex_failed`, tests).

**State transition.** Add startup registry recovery after schema setup and before the UI state is managed. `RESERVED` remains reserved and never resumes. `ACTIVE` becomes `UNAVAILABLE`, owner clears, and its nonterminal module becomes `RECOVERY_REQUIRED` with `THREAD_REACQUIRE_REQUIRED`. Any unresolved running/turn-start-side-effect cycle gets an explicit non-replayable recovery/block record; no P1 is sent.

On matching release acknowledgement, update `ACTIVE -> RELEASED`, clear owner, set `last_module_id`, then append `CODEX_THREAD_RELEASED`. On release failure/timeout/unknown, update `UNAVAILABLE` and append release failure audit. Terminal module phase never substitutes for this transition.

On termination before acquisition, delete a `NONE` reservation or restore a `RELEASED` predecessor; do not fabricate `RELEASED`. Preserve current acceptance, idle terminate, running `stop_after_turn`, final-result suppression, and session-match rules. Terminal transitions keep `resume_thread_id` and actual `codex_thread_id` facts and cannot release another module’s runtime.

**Tests.** Restart reserved no-op; restart active-to-unavailable; nonterminal-only recovery marking; no automatic resume/replay; release success/failure; terminal-before-first-prompt cleanup for both provenance values; acceptance/idle terminate/running terminate and current `UNKNOWN` regressions.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_codex_thread_restart -- --nocapture`.

**Done/dependency.** Every local ownership claim has a safe explicit release or unavailable outcome. Depends on Tasks 1 and 8. **Commit:** `feat: recover and release relay Codex thread ownership`.

### Task 10 — Structured resume observability

**Goal.** Expose intended versus acquired thread and recovery state without changing the seven cycle statuses or channel semantics.

**Files.** `src-tauri/src/lib.rs`; `src/relay-observability.ts`.

**API/model changes.** Extend serialized `RelayModuleRecord` and TypeScript `RelayModule` with nullable `resumeThreadId` and `codexRecoveryReason`. Add read-only `get_relay_codex_thread_state(module_id)` returning intended ID, actual `codex_thread_id`, registry state/owner summary, recovery reason, P1/cycle ID/number, and backend-generated actionable Chinese summary. React must not reconstruct it from `relay_events`.

Keep `relay_codex_cycles` on seven fixed statuses. Add only structured recovery error/block reason handling. `get_relay_channel_snapshot` continues to report only actually acquired/running runtime; temporary discovery is never active Codex. All resume/recovery/release transitions emit the existing `relay-codex` event, not `relay-codex-status`.

**Tests.** Serialized fields, pending-P1 summary, intended/actual distinction, each recovery reason’s Chinese action summary, active-channel behavior, and discovery absence from snapshot.

**Command.** `cargo test --manifest-path src-tauri/Cargo.toml relay_codex_thread_state -- --nocapture`.

**Done/dependency.** UI receives structured truth and cannot confuse selected/discovered with acquired runtime. Depends on Tasks 7–9. **Commit:** `feat: expose relay Codex thread recovery state`.

### Task 11 — Module-create mode selector and discovery cards

**Goal.** Extend the existing creation form without a second form or creation path.

**Files.** `src/App.tsx`; `src/relay-thread-resume.ts`; `src/App.test.tsx`; `src/styles.css`.

**Frontend changes.** Retain `createModule` as the only submit handler and add controlled **Codex 对话** radio modes `NEW`/`EXISTING`. Both retain existing name/budgets/retry template and **Codex 工作目录**. Existing mode adds **刷新对话**, invokes `list_relay_codex_threads_for_cwd`, renders pending/error/empty state, and clears stale selection whenever cwd or refresh changes.

Render metadata-only cards: `name ?? “未命名 Codex 对话”`, source, status, branch, recency/update time, short ID. Never render preview/transcript, `thread/read` result, full UUID by default, Git SHA, or origin URL. `active`/`systemError` remain visible/disabled with fixed Chinese strings; registry-disabled cards use backend reason. Existing-mode submit remains disabled without a selectable selection. Serialize exactly `codexThreadTarget`; backend stale/race errors use existing notice. Successful creation continues selecting the new module and leaving creation view.

**Tests.** Mode selector, discovery loading/error/success, metadata-only/no-preview rendering, unnamed fallback, active/systemError strings, registry-disabled card, selection, cwd invalidation, `NEW`/`EXISTING` payloads, stale-create error, and existing multiple-module creation regression.

**Command.** `npm test -- --run src/App.test.tsx`.

**Done/dependency.** Users deliberately select one eligible existing thread; normal new module creation remains intact. Depends on Tasks 3–4 and 10. **Commit:** `feat: choose an existing Codex thread for relay modules`.

### Task 12 — Recovery UX and module-level resume visibility

**Goal.** Show only backend-authorized recovery choices in the existing workspace.

**Files.** Add `src/components/RelayCodexRecoveryPanel.tsx`; update `src/App.tsx`, `src/App.test.tsx`, and `src/styles.css`.

**Frontend changes.** In the current module summary show intended thread, actual acquired thread, registry state, P1/cycle, and backend recovery summary. Render recovery panel only for `RECOVERY_REQUIRED`, invoke `recover_relay_codex`, and map actions to **重试继续原对话**, **重新取得原对话**, **新建 Codex 对话**, **重新发送 turn/start**, **刷新对话并选择现有 thread**, plus existing terminate. `THREAD_START_UNKNOWN` reuses Task 11 cards and sends SELECT only after explicit selection.

Prevent duplicate clicks while busy. After accepted action, refresh modules/cycles/channel/thread state and discovery data through existing `relay-codex`; frontend never changes DB state itself.

**Tests.** Recovery reason/action matrix rendering, prohibited-action absence, command payload, backend-denial notice, THREAD_START_UNKNOWN refresh/select UX, P1 display, and no history/preview. Keep acceptance/termination/UNKNOWN tests green.

**Command.** `npm test -- --run src/App.test.tsx`.

**Done/dependency.** `RECOVERY_REQUIRED` is an explicit safe action surface, not an opaque phase. Depends on Tasks 8–11. **Commit:** `feat: show relay Codex thread recovery actions`.

### Task 13 — Full regression and manual E2E evidence

**Goal.** Prove the capability without claiming browser/desktop E2E until it has actually run.

**Files.** Extend only test files changed above: `src-tauri/src/lib.rs` and `src/App.test.tsx`. Do not modify `spikes/chatgpt-extension/*`; run them to prove isolation.

**Automated matrix.**

1. Upgrade fixture through migrations 004/005/006: backfill and idempotence.
2. Scripted App Server worker fixture: existing module → resume exact T1 → confirmed turn/start → final result → FIFO result; confirm context ID and cycle 1.
3. New-thread fixture through accounting, acceptance/feedback, idle/running terminate, release success/failure, and global `UNKNOWN` regressions.
4. Every unknown start/resume/turn outcome has zero automatic second side effect; every SELECT preserves P1/cycle/counter/timer.
5. React suite covers both creation modes, metadata-only discovery, disabled status, recovery matrix, observability, and clean ChatGPT timeline.

**Commands.** Run `node --test spikes/chatgpt-extension/*.test.mjs`, `npm test`, `npm run build`, `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`, and `git diff --check`.

**Manual Codex E2E checklist.**

1. Create idle CLI-, VS Code-, and AppServer-created threads in the same cwd; refresh finds all three, but a different cwd does not appear.
2. Confirm active is visible/disabled with “当前正在运行，暂不可选择”; confirm systemError is visible/disabled with Bridge’s fixed recovery text.
3. Create an existing-thread module, send automation, verify exact selected ID after `thread/resume`, and ask Codex to use prior context to prove continuation rather than copy.
4. Verify its first started cycle is 1 and later prompt uses the same ID.
5. Accept/release, then resume that same thread in a later module only after release success.
6. Terminate before P1 and verify no execution App Server starts; no-row reservation disappears and released predecessor restores to RELEASED.
7. Fault-inject explicit/unknown start/resume/turn paths; verify no automatic resend/second thread and reason-specific recovery options.
8. Repeat normal new-thread P1 flow without regression.

**Done/dependency.** All commands pass and manual results are recorded as evidence or explicitly `NOT_RUN`; a visible thread card alone is not resume E2E. Depends on Tasks 1–12. **Commit:** `test: cover existing Codex thread resume`.

## Scope guards

This plan does not implement or change browser-history sync; App Server `requestUserInput`; `@@@CODEX_INPUT@@@`; ChatGPT control-block grammar; Chrome extension/background/content adapter behavior; ChatGPT FIFO/retry/UNKNOWN policy; Git-root normalization or repository/branch management; history/transcript/preview UI; thread deletion/archive; a persistent discovery daemon; multiple runtimes or external active-turn takeover; exec/sub-agent/review source resume; or automatic retry after any uncertain `thread/start`, `thread/resume`, or `turn/start` side effect.

## Design consistency result

No blocking contradiction was found between frozen design and current code. The implementation gap is explicit: current code starts only new threads, counts cycles/timer before confirmed `turn/start`, has no registry, and does not classify App Server RPC outcomes. The ordered tasks replace those gaps while preserving acceptance/termination, seven-cycle observability, and uncertain-delivery semantics.
