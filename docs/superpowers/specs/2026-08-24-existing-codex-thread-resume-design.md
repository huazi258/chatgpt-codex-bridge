# Existing Codex Thread Resume Design

- Date: 2026-08-24
- Status: Design specification
- Scope: define how a new Conversation Relay V2 module can continue an existing Codex thread. This is a design only; it adds no implementation plan or code.

## Product model

The product name is **继续现有 Codex 对话** (Existing Codex Thread Resume). Codex Desktop, CLI, VS Code, App Server, and Bridge threads inhabit one Codex thread space. Bridge never imports, copies, or replays an external thread. It acquires write access to the selected existing thread with `thread/resume(threadId)` and then continues that exact thread.

`resume_thread_id` means the thread a module intends to resume. It does not imply that Bridge created or previously released the thread. A module that resumes a thread is still a new, independent module: it has its own cycles, runtime budget, relay messages, and acceptance history. It inherits only Codex thread context. Older `COMPLETED` and `STOPPED` modules remain terminal.

V2 remains serial: Bridge controls at most one acquired Codex thread/turn at a time. This design does not change ChatGPT control blocks, FIFO, `UNKNOWN`, Codex App Server `requestUserInput` support, or the retired `@@@CODEX_INPUT@@@` protocol.

## Module creation and cwd

The new-module UI presents an explicit choice:

1. 新建 Codex 对话
2. 继续现有 Codex 对话

Bridge must not infer or select an old thread from a working directory, and it must not reopen an old terminal module.

The UI calls `working_directory` **Codex 工作目录**. It is Codex cwd/environment selection, not Git repository management. The first version does not run `git rev-parse` or normalize `/repo` and `/repo/frontend` into one repository.

- New conversation: the user-selected cwd is stored as `Module.working_directory`; later `thread/start` uses it.
- Existing conversation: the user selects cwd, explicitly clicks **刷新对话**, and chooses from `thread/list` entries whose cwd exactly equals the selected path. On creation, the backend revalidates the selected thread; its `cwd` becomes the authoritative `Module.working_directory`.

## Discovery: explicit, metadata-only refresh

Refresh is a read-only, user-initiated operation. Bridge starts a temporary App Server, performs `initialize`, exhausts paginated `thread/list`, filters to the selected cwd and `archived = false`, sorts by `updated_at`/`recency_at` descending, then closes that App Server. The first version explicitly requests `sourceKinds: ["cli", "vscode", "appServer"]`: omitting `sourceKinds` (or sending `[]`) uses the App Server default that discovers only interactive CLI and VS Code threads, so it cannot be relied on to find App Server-created durable threads after an unknown `thread/start` outcome.

The conceptual refresh request is:

```text
thread/list({
  cwd: selected_cwd,
  archived: false,
  sourceKinds: ["cli", "vscode", "appServer"],
  sortKey: "recency_at" or "updated_at",
  sortDirection: "desc"
})
```

Bridge exhausts its pagination. `exec`, `subAgent`, `subAgentReview`, `subAgentCompact`, `subAgentThreadSpawn`, and `subAgentOther` are not ordinary **继续现有 Codex 对话** candidates in the first version. The design does not assume a separate `desktop` source kind; candidate cards display the source metadata actually returned by Codex.

It must not reuse the execution App Server or introduce a discovery service. It must not call `thread/resume`, `thread/start`, `turn/start`, acquire ownership, or reserve a thread.

Creation-time discovery must not call `thread/read(includeTurns=true)`, render a transcript, or use a preview as a substitute name. Candidate cards use only `thread/list` metadata:

- `thread.name`, or “未命名 Codex 对话”;
- source, status, optional `gitInfo.branch`;
- updated/recency time and a short thread ID.

Full UUIDs, origin URLs, commit SHAs, cwd repeats, and historical turns are not default card content.

Candidates from the requested `cli`, `vscode`, and `appServer` sources are shown when they match the selected cwd. Only `idle` and `notLoaded` are selectable. `active` and `systemError` remain visible but disabled. Bridge renders `active` as “当前正在运行，暂不可选择” and renders `systemError` as “Codex 对话当前处于系统错误状态，暂不可选择；请在 Codex 中恢复后刷新。” These are fixed Bridge status explanations; `ThreadStatus.systemError` supplies no detailed error reason. Bridge never takes over an external active turn or attaches it to a module cycle.

## Creation-time validation and reservation

