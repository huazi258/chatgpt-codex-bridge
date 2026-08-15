use chrono::Utc;
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

struct AppState {
    connection: Mutex<Connection>,
    chatgpt_bridge: Arc<ChatGptBridge>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Budget {
    max_rounds: i64,
    module_timeout_minutes: i64,
    global_timeout_minutes: i64,
}

#[derive(Debug, Serialize)]
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
                        let valid_session = bridge.session.lock().ok().and_then(|session| session.clone()).is_some_and(|session| Some(session.session_id.as_str()) == session_id);
                        if !valid_session || reply.is_none() {
                            bridge.set_status(&app, ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: "收到未配对或不完整的 ChatGPT 回复。".into(),
                                tab_id: None,
                                protocol_state: None,
                            });
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
                            }
                            Err(error) => bridge.set_status(&app, ChatGptBridgeStatus {
                                phase: "BLOCKED".into(),
                                detail: format!("Protocol validation failed: {error}"),
                                tab_id: None,
                                protocol_state: None,
                            }),
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
    if text.trim().is_empty() {
        return Err("a ChatGPT message is required".into());
    }
    let session = state
        .chatgpt_bridge
        .session
        .lock()
        .map_err(|_| "ChatGPT bridge lock poisoned".to_string())?
        .clone()
        .ok_or_else(|| "no paired ChatGPT extension is connected".to_string())?;
    state.chatgpt_bridge.send_to_extension(json!({
        "type": "sendChatGptMessage",
        "sessionId": session.session_id,
        "text": text.trim()
    }))?;
    state.chatgpt_bridge.set_status(
        &app,
        ChatGptBridgeStatus {
            phase: "SENT".into(),
            detail: "已将协议消息发送到绑定的 ChatGPT 标签页。".into(),
            tab_id: Some(session.tab_id),
            protocol_state: None,
        },
    );
    Ok(())
}

fn codex_command() -> String {
    std::env::var("CODEX_APP_SERVER_COMMAND").unwrap_or_else(|_| "codex".to_string())
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
        let envelope = validate_protocol_payload("ChatGPT rendered a code block without fences.", Some(json))
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
        .setup(|app| {
            let connection = create_connection(app.handle())?;
            let chatgpt_bridge = Arc::new(ChatGptBridge::new());
            start_chatgpt_bridge(app.handle().clone(), chatgpt_bridge.clone())?;
            app.manage(AppState {
                connection: Mutex::new(connection),
                chatgpt_bridge,
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
            send_chatgpt_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running the ChatGPT × Codex Middleware application");
}
