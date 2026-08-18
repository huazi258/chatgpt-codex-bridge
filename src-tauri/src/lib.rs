mod orchestration;
mod relay_protocol;

use chrono::{DateTime, Duration, Utc};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

const INITIAL_SCHEMA: &str = include_str!("../migrations/001_initial.sql");
const ORCHESTRATION_SCHEMA: &str = include_str!("../migrations/002_orchestration_runtime.sql");
const EXECUTION_CONTROL_SCHEMA: &str = include_str!("../migrations/003_execution_control.sql");
const CONVERSATION_RELAY_SCHEMA: &str = include_str!("../migrations/004_conversation_relay_v2.sql");
const CODEX_COMMUNICATION_OBSERVABILITY_SCHEMA: &str =
    include_str!("../migrations/005_codex_communication_observability.sql");

struct AppState {
    connection: Mutex<Connection>,
    chatgpt_bridge: Arc<ChatGptBridge>,
    orchestrator: Mutex<Option<ActiveOrchestration>>,
    relay_codex: Mutex<Option<RelayCodexSession>>,
    application_started_at: DateTime<Utc>,
}

#[derive(Clone)]
struct ActiveOrchestration {
    module: ModuleRecord,
    runtime: orchestration::Runtime,
    started_at: DateTime<Utc>,
    last_commit_sha: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrchestrationSnapshot {
    module_id: String,
    phase: String,
    completed_rounds: u32,
    max_rounds: i64,
    started_at: String,
    pause_after_current_turn: bool,
    last_commit_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum AcceptanceAction {
    Approve,
    Continue,
    Stop,
    Replan,
}

const CHATGPT_BRIDGE_PORT: u16 = 8765;

struct ChatGptBridge {
    pairing_secret: String,
    session: Mutex<Option<PairedChatGptSession>>,
    outbound: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    latest_status: Mutex<ChatGptBridgeStatus>,
}

#[derive(Clone)]
struct PairedChatGptSession {
    session_id: String,
    tab_id: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatGptBridgeStatus {
    phase: String,
    detail: String,
    tab_id: Option<i64>,
    protocol_state: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatGptPairingInfo {
    endpoint: String,
    pairing_secret: String,
    paired: bool,
    bound_tab_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProtocolState {
    NextTask,
    ModuleDone,
    Pause,
    Blocked,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProtocolEnvelope {
    state: ProtocolState,
    module: String,
    reason: String,
    codex_prompt: Option<String>,
    acceptance_criteria: Vec<String>,
    review_scope: Option<String>,
    requires_user_input: bool,
}

enum BridgeFrame {
    Text(String),
    Ping(Vec<u8>),
    Ignore,
    Disconnected,
}

struct ManagedAppServer {
    child: Child,
}

impl ManagedAppServer {
    fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ManagedAppServer {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(Clone)]
struct RelayCodexSession {
    module_id: String,
    commands: std_mpsc::Sender<RelayCodexCommand>,
    turn_active: Arc<AtomicBool>,
}

enum RelayCodexCommand {
    StartTurn { cycle_id: String, prompt: String },
    Release {
        acknowledgement: std_mpsc::Sender<Result<(), String>>,
    },
}

fn release_relay_codex_session(
    sessions: &Mutex<Option<RelayCodexSession>>,
    module_id: &str,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let commands = {
        let sessions = sessions
            .lock()
            .map_err(|_| "Codex 会话锁已损坏。".to_string())?;
        let Some(session) = sessions.as_ref() else {
            return Ok(());
        };
        if session.module_id != module_id {
            return Ok(());
        }
        if session.turn_active.load(Ordering::SeqCst) {
            return Err("当前 Codex 回合仍在运行，不能释放 Codex 对话。".into());
        }
        session.commands.clone()
    };

    let (acknowledgement_sender, acknowledgement_receiver) = std_mpsc::channel();
    commands
        .send(RelayCodexCommand::Release {
            acknowledgement: acknowledgement_sender,
        })
        .map_err(|_| "Codex 对话已经退出，无法确认释放。".to_string())?;
    acknowledgement_receiver
        .recv_timeout(timeout)
        .map_err(|error| format!("等待 Codex 对话释放确认超时：{error}"))?
}

fn clear_relay_codex_session_if_matches(
    sessions: &Mutex<Option<RelayCodexSession>>,
    module_id: &str,
) -> bool {
    let Ok(mut sessions) = sessions.lock() else {
        return false;
    };
    if sessions
        .as_ref()
        .is_some_and(|session| session.module_id == module_id)
    {
        *sessions = None;
        true
    } else {
        false
    }
}

fn mark_relay_codex_session_turn_inactive(
    sessions: &Mutex<Option<RelayCodexSession>>,
    module_id: &str,
) -> bool {
    let Ok(sessions) = sessions.lock() else {
        return false;
    };
    let Some(session) = sessions.as_ref() else {
        return false;
    };
    if session.module_id != module_id {
        return false;
    }
    session.turn_active.store(false, Ordering::SeqCst);
    true
}

fn release_relay_codex_runtime(app: &AppHandle, module_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    release_relay_codex_session(
        &state.relay_codex,
        module_id,
        std::time::Duration::from_secs(5),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InactiveModuleInput {
    name: String,
    repository_path: String,
    target_branch: String,
    chatgpt_tab_id: i64,
    max_rounds: i64,
    module_timeout_minutes: i64,
    global_timeout_minutes: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Budget {
    max_rounds: i64,
    module_timeout_minutes: i64,
    global_timeout_minutes: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModuleRecord {
    id: String,
    name: String,
    repository_path: String,
    target_branch: String,
    chatgpt_tab_id: i64,
    status: String,
    budget: Budget,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CodexExecutionPhase {
    Starting,
    Running,
    Completed,
    Blocked,
    Failed,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CodexExecutionEvent {
    module_id: String,
    phase: CodexExecutionPhase,
    status_line: String,
    thread_id: Option<String>,
    turn_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexTurnResult {
    module_id: String,
    status: CodexExecutionPhase,
    summary: String,
    thread_id: Option<String>,
    turn_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelayModuleInput {
    name: String,
    working_directory: String,
    max_cycles: i64,
    max_runtime_minutes: i64,
    retry_template: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RelayModuleRecord {
    id: String,
    name: String,
    working_directory: String,
    max_cycles: i64,
    max_runtime_minutes: i64,
    retry_template: String,
    phase: String,
    codex_thread_id: Option<String>,
    module_started_at: Option<String>,
    stop_after_turn: bool,
    invalid_reply_count: i64,
    started_cycles: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum RelayMessageKind {
    Manual,
    Automation,
}

impl RelayMessageKind {
    fn as_db(self) -> &'static str {
        match self {
            Self::Manual => "MANUAL",
            Self::Automation => "AUTOMATION",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayMessageRecord {
    id: String,
    module_id: String,
    sequence_number: i64,
    direction: String,
    kind: String,
    text: String,
    delivery_state: String,
    created_at: String,
    delivered_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayRecoveryRecord {
    message_id: String,
    module_id: String,
    module_name: String,
    sequence_number: i64,
    kind: String,
    created_at: String,
}

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

enum RelayDispatchClaim {
    RecoveryBlocked(i64),
    InFlight,
    Empty,
    Message(RelayMessageRecord),
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("could not locate the application data directory: {error}"))?;
    fs::create_dir_all(&data_dir)
        .map_err(|error| format!("could not create the application data directory: {error}"))?;
    Ok(data_dir.join("middleware.sqlite3"))
}

fn create_connection(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?)
        .map_err(|error| format!("could not open the local database: {error}"))?;
    connection
        .execute_batch(INITIAL_SCHEMA)
        .map_err(|error| format!("could not initialize the local database: {error}"))?;
    connection
        .execute_batch(ORCHESTRATION_SCHEMA)
        .map_err(|error| format!("could not initialize orchestration storage: {error}"))?;
    connection
        .execute_batch(EXECUTION_CONTROL_SCHEMA)
        .map_err(|error| format!("could not initialize execution-control storage: {error}"))?;
    connection
        .execute_batch(CONVERSATION_RELAY_SCHEMA)
        .map_err(|error| format!("could not initialize conversation-relay storage: {error}"))?;
    connection
        .execute_batch(CODEX_COMMUNICATION_OBSERVABILITY_SCHEMA)
        .map_err(|error| {
            format!("could not initialize Codex communication observability storage: {error}")
        })?;
    Ok(connection)
}

fn mark_uncertain_relay_deliveries(connection: &Connection) -> Result<(), String> {
    let message_ids: Vec<String> = connection
        .prepare(
            "SELECT id FROM relay_messages
             WHERE direction = 'TO_CHATGPT' AND delivery_state = 'SENT'",
        )
        .map_err(|error| format!("could not inspect relay delivery state: {error}"))?
        .query_map([], |row| row.get(0))
        .map_err(|error| format!("could not read relay delivery state: {error}"))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("could not collect relay delivery state: {error}"))?;
    let affected = connection
        .execute(
            "UPDATE relay_messages SET delivery_state = 'UNKNOWN'
         WHERE direction = 'TO_CHATGPT' AND delivery_state = 'SENT'",
            [],
        )
        .map_err(|error| format!("could not recover relay delivery state: {error}"))?;
    if affected > 0 {
        connection.execute(
            "UPDATE relay_modules SET phase = 'RECOVERY_REQUIRED', updated_at = ?1
             WHERE id IN (SELECT DISTINCT module_id FROM relay_messages WHERE delivery_state = 'UNKNOWN')",
            [Utc::now().to_rfc3339()],
        ).map_err(|error| format!("could not mark relay recovery state: {error}"))?;
        for message_id in message_ids {
            sync_codex_cycle_for_chatgpt_message_state(
                connection,
                &message_id,
                "UNKNOWN",
                Some("应用重启后无法确认 ChatGPT 消息是否已送达；未自动重发。"),
            )?;
        }
    }
    Ok(())
}

fn validate(input: &InactiveModuleInput) -> Result<(), String> {
    if input.name.trim().is_empty()
        || input.repository_path.trim().is_empty()
        || input.target_branch.trim().is_empty()
    {
        return Err("module name, repository path, and target branch are required".into());
    }
    if input.chatgpt_tab_id <= 0
        || input.max_rounds <= 0
        || input.module_timeout_minutes <= 0
        || input.global_timeout_minutes <= 0
    {
        return Err("ChatGPT tab ID and all budgets must be positive".into());
    }
    Ok(())
}

fn row_to_module(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModuleRecord> {
    Ok(ModuleRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        repository_path: row.get(2)?,
        target_branch: row.get(3)?,
        chatgpt_tab_id: row.get(4)?,
        status: row.get(5)?,
        budget: Budget {
            max_rounds: row.get(6)?,
            module_timeout_minutes: row.get(7)?,
            global_timeout_minutes: row.get(8)?,
        },
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn get_module(connection: &Connection, id: &str) -> Result<Option<ModuleRecord>, String> {
    connection
        .query_row(
            "SELECT m.id, m.name, m.repository_path, m.target_branch, m.chatgpt_tab_id, m.status,
                    b.max_rounds, b.module_timeout_minutes, b.global_timeout_minutes, m.created_at, m.updated_at
             FROM modules m JOIN budgets b ON b.module_id = m.id
             WHERE m.id = ?1 AND m.status = 'INACTIVE'",
            [id],
            row_to_module,
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn orchestration_phase_name(phase: orchestration::Phase) -> &'static str {
    match phase {
        orchestration::Phase::WaitingForChatGptPlan => "WAITING_FOR_CHATGPT_PLAN",
        orchestration::Phase::StartingCodexTurn => "STARTING_CODEX_TURN",
        orchestration::Phase::CodexRunning => "CODEX_RUNNING",
        orchestration::Phase::WaitingForChatGptReview => "WAITING_FOR_CHATGPT_REVIEW",
        orchestration::Phase::PausedForAcceptance => "PAUSED_FOR_ACCEPTANCE",
        orchestration::Phase::Blocked => "BLOCKED",
        orchestration::Phase::Completed => "COMPLETED",
        orchestration::Phase::Stopped => "STOPPED",
    }
}

fn orchestration_phase_from_name(phase: &str) -> Result<orchestration::Phase, String> {
    match phase {
        "WAITING_FOR_CHATGPT_PLAN" => Ok(orchestration::Phase::WaitingForChatGptPlan),
        "STARTING_CODEX_TURN" => Ok(orchestration::Phase::StartingCodexTurn),
        "CODEX_RUNNING" => Ok(orchestration::Phase::CodexRunning),
        "WAITING_FOR_CHATGPT_REVIEW" => Ok(orchestration::Phase::WaitingForChatGptReview),
        "PAUSED_FOR_ACCEPTANCE" => Ok(orchestration::Phase::PausedForAcceptance),
        "BLOCKED" => Ok(orchestration::Phase::Blocked),
        "COMPLETED" => Ok(orchestration::Phase::Completed),
        "STOPPED" => Ok(orchestration::Phase::Stopped),
        _ => Err(format!("unknown persisted orchestration phase: {phase}")),
    }
}

fn persist_orchestration_state(
    connection: &Connection,
    module_id: &str,
    runtime: &orchestration::Runtime,
    message: &str,
) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();
    let phase = orchestration_phase_name(runtime.phase);
    connection
        .execute(
            "INSERT INTO module_runtime (module_id, phase, completed_rounds, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(module_id) DO UPDATE SET phase = excluded.phase, completed_rounds = excluded.completed_rounds, updated_at = excluded.updated_at",
            params![module_id, phase, runtime.completed_rounds, now],
        )
        .map_err(|error| format!("could not persist orchestration state: {error}"))?;
    connection
        .execute(
            "INSERT INTO audit_events (id, module_id, event_type, message, metadata_json, created_at)
             VALUES (?1, ?2, 'ORCHESTRATION_TRANSITION', ?3, ?4, ?5)",
            params![Uuid::new_v4().to_string(), module_id, message, json!({ "phase": phase, "completedRounds": runtime.completed_rounds, "pauseAfterCurrentTurn": runtime.pause_after_current_turn }).to_string(), now],
        )
        .map_err(|error| format!("could not write orchestration audit event: {error}"))?;
    Ok(())
}

fn persist_execution_details(
    connection: &Connection,
    active: &ActiveOrchestration,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO module_execution_state (module_id, started_at, pause_after_current_turn, last_commit_sha)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(module_id) DO UPDATE SET started_at = excluded.started_at, pause_after_current_turn = excluded.pause_after_current_turn, last_commit_sha = excluded.last_commit_sha",
            params![
                &active.module.id,
                active.started_at.to_rfc3339(),
                active.runtime.pause_after_current_turn as i64,
                active.last_commit_sha.as_deref(),
            ],
        )
        .map_err(|error| format!("could not persist execution details: {error}"))?;
    Ok(())
}

fn snapshot_from_active(active: &ActiveOrchestration) -> OrchestrationSnapshot {
    OrchestrationSnapshot {
        module_id: active.module.id.clone(),
        phase: orchestration_phase_name(active.runtime.phase).into(),
        completed_rounds: active.runtime.completed_rounds,
        max_rounds: active.module.budget.max_rounds,
        started_at: active.started_at.to_rfc3339(),
        pause_after_current_turn: active.runtime.pause_after_current_turn,
        last_commit_sha: active.last_commit_sha.clone(),
    }
}

fn run_git(repository_path: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(args)
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn extract_reported_commit_sha(summary: &str) -> Option<String> {
    summary
        .split(|character: char| !character.is_ascii_hexdigit())
        .find(|candidate| (7..=64).contains(&candidate.len()))
        .map(ToOwned::to_owned)
}

fn verify_codex_outcome(module: &ModuleRecord, summary: &str) -> Result<String, String> {
    let commit_sha = extract_reported_commit_sha(summary)
        .ok_or_else(|| "Codex final summary did not report a commit SHA".to_string())?;
    if run_git(
        &module.repository_path,
        &["rev-parse", "--is-inside-work-tree"],
    )? != "true"
    {
        return Err("selected repository is not a Git worktree".into());
    }
    let branch = run_git(&module.repository_path, &["branch", "--show-current"])?;
    if branch != module.target_branch {
        return Err(format!(
            "Codex finished on branch `{branch}`, expected `{}`",
            module.target_branch
        ));
    }
    run_git(
        &module.repository_path,
        &["rev-parse", "--verify", &format!("{commit_sha}^{{commit}}")],
    )?;
    let local_branch_head = run_git(
        &module.repository_path,
        &["rev-parse", &module.target_branch],
    )?;
    let remote = run_git(&module.repository_path, &["remote", "get-url", "origin"])?;
    if remote.trim().is_empty() {
        return Err("target repository has no origin remote".into());
    }
    let remote_head = run_git(
        &module.repository_path,
        &[
            "ls-remote",
            "--exit-code",
            "origin",
            &format!("refs/heads/{}", module.target_branch),
        ],
    )?
    .split_whitespace()
    .next()
    .ok_or_else(|| format!("origin does not expose branch `{}`", module.target_branch))?
    .to_string();
    if local_branch_head != remote_head {
        return Err(format!(
            "local `{}` ({local_branch_head}) does not match origin ({remote_head})",
            module.target_branch
        ));
    }
    let ancestry = Command::new("git")
        .arg("-C")
        .arg(&module.repository_path)
        .args([
            "merge-base",
            "--is-ancestor",
            &commit_sha,
            &module.target_branch,
        ])
        .status()
        .map_err(|error| format!("could not inspect commit ancestry: {error}"))?;
    if !ancestry.success() {
        return Err(format!(
            "reported commit {commit_sha} is not reachable from `{}`",
            module.target_branch
        ));
    }
    Ok(commit_sha)
}

fn record_completed_turn(
    connection: &Connection,
    active: &ActiveOrchestration,
    summary: &str,
    commit_sha: &str,
) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "INSERT INTO turns (id, module_id, turn_number, state, codex_summary, commit_sha, started_at, completed_at)
             VALUES (?1, ?2, ?3, 'VERIFIED', ?4, ?5, ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                &active.module.id,
                active.runtime.completed_rounds + 1,
                summary,
                commit_sha,
                active.started_at.to_rfc3339(),
                now,
            ],
        )
        .map_err(|error| format!("could not record verified Codex turn: {error}"))?;
    Ok(())
}

fn budget_pause_reason(state: &AppState, active: &ActiveOrchestration) -> Option<String> {
    if active.runtime.pause_after_current_turn {
        return Some("模块或全局时间预算已在当前 Codex 回合期间到达。".into());
    }
    if active.runtime.completed_rounds as i64 >= active.module.budget.max_rounds {
        return Some(format!(
            "已达到最大任务轮次 {}。",
            active.module.budget.max_rounds
        ));
    }
    let now = Utc::now();
    if now - active.started_at >= Duration::minutes(active.module.budget.module_timeout_minutes) {
        return Some(format!(
            "已达到模块最长运行时间 {} 分钟。",
            active.module.budget.module_timeout_minutes
        ));
    }
    if now - state.application_started_at
        >= Duration::minutes(active.module.budget.global_timeout_minutes)
    {
        return Some(format!(
            "已达到全局最长运行时间 {} 分钟。",
            active.module.budget.global_timeout_minutes
        ));
    }
    None
}

fn load_active_orchestration(
    connection: &Connection,
    module_id: &str,
) -> Result<ActiveOrchestration, String> {
    let module = get_module(connection, module_id)?
        .ok_or_else(|| "selected module was not found".to_string())?;
    let (phase, completed_rounds, started_at, pause_after_current_turn, last_commit_sha): (
        String,
        i64,
        String,
        i64,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT r.phase, r.completed_rounds, COALESCE(e.started_at, r.updated_at),
                    COALESCE(e.pause_after_current_turn, 0), e.last_commit_sha
             FROM module_runtime r
             LEFT JOIN module_execution_state e ON e.module_id = r.module_id
             WHERE r.module_id = ?1",
            [module_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|error| format!("could not load module runtime: {error}"))?;
    let started_at = DateTime::parse_from_rfc3339(&started_at)
        .map_err(|error| format!("could not parse persisted runtime timestamp: {error}"))?
        .with_timezone(&Utc);
    Ok(ActiveOrchestration {
        module,
        runtime: orchestration::Runtime {
            phase: orchestration_phase_from_name(&phase)?,
            completed_rounds: completed_rounds
                .try_into()
                .map_err(|_| "persisted completed-round count was invalid".to_string())?,
            pause_after_current_turn: pause_after_current_turn != 0,
        },
        started_at,
        last_commit_sha,
    })
}

#[tauri::command]
fn get_orchestration_snapshot(
    state: State<'_, AppState>,
    module_id: String,
) -> Result<Option<OrchestrationSnapshot>, String> {
    if let Some(active) = state
        .orchestrator
        .lock()
        .map_err(|_| "orchestrator lock poisoned".to_string())?
        .as_ref()
        .filter(|active| active.module.id == module_id)
    {
        return Ok(Some(snapshot_from_active(active)));
    }
    let connection = state
        .connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    match load_active_orchestration(&connection, &module_id) {
        Ok(active) => Ok(Some(snapshot_from_active(&active))),
        Err(error) if error.contains("Query returned no rows") => Ok(None),
        Err(error) => Err(error),
    }
}

fn user_continue_message(module: &ModuleRecord, replan_request: Option<&str>) -> String {
    match replan_request {
        Some(request) => format!(
            "用户要求重新规划“{}”模块。补充说明：{}。请检查仓库后只返回一个 NEXT_TASK 协议包；必须包含完整 codex_prompt 和至少一条 acceptance_criteria。",
            module.name,
            request.trim()
        ),
        None => format!(
            "用户已确认继续“{}”模块。请检查仓库后只返回一个 NEXT_TASK 协议包；必须包含完整 codex_prompt 和至少一条 acceptance_criteria。",
            module.name
        ),
    }
}

#[tauri::command]
fn apply_acceptance_action(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
    action: AcceptanceAction,
    replan_request: Option<String>,
) -> Result<OrchestrationSnapshot, String> {
    let restored = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        load_active_orchestration(&connection, &module_id)?
    };
    let (snapshot, message_to_chatgpt, release_module) = {
        let mut guard = state
            .orchestrator
            .lock()
            .map_err(|_| "orchestrator lock poisoned".to_string())?;
        if guard.is_none() {
            *guard = Some(restored);
        }
        let active = guard
            .as_mut()
            .ok_or_else(|| "no active orchestration is available".to_string())?;
        if active.module.id != module_id {
            return Err(
                "another module is active; its acceptance decision must be resolved first".into(),
            );
        }
        let (message, outbound, release) = match action {
            AcceptanceAction::Approve => {
                active.runtime.approve()?;
                ("用户已验收通过，模块已完成。", None, true)
            }
            AcceptanceAction::Continue => {
                active.runtime.continue_after_pause()?;
                (
                    "用户确认继续，正在等待 ChatGPT 下一任务。",
                    Some(user_continue_message(&active.module, None)),
                    false,
                )
            }
            AcceptanceAction::Stop => {
                active.runtime.stop()?;
                ("用户已终止模块。", None, true)
            }
            AcceptanceAction::Replan => {
                let request = replan_request
                    .as_deref()
                    .filter(|request| !request.trim().is_empty())
                    .ok_or_else(|| "replan requires a short user instruction".to_string())?;
                active.runtime.continue_after_pause()?;
                (
                    "用户要求重新规划，正在等待 ChatGPT。",
                    Some(user_continue_message(&active.module, Some(request))),
                    false,
                )
            }
        };
        let active_snapshot = active.clone();
        persist_active_orchestration(&state, &active_snapshot, message)?;
        (snapshot_from_active(&active_snapshot), outbound, release)
    };
    if release_module {
        *state
            .orchestrator
            .lock()
            .map_err(|_| "orchestrator lock poisoned".to_string())? = None;
    }
    if let Some(message) = message_to_chatgpt {
        if let Err(error) = send_chatgpt_message_internal(&app, &state.chatgpt_bridge, &message) {
            block_and_notify(&app, &state, format!("无法发送用户确认消息：{error}"));
            return Err(error);
        }
    }
    Ok(snapshot)
}

fn pause_unfinished_orchestrations(connection: &Connection) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "UPDATE module_runtime SET phase = 'PAUSED_FOR_ACCEPTANCE', updated_at = ?1
             WHERE phase IN ('WAITING_FOR_CHATGPT_PLAN', 'STARTING_CODEX_TURN', 'CODEX_RUNNING', 'WAITING_FOR_CHATGPT_REVIEW')",
            [now.as_str()],
        )
        .map_err(|error| format!("could not pause recovered orchestrations: {error}"))?;
    Ok(())
}

#[tauri::command]
fn list_inactive_modules(state: State<'_, AppState>) -> Result<Vec<ModuleRecord>, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT m.id, m.name, m.repository_path, m.target_branch, m.chatgpt_tab_id, m.status,
                    b.max_rounds, b.module_timeout_minutes, b.global_timeout_minutes, m.created_at, m.updated_at
             FROM modules m JOIN budgets b ON b.module_id = m.id
             WHERE m.status = 'INACTIVE' ORDER BY m.updated_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let modules = statement
        .query_map([], row_to_module)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(modules)
}

#[tauri::command]
fn create_inactive_module(
    state: State<'_, AppState>,
    input: InactiveModuleInput,
) -> Result<ModuleRecord, String> {
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    create_inactive_module_in(&mut connection, &input)
}

fn create_inactive_module_in(
    connection: &mut Connection,
    input: &InactiveModuleInput,
) -> Result<ModuleRecord, String> {
    validate(&input)?;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO modules (id, name, repository_path, target_branch, chatgpt_tab_id, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'INACTIVE', ?6, ?6)",
            params![id, input.name.trim(), input.repository_path.trim(), input.target_branch.trim(), input.chatgpt_tab_id, now],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO budgets (module_id, max_rounds, module_timeout_minutes, global_timeout_minutes)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, input.max_rounds, input.module_timeout_minutes, input.global_timeout_minutes],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    get_module(&connection, &id)?.ok_or_else(|| "saved module was not found".to_string())
}

#[tauri::command]
fn update_inactive_module(
    state: State<'_, AppState>,
    id: String,
    input: InactiveModuleInput,
) -> Result<ModuleRecord, String> {
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    update_inactive_module_in(&mut connection, &id, &input)
}

fn update_inactive_module_in(
    connection: &mut Connection,
    id: &str,
    input: &InactiveModuleInput,
) -> Result<ModuleRecord, String> {
    validate(&input)?;
    let now = Utc::now().to_rfc3339();
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE modules SET name = ?2, repository_path = ?3, target_branch = ?4, chatgpt_tab_id = ?5, updated_at = ?6
             WHERE id = ?1 AND status = 'INACTIVE'",
            params![id, input.name.trim(), input.repository_path.trim(), input.target_branch.trim(), input.chatgpt_tab_id, now],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("only an existing inactive module may be updated".into());
    }
    transaction
        .execute(
            "UPDATE budgets SET max_rounds = ?2, module_timeout_minutes = ?3, global_timeout_minutes = ?4 WHERE module_id = ?1",
            params![id, input.max_rounds, input.module_timeout_minutes, input.global_timeout_minutes],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    get_module(connection, id)?.ok_or_else(|| "updated module was not found".to_string())
}

#[tauri::command]
fn delete_inactive_module(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    delete_inactive_module_in(&connection, &id)
}

fn delete_inactive_module_in(connection: &Connection, id: &str) -> Result<(), String> {
    let changed = connection
        .execute(
            "DELETE FROM modules WHERE id = ?1 AND status = 'INACTIVE'",
            [id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("only an existing inactive module may be deleted".into());
    }
    Ok(())
}

impl ChatGptBridge {
    fn new() -> Self {
        Self {
            pairing_secret: Uuid::new_v4().to_string(),
            session: Mutex::new(None),
            outbound: Mutex::new(None),
            latest_status: Mutex::new(ChatGptBridgeStatus {
                phase: "UNPAIRED".into(),
                detail: "等待 Chrome 扩展使用一次性密钥配对。".into(),
                tab_id: None,
                protocol_state: None,
            }),
        }
    }

    fn set_status(&self, app: &AppHandle, status: ChatGptBridgeStatus) {
        if let Ok(mut latest) = self.latest_status.lock() {
            *latest = status.clone();
        }
        let _ = app.emit("chatgpt-status", status);
    }

    fn clear_connection(&self, app: &AppHandle) {
        if let Ok(mut outbound) = self.outbound.lock() {
            *outbound = None;
        }
        if let Ok(mut session) = self.session.lock() {
            *session = None;
        }
        self.set_status(
            app,
            ChatGptBridgeStatus {
                phase: "UNPAIRED".into(),
                detail: "Chrome 扩展已断开。".into(),
                tab_id: None,
                protocol_state: None,
            },
        );
    }

    fn send_to_extension(&self, payload: Value) -> Result<(), String> {
        let outbound = self
            .outbound
            .lock()
            .map_err(|_| "ChatGPT bridge lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "no paired ChatGPT extension is connected".to_string())?;
        outbound
            .send(Message::Text(payload.to_string().into()))
            .map_err(|_| "paired ChatGPT extension disconnected".to_string())
    }
}

fn extract_protocol_json(reply: &str) -> Result<&str, String> {
    let trimmed = reply.trim_end();
    let marker = "```json";
    let Some(start) = trimmed.find(marker) else {
        return extract_unfenced_protocol_json(trimmed)
            .ok_or_else(|| "response must contain one JSON code block".to_string());
    };
    if trimmed[..start].contains("```") {
        return Err("response contains more than one code block".into());
    }
    let content_start = start + marker.len();
    let remaining = &trimmed[content_start..];
    let close_relative = remaining
        .find("```")
        .ok_or_else(|| "JSON code block is not closed".to_string())?;
    let close_start = content_start + close_relative;
    if !trimmed[close_start + 3..].trim().is_empty() {
        return Err("JSON code block must be at the end of the response".into());
    }
    let json = trimmed[content_start..close_start].trim();
    if json.is_empty() {
        return Err("JSON code block cannot be empty".into());
    }
    Ok(json)
}

fn extract_unfenced_protocol_json(reply: &str) -> Option<&str> {
    let mut matches = Vec::new();
    for (start, _) in reply.match_indices('{') {
        let candidate = &reply[start..];
        let mut values = serde_json::Deserializer::from_str(candidate).into_iter::<Value>();
        let Some(Ok(value)) = values.next() else {
            continue;
        };
        if value.get("state").and_then(Value::as_str).is_none() {
            continue;
        }
        let end = values.byte_offset();
        if end > 0 {
            matches.push(&candidate[..end]);
        }
    }
    (matches.len() == 1).then(|| matches[0])
}

fn validate_protocol_json(json: &str) -> Result<ProtocolEnvelope, String> {
    let envelope: ProtocolEnvelope =
        serde_json::from_str(json).map_err(|error| format!("invalid protocol JSON: {error}"))?;
    if envelope.module.trim().is_empty() || envelope.reason.trim().is_empty() {
        return Err("protocol state, module, and reason must be non-empty".into());
    }
    match envelope.state {
        ProtocolState::NextTask => {
            if envelope
                .codex_prompt
                .as_deref()
                .is_none_or(|prompt| prompt.trim().is_empty())
            {
                return Err("NEXT_TASK requires a non-empty codex_prompt".into());
            }
            if envelope.acceptance_criteria.is_empty()
                || envelope
                    .acceptance_criteria
                    .iter()
                    .any(|criterion| criterion.trim().is_empty())
            {
                return Err("NEXT_TASK requires non-empty acceptance_criteria".into());
            }
        }
        ProtocolState::ModuleDone | ProtocolState::Pause | ProtocolState::Blocked => {
            if envelope.codex_prompt.is_some() {
                return Err("only NEXT_TASK may include codex_prompt".into());
            }
        }
    }
    Ok(envelope)
}

fn validate_protocol_payload(
    reply: &str,
    structured_json: Option<&str>,
) -> Result<ProtocolEnvelope, String> {
    let json = match structured_json {
        Some(json) => json,
        None => extract_protocol_json(reply)?,
    };
    validate_protocol_json(json)
}

fn protocol_state_name(state: &ProtocolState) -> String {
    match state {
        ProtocolState::NextTask => "NEXT_TASK",
        ProtocolState::ModuleDone => "MODULE_DONE",
        ProtocolState::Pause => "PAUSE",
        ProtocolState::Blocked => "BLOCKED",
    }
    .into()
}

fn classify_bridge_frame(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> BridgeFrame {
    match message {
        Some(Ok(Message::Text(text))) => BridgeFrame::Text(text.to_string()),
        Some(Ok(Message::Ping(payload))) => BridgeFrame::Ping(payload.to_vec()),
        Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => BridgeFrame::Ignore,
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => BridgeFrame::Disconnected,
        Some(Ok(Message::Binary(_))) => BridgeFrame::Ignore,
    }
}

async fn handle_chatgpt_bridge_connection(
    app: AppHandle,
    bridge: Arc<ChatGptBridge>,
    stream: tokio::net::TcpStream,
) {
    let Ok(socket) = accept_async(stream).await else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    let (outbound, mut outbound_receiver) = mpsc::unbounded_channel::<Message>();
    let mut owns_active_session = false;

    loop {
        tokio::select! {
            Some(message) = outbound_receiver.recv() => {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            incoming = source.next() => {
                let frame = classify_bridge_frame(incoming);
                match frame {
                    BridgeFrame::Ping(payload) => {
                        if sink.send(Message::Pong(payload.into())).await.is_err() { break; }
                        continue;
                    }
                    BridgeFrame::Ignore => continue,
                    BridgeFrame::Disconnected => break,
                    BridgeFrame::Text(text) => {
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    bridge.set_status(&app, ChatGptBridgeStatus {
                        phase: "BLOCKED".into(),
                        detail: "Chrome 扩展发送了无效的桥接 JSON。".into(),
                        tab_id: None,
                        protocol_state: None,
                    });
                    continue;
                };
                match message.get("type").and_then(Value::as_str) {
                    Some("pair") => {
                        let secret = message.get("pairingSecret").and_then(Value::as_str);
                        let tab_id = message.get("tabId").and_then(Value::as_i64);
                        if secret != Some(bridge.pairing_secret.as_str()) || tab_id.is_none_or(|id| id <= 0) {
                            let _ = outbound.send(Message::Text(json!({ "type": "pairingRejected" }).to_string().into()));
                            continue;
                        }
                        let session_id = Uuid::new_v4().to_string();
                        let paired = PairedChatGptSession { session_id: session_id.clone(), tab_id: tab_id.unwrap_or_default() };
                        if let Ok(mut session) = bridge.session.lock() { *session = Some(paired.clone()); }
                        if let Ok(mut sender) = bridge.outbound.lock() { *sender = Some(outbound.clone()); }
                        owns_active_session = true;
                        bridge.set_status(&app, ChatGptBridgeStatus {
                            phase: "PAIRED".into(),
                            detail: "已绑定一个专用 ChatGPT 标签页。".into(),
                            tab_id: Some(paired.tab_id),
                            protocol_state: None,
                        });
                        let _ = outbound.send(Message::Text(json!({ "type": "paired", "sessionId": session_id }).to_string().into()));
                    }
                    Some("chatgptReply") => {
                        let session_id = message.get("sessionId").and_then(Value::as_str);
                        let reply = message.get("text").and_then(Value::as_str);
                        let structured_json = message.get("protocolJson").and_then(Value::as_str);
                        let structured_json_present = message.get("protocolJsonPresent").and_then(Value::as_bool);
                        let adapter_version = message.get("adapterVersion").and_then(Value::as_str);
                        let adapter_error = message.get("adapterError").and_then(Value::as_str);
                        let adapter_diagnostic = message.get("adapterDiagnostic").and_then(Value::as_str);
                        let request_id = message.get("requestId").and_then(Value::as_str);
                        let relay = message.get("relay").and_then(Value::as_bool) == Some(true);
                        let valid_session = bridge.session.lock().ok().and_then(|session| session.clone()).is_some_and(|session| Some(session.session_id.as_str()) == session_id);
                        if !valid_session || reply.is_none() {
                            bridge.set_status(&app, ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: "收到未配对或不完整的 ChatGPT 回复。".into(),
                                tab_id: None,
                                protocol_state: None,
                            });
                            let _ = block_active_orchestration(
                                &app.state::<AppState>(),
                                "收到未配对或不完整的 ChatGPT 回复。".into(),
                            );
                            continue;
                        }
                        if relay {
                            if let Some(adapter_error) = adapter_error {
                                if let Err(error) = handle_relay_chatgpt_adapter_failure(
                                    app.clone(),
                                    adapter_error,
                                    adapter_diagnostic,
                                ) {
                                    bridge.set_status(&app, ChatGptBridgeStatus {
                                        phase: "RELAY_BLOCKED".into(),
                                        detail: format!("无法保存 ChatGPT 适配器错误：{error}"),
                                        tab_id: None,
                                        protocol_state: None,
                                    });
                                }
                                continue;
                            }
                            if let Err(error) = handle_relay_chatgpt_reply(app.clone(), request_id, reply.unwrap_or_default()) {
                                bridge.set_status(&app, ChatGptBridgeStatus {
                                    phase: "RELAY_BLOCKED".into(),
                                    detail: format!("传话回复无法处理：{error}"),
                                    tab_id: None,
                                    protocol_state: None,
                                });
                            }
                            continue;
                        }
                        if structured_json_present == Some(false) {
                            let version = adapter_version.unwrap_or("unknown");
                            let detail = adapter_error.unwrap_or("ChatGPT completed without a protocol JSON object.");
                            let diagnostic = adapter_diagnostic.map(|value| format!(" Observed assistant nodes: {value}")).unwrap_or_default();
                            bridge.set_status(&app, ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: format!("Protocol adapter v{version} did not return structured JSON: {detail}{diagnostic}"),
                                tab_id: None,
                                protocol_state: None,
                            });
                            let _ = block_active_orchestration(
                                &app.state::<AppState>(),
                                format!("ChatGPT 协议适配器未返回结构化 JSON：{detail}"),
                            );
                            continue;
                        }
                        match validate_protocol_payload(reply.unwrap_or_default(), structured_json) {
                            Ok(envelope) => {
                                let state = protocol_state_name(&envelope.state);
                                let tab_id = bridge.session.lock().ok().and_then(|session| session.clone()).map(|session| session.tab_id);
                                bridge.set_status(&app, ChatGptBridgeStatus {
                                    phase: "VALID_PROTOCOL".into(),
                                    detail: format!("已验证 ChatGPT 协议状态：{state}。"),
                                    tab_id,
                                    protocol_state: Some(state),
                                });
                                if let Err(error) = handle_orchestration_protocol(app.clone(), envelope) {
                                    bridge.set_status(&app, ChatGptBridgeStatus {
                                        phase: "BLOCKED".into(),
                                        detail: format!("自动编排无法处理协议状态：{error}"),
                                        tab_id: None,
                                        protocol_state: None,
                                    });
                                    let _ = block_active_orchestration(
                                        &app.state::<AppState>(),
                                        format!("无法处理 ChatGPT 协议状态：{error}"),
                                    );
                                }
                            }
                            Err(error) => {
                                bridge.set_status(&app, ChatGptBridgeStatus {
                                    phase: "BLOCKED".into(),
                                    detail: format!("Protocol validation failed: {error}"),
                                    tab_id: None,
                                    protocol_state: None,
                                });
                                let _ = block_active_orchestration(
                                    &app.state::<AppState>(),
                                    format!("ChatGPT 协议校验失败：{error}"),
                                );
                            }
                        }
                    }
                    Some("keepAlive") => {
                        let session_id = message.get("sessionId").and_then(Value::as_str);
                        let valid_session = bridge.session.lock().ok().and_then(|session| session.clone()).is_some_and(|session| Some(session.session_id.as_str()) == session_id);
                        if !valid_session {
                            bridge.set_status(&app, ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: "收到未配对的扩展心跳。".into(),
                                tab_id: None,
                                protocol_state: None,
                            });
                        }
                    }
                    _ => bridge.set_status(&app, ChatGptBridgeStatus {
                        phase: "BLOCKED".into(),
                        detail: "Chrome 扩展发送了未知桥接消息。".into(),
                        tab_id: None,
                        protocol_state: None,
                    }),
                }
                    }
                }
            }
        }
    }
    if owns_active_session {
        bridge.clear_connection(&app);
    }
}

fn start_chatgpt_bridge(app: AppHandle, bridge: Arc<ChatGptBridge>) -> Result<(), String> {
    let std_listener =
        std::net::TcpListener::bind(("127.0.0.1", CHATGPT_BRIDGE_PORT)).map_err(|error| {
            format!("could not bind ChatGPT bridge to 127.0.0.1:{CHATGPT_BRIDGE_PORT}: {error}")
        })?;
    std_listener
        .set_nonblocking(true)
        .map_err(|error| format!("could not configure ChatGPT bridge listener: {error}"))?;
    tauri::async_runtime::spawn(async move {
        let Ok(listener) = TcpListener::from_std(std_listener) else {
            return;
        };
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tauri::async_runtime::spawn(handle_chatgpt_bridge_connection(
                app.clone(),
                bridge.clone(),
                stream,
            ));
        }
    });
    Ok(())
}

#[tauri::command]
fn get_chatgpt_pairing(state: State<'_, AppState>) -> ChatGptPairingInfo {
    let session = state
        .chatgpt_bridge
        .session
        .lock()
        .ok()
        .and_then(|session| session.clone());
    ChatGptPairingInfo {
        endpoint: format!("ws://127.0.0.1:{CHATGPT_BRIDGE_PORT}"),
        pairing_secret: state.chatgpt_bridge.pairing_secret.clone(),
        paired: session.is_some(),
        bound_tab_id: session.map(|session| session.tab_id),
    }
}

fn validate_relay_module(input: &RelayModuleInput) -> Result<(), String> {
    if input.name.trim().is_empty() || input.working_directory.trim().is_empty() {
        return Err("请填写模块名称和 Codex 工作目录。".into());
    }
    if input.max_cycles <= 0 || input.max_runtime_minutes <= 0 {
        return Err("最大循环次数和最长运行时间必须为正数。".into());
    }
    if input.retry_template.trim().is_empty() {
        return Err("请填写 ChatGPT 协议重试模板。".into());
    }
    Ok(())
}

fn relay_row_to_module(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelayModuleRecord> {
    Ok(RelayModuleRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        working_directory: row.get(2)?,
        max_cycles: row.get(3)?,
        max_runtime_minutes: row.get(4)?,
        retry_template: row.get(5)?,
        phase: row.get(6)?,
        codex_thread_id: row.get(7)?,
        module_started_at: row.get(8)?,
        stop_after_turn: row.get::<_, i64>(9)? != 0,
        invalid_reply_count: row.get(10)?,
        started_cycles: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn get_relay_module(
    connection: &Connection,
    id: &str,
) -> Result<Option<RelayModuleRecord>, String> {
    connection.query_row(
        "SELECT id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase,
                codex_thread_id, module_started_at, stop_after_turn, invalid_reply_count, started_cycles, created_at, updated_at
         FROM relay_modules WHERE id = ?1",
        [id],
        relay_row_to_module,
    ).optional().map_err(|error| format!("无法读取传话模块：{error}"))
}

fn next_relay_sequence(connection: &Connection, module_id: &str) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(sequence_number), 0) + 1 FROM relay_messages WHERE module_id = ?1",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法分配消息序号：{error}"))
}

fn relay_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelayMessageRecord> {
    Ok(RelayMessageRecord {
        id: row.get(0)?,
        module_id: row.get(1)?,
        sequence_number: row.get(2)?,
        direction: row.get(3)?,
        kind: row.get(4)?,
        text: row.get(5)?,
        delivery_state: row.get(6)?,
        created_at: row.get(7)?,
        delivered_at: row.get(8)?,
    })
}

fn relay_codex_cycle_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelayCodexCycleRecord> {
    Ok(RelayCodexCycleRecord {
        id: row.get(0)?,
        module_id: row.get(1)?,
        cycle_number: row.get(2)?,
        status: row.get(3)?,
        prompt_text: row.get(4)?,
        codex_thread_id: row.get(5)?,
        codex_turn_id: row.get(6)?,
        result_text: row.get(7)?,
        outbound_chatgpt_message_id: row.get(8)?,
        error_text: row.get(9)?,
        created_at: row.get(10)?,
        codex_started_at: row.get(11)?,
        codex_completed_at: row.get(12)?,
        relay_queued_at: row.get(13)?,
        relay_delivered_at: row.get(14)?,
        updated_at: row.get(15)?,
        block_reason: None,
    })
}

const RELAY_CODEX_CYCLE_SELECT: &str = "SELECT id, module_id, cycle_number, status, prompt_text,
    codex_thread_id, codex_turn_id, result_text, outbound_chatgpt_message_id, error_text,
    created_at, codex_started_at, codex_completed_at, relay_queued_at, relay_delivered_at, updated_at
 FROM relay_codex_cycles";

fn create_relay_codex_cycle(
    connection: &Connection,
    module_id: &str,
    cycle_number: i64,
    prompt_text: &str,
) -> Result<RelayCodexCycleRecord, String> {
    let now = Utc::now().to_rfc3339();
    let cycle = RelayCodexCycleRecord {
        id: Uuid::new_v4().to_string(),
        module_id: module_id.to_string(),
        cycle_number,
        status: "WAITING_TO_SEND_CODEX".into(),
        prompt_text: prompt_text.to_string(),
        codex_thread_id: None,
        codex_turn_id: None,
        result_text: None,
        outbound_chatgpt_message_id: None,
        error_text: None,
        created_at: now.clone(),
        codex_started_at: None,
        codex_completed_at: None,
        relay_queued_at: None,
        relay_delivered_at: None,
        updated_at: now,
        block_reason: None,
    };
    connection
        .execute(
            "INSERT INTO relay_codex_cycles (
            id, module_id, cycle_number, status, prompt_text, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &cycle.id,
                &cycle.module_id,
                cycle.cycle_number,
                &cycle.status,
                &cycle.prompt_text,
                &cycle.created_at,
                &cycle.updated_at,
            ],
        )
        .map_err(|error| format!("无法创建 Codex 通讯循环：{error}"))?;
    Ok(cycle)
}

fn get_relay_codex_cycle_by_id(
    connection: &Connection,
    cycle_id: &str,
) -> Result<Option<RelayCodexCycleRecord>, String> {
    connection
        .query_row(
            &format!("{RELAY_CODEX_CYCLE_SELECT} WHERE id = ?1"),
            [cycle_id],
            relay_codex_cycle_row,
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))
}

fn get_relay_codex_cycle_by_outbound_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<RelayCodexCycleRecord>, String> {
    connection
        .query_row(
            &format!("{RELAY_CODEX_CYCLE_SELECT} WHERE outbound_chatgpt_message_id = ?1"),
            [message_id],
            relay_codex_cycle_row,
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))
}

fn relay_channel_snapshot_from_connection(
    connection: &Connection,
) -> Result<RelayChannelSnapshot, String> {
    let recovery_blocker_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM relay_messages
             WHERE direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法统计 ChatGPT 不确定送达消息：{error}"))?;

    let active_chatgpt_message = if recovery_blocker_count > 0 {
        connection
            .query_row(
                "SELECT message.module_id, module.name, message.id, message.kind, message.delivery_state
                 FROM relay_messages AS message
                 JOIN relay_modules AS module ON module.id = message.module_id
                 WHERE message.direction = 'TO_CHATGPT' AND message.delivery_state = 'UNKNOWN'
                 ORDER BY message.created_at ASC, message.id ASC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("无法读取 ChatGPT 不确定送达消息：{error}"))?
    } else {
        connection
            .query_row(
                "SELECT message.module_id, module.name, message.id, message.kind, message.delivery_state
                 FROM relay_messages AS message
                 JOIN relay_modules AS module ON module.id = message.module_id
                 WHERE message.direction = 'TO_CHATGPT' AND message.delivery_state = 'SENT'
                 ORDER BY message.created_at ASC, message.id ASC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("无法读取 ChatGPT 在途消息：{error}"))?
    };
    let chatgpt = match (recovery_blocker_count, active_chatgpt_message) {
        (count, Some((module_id, module_name, message_id, kind, phase))) if count > 0 => {
            RelayChatGptChannelSnapshot {
                status: "RECOVERY_BLOCKED".into(),
                active_module_id: Some(module_id),
                active_module_name: Some(module_name),
                active_message_id: Some(message_id),
                active_kind: Some(kind),
                active_phase: Some(phase),
                recovery_blocker_count: count,
            }
        }
        (0, Some((module_id, module_name, message_id, kind, phase))) => {
            RelayChatGptChannelSnapshot {
                status: "IN_FLIGHT".into(),
                active_module_id: Some(module_id),
                active_module_name: Some(module_name),
                active_message_id: Some(message_id),
                active_kind: Some(kind),
                active_phase: Some(phase),
                recovery_blocker_count: 0,
            }
        }
        _ => RelayChatGptChannelSnapshot {
            status: "IDLE".into(),
            active_module_id: None,
            active_module_name: None,
            active_message_id: None,
            active_kind: None,
            active_phase: None,
            recovery_blocker_count: 0,
        },
    };

    let active_codex_cycle = connection
        .query_row(
            "SELECT cycle.module_id, module.name, cycle.cycle_number,
                    cycle.codex_thread_id, cycle.codex_turn_id, cycle.status
             FROM relay_codex_cycles AS cycle
             JOIN relay_modules AS module ON module.id = cycle.module_id
             WHERE cycle.status = 'CODEX_RUNNING'
             ORDER BY cycle.codex_started_at ASC, cycle.id ASC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("无法读取运行中的 Codex 通讯循环：{error}"))?;
    let codex = match active_codex_cycle {
        Some((module_id, module_name, cycle_number, thread_id, turn_id, cycle_status)) => {
            RelayCodexChannelSnapshot {
                status: "RUNNING".into(),
                active_module_id: Some(module_id),
                active_module_name: Some(module_name),
                cycle_number: Some(cycle_number),
                codex_thread_id: thread_id,
                codex_turn_id: turn_id,
                cycle_status: Some(cycle_status),
            }
        }
        None => RelayCodexChannelSnapshot {
            status: "IDLE".into(),
            active_module_id: None,
            active_module_name: None,
            cycle_number: None,
            codex_thread_id: None,
            codex_turn_id: None,
            cycle_status: None,
        },
    };

    Ok(RelayChannelSnapshot { chatgpt, codex })
}

fn relay_codex_cycle_block_reason(
    connection: &Connection,
    cycle: &RelayCodexCycleRecord,
    snapshot: &RelayChannelSnapshot,
) -> Result<Option<String>, String> {
    if cycle.status == "CODEX_COMPLETED" && cycle.outbound_chatgpt_message_id.is_none() {
        let phase = connection
            .query_row(
                "SELECT phase FROM relay_modules WHERE id = ?1",
                [&cycle.module_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("无法读取 Codex 通讯模块状态：{error}"))?;
        if phase.as_deref() == Some("STOPPED") {
            return Ok(Some("模块已由用户终止，结果未回传 ChatGPT".into()));
        }
    }
    if cycle.status == "FAILED" {
        return Ok(cycle.error_text.clone());
    }
    if !matches!(
        cycle.status.as_str(),
        "WAITING_FOR_CHATGPT" | "SENDING_TO_CHATGPT"
    ) {
        return Ok(None);
    }

    let outbound_delivery_state = match cycle.outbound_chatgpt_message_id.as_deref() {
        Some(message_id) => connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = ?1",
                [message_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("无法读取 Codex 回传消息状态：{error}"))?,
        None => None,
    };
    if outbound_delivery_state.as_deref() == Some("UNKNOWN") {
        return Ok(Some("回传结果不确定，等待人工恢复。".into()));
    }
    if snapshot.chatgpt.status == "RECOVERY_BLOCKED" {
        return Ok(Some(format!(
            "存在待人工处理的不确定送达消息（{} 条）。",
            snapshot.chatgpt.recovery_blocker_count
        )));
    }
    if cycle.status == "SENDING_TO_CHATGPT" {
        return Ok(Some("等待 ChatGPT 完成回复。".into()));
    }
    if snapshot.chatgpt.status == "IN_FLIGHT"
        && snapshot.chatgpt.active_module_id.as_deref() != Some(cycle.module_id.as_str())
    {
        let module_name = snapshot
            .chatgpt
            .active_module_name
            .as_deref()
            .unwrap_or("未知模块");
        let message_id = snapshot
            .chatgpt
            .active_message_id
            .as_deref()
            .unwrap_or("未知消息");
        return Ok(Some(format!(
            "ChatGPT 通道当前被模块「{module_name}」占用（消息 {message_id}）。"
        )));
    }
    Ok(Some("等待全局 FIFO 调度。".into()))
}

fn list_relay_codex_cycles_in(
    connection: &Connection,
    module_id: &str,
) -> Result<Vec<RelayCodexCycleRecord>, String> {
    let snapshot = relay_channel_snapshot_from_connection(connection)?;
    let mut statement = connection
        .prepare(&format!(
            "{RELAY_CODEX_CYCLE_SELECT} WHERE module_id = ?1 ORDER BY cycle_number DESC"
        ))
        .map_err(|error| format!("无法查询 Codex 通讯循环：{error}"))?;
    let mut cycles = statement
        .query_map([module_id], relay_codex_cycle_row)
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))?;
    drop(statement);
    for cycle in &mut cycles {
        cycle.block_reason = relay_codex_cycle_block_reason(connection, cycle, &snapshot)?;
    }
    Ok(cycles)
}

fn append_relay_event(
    connection: &Connection,
    module_id: &str,
    event_type: &str,
    detail: &str,
) -> Result<(), String> {
    connection.execute(
        "INSERT INTO relay_events (id, module_id, event_type, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![Uuid::new_v4().to_string(), module_id, event_type, detail, Utc::now().to_rfc3339()],
    ).map_err(|error| format!("无法记录传话事件：{error}"))?;
    Ok(())
}

fn append_relay_event_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    module_id: &str,
    event_type: &str,
    detail: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO relay_events (id, module_id, event_type, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                module_id,
                event_type,
                detail,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|error| format!("无法记录 Codex 通讯事件：{error}"))?;
    Ok(())
}

fn mark_relay_codex_turn_started(
    connection: &Connection,
    cycle_id: &str,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始 Codex 通讯循环事务：{error}"))?;
    let (module_id, status): (String, String) = transaction
        .query_row(
            "SELECT module_id, status FROM relay_codex_cycles WHERE id = ?1",
            [cycle_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))?
        .ok_or_else(|| "Codex 通讯循环不存在。".to_string())?;
    if status != "WAITING_TO_SEND_CODEX" {
        return Err(format!(
            "Codex 通讯循环当前状态为 `{status}`，不能开始回合。"
        ));
    }
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "UPDATE relay_codex_cycles
             SET status = 'CODEX_RUNNING', codex_thread_id = ?2, codex_turn_id = ?3,
                 codex_started_at = ?4, updated_at = ?4
             WHERE id = ?1",
            params![cycle_id, thread_id, turn_id, &now],
        )
        .map_err(|error| format!("无法记录 Codex 回合启动：{error}"))?;
    append_relay_event_in_transaction(
        &transaction,
        &module_id,
        "CODEX_TURN_STARTED",
        &format!("cycleId={cycle_id}; Codex 回合已开始。"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 Codex 回合启动：{error}"))
}

fn mark_relay_codex_result_received(
    connection: &Connection,
    cycle_id: &str,
    result_text: &str,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始 Codex 结果事务：{error}"))?;
    let (module_id, status, existing_result): (String, String, Option<String>) = transaction
        .query_row(
            "SELECT module_id, status, result_text FROM relay_codex_cycles WHERE id = ?1",
            [cycle_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))?
        .ok_or_else(|| "Codex 通讯循环不存在。".to_string())?;
    if let Some(existing_result) = existing_result {
        if existing_result == result_text {
            return transaction
                .commit()
                .map_err(|error| format!("无法提交重复 Codex 结果读取：{error}"));
        }
        return Err("Codex final text 已持久化，不能覆盖。".into());
    }
    if status != "CODEX_RUNNING" {
        return Err(format!(
            "Codex 通讯循环当前状态为 `{status}`，不能记录 final text。"
        ));
    }
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "UPDATE relay_codex_cycles
             SET status = 'CODEX_COMPLETED', result_text = ?2, codex_completed_at = ?3,
                 updated_at = ?3
             WHERE id = ?1",
            params![cycle_id, result_text, &now],
        )
        .map_err(|error| format!("无法记录 Codex final text：{error}"))?;
    append_relay_event_in_transaction(
        &transaction,
        &module_id,
        "CODEX_RESULT_RECEIVED",
        &format!("cycleId={cycle_id}; 已持久化 Codex final text。"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 Codex final text：{error}"))
}

fn queue_relay_codex_result_to_chatgpt(
    connection: &Connection,
    cycle_id: &str,
) -> Result<String, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始 Codex 回传排队事务：{error}"))?;
    let cycle: RelayCodexCycleRecord = transaction
        .query_row(
            &format!("{RELAY_CODEX_CYCLE_SELECT} WHERE id = ?1"),
            [cycle_id],
            relay_codex_cycle_row,
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))?
        .ok_or_else(|| "Codex 通讯循环不存在。".to_string())?;
    if let Some(message_id) = cycle.outbound_chatgpt_message_id {
        transaction
            .commit()
            .map_err(|error| format!("无法提交已排队 Codex 回传读取：{error}"))?;
        return Ok(message_id);
    }
    if cycle.status != "CODEX_COMPLETED" {
        return Err(format!(
            "Codex 通讯循环当前状态为 `{}`，不能排队回传 ChatGPT。",
            cycle.status
        ));
    }
    let result_text = cycle
        .result_text
        .as_deref()
        .ok_or_else(|| "Codex 通讯循环没有 final text，不能排队回传。".to_string())?;
    let sequence_number: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence_number), 0) + 1 FROM relay_messages WHERE module_id = ?1",
            [&cycle.module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法分配 Codex 回传消息序号：{error}"))?;
    let message_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "INSERT INTO relay_messages (
                id, module_id, sequence_number, direction, kind, text, delivery_state, created_at, delivered_at
             ) VALUES (?1, ?2, ?3, 'TO_CHATGPT', 'AUTOMATION', ?4, 'QUEUED', ?5, NULL)",
            params![
                &message_id,
                &cycle.module_id,
                sequence_number,
                result_text,
                &now
            ],
        )
        .map_err(|error| format!("无法将 Codex 结果加入 ChatGPT 队列：{error}"))?;
    transaction
        .execute(
            "UPDATE relay_codex_cycles
             SET status = 'WAITING_FOR_CHATGPT', outbound_chatgpt_message_id = ?2,
                 relay_queued_at = ?3, updated_at = ?3
             WHERE id = ?1 AND outbound_chatgpt_message_id IS NULL",
            params![cycle_id, &message_id, &now],
        )
        .map_err(|error| format!("无法关联 Codex 回传消息：{error}"))?;
    append_relay_event_in_transaction(
        &transaction,
        &cycle.module_id,
        "CODEX_RESULT_QUEUED_TO_CHATGPT",
        &format!("cycleId={cycle_id}; requestId={message_id}; Codex 结果已加入全局 FIFO。"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 Codex 回传排队：{error}"))?;
    Ok(message_id)
}

fn sync_codex_cycle_for_chatgpt_message_state(
    connection: &Connection,
    message_id: &str,
    delivery_state: &str,
    error_text: Option<&str>,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始 Codex 回传状态同步事务：{error}"))?;
    let Some((cycle_id, module_id, current_status)) = transaction
        .query_row(
            "SELECT id, module_id, status FROM relay_codex_cycles
             WHERE outbound_chatgpt_message_id = ?1",
            [message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 回传循环：{error}"))?
    else {
        return transaction
            .commit()
            .map_err(|error| format!("无法提交非 Codex 回传状态同步：{error}"));
    };

    let (target_status, next_error_text, event_type) = match delivery_state {
        "QUEUED" => ("WAITING_FOR_CHATGPT", None, None),
        "SENT" => (
            "SENDING_TO_CHATGPT",
            None,
            Some("CODEX_RESULT_SEND_STARTED"),
        ),
        "DELIVERED" => (
            "DELIVERED_TO_CHATGPT",
            None,
            Some("CODEX_RESULT_DELIVERED_TO_CHATGPT"),
        ),
        "UNKNOWN" => (
            "WAITING_FOR_CHATGPT",
            Some(error_text.unwrap_or("ChatGPT 回传送达结果不确定，等待人工恢复。")),
            None,
        ),
        "FAILED" => (
            "FAILED",
            Some(error_text.unwrap_or("用户确认不重发 Codex 回传结果。")),
            None,
        ),
        _ => {
            return Err(format!(
                "不支持同步的 ChatGPT 送达状态：`{delivery_state}`。"
            ))
        }
    };
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "UPDATE relay_codex_cycles
             SET status = ?2, error_text = ?3,
                 relay_delivered_at = CASE
                    WHEN ?2 = 'DELIVERED_TO_CHATGPT' AND relay_delivered_at IS NULL THEN ?4
                    ELSE relay_delivered_at
                 END,
                 updated_at = ?4
             WHERE id = ?1",
            params![cycle_id, target_status, next_error_text, &now],
        )
        .map_err(|error| format!("无法同步 Codex 回传状态：{error}"))?;
    if current_status != target_status {
        if let Some(event_type) = event_type {
            append_relay_event_in_transaction(
                &transaction,
                &module_id,
                event_type,
                &format!("cycleId={cycle_id}; requestId={message_id}; ChatGPT 回传状态已变为 {delivery_state}。"),
            )?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("无法提交 Codex 回传状态同步：{error}"))
}

fn fail_relay_codex_cycle(
    connection: &Connection,
    cycle_id: &str,
    error_text: &str,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始 Codex 失败记录事务：{error}"))?;
    let module_id: String = transaction
        .query_row(
            "SELECT module_id FROM relay_codex_cycles
             WHERE id = ?1 AND result_text IS NULL AND outbound_chatgpt_message_id IS NULL",
            [cycle_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取 Codex 通讯循环：{error}"))?
        .ok_or_else(|| "Codex 通讯循环已拥有结果，不能标记为失败。".to_string())?;
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "UPDATE relay_codex_cycles
             SET status = 'FAILED', error_text = ?2, updated_at = ?3
             WHERE id = ?1",
            params![cycle_id, error_text, &now],
        )
        .map_err(|error| format!("无法记录 Codex 回合失败：{error}"))?;
    append_relay_event_in_transaction(
        &transaction,
        &module_id,
        "CODEX_TURN_FAILED",
        &format!("cycleId={cycle_id}; {error_text}"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 Codex 回合失败：{error}"))
}

fn emit_relay_codex_changed(app: &AppHandle, module_id: &str, cycle_id: &str, status: &str) {
    let _ = app.emit(
        "relay-codex",
        json!({ "moduleId": module_id, "cycleId": cycle_id, "status": status }),
    );
}

#[tauri::command]
fn create_relay_module(
    state: State<'_, AppState>,
    input: RelayModuleInput,
) -> Result<RelayModuleRecord, String> {
    validate_relay_module(&input)?;
    if !Path::new(input.working_directory.trim()).is_dir() {
        return Err("所选 Codex 工作目录不存在。".into());
    }
    let now = Utc::now().to_rfc3339();
    let module = RelayModuleRecord {
        id: Uuid::new_v4().to_string(),
        name: input.name.trim().to_string(),
        working_directory: input.working_directory.trim().to_string(),
        max_cycles: input.max_cycles,
        max_runtime_minutes: input.max_runtime_minutes,
        retry_template: input.retry_template.trim().to_string(),
        phase: "READY".into(),
        codex_thread_id: None,
        module_started_at: None,
        stop_after_turn: false,
        invalid_reply_count: 0,
        started_cycles: 0,
        created_at: now.clone(),
        updated_at: now,
    };
    let connection = state
        .connection
        .lock()
        .map_err(|_| "数据库锁已损坏。".to_string())?;
    connection.execute(
        "INSERT INTO relay_modules (id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase, codex_thread_id, module_started_at, stop_after_turn, invalid_reply_count, started_cycles, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, 0, 0, 0, ?8, ?8)",
        params![&module.id, &module.name, &module.working_directory, module.max_cycles, module.max_runtime_minutes, &module.retry_template, &module.phase, &module.created_at],
    ).map_err(|error| format!("无法创建传话模块：{error}"))?;
    append_relay_event(
        &connection,
        &module.id,
        "CREATED",
        "已创建传话模块，尚未开始自动化。",
    )?;
    Ok(module)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayModuleAcceptance {
    Accepted,
    AlreadyCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayModuleTermination {
    Stopped,
    StopRequested,
    AlreadyStopped,
    AlreadyStopRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayCodexTurnCompletion {
    ReturnedToChatGpt,
    StoppedAfterTurn,
}

fn accept_relay_module_in(
    connection: &Connection,
    module_id: &str,
) -> Result<RelayModuleAcceptance, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始模块验收事务：{error}"))?;
    let module = transaction
        .query_row(
            "SELECT id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase,
                    codex_thread_id, module_started_at, stop_after_turn, invalid_reply_count, started_cycles, created_at, updated_at
             FROM relay_modules WHERE id = ?1",
            [module_id],
            relay_row_to_module,
        )
        .optional()
        .map_err(|error| format!("无法读取传话模块：{error}"))?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if module.phase == "COMPLETED" {
        transaction
            .commit()
            .map_err(|error| format!("无法提交模块验收事务：{error}"))?;
        return Ok(RelayModuleAcceptance::AlreadyCompleted);
    }
    if module.phase == "STOPPED" {
        return Err("模块已终止，不能验收完成。".into());
    }
    if module.phase != "WAITING_FOR_ACCEPTANCE" {
        return Err("模块当前未等待人工验收，不能验收完成。".into());
    }
    if module.stop_after_turn {
        return Err("模块正在终止，不能验收完成。".into());
    }

    let unknown_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM relay_messages
             WHERE module_id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法检查不确定送达消息：{error}"))?;
    if unknown_count > 0 {
        return Err("请先处理本模块的不确定送达消息。".into());
    }
    let running_cycle_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM relay_codex_cycles
             WHERE module_id = ?1 AND status = 'CODEX_RUNNING'",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法检查 Codex 回合状态：{error}"))?;
    if running_cycle_count > 0 {
        return Err("当前 Codex 回合仍在运行，不能验收完成。".into());
    }

    let queued_message_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM relay_messages
                 WHERE module_id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'QUEUED'
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|error| format!("无法读取待取消消息：{error}"))?;
        let message_ids = statement
            .query_map([module_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取待取消消息：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法读取待取消消息：{error}"))?;
        message_ids
    };
    for message_id in &queued_message_ids {
        transaction
            .execute(
                "UPDATE relay_messages SET delivery_state = 'FAILED'
                 WHERE id = ?1 AND delivery_state = 'QUEUED'",
                [message_id],
            )
            .map_err(|error| format!("无法取消待发送消息：{error}"))?;
        append_relay_event_in_transaction(
            &transaction,
            module_id,
            "CHATGPT_QUEUED_MESSAGE_CANCELLED",
            &format!("requestId={message_id}; 模块已验收完成，消息未发送。"),
        )?;
    }
    transaction
        .execute(
            "UPDATE relay_modules SET phase = 'COMPLETED', updated_at = ?2 WHERE id = ?1",
            params![module_id, Utc::now().to_rfc3339()],
        )
        .map_err(|error| format!("无法完成传话模块：{error}"))?;
    append_relay_event_in_transaction(
        &transaction,
        module_id,
        "MODULE_ACCEPTED",
        "用户已验收完成模块。",
    )?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交模块验收事务：{error}"))?;
    Ok(RelayModuleAcceptance::Accepted)
}

fn terminate_relay_module_in(
    connection: &Connection,
    module_id: &str,
) -> Result<RelayModuleTermination, String> {
    terminate_relay_module_with_active_turn_in(connection, module_id, false)
}

fn terminate_relay_module_with_active_turn_in(
    connection: &Connection,
    module_id: &str,
    has_active_turn: bool,
) -> Result<RelayModuleTermination, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始终止模块事务：{error}"))?;
    let module = transaction
        .query_row(
            "SELECT id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase,
                    codex_thread_id, module_started_at, stop_after_turn, invalid_reply_count, started_cycles, created_at, updated_at
             FROM relay_modules WHERE id = ?1",
            [module_id],
            relay_row_to_module,
        )
        .optional()
        .map_err(|error| format!("无法读取传话模块：{error}"))?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if module.phase == "STOPPED" {
        transaction
            .commit()
            .map_err(|error| format!("无法提交终止模块事务：{error}"))?;
        return Ok(RelayModuleTermination::AlreadyStopped);
    }
    if module.phase == "COMPLETED" {
        return Err("模块已验收完成，不能终止。".into());
    }
    if module.stop_after_turn {
        transaction
            .commit()
            .map_err(|error| format!("无法提交重复终止模块事务：{error}"))?;
        return Ok(RelayModuleTermination::AlreadyStopRequested);
    }

    let unknown_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM relay_messages
             WHERE module_id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法检查不确定送达消息：{error}"))?;
    if unknown_count > 0 {
        return Err("请先处理本模块的不确定送达消息。".into());
    }
    let running_cycle_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM relay_codex_cycles
             WHERE module_id = ?1 AND status = 'CODEX_RUNNING'",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法检查 Codex 回合状态：{error}"))?;
    let queued_message_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM relay_messages
                 WHERE module_id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'QUEUED'
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|error| format!("无法读取待取消消息：{error}"))?;
        let message_ids = statement
            .query_map([module_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取待取消消息：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法读取待取消消息：{error}"))?;
        message_ids
    };
    for message_id in &queued_message_ids {
        transaction
            .execute(
                "UPDATE relay_messages SET delivery_state = 'FAILED'
                 WHERE id = ?1 AND delivery_state = 'QUEUED'",
                [message_id],
            )
            .map_err(|error| format!("无法取消待发送消息：{error}"))?;
        append_relay_event_in_transaction(
            &transaction,
            module_id,
            "CHATGPT_QUEUED_MESSAGE_CANCELLED",
            &format!("requestId={message_id}; 模块已由用户终止，消息未发送。"),
        )?;
    }
    let outcome = if running_cycle_count > 0 || has_active_turn {
        transaction
            .execute(
                "UPDATE relay_modules SET stop_after_turn = 1, updated_at = ?2 WHERE id = ?1",
                params![module_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| format!("无法记录终止请求：{error}"))?;
        append_relay_event_in_transaction(
            &transaction,
            module_id,
            "MODULE_STOP_REQUESTED",
            "用户已请求在当前 Codex 回合自然结束后终止模块。",
        )?;
        RelayModuleTermination::StopRequested
    } else {
        transaction
            .execute(
                "UPDATE relay_modules
                 SET phase = 'STOPPED', stop_after_turn = 0, updated_at = ?2
                 WHERE id = ?1",
                params![module_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| format!("无法终止传话模块：{error}"))?;
        append_relay_event_in_transaction(
            &transaction,
            module_id,
            "MODULE_TERMINATED",
            "用户已终止模块。",
        )?;
        RelayModuleTermination::Stopped
    };
    transaction
        .commit()
        .map_err(|error| format!("无法提交终止模块事务：{error}"))?;
    Ok(outcome)
}

fn submit_relay_acceptance_feedback_in(
    connection: &Connection,
    module_id: &str,
    text: &str,
) -> Result<String, String> {
    let feedback = text.trim();
    if feedback.is_empty() {
        return Err("验收反馈不能为空。".into());
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法开始验收反馈事务：{error}"))?;
    let module = transaction
        .query_row(
            "SELECT id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase,
                    codex_thread_id, module_started_at, stop_after_turn, invalid_reply_count, started_cycles, created_at, updated_at
             FROM relay_modules WHERE id = ?1",
            [module_id],
            relay_row_to_module,
        )
        .optional()
        .map_err(|error| format!("无法读取传话模块：{error}"))?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if module.phase != "WAITING_FOR_ACCEPTANCE" {
        return Err("模块当前未等待人工验收，不能提交验收反馈。".into());
    }
    if module.stop_after_turn {
        return Err("模块正在终止，不能提交验收反馈。".into());
    }

    let sequence_number: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence_number), 0) + 1 FROM relay_messages WHERE module_id = ?1",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法计算传话消息序号：{error}"))?;
    let message_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at, delivered_at)
             VALUES (?1, ?2, ?3, 'TO_CHATGPT', 'AUTOMATION', ?4, 'QUEUED', ?5, NULL)",
            params![&message_id, module_id, sequence_number, feedback, &now],
        )
        .map_err(|error| format!("无法将验收反馈加入队列：{error}"))?;
    transaction
        .execute(
            "UPDATE relay_modules SET phase = 'WAITING_FOR_CHATGPT', updated_at = ?2 WHERE id = ?1",
            params![module_id, &now],
        )
        .map_err(|error| format!("无法更新验收反馈状态：{error}"))?;
    append_relay_event_in_transaction(
        &transaction,
        module_id,
        "CHATGPT_MESSAGE_QUEUED",
        "已加入 ChatGPT 发送队列。",
    )?;
    append_relay_event_in_transaction(
        &transaction,
        module_id,
        "ACCEPTANCE_FEEDBACK_QUEUED",
        &format!("requestId={message_id}; 已将人工验收反馈加入自动化发送队列。"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交验收反馈事务：{error}"))?;
    Ok(message_id)
}

fn relay_codex_turn_is_active_for_module(state: &AppState, module_id: &str) -> Result<bool, String> {
    let sessions = state
        .relay_codex
        .lock()
        .map_err(|_| "Codex 会话锁已损坏。".to_string())?;
    Ok(sessions.as_ref().is_some_and(|session| {
        session.module_id == module_id && session.turn_active.load(Ordering::SeqCst)
    }))
}

fn finalize_relay_module_stop_after_turn_in(
    connection: &Connection,
    module_id: &str,
) -> Result<bool, String> {
    let changed = connection
        .execute(
            "UPDATE relay_modules
             SET phase = 'STOPPED', stop_after_turn = 0, updated_at = ?2
             WHERE id = ?1 AND stop_after_turn = 1 AND phase NOT IN ('COMPLETED', 'STOPPED')",
            params![module_id, Utc::now().to_rfc3339()],
        )
        .map_err(|error| format!("无法完成终止中的传话模块：{error}"))?;
    Ok(changed > 0)
}

fn relay_codex_start_block_reason(module: &RelayModuleRecord) -> Option<&'static str> {
    if module.stop_after_turn {
        Some("模块正在终止，不能启动新的 Codex 回合。")
    } else if matches!(module.phase.as_str(), "STOPPED" | "COMPLETED") {
        Some("模块已经结束，不能启动新的 Codex 回合。")
    } else {
        None
    }
}

#[tauri::command]
fn accept_relay_module(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
) -> Result<(), String> {
    if relay_codex_turn_is_active_for_module(&state, &module_id)? {
        return Err("当前 Codex 回合仍在运行，不能验收完成。".into());
    }
    let outcome = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        accept_relay_module_in(&connection, &module_id)?
    };
    if outcome == RelayModuleAcceptance::AlreadyCompleted {
        return Ok(());
    }

    match release_relay_codex_runtime(&app, &module_id) {
        Ok(()) => {
            let connection = state
                .connection
                .lock()
                .map_err(|_| "数据库锁已损坏。".to_string())?;
            append_relay_event(
                &connection,
                &module_id,
                "CODEX_THREAD_RELEASED",
                "模块已验收完成，Codex 对话已释放。",
            )?;
        }
        Err(error) => {
            let connection = state
                .connection
                .lock()
                .map_err(|_| "数据库锁已损坏。".to_string())?;
            append_relay_event(
                &connection,
                &module_id,
                "CODEX_THREAD_RELEASE_FAILED",
                &format!("模块已验收完成，但 Codex 对话释放失败：{error}"),
            )?;
            let _ = app.emit(
                "relay-control",
                json!({ "type": "MODULE_ACCEPTED", "moduleId": module_id }),
            );
            emit_relay_codex_changed(&app, &module_id, "", "COMPLETED");
            return Err(format!("模块已验收完成，但无法释放 Codex 对话：{error}"));
        }
    }
    let _ = app.emit(
        "relay-control",
        json!({ "type": "MODULE_ACCEPTED", "moduleId": module_id }),
    );
    emit_relay_codex_changed(&app, &module_id, "", "COMPLETED");
    Ok(())
}

#[tauri::command]
fn terminate_relay_module(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
) -> Result<(), String> {
    let has_active_turn = relay_codex_turn_is_active_for_module(&state, &module_id)?;
    let outcome = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        if has_active_turn {
            terminate_relay_module_with_active_turn_in(&connection, &module_id, true)?
        } else {
            terminate_relay_module_in(&connection, &module_id)?
        }
    };
    if matches!(
        outcome,
        RelayModuleTermination::AlreadyStopped | RelayModuleTermination::AlreadyStopRequested
    ) {
        return Ok(());
    }
    if outcome == RelayModuleTermination::StopRequested {
        let _ = app.emit(
            "relay-control",
            json!({ "type": "MODULE_STOP_REQUESTED", "moduleId": module_id }),
        );
        emit_relay_codex_changed(&app, &module_id, "", "CODEX_RUNNING");
        return Ok(());
    }

    match release_relay_codex_runtime(&app, &module_id) {
        Ok(()) => {
            let connection = state
                .connection
                .lock()
                .map_err(|_| "数据库锁已损坏。".to_string())?;
            append_relay_event(
                &connection,
                &module_id,
                "CODEX_THREAD_RELEASED",
                "模块已终止，Codex 对话已释放。",
            )?;
        }
        Err(error) => {
            let connection = state
                .connection
                .lock()
                .map_err(|_| "数据库锁已损坏。".to_string())?;
            append_relay_event(
                &connection,
                &module_id,
                "CODEX_THREAD_RELEASE_FAILED",
                &format!("模块已终止，但 Codex 对话释放失败：{error}"),
            )?;
            let _ = app.emit(
                "relay-control",
                json!({ "type": "MODULE_TERMINATED", "moduleId": module_id }),
            );
            emit_relay_codex_changed(&app, &module_id, "", "STOPPED");
            return Err(format!("模块已终止，但无法释放 Codex 对话：{error}"));
        }
    }
    let _ = app.emit(
        "relay-control",
        json!({ "type": "MODULE_TERMINATED", "moduleId": module_id }),
    );
    emit_relay_codex_changed(&app, &module_id, "", "STOPPED");
    Ok(())
}

#[tauri::command]
fn submit_relay_acceptance_feedback(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
    text: String,
) -> Result<(), String> {
    let message_id = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        submit_relay_acceptance_feedback_in(&connection, &module_id, &text)?
    };
    let _ = app.emit(
        "relay-control",
        json!({
            "type": "ACCEPTANCE_FEEDBACK_QUEUED",
            "moduleId": module_id,
            "requestId": message_id,
        }),
    );
    dispatch_next_relay_message(&app, &state)
}

#[tauri::command]
fn list_relay_modules(state: State<'_, AppState>) -> Result<Vec<RelayModuleRecord>, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "数据库锁已损坏。".to_string())?;
    let mut statement = connection.prepare(
        "SELECT id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase,
                codex_thread_id, module_started_at, stop_after_turn, invalid_reply_count, started_cycles, created_at, updated_at
         FROM relay_modules ORDER BY updated_at DESC",
    ).map_err(|error| format!("无法查询传话模块：{error}"))?;
    let modules = statement
        .query_map([], relay_row_to_module)
        .map_err(|error| format!("无法读取传话模块：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取传话模块：{error}"))?;
    Ok(modules)
}

#[tauri::command]
fn list_relay_messages(
    state: State<'_, AppState>,
    module_id: String,
) -> Result<Vec<RelayMessageRecord>, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "数据库锁已损坏。".to_string())?;
    let mut statement = connection.prepare(
        "SELECT id, module_id, sequence_number, direction, kind, text, delivery_state, created_at, delivered_at
         FROM relay_messages WHERE module_id = ?1 ORDER BY sequence_number ASC",
    ).map_err(|error| format!("无法查询消息历史：{error}"))?;
    let messages = statement
        .query_map([module_id], relay_message_row)
        .map_err(|error| format!("无法读取消息历史：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取消息历史：{error}"))?;
    Ok(messages)
}

#[tauri::command]
fn list_relay_codex_cycles(
    state: State<'_, AppState>,
    module_id: String,
) -> Result<Vec<RelayCodexCycleRecord>, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "数据库锁已损坏。".to_string())?;
    list_relay_codex_cycles_in(&connection, &module_id)
}

