# Codex Communication Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every `CODEX_PROMPT` cycle observable from the desktop UI from prompt receipt through Codex completion and eventual ChatGPT delivery, while preserving the existing global FIFO and uncertain-delivery safety rules.

**Architecture:** Add a durable `relay_codex_cycles` model as the single source of truth for Codex-cycle state, link each completed cycle to exactly one existing `relay_messages` outbound ChatGPT message, and expose two structured Tauri queries: cycle history for the selected module and a global ChatGPT/Codex channel snapshot. The React UI renders those structures in a separate Codex communication panel plus a compact global channel status; it never reconstructs lifecycle state from event-detail strings.

**Tech Stack:** Rust + Tauri 2, rusqlite/SQLite migrations, React 19 + TypeScript, Vitest/Testing Library, existing Chrome extension adapter unchanged.

## Global Constraints

- Preserve Conversation Relay V2 global strict FIFO for `TO_CHATGPT`.
- Preserve at most one ChatGPT reply in flight.
- Preserve one middleware-owned Codex thread/active turn semantics; do not add Codex concurrency.
- `UNKNOWN` delivery is never auto-resend; recovery remains explicit.
- Do not change `CODEX_PROMPT`, `MODULE_DONE`, `BLOCKED`, or `CODEX_INPUT` protocol semantics.
- Keep the existing ChatGPT timeline pure: no Codex lifecycle events are inserted as fake ChatGPT messages.
- Codex final text must be visible in the middleware immediately after it is received, even if ChatGPT delivery is blocked.
- One valid `CODEX_PROMPT` creates one cycle; one Codex completion creates one and only one outbound ChatGPT result message.
- Do not expose pairing secrets, cookies, passwords, Git credentials, or raw App Server secrets.
- No claim of browser E2E success unless a real Chrome/Tauri run is completed.

---

## File Structure

**Create**
- `src-tauri/migrations/005_codex_communication_observability.sql`
- `src/relay-observability.ts`
- `src/components/GlobalChannelStatus.tsx`
- `src/components/CodexCommunicationPanel.tsx`
- `src/components/CodexCycleCard.tsx`

**Modify**
- `src-tauri/src/lib.rs`
- `src/App.tsx`
- `src/App.test.tsx`
- `src/styles.css`

**Do not modify**
- `spikes/chatgpt-extension/*` unless a failing test proves a new browser-adapter defect.
- App Server protocol semantics or ChatGPT control-block grammar.

---

### Task 1: Add the durable Codex-cycle model

**Files:**
- Create: `src-tauri/migrations/005_codex_communication_observability.sql`
- Modify: `src-tauri/src/lib.rs`
- Test: Rust tests in `src-tauri/src/lib.rs`

**Interfaces:**
```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayCodexCycleRecord {
    id: String,
    module_id: String,
    cycle_number: i64,
    status: String,
    prompt_text: String,
    codex_thread_id: Option<String>,
    codex_turn_id: Option<String>,
    result_text: Option<String>,
    outbound_chatgpt_message_id: Option<String>,
    error_text: Option<String>,
    created_at: String,
    codex_started_at: Option<String>,
    codex_completed_at: Option<String>,
    relay_queued_at: Option<String>,
    relay_delivered_at: Option<String>,
    updated_at: String,
    block_reason: Option<String>,
}

fn create_relay_codex_cycle(
    connection: &Connection,
    module_id: &str,
    cycle_number: i64,
    prompt_text: &str,
) -> Result<RelayCodexCycleRecord, String>;

fn get_relay_codex_cycle_by_id(
    connection: &Connection,
    cycle_id: &str,
) -> Result<Option<RelayCodexCycleRecord>, String>;

fn get_relay_codex_cycle_by_outbound_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<RelayCodexCycleRecord>, String>;
```

- [ ] **Step 1: Write migration 005**