Refresh results are UI snapshots only. When the user creates a module for thread `T1`, the backend re-reads/verifies that `T1` still exists, its cwd equals the chosen cwd, its status is `idle`/`notLoaded`, its registry state is not unavailable, and no other nonterminal module owns or reserves it.

The reservation and module creation are one SQLite transaction:

```text
T1 available -> create Module B(resume_thread_id=T1) -> T1 RESERVED(owner=B) -> commit
```

A conditional update/unique ownership rule prevents two modules claiming `T1`. A failed race fails the entire creation, never falls back to a new thread, and tells the UI to refresh. A created module is lazy: it spawns no App Server, resumes no thread, starts no timer, and consumes no cycle until its first valid `CODEX_PROMPT`.

## Thread registry and module fields

Add `relay_codex_threads` as Bridge’s ownership/risk registry, not a Codex thread catalog. `thread/list` remains the catalog.

| Field | Meaning |
| --- | --- |
| `thread_id` | Codex thread identity. |
| `working_directory` | Exact Codex cwd used for validation. |
| `state` | `RESERVED`, `ACTIVE`, `RELEASED`, or `UNAVAILABLE`. |
| `owner_module_id` | Current Bridge owner when reserved/active. |
| `last_module_id` | Most recent Bridge owner. |
| `updated_at` | Last local ownership/risk update. |

`released_at`, error, and recovery metadata may be added without changing these semantics.

- No row: Bridge has no retained ownership/risk record; current `thread/list` decides eligibility.
- `RESERVED`: a module chose the thread but Bridge has not successfully acquired it.
- `ACTIVE`: Bridge successfully started or resumed and currently owns it.
- `RELEASED`: Bridge previously acquired it and explicit release succeeded.
- `UNAVAILABLE`: Bridge cannot safely assert that reuse is safe (release failure, unknown acquisition, restart of old active ownership).

`relay_modules.resume_thread_id TEXT NULL` records intent. `NULL` means create a new conversation; `T1` means resume `T1`. Existing `codex_thread_id` remains a fact field: it is null until Bridge actually acquires a thread, then contains the successfully started/resumed ID. A never-acquired `RESERVED` record cancelled by termination or target change is removed (or restored to its prior ownership state); it must not be called `RELEASED`.

## Pending prompt, execution, and counters

On accepting a valid `CODEX_PROMPT`, Bridge immediately persists the verbatim prompt in one stable `WAITING_TO_SEND_CODEX` cycle. It does not increment `started_cycles` merely because it received the prompt or resumed a thread.

For `resume_thread_id=T1`, execution preflight is: recheck `T1` eligibility, start the execution App Server, initialize, `thread/resume(T1)`, persist `codex_thread_id=T1` and registry `ACTIVE` only after success, then `turn/start(P1)`. For a new thread, initialize and `thread/start(cwd)` first, then `turn/start(P1)`.

Only a confirmed `turn/start` increments `started_cycles`, changes the cycle to `CODEX_RUNNING`, persists `codex_turn_id`, and starts `module_started_at` on the first turn. Therefore repeated resume attempts consume neither cycle budget nor runtime. The existing start-time budget gate rejects a new turn between cycles; it must use the normal acceptance/budget path and never leave a pending prompt plus `stop_after_turn` half-state.

`P1` is never replaced: failures and recovery retain that same cycle and prompt. No recovery requests a new ChatGPT prompt, creates `P2`, or changes the target thread’s prompt text.

## Failure, uncertainty, recovery, and restart

All recovery states reuse module `RECOVERY_REQUIRED`; cycle status remains the existing seven-state model. A structured recovery reason/block reason distinguishes:

`THREAD_RESUME_FAILED`, `THREAD_RESUME_UNKNOWN`, `THREAD_REACQUIRE_REQUIRED`, `TURN_START_FAILED`, `TURN_START_UNKNOWN`, `THREAD_START_FAILED`, `THREAD_START_UNKNOWN`, and `THREAD_BECAME_ACTIVE_BEFORE_RESUME`.

| Condition | Required result | Allowed recovery |
| --- | --- | --- |
| Resume explicit error | `T1` remains `RESERVED`; retain P1; no counters/timer. | Retry resume, start new thread, terminate. |
| Resume outcome unknown | `T1 -> UNAVAILABLE`; retain P1. | Reacquire same T1, terminate only. |
| T1 became active before Bridge acquired it | retain P1; module recovery. | Refresh/retry after idle, start new thread, terminate. |
| Resume succeeds, turn/start explicit error | T1 stays `ACTIVE`; retain P1; no cycle count/timer. | Retry turn start, terminate. |
| turn/start outcome unknown | P1 might execute; T1 unavailable; retain cycle. | No automatic resend or thread switch; explicit recovery/terminate only. |
| New-thread start explicit error | retain P1; no thread/counter/timer. | Explicit retry new-thread start or terminate. |
| New-thread start unknown | a thread may exist but ID is unknown; retain P1. | Explicitly select an eligible discovered thread, explicitly create a new thread, or terminate; no automatic second thread. |