#[tauri::command]
fn get_relay_channel_snapshot(state: State<'_, AppState>) -> Result<RelayChannelSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "数据库锁已损坏。".to_string())?;
    relay_channel_snapshot_from_connection(&connection)
}

#[tauri::command]
fn list_relay_recovery_messages(
    state: State<'_, AppState>,
) -> Result<Vec<RelayRecoveryRecord>, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "数据库锁已损坏。".to_string())?;
    list_relay_recovery_messages_in(&connection)
}

fn list_relay_recovery_messages_in(
    connection: &Connection,
) -> Result<Vec<RelayRecoveryRecord>, String> {
    let mut statement = connection.prepare(
        "SELECT message.id, message.module_id, module.name, message.sequence_number, message.kind, message.created_at
         FROM relay_messages AS message
         JOIN relay_modules AS module ON module.id = message.module_id
         WHERE message.direction = 'TO_CHATGPT' AND message.delivery_state = 'UNKNOWN'
         ORDER BY message.created_at ASC, message.id ASC",
    ).map_err(|error| format!("无法查询待恢复的不确定消息：{error}"))?;
    let messages = statement
        .query_map([], |row| {
            Ok(RelayRecoveryRecord {
                message_id: row.get(0)?,
                module_id: row.get(1)?,
                module_name: row.get(2)?,
                sequence_number: row.get(3)?,
                kind: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|error| format!("无法读取待恢复的不确定消息：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取待恢复的不确定消息：{error}"))?;
    Ok(messages)
}

#[tauri::command]
fn send_chatgpt_message(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    send_chatgpt_message_internal(&app, &state.chatgpt_bridge, &text)
}

fn send_chatgpt_message_internal(
    app: &AppHandle,
    bridge: &ChatGptBridge,
    text: &str,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("a ChatGPT message is required".into());
    }
    let session = bridge
        .session
        .lock()
        .map_err(|_| "ChatGPT bridge lock poisoned".to_string())?
        .clone()
        .ok_or_else(|| "no paired ChatGPT extension is connected".to_string())?;
    bridge.send_to_extension(json!({
        "type": "sendChatGptMessage",
        "sessionId": session.session_id,
        "text": text.trim()
    }))?;
    bridge.set_status(
        app,
        ChatGptBridgeStatus {
            phase: "SENT".into(),
            detail: "已将协议消息发送到绑定的 ChatGPT 标签页。".into(),
            tab_id: Some(session.tab_id),
            protocol_state: None,
        },
    );
    Ok(())
}

fn send_relay_chatgpt_message_internal(
    app: &AppHandle,
    bridge: &ChatGptBridge,
    request_id: &str,
    text: &str,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("传往 ChatGPT 的消息不能为空。".into());
    }
    let session = bridge
        .session
        .lock()
        .map_err(|_| "ChatGPT 桥接锁已损坏。".to_string())?
        .clone()
        .ok_or_else(|| "尚未绑定 ChatGPT 标签页。".to_string())?;
    bridge.send_to_extension(json!({
        "type": "sendChatGptMessage",
        "sessionId": session.session_id,
        "requestId": request_id,
        "text": text,
        "relay": true
    }))?;
    bridge.set_status(
        app,
        ChatGptBridgeStatus {
            phase: "RELAY_SENT".into(),
            detail: "已将传话消息发送到 ChatGPT，正在等待回复。".into(),
            tab_id: Some(session.tab_id),
            protocol_state: None,
        },
    );
    Ok(())
}

fn next_queued_relay_message(
    connection: &Connection,
) -> Result<Option<RelayMessageRecord>, String> {
    connection.query_row(
        "SELECT message.id, message.module_id, message.sequence_number, message.direction,
                message.kind, message.text, message.delivery_state, message.created_at, message.delivered_at
         FROM relay_messages AS message
         JOIN relay_modules AS module ON module.id = message.module_id
         WHERE message.direction = 'TO_CHATGPT'
           AND message.delivery_state = 'QUEUED'
           AND module.phase NOT IN ('COMPLETED', 'STOPPED')
         ORDER BY message.created_at ASC, message.id ASC LIMIT 1",
        [], relay_message_row,
    ).optional().map_err(|error| format!("无法读取待发送消息：{error}"))
}

fn claim_next_relay_message_for_dispatch(
    connection: &Connection,
) -> Result<RelayDispatchClaim, String> {
    let recovery_blocker_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM relay_messages WHERE direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
        [], |row| row.get(0),
    ).map_err(|error| format!("无法检查消息队列：{error}"))?;
    if recovery_blocker_count > 0 {
        return Ok(RelayDispatchClaim::RecoveryBlocked(recovery_blocker_count));
    }
    let already_in_flight: i64 = connection.query_row(
        "SELECT COUNT(*) FROM relay_messages WHERE direction = 'TO_CHATGPT' AND delivery_state = 'SENT'",
        [], |row| row.get(0),
    ).map_err(|error| format!("无法检查消息队列：{error}"))?;
    if already_in_flight > 0 {
        return Ok(RelayDispatchClaim::InFlight);
    }
    let Some(message) = next_queued_relay_message(connection)? else {
        return Ok(RelayDispatchClaim::Empty);
    };
    connection.execute(
        "UPDATE relay_messages SET delivery_state = 'SENT' WHERE id = ?1 AND delivery_state = 'QUEUED'",
        [&message.id],
    ).map_err(|error| format!("无法标记待发送消息：{error}"))?;
    sync_codex_cycle_for_chatgpt_message_state(connection, &message.id, "SENT", None)?;
    append_relay_event(
        connection,
        &message.module_id,
        "CHATGPT_SEND_STARTED",
        &format!(
            "requestId={}; 已从全局 FIFO 队列选择消息，等待 ChatGPT 回复。",
            message.id
        ),
    )?;
    Ok(RelayDispatchClaim::Message(message))
}