```sql
CREATE TABLE IF NOT EXISTS relay_codex_cycles (
  id TEXT PRIMARY KEY NOT NULL,
  module_id TEXT NOT NULL REFERENCES relay_modules(id) ON DELETE CASCADE,
  cycle_number INTEGER NOT NULL CHECK (cycle_number > 0),
  status TEXT NOT NULL CHECK (
    status IN (
      'WAITING_TO_SEND_CODEX',
      'CODEX_RUNNING',
      'CODEX_COMPLETED',
      'WAITING_FOR_CHATGPT',
      'SENDING_TO_CHATGPT',
      'DELIVERED_TO_CHATGPT',
      'FAILED'
    )
  ),
  prompt_text TEXT NOT NULL,
  codex_thread_id TEXT,
  codex_turn_id TEXT,
  result_text TEXT,
  outbound_chatgpt_message_id TEXT UNIQUE REFERENCES relay_messages(id) ON DELETE SET NULL,
  error_text TEXT,
  created_at TEXT NOT NULL,
  codex_started_at TEXT,
  codex_completed_at TEXT,
  relay_queued_at TEXT,
  relay_delivered_at TEXT,
  updated_at TEXT NOT NULL,
  UNIQUE(module_id, cycle_number)
);

CREATE INDEX IF NOT EXISTS idx_relay_codex_cycles_module_cycle
  ON relay_codex_cycles(module_id, cycle_number DESC);

CREATE INDEX IF NOT EXISTS idx_relay_codex_cycles_status
  ON relay_codex_cycles(status);

CREATE UNIQUE INDEX IF NOT EXISTS idx_relay_codex_cycles_single_running
  ON relay_codex_cycles(status)
  WHERE status = 'CODEX_RUNNING';
```

- [ ] **Step 2: Register migration 005** immediately after the existing 004 migration.
- [ ] **Step 3: Write failing Rust tests** for `(module_id, cycle_number)` uniqueness and one outbound message per cycle.
- [ ] **Step 4: Run** `cargo test relay_codex_cycle -- --nocapture` and confirm failure.
- [ ] **Step 5: Implement record mapper and creation/read helpers** using parameterized SQL and UTC RFC3339 timestamps; creation stores `WAITING_TO_SEND_CODEX`, all later fields `NULL`, and does not create a ChatGPT message.
- [ ] **Step 6: Run** `cargo test relay_codex_cycle -- --nocapture`.
- [ ] **Step 7: Run** `cargo test` and `cargo check`.
- [ ] **Step 8: Commit**
```powershell
git add src-tauri/migrations/005_codex_communication_observability.sql src-tauri/src/lib.rs
git commit -m "feat: persist relay Codex cycles"
```

---

### Task 2: Wire Codex execution and result queueing into the cycle state machine

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Test: Rust tests in `src-tauri/src/lib.rs`

**Interfaces:**
```rust
fn mark_relay_codex_turn_started(
    connection: &Connection,
    cycle_id: &str,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<(), String>;

fn mark_relay_codex_result_received(
    connection: &Connection,
    cycle_id: &str,
    result_text: &str,
) -> Result<(), String>;

fn queue_relay_codex_result_to_chatgpt(
    connection: &Connection,
    cycle_id: &str,
) -> Result<String, String>;

fn fail_relay_codex_cycle(
    connection: &Connection,
    cycle_id: &str,
    error_text: &str,
) -> Result<(), String>;
```

- [ ] **Step 1: Write a failing lifecycle test** asserting:
```text
WAITING_TO_SEND_CODEX
→ CODEX_RUNNING
→ CODEX_COMPLETED
→ WAITING_FOR_CHATGPT
```
and `result_text == "RELAY_E2E_OK"`, exactly one `TO_CHATGPT` result exists, its ID equals `outbound_chatgpt_message_id`, and a second queue call returns the same message ID without duplication.

- [ ] **Step 2: Write a failing failure-path test** asserting a start/turn failure stores `FAILED`, `error_text`, no result, and no outbound message.
- [ ] **Step 3: Run** `cargo test relay_codex_lifecycle -- --nocapture` and confirm failure.
- [ ] **Step 4: Implement transactional lifecycle helpers**:
  - creation → `WAITING_TO_SEND_CODEX`;
  - turn start → `CODEX_RUNNING`, thread/turn IDs and `codex_started_at`;
  - final text → `CODEX_COMPLETED`, write-once `result_text` and `codex_completed_at`;
  - result queue → exactly one `TO_CHATGPT/AUTOMATION` message, link its ID, `relay_queued_at`, status `WAITING_FOR_CHATGPT`;
  - pre-result failure → `FAILED`.