The recovery command is a single backend-validated `recover_relay_codex(module_id, action)` interface. Actions are `RETRY_RESUME`, `REACQUIRE_THREAD`, `START_NEW_THREAD`, `RETRY_TURN_START`, and `SELECT_EXISTING_THREAD(thread_id)`; persisted recovery reason is authoritative. `THREAD_RESUME_FAILED` permits retry/start-new; unknown resume permits only reacquire; explicit turn-start failure permits retry; unknown turn-start prohibits retry. `THREAD_START_UNKNOWN` permits only the user-selected `SELECT_EXISTING_THREAD(thread_id)`, user-selected `START_NEW_THREAD`, or termination; it must never cause an automatic second `thread/start`. Existing `terminate_relay_module` remains the termination action.

`SELECT_EXISTING_THREAD(thread_id)` is not import or copy. It changes the current module’s intended target only after the backend confirms that the persisted recovery reason permits this action (the first-version required case is `THREAD_START_UNKNOWN`) and then revalidates that the selected thread still exists, has `cwd == Module.working_directory`, is `idle` or `notLoaded`, belongs to a requested candidate source/category, has no registry `RESERVED`/`ACTIVE`/`UNAVAILABLE` blocker, and is not owned or reserved by another nonterminal module. The backend atomically sets `Module.resume_thread_id = T1`, leaves `Module.codex_thread_id = NULL`, and reserves `T1` as `RESERVED(owner_module_id = current Module)`. It retains the original pending cycle and P1 verbatim, does not increment `started_cycles`, does not start the timer, and does not immediately call `thread/resume`. A later explicit execution/recovery action follows the normal existing-thread resume preflight. If reservation loses a race, Bridge neither chooses another thread nor starts one; it leaves the module in recovery and requires a refresh followed by another explicit selection.

On restart, `RESERVED` remains reserved and causes no automatic resume. Old `ACTIVE` becomes `UNAVAILABLE` and its module becomes `RECOVERY_REQUIRED`. No side-effecting request or prompt is replayed automatically.

## Release and old-data safety

Module terminal state does not prove the Codex thread is safely released. Only explicit release success changes `ACTIVE -> RELEASED`, clears owner, and records `last_module_id`. Release failure or uncertainty changes it to `UNAVAILABLE`.

A registry migration may backfill existing `codex_thread_id` as `RELEASED` only when an explicit `CODEX_THREAD_RELEASED` audit exists and no later failure/nonterminal ownership contradicts it. All other historical references backfill conservatively to `UNAVAILABLE`. Migration makes no App Server call or external side effect.

Discovery filters out registry `RESERVED`, `ACTIVE`, and `UNAVAILABLE`; it may show rows with no registry record or `RELEASED`, subject to current Codex status.

## Observability, UX, and acceptance evidence

The existing seven cycle states remain unchanged. `WAITING_TO_SEND_CODEX` represents a persisted prompt that has not reached confirmed `turn/start`; recovery/block reason explains resume/start progress and failure. Channel snapshots retain the active acquired thread and turn only; no extra discovery runtime is represented.

The creation UI uses explicit radio modes, a cwd selector, and a **刷新对话** button. It renders metadata-only selectable/disabled cards and no preview/history. Recovery UI shows only actions allowed by the persisted reason.

Future implementation must test: new-thread regression; discovery across sources and exact cwd; disabled active/system-error candidates; registry filtering and reservation races; stale revalidation; lazy creation; context-preserving resume; P1/cycle/timer boundaries; explicit versus unknown resume/start outcomes; restart and release transitions; terminal-before-first-prompt reservation cleanup; and zero automatic replay of uncertain side effects.

## Non-goals

This design does not normalize Git roots, create a discovery daemon, read thread history, import/copy threads, support multiple active Codex threads, take over external active turns, add App Server human-input middleware, restore `@@@CODEX_INPUT@@@`, alter ChatGPT protocol/FIFO/UNKNOWN, or define future agent-level plain-text decision rules.