fn record_relay_message_dispatched(
    connection: &Connection,
    message: &RelayMessageRecord,
) -> Result<(), String> {
    append_relay_event(
        connection,
        &message.module_id,
        "CHATGPT_SEND_DISPATCHED",
        &format!(
            "requestId={}; 已向本机 WebSocket 发出 sendChatGptMessage。",
            message.id
        ),
    )
}

fn dispatch_next_relay_message(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let claim = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        claim_next_relay_message_for_dispatch(&connection)?
    };
    let message = match claim {
        RelayDispatchClaim::RecoveryBlocked(recovery_blocker_count) => {
            let reason = format!("存在待人工处理的不确定送达消息（{recovery_blocker_count} 条）。请明确重发，或选择不重发并继续；系统不会自动重发。");
            let _ = app.emit(
                "relay-control",
                json!({ "type": "RECOVERY_BLOCKED", "reason": reason }),
            );
            state.chatgpt_bridge.set_status(
                app,
                ChatGptBridgeStatus {
                    phase: "RELAY_RECOVERY_REQUIRED".into(),
                    detail: reason,
                    tab_id: None,
                    protocol_state: None,
                },
            );
            return Ok(());
        }
        RelayDispatchClaim::InFlight | RelayDispatchClaim::Empty => return Ok(()),
        RelayDispatchClaim::Message(message) => message,
    };
    if let Err(error) =
        send_relay_chatgpt_message_internal(app, &state.chatgpt_bridge, &message.id, &message.text)
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        pause_relay_for_uncertain_delivery(
            &connection,
            &message.id,
            "CHATGPT_TRANSPORT_FAILURE",
            &format!("ChatGPT 连接/传输失败，消息送达结果不确定，未自动重发：{error}"),
        )?;
        state.chatgpt_bridge.set_status(
            app,
            ChatGptBridgeStatus {
                phase: "RELAY_RECOVERY_REQUIRED".into(),
                detail: format!("ChatGPT 连接/传输失败，消息送达结果不确定，未自动重发。请检查已绑定标签页和扩展后，再明确决定是否重发：{error}"),
                tab_id: state
                    .chatgpt_bridge
                    .session
                    .lock()
                    .ok()
                    .and_then(|session| session.clone())
                    .map(|session| session.tab_id),
                protocol_state: None,
            },
        );
        return Err(error);
    }
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        record_relay_message_dispatched(&connection, &message)?;
    }
    Ok(())
}