- [ ] **Step 5: Append audit events** `CODEX_PROMPT_RECEIVED`, `CODEX_TURN_STARTED`, `CODEX_RESULT_RECEIVED`, `CODEX_RESULT_QUEUED_TO_CHATGPT`, `CODEX_TURN_FAILED`.
- [ ] **Step 6: Integrate into the existing `ControlBlock::CodexPrompt(prompt)` branch**: create the cycle before starting Codex; use existing started-cycle numbering; mark running only once the turn starts; mark the same cycle failed on start failure.
- [ ] **Step 7: Integrate final text**: persist result first, preserve existing `FROM_CODEX` history behavior, queue once, then invoke the existing global FIFO dispatcher. Never rerun Codex because ChatGPT delivery is blocked.
- [ ] **Step 8: Add `relay-codex` event emission** after committed lifecycle changes:
```rust
fn emit_relay_codex_changed(app: &AppHandle, module_id: &str, cycle_id: &str, status: &str) {
    let _ = app.emit(
        "relay-codex",
        json!({"moduleId": module_id, "cycleId": cycle_id, "status": status}),
    );
}
```
- [ ] **Step 9: Run** targeted tests, `cargo test`, `cargo check`.
- [ ] **Step 10: Commit**
```powershell
git add src-tauri/src/lib.rs
git commit -m "feat: track relay Codex lifecycle"
```

---

### Task 3: Link ChatGPT send/recovery states back to the originating Codex cycle

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Test: Rust tests in `src-tauri/src/lib.rs`

**Interface:**
```rust
fn sync_codex_cycle_for_chatgpt_message_state(
    connection: &Connection,
    message_id: &str,
    delivery_state: &str,
    error_text: Option<&str>,
) -> Result<(), String>;
```

Mapping:
```text
QUEUED    → WAITING_FOR_CHATGPT
SENT      → SENDING_TO_CHATGPT
DELIVERED → DELIVERED_TO_CHATGPT
UNKNOWN   → WAITING_FOR_CHATGPT + recovery error_text
FAILED    → FAILED only for explicit continue-without-resend on the linked result
```

- [ ] **Step 1: Write failing tests** for FIFO claim → `SENDING_TO_CHATGPT`, matching `chatgptReply` → `DELIVERED_TO_CHATGPT`, and one each of `CODEX_RESULT_SEND_STARTED` / `CODEX_RESULT_DELIVERED_TO_CHATGPT`.
- [ ] **Step 2: Write failing restart/adapter uncertainty test**: linked result `SENT → UNKNOWN`, cycle returns to `WAITING_FOR_CHATGPT`, `result_text` stays intact, no duplicate `TO_CHATGPT`, no Codex rerun.
- [ ] **Step 3: Write failing explicit recovery tests**: resend reuses the same message ID; “不重发并继续” marks that message and cycle failed without creating/sending a replacement.
- [ ] **Step 4: Run** `cargo test codex_cycle_chatgpt_delivery -- --nocapture` and confirm failure.
- [ ] **Step 5: In `claim_next_relay_message_for_dispatch`**, after `QUEUED → SENT`, invoke the sync helper; non-Codex messages are a no-op.
- [ ] **Step 6: In the accepted matching `chatgptReply` path**, mark the linked outbound Codex-result cycle delivered using the same `requestId` before/alongside parsing the newly received ChatGPT text.
- [ ] **Step 7: In existing uncertain-delivery and recovery paths**, sync on `UNKNOWN`, explicit requeue to `QUEUED`, and explicit continue-without-resend to `FAILED`; never allocate a replacement result row.
- [ ] **Step 8: Run** targeted tests, `cargo test`, `cargo check`.
- [ ] **Step 9: Commit**
```powershell
git add src-tauri/src/lib.rs
git commit -m "feat: correlate Codex results with ChatGPT delivery"
```

