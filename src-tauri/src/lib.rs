mod orchestration;

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
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

const INITIAL_SCHEMA: &str = include_str!("../migrations/001_initial.sql");
const ORCHESTRATION_SCHEMA: &str = include_str!("../migrations/002_orchestration_runtime.sql");
const EXECUTION_CONTROL_SCHEMA: &str = include_str!("../migrations/003_execution_control.sql");

struct AppState {
    connection: Mutex<Connection>,
    chatgpt_bridge: Arc<ChatGptBridge>,
    orchestrator: Mutex<Option<ActiveOrchestration>>,
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
    Ok(connection)
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
            let chatgpt_bridge = Arc::new(ChatGptBridge::new());
            start_chatgpt_bridge(app.handle().clone(), chatgpt_bridge.clone())?;
            app.manage(AppState {
                connection: Mutex::new(connection),
                chatgpt_bridge,
                orchestrator: Mutex::new(None),
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
            start_module_orchestration,
            get_orchestration_snapshot,
            apply_acceptance_action
        ])
        .run(tauri::generate_context!())
        .expect("error while running the ChatGPT × Codex Middleware application");
}