#[tauri::command]
fn queue_relay_message(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
    kind: RelayMessageKind,
    text: String,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("消息不能为空。".into());
    }
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        queue_relay_message_in(&connection, &module_id, kind, &text)?;
    }
    dispatch_next_relay_message(&app, &state)
}

fn queue_relay_message_in(
    connection: &Connection,
    module_id: &str,
    kind: RelayMessageKind,
    text: &str,
) -> Result<(), String> {
    let module = get_relay_module(connection, module_id)?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if matches!(module.phase.as_str(), "STOPPED" | "COMPLETED") {
        return Err("模块已经结束，不能再发送消息。".into());
    }
    let now = Utc::now().to_rfc3339();
    connection.execute(
        "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at, delivered_at)
         VALUES (?1, ?2, ?3, 'TO_CHATGPT', ?4, ?5, 'QUEUED', ?6, NULL)",
        params![Uuid::new_v4().to_string(), module_id, next_relay_sequence(connection, module_id)?, kind.as_db(), text.trim(), now],
    ).map_err(|error| format!("无法将消息加入队列：{error}"))?;
    append_relay_event(
        connection,
        module_id,
        "CHATGPT_MESSAGE_QUEUED",
        "已加入 ChatGPT 发送队列。",
    )?;
    Ok(())
}

fn append_relay_message(
    connection: &Connection,
    module_id: &str,
    direction: &str,
    kind: &str,
    text: &str,
    delivery_state: &str,
) -> Result<(), String> {
    connection.execute(
        "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at, delivered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![Uuid::new_v4().to_string(), module_id, next_relay_sequence(connection, module_id)?, direction, kind, text, delivery_state, Utc::now().to_rfc3339()],
    ).map_err(|error| format!("无法保存传话消息：{error}"))?;
    Ok(())
}

fn set_relay_phase(connection: &Connection, module_id: &str, phase: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE relay_modules SET phase = ?2, updated_at = ?3 WHERE id = ?1",
            params![module_id, phase, Utc::now().to_rfc3339()],
        )
        .map_err(|error| format!("无法更新传话模块状态：{error}"))?;
    Ok(())
}

fn pause_relay_for_uncertain_delivery(
    connection: &Connection,
    message_id: &str,
    event_type: &str,
    detail: &str,
) -> Result<String, String> {
    let module_id: String = connection
        .query_row(
            "SELECT module_id FROM relay_messages WHERE id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'SENT'",
            [message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取待确认的 ChatGPT 消息：{error}"))?
        .ok_or_else(|| "没有待确认的 ChatGPT 消息可标记为不确定。".to_string())?;
    connection
        .execute(
            "UPDATE relay_messages SET delivery_state = 'UNKNOWN' WHERE id = ?1",
            [message_id],
        )
        .map_err(|error| format!("无法保存 ChatGPT 不确定送达状态：{error}"))?;
    sync_codex_cycle_for_chatgpt_message_state(connection, message_id, "UNKNOWN", Some(detail))?;
    let phase: String = connection
        .query_row(
            "SELECT phase FROM relay_modules WHERE id = ?1",
            [&module_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取模块恢复状态：{error}"))?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if !matches!(phase.as_str(), "STOPPED" | "COMPLETED") {
        set_relay_phase(connection, &module_id, "RECOVERY_REQUIRED")?;
    }
    append_relay_event(connection, &module_id, event_type, detail)?;
    Ok(module_id)
}

fn requeue_unknown_relay_message(
    connection: &Connection,
    message_id: &str,
) -> Result<String, String> {
    let module_id: String = connection
        .query_row(
            "SELECT module_id FROM relay_messages WHERE id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
            [message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取待恢复的 ChatGPT 消息：{error}"))?
        .ok_or_else(|| "该消息不是可明确重发的不确定传话消息。".to_string())?;
    connection
        .execute(
            "UPDATE relay_messages SET delivery_state = 'QUEUED', delivered_at = NULL WHERE id = ?1",
            [message_id],
        )
        .map_err(|error| format!("无法重新排队 ChatGPT 消息：{error}"))?;
    sync_codex_cycle_for_chatgpt_message_state(connection, message_id, "QUEUED", None)?;
    set_relay_phase_after_recovery(connection, &module_id)?;
    append_relay_event(
        connection,
        &module_id,
        "CHATGPT_EXPLICIT_RESEND",
        &format!("requestId={message_id}; 用户已明确要求重发此前结果不确定的消息。"),
    )?;
    Ok(module_id)
}

fn set_relay_phase_after_recovery(connection: &Connection, module_id: &str) -> Result<(), String> {
    let phase: String = connection
        .query_row(
            "SELECT phase FROM relay_modules WHERE id = ?1",
            [module_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取模块恢复状态：{error}"))?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if matches!(phase.as_str(), "STOPPED" | "COMPLETED") {
        return Ok(());
    }
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM relay_messages
         WHERE module_id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
            [module_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法检查模块恢复状态：{error}"))?;
    set_relay_phase(
        connection,
        module_id,
        if remaining > 0 {
            "RECOVERY_REQUIRED"
        } else {
            "READY"
        },
    )
}

fn resolve_unknown_relay_message_without_resend(
    connection: &Connection,
    message_id: &str,
) -> Result<String, String> {
    let module_id: String = connection
        .query_row(
            "SELECT module_id FROM relay_messages WHERE id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'UNKNOWN'",
            [message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取待恢复的 ChatGPT 消息：{error}"))?
        .ok_or_else(|| "该消息不是可解除阻塞的不确定传话消息。".to_string())?;
    let detail =
        format!("requestId={message_id}; 用户确认不重发该送达结果不确定的消息，并解除其阻塞。");
    connection
        .execute(
            "UPDATE relay_messages SET delivery_state = 'FAILED' WHERE id = ?1",
            [message_id],
        )
        .map_err(|error| format!("无法保存不重发决定：{error}"))?;
    sync_codex_cycle_for_chatgpt_message_state(connection, message_id, "FAILED", Some(&detail))?;
    set_relay_phase_after_recovery(connection, &module_id)?;
    append_relay_event(
        connection,
        &module_id,
        "CHATGPT_EXPLICIT_CONTINUE_WITHOUT_RESEND",
        &detail,
    )?;
    Ok(module_id)
}

#[tauri::command]
fn retry_unknown_relay_message(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: String,
) -> Result<(), String> {
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        requeue_unknown_relay_message(&connection, &message_id)?;
    }
    dispatch_next_relay_message(&app, &state)
}

#[tauri::command]
fn continue_unknown_relay_message_without_resend(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: String,
) -> Result<(), String> {
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        resolve_unknown_relay_message_without_resend(&connection, &message_id)?;
    }
    dispatch_next_relay_message(&app, &state)
}

fn handle_relay_chatgpt_adapter_failure(
    app: AppHandle,
    adapter_error: &str,
    adapter_diagnostic: Option<&str>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        let message_id: String = connection
            .query_row(
                "SELECT id FROM relay_messages
                 WHERE direction = 'TO_CHATGPT' AND delivery_state = 'SENT'
                 ORDER BY created_at ASC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("无法匹配 ChatGPT 适配器错误：{error}"))?
            .ok_or_else(|| "收到没有对应待发送消息的 ChatGPT 适配器错误。".to_string())?;
        let diagnostic = adapter_diagnostic
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!(" 诊断：{value}"))
            .unwrap_or_default();
        pause_relay_for_uncertain_delivery(
            &connection,
            &message_id,
            "CHATGPT_ADAPTER_FAILURE",
            &format!(
                "ChatGPT 适配器失败，消息送达结果不确定，未自动重发：{adapter_error}{diagnostic}"
            ),
        )?;
    }
    let reason = format!("ChatGPT 浏览器适配器失败，消息送达结果不确定，未自动重发。请检查已绑定标签页和扩展后，再明确决定是否重发：{adapter_error}");
    let _ = app.emit(
        "relay-control",
        json!({ "type": "ADAPTER_FAILURE", "reason": reason }),
    );
    state.chatgpt_bridge.set_status(
        &app,
        ChatGptBridgeStatus {
            phase: "RELAY_RECOVERY_REQUIRED".into(),
            detail: reason,
            tab_id: state
                .chatgpt_bridge
                .session
                .lock()
                .ok()
                .and_then(|session| session.clone())
                .map(|session| session.tab_id),
            protocol_state: None,
        },
    );
    Ok(())
}

struct RelayChatGptReplyOutcome {
    outgoing: (String, String, String),
    module: RelayModuleRecord,
    terminal_ignored: bool,
}

fn process_relay_chatgpt_reply_in(
    connection: &Connection,
    request_id: Option<&str>,
    reply: &str,
) -> Result<RelayChatGptReplyOutcome, String> {
    let outgoing: (String, String, String) = if let Some(request_id) = request_id {
        connection
            .query_row(
                "SELECT id, module_id, kind FROM relay_messages
                 WHERE id = ?1 AND direction = 'TO_CHATGPT' AND delivery_state = 'SENT'",
                [request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| format!("无法匹配 ChatGPT 回复 requestId：{error}"))?
    } else {
        connection
            .query_row(
                "SELECT id, module_id, kind FROM relay_messages
                 WHERE direction = 'TO_CHATGPT' AND delivery_state = 'SENT'
                 ORDER BY created_at ASC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| format!("无法匹配 ChatGPT 回复：{error}"))?
    }
    .ok_or_else(|| "收到没有对应待发送消息的 ChatGPT 回复。".to_string())?;
    append_relay_event(
        connection,
        &outgoing.1,
        "CHATGPT_REPLY_RECEIVED",
        &format!("requestId={}; Rust 已收到 chatgptReply。", outgoing.0),
    )?;
    connection
        .execute(
            "UPDATE relay_messages SET delivery_state = 'DELIVERED', delivered_at = ?2 WHERE id = ?1",
            params![outgoing.0, Utc::now().to_rfc3339()],
        )
        .map_err(|error| format!("无法确认 ChatGPT 消息送达：{error}"))?;
    sync_codex_cycle_for_chatgpt_message_state(connection, &outgoing.0, "DELIVERED", None)?;
    append_relay_message(
        connection,
        &outgoing.1,
        "FROM_CHATGPT",
        &outgoing.2,
        reply,
        "DELIVERED",
    )?;
    append_relay_event(
        connection,
        &outgoing.1,
        "CHATGPT_REPLY_PERSISTED",
        &format!("requestId={}; 已持久化 FROM_CHATGPT。", outgoing.0),
    )?;

    let module =
        get_relay_module(connection, &outgoing.1)?.ok_or_else(|| "传话模块不存在。".to_string())?;
    let terminal_ignored = matches!(module.phase.as_str(), "STOPPED" | "COMPLETED");
    if terminal_ignored {
        append_relay_event(
            connection,
            &module.id,
            "LATE_CHATGPT_REPLY_IGNORED",
            &format!(
                "requestId={}; 模块已结束，已保存迟到 ChatGPT 回复但不会继续自动化。",
                outgoing.0
            ),
        )?;
    }
    Ok(RelayChatGptReplyOutcome {
        outgoing,
        module,
        terminal_ignored,
    })
}

fn handle_relay_chatgpt_reply(
    app: AppHandle,
    request_id: Option<&str>,
    reply: &str,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (module_id, outgoing_kind, next_action) = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        let reply_outcome = process_relay_chatgpt_reply_in(&connection, request_id, reply)?;
        let outgoing = reply_outcome.outgoing;
        if reply_outcome.terminal_ignored {
            (outgoing.1, outgoing.2, None)
        } else if outgoing.2 == "MANUAL" {
            append_relay_event(
                &connection,
                &outgoing.1,
                "MANUAL_REPLY",
                "已收到手动聊天回复；该回复不会触发自动化。",
            )?;
            (outgoing.1, outgoing.2, None)
        } else {
            let module = reply_outcome.module;
            match relay_protocol::parse_terminal_control_block(reply) {
                Ok(relay_protocol::ControlBlock::CodexPrompt(prompt)) => {
                    connection.execute(
                        "UPDATE relay_modules SET phase = 'CODEX_PROMPT_READY', invalid_reply_count = 0, updated_at = ?2 WHERE id = ?1",
                        params![&module.id, Utc::now().to_rfc3339()],
                    ).map_err(|error| format!("无法保存 Codex 提示词：{error}"))?;
                    append_relay_message(
                        &connection,
                        &module.id,
                        "TO_CODEX",
                        "AUTOMATION",
                        &prompt,
                        "QUEUED",
                    )?;
                    let cycle = create_relay_codex_cycle(
                        &connection,
                        &module.id,
                        module.started_cycles + 1,
                        &prompt,
                    )?;
                    append_relay_event(
                        &connection,
                        &module.id,
                        "CODEX_PROMPT_RECEIVED",
                        &format!(
                            "cycleId={}; 已识别有效的 Codex 提示词，等待执行适配器接管。",
                            cycle.id
                        ),
                    )?;
                    (
                        outgoing.1,
                        outgoing.2,
                        Some(json!({
                            "type": "CODEX_PROMPT",
                            "prompt": prompt,
                            "cycleId": cycle.id,
                        })),
                    )
                }
                Ok(relay_protocol::ControlBlock::ModuleDone) => {
                    set_relay_phase(&connection, &module.id, "WAITING_FOR_ACCEPTANCE")?;
                    append_relay_event(
                        &connection,
                        &module.id,
                        "MODULE_DONE",
                        "ChatGPT 请求人工验收。",
                    )?;
                    (
                        outgoing.1,
                        outgoing.2,
                        Some(json!({ "type": "MODULE_DONE" })),
                    )
                }
                Ok(relay_protocol::ControlBlock::Blocked(reason)) => {
                    set_relay_phase(&connection, &module.id, "BLOCKED")?;
                    append_relay_event(&connection, &module.id, "BLOCKED", &reason)?;
                    (
                        outgoing.1,
                        outgoing.2,
                        Some(json!({ "type": "BLOCKED", "reason": reason })),
                    )
                }
                Err(error) => {
                    let next_invalid_count = module.invalid_reply_count + 1;
                    connection.execute(
                        "UPDATE relay_modules SET invalid_reply_count = ?2, updated_at = ?3 WHERE id = ?1",
                        params![&module.id, next_invalid_count, Utc::now().to_rfc3339()],
                    ).map_err(|database_error| format!("无法记录协议错误：{database_error}"))?;
                    if next_invalid_count == 1 {
                        append_relay_message(
                            &connection,
                            &module.id,
                            "TO_CHATGPT",
                            "AUTOMATION",
                            &module.retry_template,
                            "QUEUED",
                        )?;
                        append_relay_event(
                            &connection,
                            &module.id,
                            "CONTROL_RETRY",
                            &format!("自动化回复无效，已排队一次重试：{error}"),
                        )?;
                        (
                            outgoing.1,
                            outgoing.2,
                            Some(json!({ "type": "RETRY", "reason": error })),
                        )
                    } else {
                        set_relay_phase(&connection, &module.id, "BLOCKED")?;
                        append_relay_event(
                            &connection,
                            &module.id,
                            "CONTROL_FAILED",
                            &format!("第二次自动化回复无效：{error}"),
                        )?;
                        (
                            outgoing.1,
                            outgoing.2,
                            Some(
                                json!({ "type": "ERROR", "reason": format!("ChatGPT 连续两次未给出有效控制块：{error}") }),
                            ),
                        )
                    }
                }
            }
        }
    };
    if let Some(action) = &next_action {
        let _ = app.emit("relay-control", action);
        if action.get("type").and_then(Value::as_str) == Some("CODEX_PROMPT") {
            if let Some(cycle_id) = action.get("cycleId").and_then(Value::as_str) {
                emit_relay_codex_changed(&app, &module_id, cycle_id, "WAITING_TO_SEND_CODEX");
            }
        }
    }
    state.chatgpt_bridge.set_status(
        &app,
        ChatGptBridgeStatus {
            phase: if outgoing_kind == "MANUAL" {
                "RELAY_MANUAL_REPLY".into()
            } else {
                "RELAY_AUTOMATION_REPLY".into()
            },
            detail: if outgoing_kind == "MANUAL" {
                "已收到手动 ChatGPT 回复。".into()
            } else {
                "已收到自动化 ChatGPT 回复。".into()
            },
            tab_id: state
                .chatgpt_bridge
                .session
                .lock()
                .ok()
                .and_then(|session| session.clone())
                .map(|session| session.tab_id),
            protocol_state: None,
        },
    );
    dispatch_next_relay_message(&app, &state)?;
    if let Some(action) = next_action {
        if action.get("type").and_then(Value::as_str) == Some("CODEX_PROMPT") {
            let prompt = action
                .get("prompt")
                .and_then(Value::as_str)
                .ok_or_else(|| "Codex 提示词事件缺少正文。".to_string())?;
            let cycle_id = action
                .get("cycleId")
                .and_then(Value::as_str)
                .ok_or_else(|| "Codex 提示词事件缺少 cycle ID。".to_string())?;
            start_or_continue_relay_codex_turn(&app, &module_id, prompt, cycle_id)?;
        }
    }
    Ok(())
}

fn mark_relay_codex_turn_starting_in(connection: &Connection, module_id: &str) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "UPDATE relay_modules SET phase = 'CODEX_STARTING', started_cycles = started_cycles + 1,
             module_started_at = COALESCE(module_started_at, ?2), updated_at = ?2 WHERE id = ?1",
            params![module_id, now],
        )
        .map_err(|error| format!("无法启动 Codex 回合：{error}"))?;
    Ok(())
}

fn send_relay_codex_start_turn(
    session: &RelayCodexSession,
    cycle_id: &str,
    prompt: &str,
) -> Result<(), ()> {
    session.turn_active.store(true, Ordering::SeqCst);
    session
        .commands
        .send(RelayCodexCommand::StartTurn {
            cycle_id: cycle_id.to_string(),
            prompt: prompt.to_string(),
        })
        .map_err(|_| ())
}

fn start_or_continue_relay_codex_turn(
    app: &AppHandle,
    module_id: &str,
    prompt: &str,
    cycle_id: &str,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let module = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        let module = get_relay_module(&connection, module_id)?
            .ok_or_else(|| "传话模块不存在。".to_string())?;
        if let Some(reason) = relay_codex_start_block_reason(&module) {
            fail_relay_codex_cycle(&connection, cycle_id, reason)?;
            emit_relay_codex_changed(app, module_id, cycle_id, "FAILED");
            return Err(reason.into());
        }
        if module.started_cycles >= module.max_cycles {
            set_relay_phase(&connection, module_id, "WAITING_FOR_ACCEPTANCE")?;
            append_relay_event(
                &connection,
                module_id,
                "CYCLE_BUDGET_REACHED",
                "已达到最大自动化循环次数，等待人工验收。",
            )?;
            let reason = "已达到最大自动化循环次数，不能启动新的 Codex 回合。";
            fail_relay_codex_cycle(&connection, cycle_id, reason)?;
            emit_relay_codex_changed(app, module_id, cycle_id, "FAILED");
            return Err(reason.into());
        }
        if let Some(started) = module.module_started_at.as_deref() {
            let started = DateTime::parse_from_rfc3339(started)
                .map_err(|error| format!("模块开始时间无法读取：{error}"))?
                .with_timezone(&Utc);
            if Utc::now() - started >= Duration::minutes(module.max_runtime_minutes) {
                set_relay_phase(&connection, module_id, "WAITING_FOR_ACCEPTANCE")?;
                append_relay_event(
                    &connection,
                    module_id,
                    "RUNTIME_BUDGET_REACHED",
                    "已达到模块最长运行时间，等待人工验收。",
                )?;
                let reason = "已达到模块最长运行时间，不能启动新的 Codex 回合。";
                fail_relay_codex_cycle(&connection, cycle_id, reason)?;
                emit_relay_codex_changed(app, module_id, cycle_id, "FAILED");
                return Err(reason.into());
            }
        }
        mark_relay_codex_turn_starting_in(&connection, module_id)?;
        module
    };

    let mut sessions = state
        .relay_codex
        .lock()
        .map_err(|_| "Codex 会话锁已损坏。".to_string())?;
    if let Some(session) = sessions.as_ref() {
        if session.module_id != module_id {
            let reason = "另一个模块正在持有 Codex 对话；当前中间件一次只能运行一个模块。";
            drop(sessions);
            relay_codex_failed(app, module_id, Some(cycle_id), reason.into());
            return Err(reason.into());
        }
        if send_relay_codex_start_turn(session, cycle_id, prompt).is_err() {
            let reason = "Codex 对话已经退出。".to_string();
            drop(sessions);
            relay_codex_failed(app, module_id, Some(cycle_id), reason.clone());
            return Err(reason);
        }
    } else {
        if !Path::new(&module.working_directory).is_dir() {
            let reason = "所选 Codex 工作目录不存在。".to_string();
            drop(sessions);
            relay_codex_failed(app, module_id, Some(cycle_id), reason.clone());
            return Err(reason);
        }
        let (sender, receiver) = std_mpsc::channel();
        let turn_active = Arc::new(AtomicBool::new(true));
        let app_for_worker = app.clone();
        let working_directory = module.working_directory.clone();
        let worker_module_id = module_id.to_string();
        let initial_cycle_id = cycle_id.to_string();
        let worker_turn_active = turn_active.clone();
        std::thread::spawn(move || {
            relay_codex_worker(
                app_for_worker,
                worker_module_id,
                working_directory,
                initial_cycle_id,
                receiver,
                worker_turn_active,
            )
        });
        let send_result = sender.send(RelayCodexCommand::StartTurn {
            cycle_id: cycle_id.to_string(),
            prompt: prompt.to_string(),
        });
        if send_result.is_err() {
            let reason = "无法启动 Codex 对话。".to_string();
            drop(sessions);
            relay_codex_failed(app, module_id, Some(cycle_id), reason.clone());
            return Err(reason);
        }
        *sessions = Some(RelayCodexSession {
            module_id: module.id,
            commands: sender,
            turn_active,
        });
    }
    let _ = app.emit(
        "relay-codex-status",
        json!({ "phase": "CODEX_STARTING", "moduleId": module_id, "cycleId": cycle_id }),
    );
    Ok(())
}

fn relay_codex_worker(
    app: AppHandle,
    module_id: String,
    working_directory: String,
    initial_cycle_id: String,
    commands: std_mpsc::Receiver<RelayCodexCommand>,
    turn_active: Arc<AtomicBool>,
) {
    let command = codex_command();
    let child = Command::new(&command)
        .arg("app-server")
        .current_dir(&working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        relay_codex_failed(
            &app,
            &module_id,
            Some(initial_cycle_id.as_str()),
            "无法启动本地 Codex App Server。".into(),
        );
        return;
    };
    let Some(mut stdin) = child.stdin.take() else {
        relay_codex_failed(
            &app,
            &module_id,
            Some(initial_cycle_id.as_str()),
            "Codex App Server 没有可用输入流。".into(),
        );
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        relay_codex_failed(
            &app,
            &module_id,
            Some(initial_cycle_id.as_str()),
            "Codex App Server 没有可用输出流。".into(),
        );
        return;
    };
    let Some(stderr) = child.stderr.take() else {
        relay_codex_failed(
            &app,
            &module_id,
            Some(initial_cycle_id.as_str()),
            "Codex App Server 没有可用错误流。".into(),
        );
        return;
    };
    let (events_sender, events) = std_mpsc::channel::<Result<Value, String>>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let event = line
                .map_err(|error| format!("无法读取 Codex 输出：{error}"))
                .and_then(|line| {
                    serde_json::from_str(&line)
                        .map_err(|error| format!("Codex 输出不是 JSON：{error}"))
                });
            if events_sender.send(event).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});
    if let Err(error) = send_rpc(
        &mut stdin,
        json!({ "method": "initialize", "id": 1, "params": { "clientInfo": { "name": "chatgpt-codex-middleware", "title": "ChatGPT × Codex Middleware", "version": env!("CARGO_PKG_VERSION") } } }),
    ) {
        relay_codex_failed(&app, &module_id, Some(initial_cycle_id.as_str()), error);
        return;
    }

    let mut thread_id: Option<String> = None;
    let mut pending_turn: Option<(String, String)> = None;
    let mut active_cycle_id = Some(initial_cycle_id);
    let mut next_request_id = 3_i64;
    let mut final_summary = String::new();
    let mut release_acknowledgement: Option<std_mpsc::Sender<Result<(), String>>> = None;
    'worker: loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                RelayCodexCommand::StartTurn { cycle_id, prompt } => {
                    turn_active.store(true, Ordering::SeqCst);
                    if active_cycle_id.is_some() && pending_turn.is_none() && thread_id.is_some() {
                        relay_codex_failed(
                            &app,
                            &module_id,
                            Some(cycle_id.as_str()),
                            "上一 Codex 回合尚未结束，不能启动新的回合。".into(),
                        );
                        continue;
                    }
                    active_cycle_id = Some(cycle_id.clone());
                    if let Some(thread) = thread_id.as_deref() {
                        if let Err(error) = send_rpc(
                            &mut stdin,
                            json!({ "method": "turn/start", "id": next_request_id, "params": { "threadId": thread, "input": [{ "type": "text", "text": prompt }] } }),
                        ) {
                            relay_codex_failed(&app, &module_id, active_cycle_id.as_deref(), error);
                            break 'worker;
                        }
                        if let Err(error) = relay_codex_turn_started(
                            &app,
                            &module_id,
                            &cycle_id,
                            Some(thread),
                            None,
                        ) {
                            relay_codex_failed(&app, &module_id, Some(cycle_id.as_str()), error);
                            break 'worker;
                        }
                        next_request_id += 1;
                        final_summary.clear();
                    } else {
                        pending_turn = Some((cycle_id, prompt));
                    }
                }
                RelayCodexCommand::Release { acknowledgement } => {
                    if active_cycle_id.is_some() || pending_turn.is_some() {
                        let _ = acknowledgement.send(Err(
                            "当前 Codex 回合仍在运行，不能释放 Codex 对话。".into(),
                        ));
                    } else {
                        release_acknowledgement = Some(acknowledgement);
                        break 'worker;
                    }
                }
            }
        }
        match events.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(Err(error)) => {
                relay_codex_failed(&app, &module_id, active_cycle_id.as_deref(), error);
                break;
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                relay_codex_failed(
                    &app,
                    &module_id,
                    active_cycle_id.as_deref(),
                    "Codex App Server 已在回合完成前退出。".into(),
                );
                break;
            }
            Ok(Ok(message)) => {
                if let Some(error) = message
                    .get("error")
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                {
                    relay_codex_failed(
                        &app,
                        &module_id,
                        active_cycle_id.as_deref(),
                        format!("Codex App Server 错误：{error}"),
                    );
                    break;
                }
                match message.get("id").and_then(Value::as_i64) {
                    Some(1) => {
                        if send_rpc(&mut stdin, json!({ "method": "initialized", "params": {} }))
                            .and_then(|_| {
                                send_rpc(
                                    &mut stdin,
                                    json!({ "method": "thread/start", "id": 2, "params": {} }),
                                )
                            })
                            .is_err()
                        {
                            relay_codex_failed(
                                &app,
                                &module_id,
                                active_cycle_id.as_deref(),
                                "无法初始化 Codex 对话。".into(),
                            );
                            break;
                        }
                    }
                    Some(2) => {
                        thread_id = message
                            .pointer("/result/thread/id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        let Some(thread) = thread_id.as_deref() else {
                            relay_codex_failed(
                                &app,
                                &module_id,
                                active_cycle_id.as_deref(),
                                "Codex 未返回对话 ID。".into(),
                            );
                            break;
                        };
                        relay_codex_thread_ready(&app, &module_id, thread);
                        if let Some((cycle_id, prompt)) = pending_turn.take() {
                            if let Err(error) = send_rpc(
                                &mut stdin,
                                json!({ "method": "turn/start", "id": next_request_id, "params": { "threadId": thread, "input": [{ "type": "text", "text": prompt }] } }),
                            ) {
                                relay_codex_failed(
                                    &app,
                                    &module_id,
                                    Some(cycle_id.as_str()),
                                    error,
                                );
                                break;
                            }
                            if let Err(error) = relay_codex_turn_started(
                                &app,
                                &module_id,
                                &cycle_id,
                                Some(thread),
                                None,
                            ) {
                                relay_codex_failed(
                                    &app,
                                    &module_id,
                                    Some(cycle_id.as_str()),
                                    error,
                                );
                                break;
                            }
                            next_request_id += 1;
                            final_summary.clear();
                        }
                    }
                    Some(_) => {}
                    None => {}
                }
                if message.get("method").and_then(Value::as_str) == Some("item/agentMessage/delta")
                {
                    if let Some(delta) = message.pointer("/params/delta").and_then(Value::as_str) {
                        final_summary.push_str(delta);
                    }
                }
                if message.get("method").and_then(Value::as_str) == Some("item/completed")
                    && message.pointer("/params/item/type").and_then(Value::as_str)
                        == Some("agentMessage")
                {
                    if let Some(text) = message.pointer("/params/item/text").and_then(Value::as_str)
                    {
                        final_summary = text.to_string();
                    }
                }
                if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
                    let status = message
                        .pointer("/params/turn/status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    if status == "completed" {
                        if let Some(cycle_id) = active_cycle_id.take() {
                            turn_active.store(false, Ordering::SeqCst);
                            relay_codex_turn_completed(
                                &app,
                                &module_id,
                                &cycle_id,
                                final_summary.trim(),
                            );
                        } else {
                            relay_codex_failed(
                                &app,
                                &module_id,
                                None,
                                "Codex 回合完成事件没有对应的通讯循环。".into(),
                            );
                        }
                    } else {
                        turn_active.store(false, Ordering::SeqCst);
                        relay_codex_failed(
                            &app,
                            &module_id,
                            active_cycle_id.as_deref(),
                            format!("Codex 回合以 `{status}` 结束。"),
                        );
                        active_cycle_id = None;
                    }
                }
            }
        }
    }
    turn_active.store(false, Ordering::SeqCst);
    drop(stdin);
    let _ = child.kill();
    let release_result = child
        .wait()
        .map(|_| ())
        .map_err(|error| format!("无法结束 Codex 对话：{error}"));
    let state = app.state::<AppState>();
    let cleared_session = clear_relay_codex_session_if_matches(&state.relay_codex, &module_id);
    if release_acknowledgement.is_none() && cleared_session {
        if let Ok(connection) = state.connection.lock() {
            let stopped = connection
                .query_row(
                    "SELECT phase = 'STOPPED' FROM relay_modules WHERE id = ?1",
                    [&module_id],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false);
            if stopped {
                let _ = append_relay_event(
                    &connection,
                    &module_id,
                    "CODEX_THREAD_RELEASED",
                    "模块已终止，Codex 对话已在当前回合收尾后释放。",
                );
            }
        }
    }
    if let Some(acknowledgement) = release_acknowledgement {
        let _ = acknowledgement.send(release_result);
    }
}