---

### Task 4: Add structured cycle queries and global channel snapshot

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Test: Rust tests in `src-tauri/src/lib.rs`

**Interfaces:**
```rust
#[tauri::command]
fn list_relay_codex_cycles(
    state: State<'_, AppState>,
    module_id: String,
) -> Result<Vec<RelayCodexCycleRecord>, String>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayChatGptChannelSnapshot {
    status: String,
    active_module_id: Option<String>,
    active_module_name: Option<String>,
    active_message_id: Option<String>,
    active_kind: Option<String>,
    active_phase: Option<String>,
    recovery_blocker_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayCodexChannelSnapshot {
    status: String,
    active_module_id: Option<String>,
    active_module_name: Option<String>,
    cycle_number: Option<i64>,
    codex_thread_id: Option<String>,
    codex_turn_id: Option<String>,
    cycle_status: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayChannelSnapshot {
    chatgpt: RelayChatGptChannelSnapshot,
    codex: RelayCodexChannelSnapshot,
}

#[tauri::command]
fn get_relay_channel_snapshot(
    state: State<'_, AppState>,
) -> Result<RelayChannelSnapshot, String>;
```

- [ ] **Step 1: Write failing snapshot tests**:
  1. no `UNKNOWN`, no `SENT`, no `CODEX_RUNNING` → both `IDLE`;
  2. one `SENT` → ChatGPT `IN_FLIGHT` with module/message/kind;
  3. any `UNKNOWN` takes precedence → `RECOVERY_BLOCKED`;
  4. one `CODEX_RUNNING` → Codex `RUNNING`;
  5. completed cycle waiting behind another module's `SENT` gets a `block_reason` naming module and message;
  6. any `UNKNOWN` produces a recovery blocker reason.
- [ ] **Step 2: Run** `cargo test relay_channel_snapshot -- --nocapture` and confirm failure.
- [ ] **Step 3: Implement snapshot in one database lock/read** with fixed priority:
```text
ChatGPT: UNKNOWN > SENT > IDLE
Codex: CODEX_RUNNING > IDLE
```
- [ ] **Step 4: Implement `list_relay_codex_cycles`** ordered `cycle_number DESC` and compute `block_reason` only from structured cycle/message/snapshot state:
  - `FAILED` → `error_text`;
  - linked `UNKNOWN` → `回传结果不确定，等待人工恢复。`;
  - recovery blocked → `存在待人工处理的不确定送达消息（N 条）。`;
  - another module owns `SENT` → `ChatGPT 通道当前被模块「<name>」占用（消息 <id>）。`;
  - otherwise waiting → `等待全局 FIFO 调度。`;
  - sending → `等待 ChatGPT 完成回复。`.
- [ ] **Step 5: Register both Tauri commands** in the existing `generate_handler!`.
- [ ] **Step 6: Run** targeted tests, `cargo test`, `cargo check`.
- [ ] **Step 7: Commit**
```powershell
git add src-tauri/src/lib.rs
git commit -m "feat: expose relay channel observability"
```

---

### Task 5: Build isolated frontend observability components

**Files:**
- Create: `src/relay-observability.ts`
- Create: `src/components/GlobalChannelStatus.tsx`
- Create: `src/components/CodexCycleCard.tsx`
- Create: `src/components/CodexCommunicationPanel.tsx`
- Modify/Test: `src/App.test.tsx`