fn relay_codex_thread_ready(app: &AppHandle, module_id: &str, thread_id: &str) {
    let state = app.state::<AppState>();
    if let Ok(connection) = state.connection.lock() {
        let _ = connection.execute(
            "UPDATE relay_modules SET codex_thread_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![module_id, thread_id, Utc::now().to_rfc3339()],
        );
        let _ = append_relay_event(
            &connection,
            module_id,
            "CODEX_THREAD_STARTED",
            "已创建中间件持有的 Codex 对话。",
        );
    };
}

fn relay_codex_turn_started(
    app: &AppHandle,
    module_id: &str,
    cycle_id: &str,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        mark_relay_codex_turn_started(&connection, cycle_id, thread_id, turn_id)?;
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'CODEX_RUNNING', updated_at = ?2 WHERE id = ?1",
                params![module_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| format!("无法更新 Codex 运行状态：{error}"))?;
    }
    emit_relay_codex_changed(app, module_id, cycle_id, "CODEX_RUNNING");
    let _ = app.emit(
        "relay-codex-status",
        json!({
            "phase": "CODEX_RUNNING",
            "moduleId": module_id,
            "cycleId": cycle_id,
            "threadId": thread_id,
            "turnId": turn_id,
        }),
    );
    Ok(())
}

fn complete_relay_codex_turn_in(
    connection: &Connection,
    module_id: &str,
    cycle_id: &str,
    summary: &str,
) -> Result<RelayCodexTurnCompletion, String> {
    let summary = if summary.is_empty() {
        "Codex 回合已完成，但没有返回文字。"
    } else {
        summary
    };
    let already_received = get_relay_codex_cycle_by_id(connection, cycle_id)?
        .ok_or_else(|| "Codex 通讯循环不存在。".to_string())?
        .result_text
        .is_some();
    mark_relay_codex_result_received(connection, cycle_id, summary)?;
    if !already_received {
        append_relay_message(
            connection,
            module_id,
            "FROM_CODEX",
            "AUTOMATION",
            summary,
            "DELIVERED",
        )?;
    }
    let module = get_relay_module(connection, module_id)?
        .ok_or_else(|| "传话模块不存在。".to_string())?;
    if module.stop_after_turn {
        if !finalize_relay_module_stop_after_turn_in(connection, module_id)? {
            return Err("终止中的传话模块状态已被并发更新。".into());
        }
        return Ok(RelayCodexTurnCompletion::StoppedAfterTurn);
    }
    if module.phase == "STOPPED" {
        return Ok(RelayCodexTurnCompletion::StoppedAfterTurn);
    }
    if module.phase == "COMPLETED" {
        return Err("模块已验收完成，不能回传 Codex 结果。".into());
    }
    queue_relay_codex_result_to_chatgpt(connection, cycle_id)?;
    set_relay_phase(connection, module_id, "READY")?;
    Ok(RelayCodexTurnCompletion::ReturnedToChatGpt)
}

fn finish_stopped_relay_codex_runtime(app: AppHandle, module_id: String, cycle_id: String) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let release_result = release_relay_codex_runtime(&app, &module_id);
        if let Ok(connection) = state.connection.lock() {
            let (event_type, detail) = match release_result {
                Ok(()) => (
                    "CODEX_THREAD_RELEASED",
                    "模块已终止，当前 Codex 回合结束后已释放 Codex 对话。".to_string(),
                ),
                Err(error) => (
                    "CODEX_THREAD_RELEASE_FAILED",
                    format!("模块已终止，但 Codex 对话释放失败：{error}"),
                ),
            };
            let _ = append_relay_event(&connection, &module_id, event_type, &detail);
        }
        emit_relay_codex_changed(&app, &module_id, &cycle_id, "STOPPED");
        let _ = app.emit(
            "relay-control",
            json!({ "type": "MODULE_TERMINATED", "moduleId": module_id }),
        );
    });
}

fn relay_codex_turn_completed(app: &AppHandle, module_id: &str, cycle_id: &str, summary: &str) {
    let state = app.state::<AppState>();
    let result = (|| -> Result<RelayCodexTurnCompletion, String> {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "数据库锁已损坏。".to_string())?;
        complete_relay_codex_turn_in(&connection, module_id, cycle_id, summary)
    })();
    match result {
        Ok(RelayCodexTurnCompletion::ReturnedToChatGpt) => {
            emit_relay_codex_changed(app, module_id, cycle_id, "CODEX_COMPLETED");
            emit_relay_codex_changed(app, module_id, cycle_id, "WAITING_FOR_CHATGPT");
            let _ = app.emit(
                "relay-codex-status",
                json!({ "phase": "CODEX_COMPLETED", "moduleId": module_id, "cycleId": cycle_id }),
            );
            let _ = dispatch_next_relay_message(app, &state);
        }
        Ok(RelayCodexTurnCompletion::StoppedAfterTurn) => {
            emit_relay_codex_changed(app, module_id, cycle_id, "CODEX_COMPLETED");
            let _ = app.emit(
                "relay-codex-status",
                json!({ "phase": "CODEX_COMPLETED", "moduleId": module_id, "cycleId": cycle_id }),
            );
            finish_stopped_relay_codex_runtime(
                app.clone(),
                module_id.to_string(),
                cycle_id.to_string(),
            );
        }
        Err(error) => relay_codex_failed(app, module_id, Some(cycle_id), error),
    }
}

fn relay_codex_failed(app: &AppHandle, module_id: &str, cycle_id: Option<&str>, reason: String) {
    let state = app.state::<AppState>();
    let mut stopped_after_turn = false;
    if let Ok(connection) = state.connection.lock() {
        if let Some(cycle_id) = cycle_id {
            let _ = fail_relay_codex_cycle(&connection, cycle_id, &reason);
            emit_relay_codex_changed(app, module_id, cycle_id, "FAILED");
        }
        let stop_requested = get_relay_module(&connection, module_id)
            .ok()
            .flatten()
            .is_some_and(|module| module.stop_after_turn);
        if stop_requested {
            stopped_after_turn = finalize_relay_module_stop_after_turn_in(&connection, module_id)
                .unwrap_or(false);
        } else {
            let _ = set_relay_phase(&connection, module_id, "BLOCKED");
        }
        let _ = append_relay_event(&connection, module_id, "CODEX_FAILED", &reason);
    }
    if !stopped_after_turn {
        clear_relay_codex_session_if_matches(&state.relay_codex, module_id);
    } else {
        mark_relay_codex_session_turn_inactive(&state.relay_codex, module_id);
        finish_stopped_relay_codex_runtime(
            app.clone(),
            module_id.to_string(),
            cycle_id.unwrap_or_default().to_string(),
        );
    }
    let _ = app.emit(
        "relay-control",
        json!({ "type": "ERROR", "reason": reason }),
    );
}

fn module_planning_message(module: &ModuleRecord) -> String {
    format!(
        "你是“{}”模块的规划与 Review 决策者。模块仓库：{}；目标分支：{}。现在开始自动编排。请先规划并只返回一个 NEXT_TASK 协议包：必须包含完整 codex_prompt、至少一条 acceptance_criteria；JSON 代码块必须是回复最后且唯一的代码块。",
        module.name, module.repository_path, module.target_branch
    )
}

#[tauri::command]
fn start_module_orchestration(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
) -> Result<(), String> {
    let module = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        let module = get_module(&connection, &module_id)?
            .ok_or_else(|| "selected module was not found".to_string())?;
        let previous_phase = connection
            .query_row(
                "SELECT phase FROM module_runtime WHERE module_id = ?1",
                [&module_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("could not inspect module runtime: {error}"))?;
        if matches!(previous_phase.as_deref(), Some(phase) if phase != "COMPLETED" && phase != "STOPPED")
        {
            return Err("this module already has a paused or active runtime; use its acceptance controls instead of starting a new run".into());
        }
        module
    };
    if state
        .chatgpt_bridge
        .session
        .lock()
        .map_err(|_| "ChatGPT bridge lock poisoned".to_string())?
        .is_none()
    {
        return Err("bind the dedicated ChatGPT tab before starting a module".into());
    }
    let (runtime, action) = orchestration::Runtime::start();
    let initial_active = {
        let mut active = state
            .orchestrator
            .lock()
            .map_err(|_| "orchestrator lock poisoned".to_string())?;
        if active.is_some() {
            return Err(
                "another module is already active; pause or stop it before starting a new module"
                    .into(),
            );
        }
        let initial_active = ActiveOrchestration {
            module: module.clone(),
            runtime: runtime.clone(),
            started_at: Utc::now(),
            last_commit_sha: None,
        };
        *active = Some(initial_active.clone());
        initial_active
    };
    persist_active_orchestration(
        &state,
        &initial_active,
        "模块已启动，正在等待 ChatGPT 规划。",
    )?;
    if action == orchestration::Action::SendPlanningRequest {
        if let Err(error) = send_chatgpt_message_internal(
            &app,
            &state.chatgpt_bridge,
            &module_planning_message(&module),
        ) {
            block_active_orchestration(&state, format!("无法发送 ChatGPT 规划请求：{error}"))?;
            return Err(error);
        }
    }
    Ok(())
}

fn default_codex_command() -> &'static str {
    if cfg!(windows) {
        "codex.cmd"
    } else {
        "codex"
    }
}

fn codex_command() -> String {
    std::env::var("CODEX_APP_SERVER_COMMAND")
        .unwrap_or_else(|_| default_codex_command().to_string())
}

fn wrap_codex_task(module: &ModuleRecord, task: &str) -> String {
    format!(
        "You are executing one controlled middleware turn.\n\
Repository: {}\n\
Target branch: {}\n\n\
Task:\n{}\n\n\
Required completion behavior:\n\
1. Work only within the selected repository and do not change branches.\n\
2. If you modify code, run relevant tests or builds, inspect the changed scope, create a clear git commit, and push it to the target branch.\n\
3. If this task explicitly forbids changes, do not run commands, inspect files, modify files, commit, or push.\n\
4. End with a concise text summary: completed work, changed files, tests, commit SHA, push result, and residual risks.\n\
5. If any required action cannot be safely completed, stop and state the blocking reason; do not expand the task.",
        module.repository_path, module.target_branch, task.trim()
    )
}

fn emit_codex_event(app: &AppHandle, event: CodexExecutionEvent) {
    let _ = app.emit("codex-status", event);
}

fn event_from_notification(
    module_id: &str,
    message: &Value,
    thread_id: &Option<String>,
    turn_id: &Option<String>,
) -> Option<CodexExecutionEvent> {
    let method = message.get("method")?.as_str()?;
    let params = message.get("params").unwrap_or(&Value::Null);
    let status_line = match method {
        "turn/started" => "Codex 回合已开始。".to_string(),
        "item/started" => {
            let item_type = params
                .pointer("/item/type")
                .and_then(Value::as_str)
                .unwrap_or("工作项");
            format!("Codex 正在处理：{item_type}。")
        }
        "item/completed" => {
            let item_type = params
                .pointer("/item/type")
                .and_then(Value::as_str)
                .unwrap_or("工作项");
            format!("Codex 已完成：{item_type}。")
        }
        "turn/completed" => {
            let status = params
                .pointer("/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("Codex 回合结束：{status}。")
        }
        _ => return None,
    };
    Some(CodexExecutionEvent {
        module_id: module_id.to_string(),
        phase: CodexExecutionPhase::Running,
        status_line,
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
    })
}

fn send_rpc(stdin: &mut impl Write, message: Value) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, &message)
        .map_err(|error| format!("could not encode App Server request: {error}"))?;
    stdin
        .write_all(b"\n")
        .map_err(|error| format!("could not send App Server request: {error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("could not flush App Server request: {error}"))
}

fn process_app_server_turn(
    app: &AppHandle,
    module: &ModuleRecord,
    task: &str,
) -> Result<CodexTurnResult, String> {
    if !Path::new(&module.repository_path).is_dir() {
        return Err(format!(
            "selected repository directory does not exist: {}",
            module.repository_path
        ));
    }

    let command = codex_command();
    let mut app_server = ManagedAppServer {
        child: Command::new(&command)
            .arg("app-server")
            .current_dir(&module.repository_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start `{command} app-server`: {error}"))?,
    };
    let mut stdin = app_server
        .child
        .stdin
        .take()
        .ok_or_else(|| "App Server stdin was unavailable".to_string())?;
    let stdout = app_server
        .child
        .stdout
        .take()
        .ok_or_else(|| "App Server stdout was unavailable".to_string())?;
    let stderr = app_server
        .child
        .stderr
        .take()
        .ok_or_else(|| "App Server stderr was unavailable".to_string())?;
    let stderr_reader = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .filter_map(Result::ok)
            .collect::<Vec<_>>()
            .join("\n")
    });

    let module_id = module.id.clone();
    let mut thread_id = None;
    let mut turn_id = None;
    let mut final_summary = String::new();
    let mut result = None;

    emit_codex_event(
        app,
        CodexExecutionEvent {
            module_id: module_id.clone(),
            phase: CodexExecutionPhase::Starting,
            status_line: "正在启动本地 Codex App Server…".into(),
            thread_id: None,
            turn_id: None,
        },
    );
    send_rpc(
        &mut stdin,
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "chatgpt-codex-middleware",
                    "title": "ChatGPT × Codex Middleware",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
    )?;

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| format!("could not read App Server output: {error}"))?;
        let message: Value = serde_json::from_str(&line)
            .map_err(|error| format!("App Server emitted invalid JSON: {error}"))?;

        if let Some(error) = message.get("error") {
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown App Server error");
            return Err(format!("App Server request failed: {detail}"));
        }

        if message.get("id").is_some() && message.get("method").is_some() {
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let blocked = CodexTurnResult {
                module_id: module_id.clone(),
                status: CodexExecutionPhase::Blocked,
                summary: format!(
                    "Codex requested user input through `{method}`. The middleware paused without answering it."
                ),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            };
            emit_codex_event(
                app,
                CodexExecutionEvent {
                    module_id: module_id.clone(),
                    phase: CodexExecutionPhase::Blocked,
                    status_line: blocked.summary.clone(),
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                },
            );
            result = Some(blocked);
            break;
        }

        match message.get("id").and_then(Value::as_i64) {
            Some(1) => {
                send_rpc(&mut stdin, json!({ "method": "initialized", "params": {} }))?;
                send_rpc(
                    &mut stdin,
                    json!({ "method": "thread/start", "id": 2, "params": {} }),
                )?;
            }
            Some(2) => {
                thread_id = message
                    .pointer("/result/thread/id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let thread = thread_id
                    .clone()
                    .ok_or_else(|| "App Server did not return a thread ID".to_string())?;
                send_rpc(
                    &mut stdin,
                    json!({
                        "method": "turn/start",
                        "id": 3,
                        "params": {
                            "threadId": thread,
                            "input": [{ "type": "text", "text": wrap_codex_task(module, task) }]
                        }
                    }),
                )?;
            }
            Some(3) => {
                turn_id = message
                    .pointer("/result/turn/id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                emit_codex_event(
                    app,
                    CodexExecutionEvent {
                        module_id: module_id.clone(),
                        phase: CodexExecutionPhase::Running,
                        status_line: "Codex 已接受受控任务，正在执行。".into(),
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                    },
                );
            }
            _ => {}
        }

        if let Some(event) = event_from_notification(&module_id, &message, &thread_id, &turn_id) {
            emit_codex_event(app, event);
        }

        if message.get("method").and_then(Value::as_str) == Some("item/agentMessage/delta") {
            if let Some(delta) = message.pointer("/params/delta").and_then(Value::as_str) {
                final_summary.push_str(delta);
            }
        }
        if message.get("method").and_then(Value::as_str) == Some("item/completed")
            && message.pointer("/params/item/type").and_then(Value::as_str) == Some("agentMessage")
        {
            if let Some(text) = message.pointer("/params/item/text").and_then(Value::as_str) {
                final_summary = text.to_string();
            }
        }
        if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
            let completed_status = message
                .pointer("/params/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let phase = if completed_status == "completed" {
                CodexExecutionPhase::Completed
            } else {
                CodexExecutionPhase::Blocked
            };
            let summary = if final_summary.trim().is_empty() {
                format!(
                    "Codex turn ended with status `{completed_status}` and no final text summary."
                )
            } else {
                final_summary.trim().to_string()
            };
            let completed = CodexTurnResult {
                module_id: module_id.clone(),
                status: phase.clone(),
                summary,
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            };
            emit_codex_event(
                app,
                CodexExecutionEvent {
                    module_id: module_id.clone(),
                    phase,
                    status_line: format!("Codex 回合结束：{completed_status}。"),
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                },
            );
            result = Some(completed);
            break;
        }
    }

    app_server.terminate();
    let stderr = stderr_reader
        .join()
        .unwrap_or_else(|_| "could not read App Server stderr".into());
    result.ok_or_else(|| {
        if stderr.trim().is_empty() {
            "App Server closed before a turn/completed notification.".into()
        } else {
            format!("App Server closed before a turn/completed notification: {stderr}")
        }
    })
}

#[tauri::command]
fn execute_controlled_codex_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    module_id: String,
    task: String,
) -> Result<CodexTurnResult, String> {
    if task.trim().is_empty() {
        return Err("a Codex task is required".into());
    }
    let module = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        get_module(&connection, &module_id)?
            .ok_or_else(|| "selected inactive module was not found".to_string())?
    };
    let result = process_app_server_turn(&app, &module, &task);
    if let Err(message) = &result {
        emit_codex_event(
            &app,
            CodexExecutionEvent {
                module_id,
                phase: CodexExecutionPhase::Failed,
                status_line: format!("Codex App Server 错误：{message}"),
                thread_id: None,
                turn_id: None,
            },
        );
    }
    result
}

fn persist_active_orchestration(
    state: &AppState,
    active: &ActiveOrchestration,
    message: &str,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    persist_orchestration_state(&connection, &active.module.id, &active.runtime, message)?;
    persist_execution_details(&connection, active)
}

fn block_active_orchestration(state: &AppState, reason: String) -> Result<(), String> {
    let mut active = state
        .orchestrator
        .lock()
        .map_err(|_| "orchestrator lock poisoned".to_string())?;
    let active = active
        .as_mut()
        .ok_or_else(|| "no active orchestration to block".to_string())?;
    active.runtime.block(reason);
    let snapshot = active.clone();
    persist_active_orchestration(state, &snapshot, "自动编排外部通信失败，模块已阻塞。")
}

fn block_and_notify(app: &AppHandle, state: &AppState, reason: String) {
    let _ = block_active_orchestration(state, reason.clone());
    state.chatgpt_bridge.set_status(
        app,
        ChatGptBridgeStatus {
            phase: "BLOCKED".into(),
            detail: reason,
            tab_id: None,
            protocol_state: None,
        },
    );
}

fn review_message(module: &ModuleRecord, round: u32, commit_sha: &str, summary: &str) -> String {
    format!(
        "“{}”模块第 {} 轮 Codex 已结束。分支：{}；已验证 commit：{}。Codex 最终摘要：\n{}\n\n请检查仓库后仅返回一个协议包：如需继续则 NEXT_TASK；如模块完成则 MODULE_DONE；如需要用户处理则 PAUSE 或 BLOCKED。",
        module.name, round, module.target_branch, commit_sha, summary.trim()
    )
}

fn run_orchestration_action(app: AppHandle, action: orchestration::Action) -> Result<(), String> {
    match action {
        orchestration::Action::SendPlanningRequest => Ok(()),
        orchestration::Action::StartCodexTurn {
            round,
            codex_prompt,
        } => {
            let state = app.state::<AppState>();
            let (module, budget_pause) = {
                let mut active = state
                    .orchestrator
                    .lock()
                    .map_err(|_| "orchestrator lock poisoned".to_string())?;
                let active = active
                    .as_mut()
                    .ok_or_else(|| "no active orchestration owns the Codex turn".to_string())?;
                if let Some(reason) = budget_pause_reason(&state, active) {
                    active.runtime.pause(reason.clone());
                    let snapshot = active.clone();
                    persist_active_orchestration(
                        &state,
                        &snapshot,
                        "预算已到达，未启动下一轮 Codex。",
                    )?;
                    (None, Some(reason))
                } else {
                    active.runtime.codex_started()?;
                    let snapshot = active.clone();
                    persist_active_orchestration(&state, &snapshot, "正在启动 Codex 回合。")?;
                    (Some(snapshot.module), None)
                }
            };
            if let Some(reason) = budget_pause {
                state.chatgpt_bridge.set_status(
                    &app,
                    ChatGptBridgeStatus {
                        phase: "PAUSED_FOR_ACCEPTANCE".into(),
                        detail: reason,
                        tab_id: None,
                        protocol_state: None,
                    },
                );
                return Ok(());
            }
            let module = module.expect("module exists when budget did not pause");
            let app_for_turn = app.clone();
            let module_id = module.id.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let result = process_app_server_turn(&app_for_turn, &module, &codex_prompt);
                let state = app_for_turn.state::<AppState>();
                match result {
                    Ok(result) if result.status == CodexExecutionPhase::Completed => {
                        let commit_sha = match verify_codex_outcome(&module, &result.summary) {
                            Ok(commit_sha) => commit_sha,
                            Err(error) => {
                                block_and_notify(
                                    &app_for_turn,
                                    &state,
                                    format!("Git 推送验证失败：{error}"),
                                );
                                return;
                            }
                        };
                        let next_action = {
                            let mut active = match state.orchestrator.lock() {
                                Ok(active) => active,
                                Err(_) => return,
                            };
                            let Some(active) = active.as_mut() else {
                                return;
                            };
                            active.last_commit_sha = Some(commit_sha.clone());
                            let turn_recorded = match state.connection.lock() {
                                Ok(connection) => record_completed_turn(
                                    &connection,
                                    active,
                                    &result.summary,
                                    &commit_sha,
                                ),
                                Err(_) => return,
                            };
                            if turn_recorded.is_err() {
                                return;
                            }
                            if budget_pause_reason(&state, active).is_some()
                                && active.runtime.request_pause_after_current_turn().is_err()
                            {
                                return;
                            }
                            let action =
                                match active.runtime.codex_completed(result.summary.clone()) {
                                    Ok(action) => action,
                                    Err(_) => return,
                                };
                            let action = if let Some(reason) = budget_pause_reason(&state, active) {
                                active.runtime.pause(reason)
                            } else {
                                action
                            };
                            let snapshot = active.clone();
                            if persist_active_orchestration(
                                &state,
                                &snapshot,
                                if matches!(
                                    action,
                                    orchestration::Action::PauseForAcceptance { .. }
                                ) {
                                    "Codex 回合完成，预算已到达，等待用户验收。"
                                } else {
                                    "Codex 回合完成，正在等待 ChatGPT Review。"
                                },
                            )
                            .is_err()
                            {
                                return;
                            }
                            action
                        };
                        if let orchestration::Action::SendReviewRequest {
                            round,
                            codex_summary,
                        } = next_action
                        {
                            let message =
                                review_message(&module, round, &commit_sha, &codex_summary);
                            if let Err(error) = send_chatgpt_message_internal(
                                &app_for_turn,
                                &state.chatgpt_bridge,
                                &message,
                            ) {
                                block_and_notify(
                                    &app_for_turn,
                                    &state,
                                    format!("无法发送 ChatGPT Review 请求：{error}"),
                                );
                            }
                        } else if matches!(
                            next_action,
                            orchestration::Action::PauseForAcceptance { .. }
                        ) {
                            state.chatgpt_bridge.set_status(
                                &app_for_turn,
                                ChatGptBridgeStatus {
                                    phase: "PAUSED_FOR_ACCEPTANCE".into(),
                                    detail: "预算已到达；当前 Codex 回合已完成，等待用户验收。"
                                        .into(),
                                    tab_id: None,
                                    protocol_state: None,
                                },
                            );
                        }
                    }
                    Ok(result) => {
                        let mut active = match state.orchestrator.lock() {
                            Ok(active) => active,
                            Err(_) => return,
                        };
                        let Some(active) = active.as_mut() else {
                            return;
                        };
                        active.runtime.block(result.summary);
                        let snapshot = active.clone();
                        let _ = persist_active_orchestration(
                            &state,
                            &snapshot,
                            "Codex 回合未完成，模块已阻塞。",
                        );
                        state.chatgpt_bridge.set_status(
                            &app_for_turn,
                            ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: "Codex 回合未完成，模块已阻塞。".into(),
                                tab_id: None,
                                protocol_state: None,
                            },
                        );
                    }
                    Err(error) => {
                        let mut active = match state.orchestrator.lock() {
                            Ok(active) => active,
                            Err(_) => return,
                        };
                        let Some(active) = active.as_mut() else {
                            return;
                        };
                        active
                            .runtime
                            .block(format!("Codex App Server 错误：{error}"));
                        let snapshot = active.clone();
                        let _ = persist_active_orchestration(
                            &state,
                            &snapshot,
                            "Codex App Server 错误，模块已阻塞。",
                        );
                        state.chatgpt_bridge.set_status(
                            &app_for_turn,
                            ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: "Codex App Server 错误，模块已阻塞。".into(),
                                tab_id: None,
                                protocol_state: None,
                            },
                        );
                    }
                }
            });
            emit_codex_event(
                &app,
                CodexExecutionEvent {
                    module_id,
                    phase: CodexExecutionPhase::Starting,
                    status_line: format!("正在开始自动编排第 {round} 轮 Codex 任务。"),
                    thread_id: None,
                    turn_id: None,
                },
            );
            Ok(())
        }
        orchestration::Action::SendReviewRequest { .. } => Ok(()),
        orchestration::Action::PauseForAcceptance { .. } | orchestration::Action::Block { .. } => {
            Ok(())
        }
    }
}