**Frontend types:**
```ts
export type CodexCycleStatus =
  | 'WAITING_TO_SEND_CODEX'
  | 'CODEX_RUNNING'
  | 'CODEX_COMPLETED'
  | 'WAITING_FOR_CHATGPT'
  | 'SENDING_TO_CHATGPT'
  | 'DELIVERED_TO_CHATGPT'
  | 'FAILED';

export interface RelayCodexCycle {
  id: string;
  moduleId: string;
  cycleNumber: number;
  status: CodexCycleStatus;
  promptText: string;
  codexThreadId?: string | null;
  codexTurnId?: string | null;
  resultText?: string | null;
  outboundChatgptMessageId?: string | null;
  errorText?: string | null;
  createdAt: string;
  codexStartedAt?: string | null;
  codexCompletedAt?: string | null;
  relayQueuedAt?: string | null;
  relayDeliveredAt?: string | null;
  updatedAt: string;
  blockReason?: string | null;
}

export interface RelayChannelSnapshot {
  chatgpt: {
    status: 'IDLE' | 'IN_FLIGHT' | 'RECOVERY_BLOCKED';
    activeModuleId?: string | null;
    activeModuleName?: string | null;
    activeMessageId?: string | null;
    activeKind?: 'MANUAL' | 'AUTOMATION' | 'SYSTEM' | null;
    activePhase?: string | null;
    recoveryBlockerCount: number;
  };
  codex: {
    status: 'IDLE' | 'RUNNING';
    activeModuleId?: string | null;
    activeModuleName?: string | null;
    cycleNumber?: number | null;
    codexThreadId?: string | null;
    codexTurnId?: string | null;
    cycleStatus?: CodexCycleStatus | null;
  };
}
```

Status labels:
```ts
WAITING_TO_SEND_CODEX: '等待发送 Codex'
CODEX_RUNNING: 'Codex 运行中'
CODEX_COMPLETED: 'Codex 已完成'
WAITING_FOR_CHATGPT: '等待回传 ChatGPT'
SENDING_TO_CHATGPT: '回传 ChatGPT 中'
DELIVERED_TO_CHATGPT: '回传完成'
FAILED: '失败'
```

- [ ] **Step 1: Write failing component tests** for busy ChatGPT + occupying module, recovery blocker count, running Codex module/cycle/thread, cycle prompt/thread/result, missing turn ID `尚未获得`, block reason, and failure state without success decoration.
- [ ] **Step 2: Run** `npm test -- --run` and confirm failure.
- [ ] **Step 3: Implement `codexCycleStatusLabel`** with an exhaustive `switch`.
- [ ] **Step 4: Implement `GlobalChannelStatus`** with two compact cards; no action buttons.
- [ ] **Step 5: Implement `CodexCycleCard`** always showing cycle/status/prompt, conditionally thread/result/outbound ID/block reason/error, and `<pre>` for exact prompt/result.
- [ ] **Step 6: Implement `CodexCommunicationPanel`** with loading, empty state `尚未开始 Codex 循环。`, and backend-order cards.
- [ ] **Step 7: Run** `npm test -- --run` and `npm run build`.
- [ ] **Step 8: Commit**
```powershell
git add src/relay-observability.ts src/components src/App.test.tsx
git commit -m "feat: add Codex communication components"
```

---

### Task 6: Wire observability into the desktop workspace

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/App.test.tsx`
- Modify: `src/styles.css`

**Consumes:**
```text
list_relay_codex_cycles({ moduleId })
get_relay_channel_snapshot()
relay-codex event
```

- [ ] **Step 1: Extend App integration tests first**: mock one completed cycle containing `RELAY_E2E_OK`, ChatGPT busy with another module, and assert global status, Codex panel, result, blocking module, and unchanged `常驻 ChatGPT 对话`. Assert ChatGPT `.message-history` does not contain synthetic lifecycle rows.
- [ ] **Step 2: Run** `npm test -- --run` and confirm failure.
- [ ] **Step 3: Add App state**
```ts
const [codexCycles, setCodexCycles] = useState<RelayCodexCycle[]>([]);
const [channelSnapshot, setChannelSnapshot] = useState<RelayChannelSnapshot | null>(null);
```
- [ ] **Step 4: Add refresh functions**
```ts
async function refreshCodexCycles(moduleId = selectedId) {
  if (!moduleId) {
    setCodexCycles([]);
    return;
  }
  setCodexCycles(
    await invoke<RelayCodexCycle[]>('list_relay_codex_cycles', { moduleId }),
  );
}