fn handle_orchestration_protocol(app: AppHandle, envelope: ProtocolEnvelope) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (action, message) = {
        let mut active = state
            .orchestrator
            .lock()
            .map_err(|_| "orchestrator lock poisoned".to_string())?;
        let Some(active) = active.as_mut() else {
            return Ok(());
        };
        let (action, message) = match envelope.state {
            ProtocolState::NextTask => (
                active
                    .runtime
                    .receive_next_task(envelope.codex_prompt.unwrap_or_default())?,
                "已接收 ChatGPT NEXT_TASK，正在交给 Codex。",
            ),
            ProtocolState::ModuleDone => (
                active.runtime.receive_module_done(envelope.reason)?,
                "ChatGPT 已报告 MODULE_DONE，等待用户验收。",
            ),
            ProtocolState::Pause => (
                active.runtime.receive_pause(envelope.reason),
                "ChatGPT 请求暂停，等待用户处理。",
            ),
            ProtocolState::Blocked => (
                active.runtime.block(envelope.reason),
                "ChatGPT 报告阻塞，自动编排已停止。",
            ),
        };
        let snapshot = active.clone();
        persist_active_orchestration(&state, &snapshot, message)?;
        (action, message.to_string())
    };
    if matches!(
        action,
        orchestration::Action::PauseForAcceptance { .. } | orchestration::Action::Block { .. }
    ) {
        state.chatgpt_bridge.set_status(
            &app,
            ChatGptBridgeStatus {
                phase: orchestration_phase_name(match action {
                    orchestration::Action::PauseForAcceptance { .. } => {
                        orchestration::Phase::PausedForAcceptance
                    }
                    _ => orchestration::Phase::Blocked,
                })
                .into(),
                detail: message,
                tab_id: None,
                protocol_state: None,
            },
        );
    }
    run_orchestration_action(app, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> InactiveModuleInput {
        InactiveModuleInput {
            name: "Desktop foundation".into(),
            repository_path: r"G:\projects\personal-repo".into(),
            target_branch: "main".into(),
            chatgpt_tab_id: 42,
            max_rounds: 6,
            module_timeout_minutes: 120,
            global_timeout_minutes: 240,
        }
    }

    fn relay_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(CONVERSATION_RELAY_SCHEMA)
            .expect("relay schema");
        connection
            .execute_batch(CODEX_COMMUNICATION_OBSERVABILITY_SCHEMA)
            .expect("Codex communication observability schema");
        connection
    }

    fn insert_relay_module(connection: &Connection, id: &str, name: &str) {
        connection.execute(
            "INSERT INTO relay_modules (id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase, invalid_reply_count, started_cycles, created_at, updated_at)
             VALUES (?1, ?2, 'G:\\workspace', 12, 240, 'retry', 'READY', 0, 0, '2026-08-17T00:00:00Z', '2026-08-17T00:00:00Z')",
            params![id, name],
        ).expect("relay module");
    }

    fn insert_relay_message(
        connection: &Connection,
        id: &str,
        module_id: &str,
        sequence_number: i64,
        delivery_state: &str,
        created_at: &str,
    ) {
        connection.execute(
            "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at)
             VALUES (?1, ?2, ?3, 'TO_CHATGPT', 'AUTOMATION', ?1, ?4, ?5)",
            params![id, module_id, sequence_number, delivery_state, created_at],
        ).expect("relay message");
    }

    fn insert_relay_codex_cycle(
        connection: &Connection,
        id: &str,
        module_id: &str,
        cycle_number: i64,
        outbound_chatgpt_message_id: Option<&str>,
    ) -> rusqlite::Result<usize> {
        connection.execute(
            "INSERT INTO relay_codex_cycles (
                id, module_id, cycle_number, status, prompt_text,
                outbound_chatgpt_message_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'WAITING_TO_SEND_CODEX', 'Codex prompt', ?4, ?5, ?5)",
            params![
                id,
                module_id,
                cycle_number,
                outbound_chatgpt_message_id,
                "2026-08-17T00:00:00Z"
            ],
        )
    }

    #[test]
    fn terminal_relay_chatgpt_reply_persists_delivery_without_restarting_automation() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-stopped", "已终止模块");
        insert_relay_module(&connection, "module-completed", "已完成模块");

        let cycle =
            create_relay_codex_cycle(&connection, "module-stopped", 1, "已完成的 Codex 工作")
                .expect("create Codex cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), Some("turn-a"))
            .expect("record Codex turn");
        mark_relay_codex_result_received(&connection, &cycle.id, "Codex final text")
            .expect("record Codex result");
        let automation_request_id = queue_relay_codex_result_to_chatgpt(&connection, &cycle.id)
            .expect("queue Codex result");
        match claim_next_relay_message_for_dispatch(&connection).expect("claim automation message")
        {
            RelayDispatchClaim::Message(message) => assert_eq!(message.id, automation_request_id),
            _ => panic!("automation message must be in flight"),
        }
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'STOPPED' WHERE id = 'module-stopped'",
                [],
            )
            .expect("stop module");

        let prompt_reply = "@@@CODEX_PROMPT@@@\n不得启动新的 Codex 回合\n@@@END_CODEX_PROMPT@@@";
        let stopped_outcome =
            process_relay_chatgpt_reply_in(&connection, Some(&automation_request_id), prompt_reply)
                .expect("persist stopped automation reply");
        assert!(stopped_outcome.terminal_ignored);

        let linked_cycle = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read linked cycle")
            .expect("linked cycle exists");
        assert_eq!(linked_cycle.status, "DELIVERED_TO_CHATGPT");
        let automation_delivery: String = connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = ?1",
                [&automation_request_id],
                |row| row.get(0),
            )
            .expect("automation delivery");
        assert_eq!(automation_delivery, "DELIVERED");

        queue_relay_message_in(
            &connection,
            "module-completed",
            RelayMessageKind::Manual,
            "手动消息",
        )
        .expect("queue manual message");
        match claim_next_relay_message_for_dispatch(&connection).expect("claim manual message") {
            RelayDispatchClaim::Message(message) => assert_eq!(message.kind, "MANUAL"),
            _ => panic!("manual message must be in flight"),
        };
        let manual_request_id: String = connection
            .query_row(
                "SELECT id FROM relay_messages
                 WHERE module_id = 'module-completed' AND direction = 'TO_CHATGPT'
                   AND delivery_state = 'SENT'",
                [],
                |row| row.get(0),
            )
            .expect("manual request id");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'COMPLETED' WHERE id = 'module-completed'",
                [],
            )
            .expect("complete module");
        let completed_outcome =
            process_relay_chatgpt_reply_in(&connection, Some(&manual_request_id), prompt_reply)
                .expect("persist completed manual reply");
        assert!(completed_outcome.terminal_ignored);

        for (module_id, request_id, expected_phase) in [
            ("module-stopped", automation_request_id.as_str(), "STOPPED"),
            ("module-completed", manual_request_id.as_str(), "COMPLETED"),
        ] {
            let phase: String = connection
                .query_row(
                    "SELECT phase FROM relay_modules WHERE id = ?1",
                    [module_id],
                    |row| row.get(0),
                )
                .expect("terminal phase");
            assert_eq!(phase, expected_phase);
            let invalid_reply_count: i64 = connection
                .query_row(
                    "SELECT invalid_reply_count FROM relay_modules WHERE id = ?1",
                    [module_id],
                    |row| row.get(0),
                )
                .expect("invalid reply count");
            assert_eq!(invalid_reply_count, 0);
            let persisted_reply: String = connection
                .query_row(
                    "SELECT text FROM relay_messages
                     WHERE module_id = ?1 AND direction = 'FROM_CHATGPT'
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    [module_id],
                    |row| row.get(0),
                )
                .expect("persisted ChatGPT reply");
            assert_eq!(persisted_reply, prompt_reply);
            let delivery: String = connection
                .query_row(
                    "SELECT delivery_state FROM relay_messages WHERE id = ?1",
                    [request_id],
                    |row| row.get(0),
                )
                .expect("outbound delivery");
            assert_eq!(delivery, "DELIVERED");
            let late_events: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM relay_events
                     WHERE module_id = ?1 AND event_type = 'LATE_CHATGPT_REPLY_IGNORED'",
                    [module_id],
                    |row| row.get(0),
                )
                .expect("late reply audit event");
            assert_eq!(late_events, 1);
            let retries: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM relay_events
                     WHERE module_id = ?1 AND event_type = 'CONTROL_RETRY'",
                    [module_id],
                    |row| row.get(0),
                )
                .expect("retry count");
            assert_eq!(retries, 0);
        }
        let stopped_cycles: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_codex_cycles WHERE module_id = 'module-stopped'",
                [],
                |row| row.get(0),
            )
            .expect("stopped cycle count");
        let stopped_turns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages
                 WHERE module_id = 'module-stopped' AND direction = 'TO_CODEX'",
                [],
                |row| row.get(0),
            )
            .expect("stopped turn count");
        assert_eq!(stopped_cycles, 1);
        assert_eq!(stopped_turns, 0);
    }

    #[test]
    fn relay_codex_release_acknowledges_and_only_clears_the_matching_session() {
        let (sender, receiver) = std_mpsc::channel();
        let sessions = Mutex::new(Some(RelayCodexSession {
            module_id: "module-a".into(),
            commands: sender,
            turn_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }));
        let worker = std::thread::spawn(move || match receiver.recv().expect("release command") {
            RelayCodexCommand::Release { acknowledgement } => acknowledgement
                .send(Ok(()))
                .expect("acknowledge idle release"),
            RelayCodexCommand::StartTurn { .. } => panic!("release must not start a turn"),
        });

        release_relay_codex_session(
            &sessions,
            "module-a",
            std::time::Duration::from_millis(100),
        )
        .expect("idle release is acknowledged");
        worker.join().expect("worker exits");
        assert!(clear_relay_codex_session_if_matches(&sessions, "module-a"));
        assert!(sessions.lock().expect("session lock").is_none());

        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        connection
            .execute(
                "UPDATE relay_modules SET codex_thread_id = 'thread-a' WHERE id = 'module-a'",
                [],
            )
            .expect("persist thread id");
        let thread_id: Option<String> = connection
            .query_row(
                "SELECT codex_thread_id FROM relay_modules WHERE id = 'module-a'",
                [],
                |row| row.get(0),
            )
            .expect("read thread id");
        assert_eq!(thread_id.as_deref(), Some("thread-a"));

        release_relay_codex_session(
            &sessions,
            "module-a",
            std::time::Duration::from_millis(100),
        )
        .expect("repeat release without a session is a no-op");

        let (next_sender, _next_receiver) = std_mpsc::channel();
        *sessions.lock().expect("session lock") = Some(RelayCodexSession {
            module_id: "module-b".into(),
            commands: next_sender,
            turn_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        assert!(!clear_relay_codex_session_if_matches(&sessions, "module-a"));
        assert_eq!(
            sessions
                .lock()
                .expect("session lock")
                .as_ref()
                .map(|session| session.module_id.as_str()),
            Some("module-b")
        );
    }

    #[test]
    fn relay_codex_release_does_not_send_release_to_a_running_turn() {
        let (sender, receiver) = std_mpsc::channel();
        let sessions = Mutex::new(Some(RelayCodexSession {
            module_id: "module-a".into(),
            commands: sender,
            turn_active: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }));

        let error = release_relay_codex_session(
            &sessions,
            "module-a",
            std::time::Duration::from_millis(20),
        )
        .expect_err("a running turn must not be released");

        assert!(error.contains("仍在运行"));
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(20)),
            Err(std_mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn terminate_running_relay_codex_failure_makes_the_matching_session_releasable() {
        let (sender, receiver) = std_mpsc::channel();
        let sessions = Mutex::new(Some(RelayCodexSession {
            module_id: "module-a".into(),
            commands: sender,
            turn_active: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }));

        assert!(mark_relay_codex_session_turn_inactive(&sessions, "module-a"));
        let worker = std::thread::spawn(move || match receiver.recv().expect("release command") {
            RelayCodexCommand::Release { acknowledgement } => acknowledgement
                .send(Ok(()))
                .expect("acknowledge failed-turn release"),
            RelayCodexCommand::StartTurn { .. } => panic!("failed turn must only schedule release"),
        });
        release_relay_codex_session(
            &sessions,
            "module-a",
            std::time::Duration::from_millis(100),
        )
        .expect("failed turn session is releasable after turn activity is cleared");
        worker.join().expect("release worker joins");
    }

    #[test]
    fn terminal_relay_modules_are_skipped_without_changing_active_global_fifo() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-stopped", "已终止模块");
        insert_relay_module(&connection, "module-completed", "已完成模块");
        insert_relay_module(&connection, "module-active-a", "活动模块 A");
        insert_relay_module(&connection, "module-active-b", "活动模块 B");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'STOPPED' WHERE id = 'module-stopped'",
                [],
            )
            .expect("mark stopped");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'COMPLETED' WHERE id = 'module-completed'",
                [],
            )
            .expect("mark completed");
        insert_relay_message(
            &connection,
            "terminal-stopped",
            "module-stopped",
            1,
            "QUEUED",
            "2026-08-17T00:00:01Z",
        );
        insert_relay_message(
            &connection,
            "terminal-completed",
            "module-completed",
            1,
            "QUEUED",
            "2026-08-17T00:00:02Z",
        );
        insert_relay_message(
            &connection,
            "active-first",
            "module-active-a",
            1,
            "QUEUED",
            "2026-08-17T00:00:03Z",
        );
        insert_relay_message(
            &connection,
            "active-second",
            "module-active-b",
            1,
            "QUEUED",
            "2026-08-17T00:00:04Z",
        );

        let claimed = match claim_next_relay_message_for_dispatch(&connection)
            .expect("claim next active message")
        {
            RelayDispatchClaim::Message(message) => message,
            _ => panic!("an active queued message should be selected"),
        };

        assert_eq!(claimed.id, "active-first");
        let terminal_states: Vec<String> = ["terminal-stopped", "terminal-completed"]
            .iter()
            .map(|id| {
                connection
                    .query_row(
                        "SELECT delivery_state FROM relay_messages WHERE id = ?1",
                        [id],
                        |row| row.get(0),
                    )
                    .expect("terminal message state")
            })
            .collect();
        assert_eq!(terminal_states, ["QUEUED", "QUEUED"]);
    }

    #[test]
    fn terminal_relay_modules_reject_new_queue_messages() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-stopped", "已终止模块");
        insert_relay_module(&connection, "module-completed", "已完成模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'STOPPED' WHERE id = 'module-stopped'",
                [],
            )
            .expect("mark stopped");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'COMPLETED' WHERE id = 'module-completed'",
                [],
            )
            .expect("mark completed");

        for module_id in ["module-stopped", "module-completed"] {
            let error = queue_relay_message_in(
                &connection,
                module_id,
                RelayMessageKind::Manual,
                "不得发送到终态模块",
            )
            .expect_err("terminal modules must reject new queue messages");
            assert_eq!(error, "模块已经结束，不能再发送消息。");
        }
        let queued_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM relay_messages", [], |row| row.get(0))
            .expect("queued count");
        assert_eq!(queued_count, 0);
    }

    #[test]
    fn terminal_relay_recovery_never_resets_phase_to_ready() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-completed", "已完成模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'COMPLETED' WHERE id = 'module-completed'",
                [],
            )
            .expect("mark completed");
        insert_relay_message(
            &connection,
            "unknown-terminal",
            "module-completed",
            1,
            "UNKNOWN",
            "2026-08-17T00:00:01Z",
        );

        resolve_unknown_relay_message_without_resend(&connection, "unknown-terminal")
            .expect("explicitly resolve legacy unknown message");

        let phase: String = connection
            .query_row(
                "SELECT phase FROM relay_modules WHERE id = 'module-completed'",
                [],
                |row| row.get(0),
            )
            .expect("terminal phase");
        assert_eq!(phase, "COMPLETED");
    }

    #[test]
    fn terminal_relay_adapter_failure_keeps_terminal_phase() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-stopped", "已终止模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'STOPPED' WHERE id = 'module-stopped'",
                [],
            )
            .expect("mark stopped");
        insert_relay_message(
            &connection,
            "sent-terminal",
            "module-stopped",
            1,
            "SENT",
            "2026-08-18T00:00:00Z",
        );

        pause_relay_for_uncertain_delivery(
            &connection,
            "sent-terminal",
            "CHATGPT_ADAPTER_FAILURE",
            "适配器失败",
        )
        .expect("terminal sent message can still become explicitly recoverable");

        let phase: String = connection
            .query_row(
                "SELECT phase FROM relay_modules WHERE id = 'module-stopped'",
                [],
                |row| row.get(0),
            )
            .expect("terminal phase");
        assert_eq!(phase, "STOPPED");
    }

    #[test]
    fn accept_relay_module_requires_waiting_for_acceptance_without_local_blockers() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");

        let not_waiting = accept_relay_module_in(&connection, "module-a")
            .expect_err("only a module waiting for acceptance may be accepted");
        assert!(not_waiting.contains("等待人工验收"));

        connection
            .execute(
                "UPDATE relay_modules SET phase = 'WAITING_FOR_ACCEPTANCE' WHERE id = 'module-a'",
                [],
            )
            .expect("mark module waiting for acceptance");
        insert_relay_message(
            &connection,
            "unknown-a",
            "module-a",
            1,
            "UNKNOWN",
            "2026-08-18T00:00:00Z",
        );
        let local_unknown = accept_relay_module_in(&connection, "module-a")
            .expect_err("local unknown delivery must block acceptance");
        assert!(local_unknown.contains("不确定送达"));

        connection
            .execute("DELETE FROM relay_messages WHERE id = 'unknown-a'", [])
            .expect("remove local recovery fixture");
        connection
            .execute(
                "UPDATE relay_modules SET stop_after_turn = 1 WHERE id = 'module-a'",
                [],
            )
            .expect("mark termination requested");
        let terminating = accept_relay_module_in(&connection, "module-a")
            .expect_err("a terminating module must not be accepted");
        assert!(terminating.contains("正在终止"));
        connection
            .execute(
                "UPDATE relay_modules SET stop_after_turn = 0 WHERE id = 'module-a'",
                [],
            )
            .expect("clear termination fixture");
        insert_relay_module(&connection, "module-b", "模块 B");
        insert_relay_message(
            &connection,
            "unknown-b",
            "module-b",
            1,
            "UNKNOWN",
            "2026-08-18T00:00:01Z",
        );
        insert_relay_codex_cycle(&connection, "cycle-a", "module-a", 1, None)
            .expect("insert cycle");
        connection
            .execute(
                "UPDATE relay_codex_cycles SET status = 'CODEX_RUNNING' WHERE id = 'cycle-a'",
                [],
            )
            .expect("mark cycle running");
        let running = accept_relay_module_in(&connection, "module-a")
            .expect_err("a running Codex cycle must block acceptance");
        assert!(running.contains("当前 Codex 回合仍在运行"));

        connection
            .execute(
                "UPDATE relay_codex_cycles SET status = 'CODEX_COMPLETED' WHERE id = 'cycle-a'",
                [],
            )
            .expect("complete cycle");
        accept_relay_module_in(&connection, "module-a")
            .expect("other module recovery must not block acceptance");
    }

    #[test]
    fn accept_relay_module_completes_once_and_preserves_sent_thread_and_history() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        connection
            .execute(
                "UPDATE relay_modules
                 SET phase = 'WAITING_FOR_ACCEPTANCE', codex_thread_id = 'thread-a'
                 WHERE id = 'module-a'",
                [],
            )
            .expect("prepare acceptance");
        insert_relay_message(
            &connection,
            "queued-a",
            "module-a",
            1,
            "QUEUED",
            "2026-08-18T00:00:00Z",
        );
        insert_relay_message(
            &connection,
            "sent-a",
            "module-a",
            2,
            "SENT",
            "2026-08-18T00:00:01Z",
        );

        accept_relay_module_in(&connection, "module-a").expect("accept module");
        let module: (String, Option<String>) = connection
            .query_row(
                "SELECT phase, codex_thread_id FROM relay_modules WHERE id = 'module-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("accepted module");
        assert_eq!(module.0, "COMPLETED");
        assert_eq!(module.1.as_deref(), Some("thread-a"));
        let messages: Vec<(String, String, String)> = connection
            .prepare("SELECT id, text, delivery_state FROM relay_messages WHERE module_id = 'module-a' ORDER BY sequence_number")
            .expect("message query")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("message rows")
            .collect::<Result<_, _>>()
            .expect("collect messages");
        assert_eq!(messages, vec![
            ("queued-a".into(), "queued-a".into(), "FAILED".into()),
            ("sent-a".into(), "sent-a".into(), "SENT".into()),
        ]);
        let accepted_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events WHERE module_id = 'module-a' AND event_type = 'MODULE_ACCEPTED'",
                [],
                |row| row.get(0),
            )
            .expect("accepted audit count");
        let cancelled_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events WHERE module_id = 'module-a' AND event_type = 'CHATGPT_QUEUED_MESSAGE_CANCELLED'",
                [],
                |row| row.get(0),
            )
            .expect("queued cancellation audit count");
        assert_eq!(accepted_events, 1);
        assert_eq!(cancelled_events, 1);

        accept_relay_module_in(&connection, "module-a")
            .expect("repeated acceptance is an idempotent success");
        let repeated_accepted_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events WHERE module_id = 'module-a' AND event_type = 'MODULE_ACCEPTED'",
                [],
                |row| row.get(0),
            )
            .expect("repeated accepted audit count");
        assert_eq!(repeated_accepted_events, 1);
        assert!(queue_relay_message_in(
            &connection,
            "module-a",
            RelayMessageKind::Automation,
            "不应排队",
        )
        .is_err());

        insert_relay_module(&connection, "module-stopped", "已终止模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'STOPPED' WHERE id = 'module-stopped'",
                [],
            )
            .expect("stop fixture");
        let stopped = accept_relay_module_in(&connection, "module-stopped")
            .expect_err("stopped modules cannot be accepted");
        assert!(stopped.contains("已终止"));
    }

    #[test]
    fn relay_acceptance_feedback_requires_waiting_phase_and_nonempty_text() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");

        let phase_error = submit_relay_acceptance_feedback_in(
            &connection,
            "module-a",
            "请继续处理验收反馈。",
        )
        .expect_err("only modules waiting for acceptance may submit feedback");
        assert!(phase_error.contains("等待人工验收"));

        connection
            .execute(
                "UPDATE relay_modules SET phase = 'WAITING_FOR_ACCEPTANCE' WHERE id = 'module-a'",
                [],
            )
            .expect("prepare acceptance");
        let empty_error = submit_relay_acceptance_feedback_in(&connection, "module-a", "  ")
            .expect_err("blank feedback must be rejected");
        assert!(empty_error.contains("不能为空"));
    }

    #[test]
    fn relay_acceptance_feedback_queues_once_and_waits_for_chatgpt() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        connection
            .execute(
                "UPDATE relay_modules
                 SET phase = 'WAITING_FOR_ACCEPTANCE', codex_thread_id = 'thread-a', started_cycles = 1
                 WHERE id = 'module-a'",
                [],
            )
            .expect("prepare acceptance");

        let message_id = submit_relay_acceptance_feedback_in(
            &connection,
            "module-a",
            "  请根据验收反馈继续。  ",
        )
        .expect("queue acceptance feedback");
        let message: (String, String, String, i64) = connection
            .query_row(
                "SELECT kind, text, delivery_state, sequence_number
                 FROM relay_messages WHERE id = ?1",
                [&message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("queued feedback message");
        assert_eq!(message, ("AUTOMATION".into(), "请根据验收反馈继续。".into(), "QUEUED".into(), 1));
        let module: (String, Option<String>, i64) = connection
            .query_row(
                "SELECT phase, codex_thread_id, started_cycles FROM relay_modules WHERE id = 'module-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("module after feedback");
        assert_eq!(module.0, "WAITING_FOR_CHATGPT");
        assert_eq!(module.1.as_deref(), Some("thread-a"));
        assert_eq!(module.2, 1);
        for event_type in ["CHATGPT_MESSAGE_QUEUED", "ACCEPTANCE_FEEDBACK_QUEUED"] {
            let event_count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM relay_events WHERE module_id = 'module-a' AND event_type = ?1",
                    [event_type],
                    |row| row.get(0),
                )
                .expect("feedback audit event");
            assert_eq!(event_count, 1, "{event_type} must be recorded once");
        }

        let duplicate = submit_relay_acceptance_feedback_in(
            &connection,
            "module-a",
            "不应加入第二条反馈。",
        )
        .expect_err("feedback may only be submitted once per acceptance pause");
        assert!(duplicate.contains("等待人工验收"));
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages WHERE module_id = 'module-a' AND direction = 'TO_CHATGPT'",
                [],
                |row| row.get(0),
            )
            .expect("feedback message count");
        assert_eq!(message_count, 1);
    }

    #[test]
    fn relay_acceptance_feedback_can_queue_behind_global_unknown_without_bypassing_it() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_module(&connection, "module-b", "模块 B");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'WAITING_FOR_ACCEPTANCE' WHERE id = 'module-a'",
                [],
            )
            .expect("prepare acceptance");
        insert_relay_message(
            &connection,
            "unknown-a",
            "module-a",
            1,
            "UNKNOWN",
            "2026-08-18T00:00:00Z",
        );
        insert_relay_message(
            &connection,
            "unknown-b",
            "module-b",
            1,
            "UNKNOWN",
            "2026-08-18T00:00:01Z",
        );

        let message_id = submit_relay_acceptance_feedback_in(
            &connection,
            "module-a",
            "请继续处理。",
        )
        .expect("feedback is safe to queue while recovery is pending");
        assert!(matches!(
            claim_next_relay_message_for_dispatch(&connection).expect("claim state"),
            RelayDispatchClaim::RecoveryBlocked(2)
        ));
        let state: String = connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = ?1",
                [&message_id],
                |row| row.get(0),
            )
            .expect("feedback remains queued");
        assert_eq!(state, "QUEUED");
    }

    #[test]
    fn relay_acceptance_feedback_follow_up_prompt_reuses_worker_and_increments_cycles() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        connection
            .execute(
                "UPDATE relay_modules
                 SET phase = 'WAITING_FOR_ACCEPTANCE', codex_thread_id = 'thread-a', started_cycles = 1
                 WHERE id = 'module-a'",
                [],
            )
            .expect("prepare acceptance");
        submit_relay_acceptance_feedback_in(&connection, "module-a", "请继续修复验收发现的问题。")
            .expect("queue feedback");

        let control = relay_protocol::parse_terminal_control_block(
            "已收到验收反馈。\n\n@@@CODEX_PROMPT@@@\n继续处理验收反馈。\n@@@END_CODEX_PROMPT@@@",
        )
        .expect("valid follow-up CODEX_PROMPT");
        let relay_protocol::ControlBlock::CodexPrompt(prompt) = control else {
            panic!("expected a CODEX_PROMPT");
        };
        let cycle = create_relay_codex_cycle(&connection, "module-a", 2, &prompt)
            .expect("create follow-up cycle");
        mark_relay_codex_turn_starting_in(&connection, "module-a")
            .expect("start follow-up Codex turn");

        let (sender, receiver) = std_mpsc::channel();
        let session = RelayCodexSession {
            module_id: "module-a".into(),
            commands: sender,
            turn_active: Arc::new(AtomicBool::new(false)),
        };
        send_relay_codex_start_turn(&session, &cycle.id, &prompt)
            .expect("the existing module worker accepts the follow-up turn");
        match receiver.recv().expect("follow-up worker command") {
            RelayCodexCommand::StartTurn { cycle_id, prompt: received_prompt } => {
                assert_eq!(cycle_id, cycle.id);
                assert_eq!(received_prompt, prompt);
            }
            RelayCodexCommand::Release { .. } => panic!("follow-up feedback must not release worker"),
        }
        assert!(session.turn_active.load(Ordering::SeqCst));
        let module: (String, Option<String>, i64) = connection
            .query_row(
                "SELECT phase, codex_thread_id, started_cycles FROM relay_modules WHERE id = 'module-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("module after follow-up prompt");
        assert_eq!(module.0, "CODEX_STARTING");
        assert_eq!(module.1.as_deref(), Some("thread-a"));
        assert_eq!(module.2, 2);
    }

    #[test]
    fn terminate_idle_relay_module_stops_only_safe_non_running_modules() {
        let connection = relay_connection();
        for (id, phase) in [
            ("module-ready", "READY"),
            ("module-acceptance", "WAITING_FOR_ACCEPTANCE"),
            ("module-blocked", "BLOCKED"),
            ("module-recovery", "RECOVERY_REQUIRED"),
            ("module-chatgpt", "WAITING_FOR_CHATGPT"),
        ] {
            insert_relay_module(&connection, id, id);
            connection
                .execute("UPDATE relay_modules SET phase = ?2 WHERE id = ?1", params![id, phase])
                .expect("prepare idle phase");
            connection
                .execute(
                    "UPDATE relay_modules SET codex_thread_id = 'thread-kept' WHERE id = ?1",
                    [id],
                )
                .expect("retain thread id");
            insert_relay_message(
                &connection,
                &format!("queued-{id}"),
                id,
                1,
                "QUEUED",
                "2026-08-18T00:00:00Z",
            );
            insert_relay_message(
                &connection,
                &format!("sent-{id}"),
                id,
                2,
                "SENT",
                "2026-08-18T00:00:01Z",
            );

            assert_eq!(
                terminate_relay_module_in(&connection, id).expect("terminate idle module"),
                RelayModuleTermination::Stopped
            );
            let module: (String, Option<String>) = connection
                .query_row(
                    "SELECT phase, codex_thread_id FROM relay_modules WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("stopped module");
            assert_eq!(module.0, "STOPPED");
            assert_eq!(module.1.as_deref(), Some("thread-kept"));
            let message_states: Vec<(String, String, String)> = connection
                .prepare(
                    "SELECT id, text, delivery_state FROM relay_messages
                     WHERE module_id = ?1 ORDER BY sequence_number",
                )
                .expect("message query")
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("message rows")
                .collect::<Result<_, _>>()
                .expect("collect messages");
            assert_eq!(
                message_states,
                vec![
                    (format!("queued-{id}"), format!("queued-{id}"), "FAILED".into()),
                    (format!("sent-{id}"), format!("sent-{id}"), "SENT".into()),
                ]
            );
            let termination_events: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM relay_events
                     WHERE module_id = ?1 AND event_type = 'MODULE_TERMINATED'",
                    [id],
                    |row| row.get(0),
                )
                .expect("termination event count");
            let cancelled_events: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM relay_events
                     WHERE module_id = ?1 AND event_type = 'CHATGPT_QUEUED_MESSAGE_CANCELLED'",
                    [id],
                    |row| row.get(0),
                )
                .expect("queued cancellation count");
            assert_eq!(termination_events, 1);
            assert_eq!(cancelled_events, 1);
            assert_eq!(
                terminate_relay_module_in(&connection, id).expect("repeat termination is inert"),
                RelayModuleTermination::AlreadyStopped
            );
            let repeated_events: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM relay_events
                     WHERE module_id = ?1 AND event_type = 'MODULE_TERMINATED'",
                    [id],
                    |row| row.get(0),
                )
                .expect("repeated termination event count");
            assert_eq!(repeated_events, 1);
            assert!(queue_relay_message_in(
                &connection,
                id,
                RelayMessageKind::Manual,
                "终态模块不得发送",
            )
            .is_err());
        }

        insert_relay_module(&connection, "module-unknown", "本模块不确定送达");
        insert_relay_message(
            &connection,
            "unknown-local",
            "module-unknown",
            1,
            "UNKNOWN",
            "2026-08-18T00:00:02Z",
        );
        let unknown_error = terminate_relay_module_in(&connection, "module-unknown")
            .expect_err("local unknown must block termination");
        assert!(unknown_error.contains("不确定送达"));

        insert_relay_module(&connection, "module-other-unknown", "其他模块不确定送达");
        insert_relay_message(
            &connection,
            "unknown-other",
            "module-other-unknown",
            1,
            "UNKNOWN",
            "2026-08-18T00:00:03Z",
        );
        insert_relay_module(&connection, "module-safe", "可终止模块");
        terminate_relay_module_in(&connection, "module-safe")
            .expect("other module unknown must not block termination");

        insert_relay_module(&connection, "module-running", "运行模块");
        insert_relay_codex_cycle(&connection, "cycle-running", "module-running", 1, None)
            .expect("insert cycle");
        connection
            .execute(
                "UPDATE relay_codex_cycles SET status = 'CODEX_RUNNING' WHERE id = 'cycle-running'",
                [],
            )
            .expect("mark running cycle");
        assert_eq!(
            terminate_relay_module_in(&connection, "module-running")
                .expect("Task 6 records a stop request for a running Codex turn"),
            RelayModuleTermination::StopRequested
        );

        insert_relay_module(&connection, "module-completed", "已完成模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'COMPLETED' WHERE id = 'module-completed'",
                [],
            )
            .expect("complete module");
        let completed_error = terminate_relay_module_in(&connection, "module-completed")
            .expect_err("completed module cannot be stopped");
        assert!(completed_error.contains("已验收完成"));
    }

    #[test]
    fn terminate_running_relay_codex_persists_stop_intent_and_finishes_without_returning_result() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-running", "运行模块");
        connection
            .execute(
                "UPDATE relay_modules
                 SET phase = 'CODEX_RUNNING', codex_thread_id = 'thread-kept'
                 WHERE id = 'module-running'",
                [],
            )
            .expect("mark module running");
        insert_relay_message(
            &connection,
            "queued-before-stop",
            "module-running",
            1,
            "QUEUED",
            "2026-08-18T00:00:00Z",
        );
        insert_relay_codex_cycle(&connection, "cycle-running", "module-running", 1, None)
            .expect("insert running cycle");
        connection
            .execute(
                "UPDATE relay_codex_cycles SET status = 'CODEX_RUNNING' WHERE id = 'cycle-running'",
                [],
            )
            .expect("mark cycle running");

        assert_eq!(
            terminate_relay_module_in(&connection, "module-running")
                .expect("running turn accepts a stop request"),
            RelayModuleTermination::StopRequested
        );
        let stop_requested: (String, i64, Option<String>) = connection
            .query_row(
                "SELECT phase, stop_after_turn, codex_thread_id
                 FROM relay_modules WHERE id = 'module-running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("module after stop request");
        assert_eq!(stop_requested.0, "CODEX_RUNNING");
        assert_eq!(stop_requested.1, 1);
        assert_eq!(stop_requested.2.as_deref(), Some("thread-kept"));
        let queued_state: String = connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = 'queued-before-stop'",
                [],
                |row| row.get(0),
            )
            .expect("queued message state");
        assert_eq!(queued_state, "FAILED");
        let stop_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events
                 WHERE module_id = 'module-running' AND event_type = 'MODULE_STOP_REQUESTED'",
                [],
                |row| row.get(0),
            )
            .expect("stop request count");
        assert_eq!(stop_events, 1);
        assert!(matches!(
            terminate_relay_module_in(&connection, "module-running"),
            Ok(RelayModuleTermination::AlreadyStopRequested)
        ));

        assert_eq!(
            complete_relay_codex_turn_in(
                &connection,
                "module-running",
                "cycle-running",
                "最终结果仍需保留。",
            )
            .expect("running turn may naturally complete after termination"),
            RelayCodexTurnCompletion::StoppedAfterTurn
        );
        let stopped: (String, i64, Option<String>) = connection
            .query_row(
                "SELECT phase, stop_after_turn, codex_thread_id
                 FROM relay_modules WHERE id = 'module-running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("stopped module");
        assert_eq!(stopped.0, "STOPPED");
        assert_eq!(stopped.1, 0);
        assert_eq!(stopped.2.as_deref(), Some("thread-kept"));
        let cycle: (String, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT status, result_text, outbound_chatgpt_message_id
                 FROM relay_codex_cycles WHERE id = 'cycle-running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("completed cycle");
        assert_eq!(cycle.0, "CODEX_COMPLETED");
        assert_eq!(cycle.1.as_deref(), Some("最终结果仍需保留。"));
        assert!(cycle.2.is_none());
        let from_codex_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages
                 WHERE module_id = 'module-running' AND direction = 'FROM_CODEX'",
                [],
                |row| row.get(0),
            )
            .expect("saved final text count");
        let outbound_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages
                 WHERE module_id = 'module-running' AND direction = 'TO_CHATGPT'
                   AND id != 'queued-before-stop'",
                [],
                |row| row.get(0),
            )
            .expect("result outbound count");
        assert_eq!(from_codex_count, 1);
        assert_eq!(outbound_count, 0);
    }

    #[test]
    fn terminate_running_relay_codex_finishes_once_and_keeps_stop_semantics_for_failure() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-complete", "完成竞态模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'CODEX_RUNNING', stop_after_turn = 1
                 WHERE id = 'module-complete'",
                [],
            )
            .expect("prepare completion stop request");
        insert_relay_codex_cycle(&connection, "cycle-complete", "module-complete", 1, None)
            .expect("insert completion cycle");
        connection
            .execute(
                "UPDATE relay_codex_cycles SET status = 'CODEX_RUNNING' WHERE id = 'cycle-complete'",
                [],
            )
            .expect("mark completion cycle running");

        for _ in 0..2 {
            assert_eq!(
                complete_relay_codex_turn_in(
                    &connection,
                    "module-complete",
                    "cycle-complete",
                    "同一份最终结果。",
                )
                .expect("completion interleaving remains idempotent"),
                RelayCodexTurnCompletion::StoppedAfterTurn
            );
        }
        let final_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages
                 WHERE module_id = 'module-complete' AND direction = 'FROM_CODEX'",
                [],
                |row| row.get(0),
            )
            .expect("one final history row");
        assert_eq!(final_rows, 1);

        insert_relay_module(&connection, "module-failed", "失败收尾模块");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'CODEX_RUNNING', stop_after_turn = 1
                 WHERE id = 'module-failed'",
                [],
            )
            .expect("prepare failed stop request");
        insert_relay_codex_cycle(&connection, "cycle-failed", "module-failed", 1, None)
            .expect("insert failure cycle");
        connection
            .execute(
                "UPDATE relay_codex_cycles SET status = 'CODEX_RUNNING' WHERE id = 'cycle-failed'",
                [],
            )
            .expect("mark failure cycle running");
        fail_relay_codex_cycle(&connection, "cycle-failed", "App Server 失败")
            .expect("record actual failure");
        assert!(
            finalize_relay_module_stop_after_turn_in(&connection, "module-failed")
                .expect("finish stop after actual failure")
        );
        let failed: (String, String, i64) = connection
            .query_row(
                "SELECT module.phase, cycle.status, module.stop_after_turn
                 FROM relay_modules AS module
                 JOIN relay_codex_cycles AS cycle ON cycle.module_id = module.id
                 WHERE module.id = 'module-failed'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("failed stop result");
        assert_eq!(failed, ("STOPPED".into(), "FAILED".into(), 0));

        insert_relay_module(&connection, "module-no-new-turn", "禁止新回合模块");
        connection
            .execute(
                "UPDATE relay_modules SET stop_after_turn = 1 WHERE id = 'module-no-new-turn'",
                [],
            )
            .expect("prepare stop requested module");
        let module = get_relay_module(&connection, "module-no-new-turn")
            .expect("read module")
            .expect("module exists");
        assert_eq!(
            relay_codex_start_block_reason(&module),
            Some("模块正在终止，不能启动新的 Codex 回合。")
        );
    }

    #[test]
    fn terminate_running_relay_codex_exposes_completed_result_without_chatgpt_return() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        connection
            .execute(
                "UPDATE relay_modules SET phase = 'STOPPED' WHERE id = 'module-a'",
                [],
            )
            .expect("stop module");
        insert_relay_codex_cycle(&connection, "cycle-a", "module-a", 1, None)
            .expect("insert cycle");
        connection
            .execute(
                "UPDATE relay_codex_cycles
                 SET status = 'CODEX_COMPLETED', result_text = '已完成', codex_completed_at = '2026-08-18T00:00:00Z'
                 WHERE id = 'cycle-a'",
                [],
            )
            .expect("complete cycle without outbound");

        let cycles = list_relay_codex_cycles_in(&connection, "module-a")
            .expect("list stopped module cycles");
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0].block_reason.as_deref(),
            Some("模块已由用户终止，结果未回传 ChatGPT")
        );
    }

    #[test]
    fn relay_codex_cycle_enforces_one_cycle_number_per_module() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");

        insert_relay_codex_cycle(&connection, "cycle-a", "module-a", 1, None).expect("first cycle");
        let duplicate = insert_relay_codex_cycle(&connection, "cycle-b", "module-a", 1, None);

        assert!(duplicate.is_err(), "a module cycle number must be unique");
    }

    #[test]
    fn relay_codex_cycle_create_and_read_helpers_preserve_initial_state() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_message(
            &connection,
            "outbound-a",
            "module-a",
            1,
            "QUEUED",
            "2026-08-17T00:00:00Z",
        );

        let cycle = create_relay_codex_cycle(&connection, "module-a", 1, "请只回复 RELAY_E2E_OK")
            .expect("create cycle");
        assert_eq!(cycle.status, "WAITING_TO_SEND_CODEX");
        assert_eq!(cycle.prompt_text, "请只回复 RELAY_E2E_OK");
        assert!(cycle.codex_thread_id.is_none());
        assert!(cycle.codex_turn_id.is_none());
        assert!(cycle.result_text.is_none());
        assert!(cycle.outbound_chatgpt_message_id.is_none());
        assert!(cycle.error_text.is_none());
        assert!(cycle.codex_started_at.is_none());
        assert!(cycle.codex_completed_at.is_none());
        assert!(cycle.relay_queued_at.is_none());
        assert!(cycle.relay_delivered_at.is_none());
        assert!(cycle.block_reason.is_none());
        DateTime::parse_from_rfc3339(&cycle.created_at).expect("UTC RFC3339 created timestamp");
        DateTime::parse_from_rfc3339(&cycle.updated_at).expect("UTC RFC3339 updated timestamp");

        let by_id = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read cycle")
            .expect("cycle exists");
        assert_eq!(by_id.id, cycle.id);
        assert!(
            get_relay_codex_cycle_by_outbound_message(&connection, "outbound-a")
                .expect("read unlinked message")
                .is_none()
        );

        connection
            .execute(
                "UPDATE relay_codex_cycles SET outbound_chatgpt_message_id = ?1 WHERE id = ?2",
                params!["outbound-a", &cycle.id],
            )
            .expect("link outbound message");
        let by_outbound = get_relay_codex_cycle_by_outbound_message(&connection, "outbound-a")
            .expect("read linked message")
            .expect("cycle is linked");
        assert_eq!(by_outbound.id, cycle.id);
    }

    #[test]
    fn relay_codex_cycle_enforces_one_outbound_message_per_cycle() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_message(
            &connection,
            "outbound-a",
            "module-a",
            1,
            "QUEUED",
            "2026-08-17T00:00:00Z",
        );

        insert_relay_codex_cycle(&connection, "cycle-a", "module-a", 1, Some("outbound-a"))
            .expect("first linked cycle");
        let duplicate =
            insert_relay_codex_cycle(&connection, "cycle-b", "module-a", 2, Some("outbound-a"));

        assert!(
            duplicate.is_err(),
            "an outbound message must link to only one cycle"
        );
    }

    #[test]
    fn relay_codex_lifecycle_persists_result_and_queues_it_once() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        let cycle = create_relay_codex_cycle(&connection, "module-a", 1, "请只回复 RELAY_E2E_OK")
            .expect("create cycle");

        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), Some("turn-a"))
            .expect("mark turn started");
        let running = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read running cycle")
            .expect("cycle exists");
        assert_eq!(running.status, "CODEX_RUNNING");
        assert_eq!(running.codex_thread_id.as_deref(), Some("thread-a"));
        assert_eq!(running.codex_turn_id.as_deref(), Some("turn-a"));
        assert!(running.codex_started_at.is_some());

        mark_relay_codex_result_received(&connection, &cycle.id, "RELAY_E2E_OK")
            .expect("mark final text");
        let completed = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read completed cycle")
            .expect("cycle exists");
        assert_eq!(completed.status, "CODEX_COMPLETED");
        assert_eq!(completed.result_text.as_deref(), Some("RELAY_E2E_OK"));
        assert!(completed.codex_completed_at.is_some());

        let outbound_message_id = queue_relay_codex_result_to_chatgpt(&connection, &cycle.id)
            .expect("queue Codex result");
        let repeated_message_id = queue_relay_codex_result_to_chatgpt(&connection, &cycle.id)
            .expect("repeat queue is idempotent");
        assert_eq!(repeated_message_id, outbound_message_id);

        let queued = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read queued cycle")
            .expect("cycle exists");
        assert_eq!(queued.status, "WAITING_FOR_CHATGPT");
        assert_eq!(
            queued.outbound_chatgpt_message_id.as_deref(),
            Some(outbound_message_id.as_str())
        );
        assert!(queued.relay_queued_at.is_some());
        let result_messages: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages
                 WHERE module_id = ?1 AND direction = 'TO_CHATGPT' AND kind = 'AUTOMATION'",
                ["module-a"],
                |row| row.get(0),
            )
            .expect("count result messages");
        assert_eq!(result_messages, 1);

        let event_types: Vec<String> = connection
            .prepare(
                "SELECT event_type FROM relay_events WHERE module_id = 'module-a'
                 ORDER BY created_at ASC, id ASC",
            )
            .expect("prepare events")
            .query_map([], |row| row.get(0))
            .expect("query events")
            .collect::<Result<_, _>>()
            .expect("read events");
        assert!(event_types
            .iter()
            .any(|event| event == "CODEX_TURN_STARTED"));
        assert!(event_types
            .iter()
            .any(|event| event == "CODEX_RESULT_RECEIVED"));
        assert!(event_types
            .iter()
            .any(|event| event == "CODEX_RESULT_QUEUED_TO_CHATGPT"));
    }

    #[test]
    fn relay_codex_lifecycle_turn_failure_persists_failure_without_result() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        let cycle = create_relay_codex_cycle(&connection, "module-a", 1, "请执行失败路径")
            .expect("create cycle");

        fail_relay_codex_cycle(&connection, &cycle.id, "无法启动 Codex App Server")
            .expect("record start failure");

        let failed = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read failed cycle")
            .expect("cycle exists");
        assert_eq!(failed.status, "FAILED");
        assert_eq!(
            failed.error_text.as_deref(),
            Some("无法启动 Codex App Server")
        );
        assert!(failed.result_text.is_none());
        assert!(failed.outbound_chatgpt_message_id.is_none());
        let outbound_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages
                 WHERE module_id = ?1 AND direction = 'TO_CHATGPT'",
                ["module-a"],
                |row| row.get(0),
            )
            .expect("count outbound messages");
        assert_eq!(outbound_count, 0);
        let failure_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events
                 WHERE module_id = ?1 AND event_type = 'CODEX_TURN_FAILED'",
                ["module-a"],
                |row| row.get(0),
            )
            .expect("count failure events");
        assert_eq!(failure_events, 1);
    }

    #[test]
    fn codex_cycle_chatgpt_delivery_tracks_fifo_claim_and_matching_reply_once() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        let cycle = create_relay_codex_cycle(&connection, "module-a", 1, "请只回复 RELAY_E2E_OK")
            .expect("create cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), Some("turn-a"))
            .expect("start turn");
        mark_relay_codex_result_received(&connection, &cycle.id, "RELAY_E2E_OK")
            .expect("persist result");
        let message_id =
            queue_relay_codex_result_to_chatgpt(&connection, &cycle.id).expect("queue result");

        let message = match claim_next_relay_message_for_dispatch(&connection)
            .expect("claim global FIFO message")
        {
            RelayDispatchClaim::Message(message) => message,
            _ => panic!("Codex result should be claimed for dispatch"),
        };
        assert_eq!(message.id, message_id);
        let sending = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read sending cycle")
            .expect("cycle exists");
        assert_eq!(sending.status, "SENDING_TO_CHATGPT");

        connection
            .execute(
                "UPDATE relay_messages SET delivery_state = 'DELIVERED', delivered_at = ?2 WHERE id = ?1",
                params![&message_id, "2026-08-17T00:00:02Z"],
            )
            .expect("accept matching chatgptReply");
        sync_codex_cycle_for_chatgpt_message_state(&connection, &message_id, "DELIVERED", None)
            .expect("sync matching reply delivery");

        let delivered = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read delivered cycle")
            .expect("cycle exists");
        assert_eq!(delivered.status, "DELIVERED_TO_CHATGPT");
        assert!(delivered.relay_delivered_at.is_some());
        let send_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events
                 WHERE module_id = 'module-a' AND event_type = 'CODEX_RESULT_SEND_STARTED'",
                [],
                |row| row.get(0),
            )
            .expect("count send-start events");
        let delivered_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_events
                 WHERE module_id = 'module-a' AND event_type = 'CODEX_RESULT_DELIVERED_TO_CHATGPT'",
                [],
                |row| row.get(0),
            )
            .expect("count delivered events");
        assert_eq!(send_events, 1);
        assert_eq!(delivered_events, 1);
    }

    #[test]
    fn codex_cycle_chatgpt_delivery_restart_marks_result_unknown_without_rerunning_codex() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        let cycle = create_relay_codex_cycle(&connection, "module-a", 1, "请只回复 RELAY_E2E_OK")
            .expect("create cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), Some("turn-a"))
            .expect("start turn");
        mark_relay_codex_result_received(&connection, &cycle.id, "RELAY_E2E_OK")
            .expect("persist result");
        let message_id =
            queue_relay_codex_result_to_chatgpt(&connection, &cycle.id).expect("queue result");
        match claim_next_relay_message_for_dispatch(&connection).expect("claim result") {
            RelayDispatchClaim::Message(message) => assert_eq!(message.id, message_id),
            _ => panic!("Codex result should be in flight before restart"),
        }

        mark_uncertain_relay_deliveries(&connection).expect("restart recovery");

        let uncertain = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read uncertain cycle")
            .expect("cycle exists");
        assert_eq!(uncertain.status, "WAITING_FOR_CHATGPT");
        assert_eq!(uncertain.result_text.as_deref(), Some("RELAY_E2E_OK"));
        assert!(uncertain.error_text.is_some());
        let outbound_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages WHERE module_id = 'module-a' AND direction = 'TO_CHATGPT'",
                [],
                |row| row.get(0),
            )
            .expect("count outbound results");
        let cycle_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_codex_cycles WHERE module_id = 'module-a'",
                [],
                |row| row.get(0),
            )
            .expect("count cycles");
        assert_eq!(outbound_count, 1);
        assert_eq!(cycle_count, 1);
        assert!(matches!(
            claim_next_relay_message_for_dispatch(&connection),
            Ok(RelayDispatchClaim::RecoveryBlocked(1))
        ));
    }

    #[test]
    fn codex_cycle_chatgpt_delivery_explicit_recovery_reuses_or_abandons_the_linked_result() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        let cycle = create_relay_codex_cycle(&connection, "module-a", 1, "请只回复 RELAY_E2E_OK")
            .expect("create cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), Some("turn-a"))
            .expect("start turn");
        mark_relay_codex_result_received(&connection, &cycle.id, "RELAY_E2E_OK")
            .expect("persist result");
        let message_id =
            queue_relay_codex_result_to_chatgpt(&connection, &cycle.id).expect("queue result");
        match claim_next_relay_message_for_dispatch(&connection).expect("claim result") {
            RelayDispatchClaim::Message(message) => assert_eq!(message.id, message_id),
            _ => panic!("Codex result should be in flight"),
        }
        pause_relay_for_uncertain_delivery(
            &connection,
            &message_id,
            "CHATGPT_TRANSPORT_FAILURE",
            "transport outcome unknown",
        )
        .expect("mark result uncertain");

        requeue_unknown_relay_message(&connection, &message_id).expect("explicit resend");
        let requeued = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read requeued cycle")
            .expect("cycle exists");
        assert_eq!(requeued.status, "WAITING_FOR_CHATGPT");
        assert_eq!(
            requeued.outbound_chatgpt_message_id.as_deref(),
            Some(message_id.as_str())
        );

        match claim_next_relay_message_for_dispatch(&connection).expect("claim explicit resend") {
            RelayDispatchClaim::Message(message) => assert_eq!(message.id, message_id),
            _ => panic!("explicit resend must reuse the original result message"),
        }
        pause_relay_for_uncertain_delivery(
            &connection,
            &message_id,
            "CHATGPT_TRANSPORT_FAILURE",
            "transport outcome unknown again",
        )
        .expect("mark retried result uncertain");
        resolve_unknown_relay_message_without_resend(&connection, &message_id)
            .expect("explicitly continue without resending");

        let failed = get_relay_codex_cycle_by_id(&connection, &cycle.id)
            .expect("read failed cycle")
            .expect("cycle exists");
        assert_eq!(failed.status, "FAILED");
        assert_eq!(failed.result_text.as_deref(), Some("RELAY_E2E_OK"));
        let outbound_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages WHERE module_id = 'module-a' AND direction = 'TO_CHATGPT'",
                [],
                |row| row.get(0),
            )
            .expect("count outbound results");
        assert_eq!(
            outbound_count, 1,
            "continue without resend must not allocate a replacement"
        );
        let message_state: String = connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = ?1",
                [&message_id],
                |row| row.get(0),
            )
            .expect("read original message state");
        assert_eq!(message_state, "FAILED");
    }

    #[test]
    fn relay_channel_snapshot_is_idle_without_in_flight_chatgpt_or_running_codex() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");

        let snapshot = relay_channel_snapshot_from_connection(&connection)
            .expect("read idle channel snapshot");

        assert_eq!(snapshot.chatgpt.status, "IDLE");
        assert_eq!(snapshot.chatgpt.recovery_blocker_count, 0);
        assert!(snapshot.chatgpt.active_message_id.is_none());
        assert_eq!(snapshot.codex.status, "IDLE");
        assert!(snapshot.codex.active_module_id.is_none());
    }

    #[test]
    fn relay_channel_snapshot_reports_sent_message_as_chatgpt_in_flight() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_message(
            &connection,
            "message-a",
            "module-a",
            1,
            "SENT",
            "2026-08-17T00:00:00Z",
        );

        let snapshot = relay_channel_snapshot_from_connection(&connection)
            .expect("read in-flight channel snapshot");

        assert_eq!(snapshot.chatgpt.status, "IN_FLIGHT");
        assert_eq!(
            snapshot.chatgpt.active_module_id.as_deref(),
            Some("module-a")
        );
        assert_eq!(
            snapshot.chatgpt.active_module_name.as_deref(),
            Some("模块 A")
        );
        assert_eq!(
            snapshot.chatgpt.active_message_id.as_deref(),
            Some("message-a")
        );
        assert_eq!(snapshot.chatgpt.active_kind.as_deref(), Some("AUTOMATION"));
        assert_eq!(snapshot.chatgpt.active_phase.as_deref(), Some("SENT"));
        assert_eq!(snapshot.chatgpt.recovery_blocker_count, 0);
    }

    #[test]
    fn relay_channel_snapshot_gives_unknown_priority_over_sent_message() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_module(&connection, "module-b", "模块 B");
        insert_relay_message(
            &connection,
            "sent-a",
            "module-a",
            1,
            "SENT",
            "2026-08-17T00:00:00Z",
        );
        insert_relay_message(
            &connection,
            "unknown-b",
            "module-b",
            1,
            "UNKNOWN",
            "2026-08-17T00:00:01Z",
        );

        let snapshot = relay_channel_snapshot_from_connection(&connection)
            .expect("read recovery-blocked channel snapshot");

        assert_eq!(snapshot.chatgpt.status, "RECOVERY_BLOCKED");
        assert_eq!(snapshot.chatgpt.recovery_blocker_count, 1);
        assert_eq!(
            snapshot.chatgpt.active_message_id.as_deref(),
            Some("unknown-b")
        );
        assert_eq!(snapshot.chatgpt.active_phase.as_deref(), Some("UNKNOWN"));
    }

    #[test]
    fn relay_channel_snapshot_reports_the_single_running_codex_cycle() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        let cycle =
            create_relay_codex_cycle(&connection, "module-a", 4, "执行任务").expect("create cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), Some("turn-a"))
            .expect("start cycle");

        let snapshot = relay_channel_snapshot_from_connection(&connection)
            .expect("read running Codex snapshot");

        assert_eq!(snapshot.codex.status, "RUNNING");
        assert_eq!(snapshot.codex.active_module_id.as_deref(), Some("module-a"));
        assert_eq!(snapshot.codex.active_module_name.as_deref(), Some("模块 A"));
        assert_eq!(snapshot.codex.cycle_number, Some(4));
        assert_eq!(snapshot.codex.codex_thread_id.as_deref(), Some("thread-a"));
        assert_eq!(snapshot.codex.codex_turn_id.as_deref(), Some("turn-a"));
        assert_eq!(
            snapshot.codex.cycle_status.as_deref(),
            Some("CODEX_RUNNING")
        );
    }

    #[test]
    fn relay_channel_snapshot_explains_when_completed_cycle_waits_for_another_module() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_module(&connection, "module-b", "模块 B");
        insert_relay_message(
            &connection,
            "sent-b",
            "module-b",
            1,
            "SENT",
            "2026-08-17T00:00:00Z",
        );
        let cycle =
            create_relay_codex_cycle(&connection, "module-a", 1, "执行任务").expect("create cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), None)
            .expect("start cycle");
        mark_relay_codex_result_received(&connection, &cycle.id, "RELAY_E2E_OK")
            .expect("complete cycle");
        queue_relay_codex_result_to_chatgpt(&connection, &cycle.id).expect("queue result");

        let cycles =
            list_relay_codex_cycles_in(&connection, "module-a").expect("list module cycles");

        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].status, "WAITING_FOR_CHATGPT");
        assert_eq!(
            cycles[0].block_reason.as_deref(),
            Some("ChatGPT 通道当前被模块「模块 B」占用（消息 sent-b）。")
        );
    }

    #[test]
    fn relay_channel_snapshot_explains_recovery_blockers_for_waiting_cycles() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_module(&connection, "module-b", "模块 B");
        insert_relay_message(
            &connection,
            "unknown-b",
            "module-b",
            1,
            "UNKNOWN",
            "2026-08-17T00:00:00Z",
        );
        let cycle =
            create_relay_codex_cycle(&connection, "module-a", 1, "执行任务").expect("create cycle");
        mark_relay_codex_turn_started(&connection, &cycle.id, Some("thread-a"), None)
            .expect("start cycle");
        mark_relay_codex_result_received(&connection, &cycle.id, "RELAY_E2E_OK")
            .expect("complete cycle");
        queue_relay_codex_result_to_chatgpt(&connection, &cycle.id).expect("queue result");

        let cycles =
            list_relay_codex_cycles_in(&connection, "module-a").expect("list module cycles");

        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0].block_reason.as_deref(),
            Some("存在待人工处理的不确定送达消息（1 条）。")
        );
    }

    #[test]
    fn adapter_failure_marks_relay_delivery_unknown_without_parsing_or_retrying() {
        let connection = relay_connection();
        let now = "2026-08-17T00:00:00Z";
        connection.execute(
            "INSERT INTO relay_modules (id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase, invalid_reply_count, started_cycles, created_at, updated_at)
             VALUES ('relay-1', 'Relay', 'G:\\workspace', 12, 240, 'retry', 'READY', 0, 0, ?1, ?1)",
            [now],
        ).expect("relay module");
        connection.execute(
            "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at)
             VALUES ('outgoing-1', 'relay-1', 1, 'TO_CHATGPT', 'AUTOMATION', 'request', 'SENT', ?1)",
            [now],
        ).expect("outgoing relay message");

        pause_relay_for_uncertain_delivery(
            &connection,
            "outgoing-1",
            "CHATGPT_ADAPTER_FAILURE",
            "adapter failed after the send may have started",
        )
        .expect("adapter failure is recorded");

        let (delivery_state, phase, invalid_reply_count): (String, String, i64) = connection.query_row(
            "SELECT message.delivery_state, module.phase, module.invalid_reply_count
             FROM relay_messages AS message JOIN relay_modules AS module ON module.id = message.module_id
             WHERE message.id = 'outgoing-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).expect("relay state");
        assert_eq!(delivery_state, "UNKNOWN");
        assert_eq!(phase, "RECOVERY_REQUIRED");
        assert_eq!(invalid_reply_count, 0);

        let incoming_replies: i64 = connection.query_row(
            "SELECT COUNT(*) FROM relay_messages WHERE module_id = 'relay-1' AND direction = 'FROM_CHATGPT'",
            [],
            |row| row.get(0),
        ).expect("incoming reply count");
        let queued_retries: i64 = connection.query_row(
            "SELECT COUNT(*) FROM relay_messages WHERE module_id = 'relay-1' AND direction = 'TO_CHATGPT' AND delivery_state = 'QUEUED'",
            [],
            |row| row.get(0),
        ).expect("retry count");
        assert_eq!(incoming_replies, 0);
        assert_eq!(queued_retries, 0);
    }

    #[test]
    fn unknown_relay_delivery_only_requeues_after_an_explicit_user_action() {
        let connection = relay_connection();
        let now = "2026-08-17T00:00:00Z";
        connection.execute(
            "INSERT INTO relay_modules (id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase, invalid_reply_count, started_cycles, created_at, updated_at)
             VALUES ('relay-1', 'Relay', 'G:\\workspace', 12, 240, 'retry', 'RECOVERY_REQUIRED', 0, 0, ?1, ?1)",
            [now],
        ).expect("relay module");
        connection.execute(
            "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at)
             VALUES ('outgoing-1', 'relay-1', 1, 'TO_CHATGPT', 'AUTOMATION', 'request', 'UNKNOWN', ?1)",
            [now],
        ).expect("unknown relay message");

        requeue_unknown_relay_message(&connection, "outgoing-1")
            .expect("explicit resend requeues the message");

        let (delivery_state, phase): (String, String) = connection.query_row(
            "SELECT message.delivery_state, module.phase
             FROM relay_messages AS message JOIN relay_modules AS module ON module.id = message.module_id
             WHERE message.id = 'outgoing-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("requeued relay state");
        assert_eq!(delivery_state, "QUEUED");
        assert_eq!(phase, "READY");
    }

    #[test]
    fn all_global_unknown_blockers_remain_visible_until_each_is_explicitly_resolved() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_module(&connection, "module-b", "模块 B");
        insert_relay_module(&connection, "module-c", "模块 C");
        insert_relay_message(
            &connection,
            "unknown-a",
            "module-a",
            4,
            "UNKNOWN",
            "2026-08-17T00:00:01Z",
        );
        insert_relay_message(
            &connection,
            "queued-b",
            "module-b",
            1,
            "QUEUED",
            "2026-08-17T00:00:02Z",
        );
        insert_relay_message(
            &connection,
            "unknown-c",
            "module-c",
            2,
            "UNKNOWN",
            "2026-08-17T00:00:03Z",
        );

        let blockers = list_relay_recovery_messages_in(&connection).expect("all recovery blockers");
        assert_eq!(blockers.len(), 2);
        assert_eq!(blockers[0].module_name, "模块 A");
        assert_eq!(blockers[1].module_name, "模块 C");
        assert!(matches!(
            claim_next_relay_message_for_dispatch(&connection),
            Ok(RelayDispatchClaim::RecoveryBlocked(2))
        ));

        resolve_unknown_relay_message_without_resend(&connection, "unknown-a")
            .expect("explicit continue without resend");
        let remaining = list_relay_recovery_messages_in(&connection).expect("remaining blocker");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].message_id, "unknown-c");
        assert!(matches!(
            claim_next_relay_message_for_dispatch(&connection),
            Ok(RelayDispatchClaim::RecoveryBlocked(1))
        ));

        resolve_unknown_relay_message_without_resend(&connection, "unknown-c")
            .expect("resolve final blocker without resending it");
        let old_message_state: String = connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = 'unknown-a'",
                [],
                |row| row.get(0),
            )
            .expect("old message state");
        assert_eq!(old_message_state, "FAILED");
        let message =
            match claim_next_relay_message_for_dispatch(&connection).expect("queue resumes") {
                RelayDispatchClaim::Message(message) => message,
                _ => panic!("the queued message must dispatch after every UNKNOWN is resolved"),
            };
        assert_eq!(message.id, "queued-b");
        record_relay_message_dispatched(&connection, &message).expect("dispatch event");
        let events: i64 = connection.query_row(
            "SELECT COUNT(*) FROM relay_events WHERE module_id = 'module-b' AND event_type IN ('CHATGPT_SEND_STARTED', 'CHATGPT_SEND_DISPATCHED')",
            [], |row| row.get(0),
        ).expect("dispatch event count");
        assert_eq!(events, 2);
    }

    #[test]
    fn explicit_resend_requeues_only_the_selected_unknown_message_once() {
        let connection = relay_connection();
        insert_relay_module(&connection, "module-a", "模块 A");
        insert_relay_message(
            &connection,
            "unknown-a",
            "module-a",
            1,
            "UNKNOWN",
            "2026-08-17T00:00:01Z",
        );

        requeue_unknown_relay_message(&connection, "unknown-a").expect("explicit resend");
        let message = match claim_next_relay_message_for_dispatch(&connection)
            .expect("requeued message dispatches")
        {
            RelayDispatchClaim::Message(message) => message,
            _ => panic!("the selected message must be the only dispatch candidate"),
        };
        assert_eq!(message.id, "unknown-a");
        let resend_events: i64 = connection.query_row(
            "SELECT COUNT(*) FROM relay_events WHERE module_id = 'module-a' AND event_type = 'CHATGPT_EXPLICIT_RESEND'",
            [], |row| row.get(0),
        ).expect("explicit resend count");
        assert_eq!(resend_events, 1);
    }

    #[test]
    fn restart_marks_sent_relay_messages_unknown_without_requeueing_them() {
        let connection = relay_connection();
        let now = "2026-08-17T00:00:00Z";
        connection.execute(
            "INSERT INTO relay_modules (id, name, working_directory, max_cycles, max_runtime_minutes, retry_template, phase, invalid_reply_count, started_cycles, created_at, updated_at)
             VALUES ('relay-1', 'Relay', 'G:\\workspace', 12, 240, 'retry', 'READY', 0, 0, ?1, ?1)",
            [now],
        ).expect("relay module");
        connection.execute(
            "INSERT INTO relay_messages (id, module_id, sequence_number, direction, kind, text, delivery_state, created_at)
             VALUES ('outgoing-1', 'relay-1', 1, 'TO_CHATGPT', 'AUTOMATION', 'request', 'SENT', ?1)",
            [now],
        ).expect("sent relay message");

        mark_uncertain_relay_deliveries(&connection).expect("restart recovery");

        let delivery_state: String = connection
            .query_row(
                "SELECT delivery_state FROM relay_messages WHERE id = 'outgoing-1'",
                [],
                |row| row.get(0),
            )
            .expect("recovered delivery state");
        let queued_messages: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM relay_messages WHERE delivery_state = 'QUEUED'",
                [],
                |row| row.get(0),
            )
            .expect("queued message count");
        assert_eq!(delivery_state, "UNKNOWN");
        assert_eq!(queued_messages, 0);
    }

    #[test]
    fn inactive_module_can_be_saved_reopened_updated_and_deleted() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(INITIAL_SCHEMA)
            .expect("initial schema");

        let created = create_inactive_module_in(&mut connection, &input()).expect("create module");
        assert_eq!(created.status, "INACTIVE");
        assert_eq!(created.repository_path, r"G:\projects\personal-repo");

        let reopened = get_module(&connection, &created.id)
            .expect("read module")
            .expect("module exists");
        assert_eq!(reopened.budget.max_rounds, 6);

        let mut changed_input = input();
        changed_input.name = "Desktop foundation revised".into();
        changed_input.max_rounds = 8;
        let updated = update_inactive_module_in(&mut connection, &created.id, &changed_input)
            .expect("update module");
        assert_eq!(updated.repository_path, r"G:\projects\personal-repo");
        assert_eq!(updated.budget.max_rounds, 8);

        delete_inactive_module_in(&connection, &created.id).expect("delete module");
        assert!(get_module(&connection, &created.id)
            .expect("read after delete")
            .is_none());
    }

    #[test]
    fn orchestration_transition_is_persisted_with_an_audit_event() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(INITIAL_SCHEMA)
            .expect("initial schema");
        connection
            .execute_batch(ORCHESTRATION_SCHEMA)
            .expect("orchestration schema");
        let module = create_inactive_module_in(&mut connection, &input()).expect("create module");
        let (runtime, _) = orchestration::Runtime::start();
        persist_orchestration_state(&connection, &module.id, &runtime, "waiting for planning")
            .expect("persist transition");

        let phase: String = connection
            .query_row(
                "SELECT phase FROM module_runtime WHERE module_id = ?1",
                [&module.id],
                |row| row.get(0),
            )
            .expect("runtime record");
        assert_eq!(phase, "WAITING_FOR_CHATGPT_PLAN");
        let events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE module_id = ?1",
                [&module.id],
                |row| row.get(0),
            )
            .expect("audit record");
        assert_eq!(events, 1);
    }

    #[test]
    fn execution_details_keep_budget_pause_and_verified_commit() {
        let mut connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(INITIAL_SCHEMA)
            .expect("initial schema");
        connection
            .execute_batch(ORCHESTRATION_SCHEMA)
            .expect("orchestration schema");
        connection
            .execute_batch(EXECUTION_CONTROL_SCHEMA)
            .expect("execution control schema");
        let module = create_inactive_module_in(&mut connection, &input()).expect("create module");
        let (mut runtime, _) = orchestration::Runtime::start();
        runtime.pause_after_current_turn = true;
        let active = ActiveOrchestration {
            module: module.clone(),
            runtime,
            started_at: Utc::now(),
            last_commit_sha: Some("abcdef1234567".into()),
        };
        persist_execution_details(&connection, &active).expect("persist execution details");
        let (paused, commit): (i64, Option<String>) = connection
            .query_row(
                "SELECT pause_after_current_turn, last_commit_sha FROM module_execution_state WHERE module_id = ?1",
                [&module.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("execution details");
        assert_eq!(paused, 1);
        assert_eq!(commit.as_deref(), Some("abcdef1234567"));
    }

    #[test]
    fn commit_sha_extraction_requires_a_git_sized_token() {
        assert_eq!(
            extract_reported_commit_sha("commit: abcdef1234567"),
            Some("abcdef1234567".into())
        );
        assert_eq!(extract_reported_commit_sha("no commit reported"), None);
    }

    #[test]
    fn task_wrapper_preserves_safe_smoke_instruction_and_branch() {
        let module = ModuleRecord {
            id: "module-1".into(),
            name: "Test".into(),
            repository_path: r"G:\projects\personal-repo".into(),
            target_branch: "main".into(),
            chatgpt_tab_id: 42,
            status: "INACTIVE".into(),
            budget: Budget {
                max_rounds: 6,
                module_timeout_minutes: 120,
                global_timeout_minutes: 240,
            },
            created_at: "2026-08-15T00:00:00Z".into(),
            updated_at: "2026-08-15T00:00:00Z".into(),
        };
        let wrapped = wrap_codex_task(
            &module,
            "Reply exactly CODEX_ADAPTER_SMOKE_OK. Do not run commands or modify anything.",
        );
        assert!(wrapped.contains("Target branch: main"));
        assert!(wrapped.contains("CODEX_ADAPTER_SMOKE_OK"));
        assert!(
            wrapped.contains("do not run commands, inspect files, modify files, commit, or push")
        );
    }

    #[test]
    fn default_codex_command_uses_the_windows_cmd_launcher() {
        #[cfg(windows)]
        assert_eq!(default_codex_command(), "codex.cmd");

        #[cfg(not(windows))]
        assert_eq!(default_codex_command(), "codex");
    }

    #[test]
    fn app_server_notification_maps_to_a_typed_status_event() {
        let message = json!({
            "method": "item/completed",
            "params": { "item": { "type": "agentMessage", "text": "done" } }
        });
        let event = event_from_notification(
            "module-1",
            &message,
            &Some("thread-1".into()),
            &Some("turn-1".into()),
        )
        .expect("known notification becomes an event");
        assert_eq!(event.phase, CodexExecutionPhase::Running);
        assert_eq!(event.thread_id.as_deref(), Some("thread-1"));
        assert!(event.status_line.contains("agentMessage"));
    }

    #[test]
    fn protocol_validator_accepts_one_final_pause_envelope() {
        let reply = r#"协议已加载。
```json
{
  "state": "PAUSE",
  "module": "Adapter",
  "reason": "等待用户开始模块",
  "acceptance_criteria": [],
  "requires_user_input": false
}
```"#;
        let envelope = validate_protocol_payload(reply, None).expect("valid protocol reply");
        assert_eq!(protocol_state_name(&envelope.state), "PAUSE");
    }

    #[test]
    fn protocol_validator_accepts_structured_json_from_extension() {
        let json = r#"{
  "state": "PAUSE",
  "module": "Adapter",
  "reason": "等待用户开始模块",
  "acceptance_criteria": [],
  "requires_user_input": false
}"#;
        let envelope =
            validate_protocol_payload("ChatGPT rendered a code block without fences.", Some(json))
                .expect("structured extension JSON is a valid protocol reply");
        assert_eq!(protocol_state_name(&envelope.state), "PAUSE");
    }

    #[test]
    fn protocol_validator_accepts_unfenced_json_with_chatgpt_presentation_chrome() {
        let reply = r#"协议已加载。
JSON
{
  "state": "PAUSE",
  "module": "Adapter",
  "reason": "等待用户开始模块",
  "acceptance_criteria": [],
  "requires_user_input": false
}
Copy code"#;
        let envelope = validate_protocol_payload(reply, None)
            .expect("unfenced ChatGPT-rendered JSON is a valid protocol reply");
        assert_eq!(protocol_state_name(&envelope.state), "PAUSE");
    }

    #[test]
    fn protocol_validator_blocks_invalid_next_task_and_extra_blocks() {
        let missing_prompt = r#"```json
{"state":"NEXT_TASK","module":"Adapter","reason":"开始","acceptance_criteria":["works"],"requires_user_input":false}
```"#;
        assert!(validate_protocol_payload(missing_prompt, None).is_err());

        let extra_block = r#"```json
{"state":"PAUSE","module":"Adapter","reason":"等待","acceptance_criteria":[],"requires_user_input":false}
```
```json
{} 
```"#;
        assert!(validate_protocol_payload(extra_block, None).is_err());
    }

    #[test]
    fn bridge_keeps_a_paired_connection_alive_when_it_receives_a_ping() {
        let frame = classify_bridge_frame(Some(Ok(Message::Ping(vec![1, 2, 3].into()))));
        assert!(matches!(frame, BridgeFrame::Ping(_)));
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let connection = create_connection(app.handle())?;
            pause_unfinished_orchestrations(&connection)?;
            mark_uncertain_relay_deliveries(&connection)?;
            let chatgpt_bridge = Arc::new(ChatGptBridge::new());
            start_chatgpt_bridge(app.handle().clone(), chatgpt_bridge.clone())?;
            app.manage(AppState {
                connection: Mutex::new(connection),
                chatgpt_bridge,
                orchestrator: Mutex::new(None),
                relay_codex: Mutex::new(None),
                application_started_at: Utc::now(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_inactive_modules,
            create_inactive_module,
            update_inactive_module,
            delete_inactive_module,
            execute_controlled_codex_turn,
            get_chatgpt_pairing,
            send_chatgpt_message,
            create_relay_module,
            accept_relay_module,
            terminate_relay_module,
            submit_relay_acceptance_feedback,
            list_relay_modules,
            list_relay_messages,
            list_relay_codex_cycles,
            get_relay_channel_snapshot,
            list_relay_recovery_messages,
            queue_relay_message,
            retry_unknown_relay_message,
            continue_unknown_relay_message_without_resend,
            start_module_orchestration,
            get_orchestration_snapshot,
            apply_acceptance_action
        ])
        .run(tauri::generate_context!())
        .expect("error while running the ChatGPT × Codex Middleware application");
}