async function refreshChannelSnapshot() {
  setChannelSnapshot(
    await invoke<RelayChannelSnapshot>('get_relay_channel_snapshot'),
  );
}
```
- [ ] **Step 5: Refresh structured observability** on initial load, selected-module change, existing `chatgpt-status`, existing `relay-control`, and new `relay-codex`; clean up all listeners.
- [ ] **Step 6: Render** `GlobalChannelStatus` above module summary and `CodexCommunicationPanel` before the existing ChatGPT conversation. Do not feed `relay_events` into the ChatGPT timeline.
- [ ] **Step 7: Add focused CSS** for `.global-channel-status`, `.channel-card`, `.codex-communication`, `.codex-cycle-card`, `.cycle-progress`, `.cycle-block-reason`; ensure `<pre>` wraps and no new dependency is introduced.
- [ ] **Step 8: Run** `npm test -- --run`, `npm run build`, then `cargo test`, `cargo check`.
- [ ] **Step 9: Commit**
```powershell
git add src/App.tsx src/App.test.tsx src/styles.css
git commit -m "feat: show Codex relay observability"
```

---

### Task 7: Full regression verification and real E2E handoff

**Files:** No feature files unless verification exposes a defect.

- [ ] **Step 1: Extension regression**
```powershell
node .\spikes\chatgpt-extension\protocol-text.test.mjs
node .\spikes\chatgpt-extension\adapter-version.test.mjs
node .\spikes\chatgpt-extension\background-relay.test.mjs
```
Expected: exit 0.

- [ ] **Step 2: Frontend**
```powershell
npm test -- --run
npm run build
```
Expected: pass.

- [ ] **Step 3: Rust**
```powershell
Set-Location src-tauri
cargo test
cargo check
Set-Location ..
```
Expected: pass.

- [ ] **Step 4: Migration regression** against a safe existing-database fixture/copy: migration creates `relay_codex_cycles` without changing pre-existing `UNKNOWN`, `QUEUED`, or `SENT` rows.

- [ ] **Step 5: Real Chrome/Tauri E2E**
Require extension smoke:
```text
CHATGPT_EXTENSION_SMOKE_OK
```
Then use:
```text
@@@CODEX_PROMPT@@@
只回复完全一致的一行：RELAY_E2E_OK
不要运行命令，不要读取或修改文件。
@@@END_CODEX_PROMPT@@@
```
The UI must visibly progress:
```text
收到 CODEX_PROMPT
→ Codex 运行中
→ Codex 已完成：RELAY_E2E_OK
→ 等待/开始回传 ChatGPT
→ 回传完成
```

- [ ] **Step 6: Cross-module contention**: while another module owns ChatGPT `SENT`, completed cycle must show `等待回传 ChatGPT`, occupying module name/message ID, while preserving visible `RELAY_E2E_OK`.

- [ ] **Step 7: `UNKNOWN` recovery**: global status `RECOVERY_BLOCKED`, blocker count visible, result remains visible, no auto-resend, all blockers resolved → FIFO resumes exactly once.

- [ ] **Step 8: Commit only verified fixes**, if any:
```powershell
git add <verified-fix-files>
git commit -m "fix: complete Codex observability verification"
```

- [ ] **Step 9: Push and report**
```powershell
git push
git log --oneline -7
```
Report exact automated results, whether real E2E ran, and final commit SHAs.

---

## Plan Self-Review

**Spec coverage:** durable cycles (Task 1), lifecycle/result visibility (Task 2), ChatGPT delivery/recovery correlation (Task 3), structured snapshots and cross-module blocking reasons (Task 4), separate Codex UI (Task 5–6), pure ChatGPT timeline (Task 5–6 tests), restart/no-auto-resend safety (Task 3/7), full verification and honest E2E status (Task 7).

**Placeholder scan:** no TODO/TBD or unspecified “add tests” steps.

**Type consistency:** backend/frontend statuses match exactly:
`WAITING_TO_SEND_CODEX`, `CODEX_RUNNING`, `CODEX_COMPLETED`, `WAITING_FOR_CHATGPT`, `SENDING_TO_CHATGPT`, `DELIVERED_TO_CHATGPT`, `FAILED`.
