//! Local Axum control plane + static Cursor Agents Home UI.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use tower_http::services::ServeDir;
use xai_hunk_tracker::HunkAction;

use crate::agent_bridge::{AgentRuntimeEvent, bind_events};
use crate::agent_driver::{
    AgentPromptRequest, RealGrokAgentDriver, apply_change_to_disk, simulate_representative_turn,
};
use crate::diff_review::ChangeDecision;
use crate::history::{AgentHistoryStore, SessionMeta};
use crate::layout_home::{AgentsView, HomeLayoutSnapshot};
use crate::projects::{
    ProjectEntry, default_project_roots, git_branch, list_projects, resolve_project,
};
use crate::session::{AgentMode, CursorAction, CursorSession, SessionEffect};
use crate::slash::{
    SlashCommandInfo, SlashExecCtx, builtin_slash_commands, exec_local_slash, filter_slash_commands,
    parse_slash_line,
};

#[derive(Clone)]
pub struct ServerOptions {
    pub cwd: PathBuf,
    pub ui_dir: PathBuf,
    pub agent_bin: Option<PathBuf>,
    pub allow_simulated_runtime: bool,
    pub agent_timeout_secs: u64,
    pub history_path: PathBuf,
    pub project_roots: Vec<PathBuf>,
}

pub struct AppStateInner {
    pub session: CursorSession,
    pub history: AgentHistoryStore,
    pub opts: ServerOptions,
    pub tx: broadcast::Sender<String>,
}

pub type SharedState = Arc<Mutex<AppStateInner>>;

#[derive(Debug, Deserialize)]
pub struct PromptBody {
    pub prompt: String,
    #[serde(default)]
    pub plan_mode: Option<bool>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub attachments: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ModeBody {
    pub mode: String,
}

#[derive(Debug, Deserialize)]
pub struct ProjectBody {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct AttachBody {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SlashQuery {
    #[serde(default)]
    pub q: String,
}

#[derive(Debug, Serialize)]
pub struct UiSnapshot {
    #[serde(rename = "type")]
    pub kind: String,
    pub view: String,
    pub layout: HomeLayoutSnapshot,
    pub history: Vec<SessionMeta>,
    pub chat: Vec<ChatDto>,
    pub activity: Vec<ActivityDto>,
    pub diffs: Vec<DiffDto>,
    pub status: String,
    pub plan_mode: bool,
    pub agent_mode: String,
    pub model_label: String,
    pub workspace: String,
    pub branch: Option<String>,
    pub agent_busy: bool,
    pub attachments: Vec<String>,
    pub projects: Vec<ProjectEntry>,
    pub slash_commands: Vec<SlashCommandInfo>,
}

#[derive(Debug, Serialize)]
pub struct ChatDto {
    pub role: String,
    pub content: String,
    pub streaming: bool,
}

#[derive(Debug, Serialize)]
pub struct ActivityDto {
    pub id: String,
    pub title: String,
    pub status: String,
    pub tool_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffDto {
    pub id: String,
    pub path: String,
    pub summary: String,
    pub decision: String,
    pub inspect_preview: String,
}

pub fn build_router(state: SharedState, ui_dir: PathBuf) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/snapshot", get(api_snapshot))
        .route("/api/prompt", post(api_prompt))
        .route("/api/new_agent", post(api_new_agent))
        .route("/api/mode", post(api_mode))
        .route("/api/mode/cycle", post(api_mode_cycle))
        .route("/api/projects", get(api_projects))
        .route("/api/project", post(api_set_project))
        .route("/api/attach", post(api_attach))
        .route("/api/attach/clear", post(api_attach_clear))
        .route("/api/slash", get(api_slash))
        .route("/api/diff/{id}/accept", post(api_diff_accept))
        .route("/api/diff/{id}/reject", post(api_diff_reject))
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(ui_dir).append_index_html_on_directories(true))
        .with_state(state)
}

pub async fn run_server(opts: ServerOptions, addr: SocketAddr) -> Result<()> {
    let session = CursorSession::new(opts.cwd.clone());
    let history = AgentHistoryStore::new(opts.history_path.clone());
    let (tx, _) = broadcast::channel(256);
    let state = Arc::new(Mutex::new(AppStateInner {
        session,
        history,
        opts: opts.clone(),
        tx,
    }));
    let app = build_router(state, opts.ui_dir.clone());
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "product": "cursor-agents-home" }))
}

async fn api_snapshot(State(state): State<SharedState>) -> impl IntoResponse {
    let g = state.lock().await;
    Json(build_snapshot(&g))
}

async fn api_prompt(
    State(state): State<SharedState>,
    Json(body): Json<PromptBody>,
) -> impl IntoResponse {
    let snap = handle_submit(
        state,
        body.prompt,
        body.plan_mode,
        body.mode,
        body.attachments.unwrap_or_default(),
    )
    .await;
    Json(snap)
}

async fn api_new_agent(State(state): State<SharedState>) -> impl IntoResponse {
    let mut g = state.lock().await;
    g.session.reduce(CursorAction::NewAgent);
    broadcast_snap(&g)
}

async fn api_mode(
    State(state): State<SharedState>,
    Json(body): Json<ModeBody>,
) -> impl IntoResponse {
    let mode = parse_mode(&body.mode).unwrap_or(AgentMode::Agent);
    let mut g = state.lock().await;
    g.session.reduce(CursorAction::SetAgentMode(mode));
    broadcast_snap(&g)
}

async fn api_mode_cycle(State(state): State<SharedState>) -> impl IntoResponse {
    let mut g = state.lock().await;
    g.session.reduce(CursorAction::CycleAgentMode);
    broadcast_snap(&g)
}

async fn api_projects(State(state): State<SharedState>) -> impl IntoResponse {
    let g = state.lock().await;
    let projects = list_projects(&g.opts.project_roots, 200);
    Json(serde_json::json!({ "projects": projects }))
}

async fn api_set_project(
    State(state): State<SharedState>,
    Json(body): Json<ProjectBody>,
) -> impl IntoResponse {
    let mut g = state.lock().await;
    let roots = g.opts.project_roots.clone();
    if let Some(path) = resolve_project(&body.path, &roots) {
        g.opts.cwd = path.clone();
        g.session.reduce(CursorAction::SetWorkspace(path));
    } else {
        g.session.chat.push_system(format!(
            "Project not found: {}. Try /projects or pick from the list.",
            body.path
        ));
        g.session.view = AgentsView::Session;
    }
    broadcast_snap(&g)
}

async fn api_attach(
    State(state): State<SharedState>,
    Json(body): Json<AttachBody>,
) -> impl IntoResponse {
    let mut g = state.lock().await;
    let path = PathBuf::from(body.path.trim());
    if path.exists() {
        g.session
            .reduce(CursorAction::AttachFile { path: path.clone() });
    } else {
        // Still attach absolute path under workspace if relative
        let under = g.session.workspace.root.join(&path);
        if under.exists() {
            g.session
                .reduce(CursorAction::AttachFile { path: under });
        } else {
            g.session
                .chat
                .push_system(format!("File not found: {}", path.display()));
            g.session.view = AgentsView::Session;
        }
    }
    broadcast_snap(&g)
}

async fn api_attach_clear(State(state): State<SharedState>) -> impl IntoResponse {
    let mut g = state.lock().await;
    g.session.reduce(CursorAction::ClearAttachments);
    broadcast_snap(&g)
}

async fn api_slash(
    Query(q): Query<SlashQuery>,
) -> impl IntoResponse {
    let cmds = filter_slash_commands(&q.q);
    Json(serde_json::json!({ "commands": cmds }))
}

async fn api_diff_accept(
    State(state): State<SharedState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    Json(handle_diff(state, &id, true).await)
}

async fn api_diff_reject(
    State(state): State<SharedState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    Json(handle_diff(state, &id, false).await)
}

fn broadcast_snap(g: &AppStateInner) -> Json<UiSnapshot> {
    let snap = build_snapshot(g);
    let _ = g
        .tx
        .send(serde_json::to_string(&snap).unwrap_or_default());
    Json(snap)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<SharedState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: SharedState) {
    {
        let g = state.lock().await;
        let snap = build_snapshot(&g);
        if let Ok(s) = serde_json::to_string(&snap) {
            let _ = socket.send(Message::Text(s.into())).await;
        }
    }
    let mut rx = {
        let g = state.lock().await;
        g.tx.subscribe()
    };

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                            let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                            match ty {
                                "submit" => {
                                    let prompt = v.get("prompt").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    let plan = v.get("plan_mode").and_then(|x| x.as_bool());
                                    let mode = v.get("mode").and_then(|x| x.as_str()).map(|s| s.to_string());
                                    let attachments = v.get("attachments")
                                        .and_then(|a| a.as_array())
                                        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                                        .unwrap_or_default();
                                    let snap = handle_submit(state.clone(), prompt, plan, mode, attachments).await;
                                    if let Ok(s) = serde_json::to_string(&snap) {
                                        let _ = socket.send(Message::Text(s.into())).await;
                                    }
                                }
                                "new_agent" => {
                                    let mut g = state.lock().await;
                                    g.session.reduce(CursorAction::NewAgent);
                                    let snap = build_snapshot(&g);
                                    let s = serde_json::to_string(&snap).unwrap_or_default();
                                    let _ = g.tx.send(s.clone());
                                    let _ = socket.send(Message::Text(s.into())).await;
                                }
                                "cycle_mode" => {
                                    let mut g = state.lock().await;
                                    g.session.reduce(CursorAction::CycleAgentMode);
                                    let snap = build_snapshot(&g);
                                    let s = serde_json::to_string(&snap).unwrap_or_default();
                                    let _ = g.tx.send(s.clone());
                                    let _ = socket.send(Message::Text(s.into())).await;
                                }
                                "set_mode" => {
                                    let mode = v.get("mode").and_then(|x| x.as_str()).unwrap_or("agent");
                                    let mut g = state.lock().await;
                                    if let Some(m) = parse_mode(mode) {
                                        g.session.reduce(CursorAction::SetAgentMode(m));
                                    }
                                    let snap = build_snapshot(&g);
                                    let s = serde_json::to_string(&snap).unwrap_or_default();
                                    let _ = g.tx.send(s.clone());
                                    let _ = socket.send(Message::Text(s.into())).await;
                                }
                                "set_project" => {
                                    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    let mut g = state.lock().await;
                                    let roots = g.opts.project_roots.clone();
                                    if let Some(p) = resolve_project(&path, &roots) {
                                        g.opts.cwd = p.clone();
                                        g.session.reduce(CursorAction::SetWorkspace(p));
                                    }
                                    let snap = build_snapshot(&g);
                                    let s = serde_json::to_string(&snap).unwrap_or_default();
                                    let _ = g.tx.send(s.clone());
                                    let _ = socket.send(Message::Text(s.into())).await;
                                }
                                "attach" => {
                                    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
                                    let mut g = state.lock().await;
                                    let p = PathBuf::from(path);
                                    let path = if p.exists() {
                                        p
                                    } else {
                                        g.session.workspace.root.join(path)
                                    };
                                    if path.exists() {
                                        g.session.reduce(CursorAction::AttachFile { path });
                                    }
                                    let snap = build_snapshot(&g);
                                    let s = serde_json::to_string(&snap).unwrap_or_default();
                                    let _ = g.tx.send(s.clone());
                                    let _ = socket.send(Message::Text(s.into())).await;
                                }
                                "accept_diff" => {
                                    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
                                    let snap = handle_diff(state.clone(), id, true).await;
                                    if let Ok(s) = serde_json::to_string(&snap) {
                                        let _ = socket.send(Message::Text(s.into())).await;
                                    }
                                }
                                "reject_diff" => {
                                    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
                                    let snap = handle_diff(state.clone(), id, false).await;
                                    if let Ok(s) = serde_json::to_string(&snap) {
                                        let _ = socket.send(Message::Text(s.into())).await;
                                    }
                                }
                                "hello" => {
                                    let g = state.lock().await;
                                    let snap = build_snapshot(&g);
                                    if let Ok(s) = serde_json::to_string(&snap) {
                                        let _ = socket.send(Message::Text(s.into())).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            Ok(msg) = rx.recv() => {
                let _ = socket.send(Message::Text(msg.into())).await;
            }
        }
    }
}

async fn handle_submit(
    state: SharedState,
    prompt: String,
    plan_mode: Option<bool>,
    mode: Option<String>,
    attachments: Vec<String>,
) -> UiSnapshot {
    // Slash command path (local handlers). Non-local slash continues as agent prompt.
    let mut agent_prompt = prompt;
    if let Some((name, args)) = parse_slash_line(&agent_prompt) {
        if !name.is_empty() {
            if let Some(snap) = try_handle_local_slash(state.clone(), &name, &args).await {
                return snap;
            }
            // Keep original text for agent (e.g. /strings query)
            agent_prompt = format!("/{name} {args}").trim().to_string();
        }
    }

    let (effects, cwd, opts) = {
        let mut g = state.lock().await;
        if let Some(m) = mode.as_deref().and_then(parse_mode) {
            g.session.reduce(CursorAction::SetAgentMode(m));
        } else if let Some(pm) = plan_mode {
            g.session.reduce(CursorAction::SetPlanMode(pm));
        }
        for a in attachments {
            let p = PathBuf::from(&a);
            let path = if p.exists() {
                p
            } else {
                g.session.workspace.root.join(&a)
            };
            if path.exists() {
                g.session.reduce(CursorAction::AttachFile { path });
            }
        }
        g.session
            .reduce(CursorAction::ComposerInsertStr(agent_prompt));
        let effects = g.session.reduce(CursorAction::ComposerSubmit);
        (effects, g.session.workspace.root.clone(), g.opts.clone())
    };

    for effect in &effects {
        match effect {
            SessionEffect::RecordHistory { title } => {
                let mut g = state.lock().await;
                let meta = SessionMeta::new(title, &cwd);
                let _ = g.history.add(meta);
            }
            SessionEffect::SubmitToAgent { prompt } => {
                let events = drive_agent(&opts, &cwd, prompt).await;
                let mut g = state.lock().await;
                g.session.reduce_all(bind_events(events));
            }
            SessionEffect::ApplyHunkAction { hunk_id, action } => {
                let mut g = state.lock().await;
                apply_hunk(&mut g.session, hunk_id, *action);
            }
            _ => {}
        }
    }

    let g = state.lock().await;
    let snap = build_snapshot(&g);
    let _ = g
        .tx
        .send(serde_json::to_string(&snap).unwrap_or_default());
    snap
}

/// Returns Some(snapshot) if the slash was handled locally; None to forward to agent.
async fn try_handle_local_slash(
    state: SharedState,
    name: &str,
    args: &str,
) -> Option<UiSnapshot> {
    let result = {
        let g = state.lock().await;
        let ws = g.session.workspace.root.display().to_string();
        let mode = g.session.agent_mode.label().to_string();
        let model = g.session.model_label.clone();
        let ctx = SlashExecCtx {
            workspace: &ws,
            mode: &mode,
            model: &model,
            plan_mode: g.session.plan_mode,
            busy: g.session.agent_busy,
        };
        exec_local_slash(name, args, &ctx)
    };

    if !result.handled {
        return None;
    }

    let mut g = state.lock().await;
    g.session.view = AgentsView::Session;
    if let Some(msg) = &result.message {
        if result.action.as_deref() != Some("list_projects") {
            g.session.chat.push_system(msg.clone());
        }
    }
    match result.action.as_deref() {
        Some("toggle_plan") => {
            g.session.reduce(CursorAction::TogglePlanMode);
        }
        Some("new_agent") => {
            g.session.reduce(CursorAction::NewAgent);
        }
        Some("clear") => {
            g.session.reduce(CursorAction::ClearTranscript);
        }
        Some("set_model") => {
            if let Some(arg) = &result.action_arg {
                g.session
                    .reduce(CursorAction::SetModelLabel(arg.clone()));
            }
        }
        Some("set_project") => {
            if let Some(arg) = &result.action_arg {
                let roots = g.opts.project_roots.clone();
                if let Some(p) = resolve_project(arg, &roots) {
                    let display = p.display().to_string();
                    g.opts.cwd = p.clone();
                    g.session.reduce(CursorAction::SetWorkspace(p));
                    g.session
                        .chat
                        .push_system(format!("Switched project to {display}"));
                } else {
                    g.session
                        .chat
                        .push_system(format!("Project not found: {arg}"));
                }
            }
        }
        Some("list_projects") => {
            let projects = list_projects(&g.opts.project_roots, 100);
            let lines = if projects.is_empty() {
                "No projects found under ~/projects (and common roots).".into()
            } else {
                projects
                    .iter()
                    .map(|p| {
                        format!(
                            "· {} {}{}",
                            p.name,
                            p.path.display(),
                            if p.is_git { " (git)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            g.session
                .chat
                .push_system(format!("Projects:\n{lines}\n\nUse /cd <name> or the project picker."));
        }
        _ => {}
    }
    let snap = build_snapshot(&g);
    let _ = g
        .tx
        .send(serde_json::to_string(&snap).unwrap_or_default());
    Some(snap)
}

async fn handle_diff(state: SharedState, id: &str, accept: bool) -> UiSnapshot {
    let mut g = state.lock().await;
    if let Some(idx) = g.session.diffs.items.iter().position(|i| i.id == id) {
        g.session.diffs.selected = idx;
    }
    let effects = if accept {
        g.session.reduce(CursorAction::AcceptSelectedChange)
    } else {
        g.session.reduce(CursorAction::RejectSelectedChange)
    };
    for effect in effects {
        if let SessionEffect::ApplyHunkAction { hunk_id, action } = effect {
            apply_hunk(&mut g.session, &hunk_id, action);
        }
    }
    let snap = build_snapshot(&g);
    let _ = g
        .tx
        .send(serde_json::to_string(&snap).unwrap_or_default());
    snap
}

fn apply_hunk(session: &mut CursorSession, hunk_id: &str, action: HunkAction) {
    let accept = matches!(action, HunkAction::Accept);
    let item = session
        .diffs
        .items
        .iter()
        .find(|i| i.id == hunk_id)
        .cloned();
    if let Some(item) = item {
        let path = if item.path.is_absolute() {
            item.path.clone()
        } else {
            session.workspace.root.join(&item.path)
        };
        let _ = apply_change_to_disk(
            &path,
            item.old_text.as_deref(),
            &item.new_text,
            accept,
        );
        session.activity.push_status(format!(
            "{} {}",
            if accept { "accepted" } else { "rejected" },
            path.display()
        ));
    }
}

async fn drive_agent(
    opts: &ServerOptions,
    cwd: &Path,
    prompt: &str,
) -> Vec<AgentRuntimeEvent> {
    let mut driver = RealGrokAgentDriver::new(cwd)
        .with_read_timeout(Duration::from_secs(opts.agent_timeout_secs));
    if let Some(bin) = &opts.agent_bin {
        driver = driver.with_bin(bin.clone());
    }
    // Always-approve: pass via env the agent may honor
    if prompt.contains("[always-approve]") {
        // note: actual agent flags would be on spawn; best-effort via prompt prefix
    }
    if !opts.allow_simulated_runtime {
        if driver.resolve_bin().is_err() {
            return vec![
                AgentRuntimeEvent::Error {
                    message: "require-agent: binary not found".into(),
                },
                AgentRuntimeEvent::TurnCompleted { ok: false },
            ];
        }
    }
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let req = AgentPromptRequest {
        prompt: prompt.to_string(),
        cwd: cwd.to_path_buf(),
    };
    match driver.submit_prompt(req, tx).await {
        Ok(r) => r.events,
        Err(err) => {
            if opts.allow_simulated_runtime {
                let mut events = vec![AgentRuntimeEvent::Status {
                    message: format!("Agent spawn failed ({err}); representative fallback"),
                }];
                events.extend(simulate_representative_turn(prompt));
                events
            } else {
                vec![
                    AgentRuntimeEvent::Error {
                        message: format!("require-agent: {err}"),
                    },
                    AgentRuntimeEvent::TurnCompleted { ok: false },
                ]
            }
        }
    }
}

fn parse_mode(s: &str) -> Option<AgentMode> {
    match s.to_lowercase().as_str() {
        "agent" | "normal" | "build" => Some(AgentMode::Agent),
        "plan" => Some(AgentMode::Plan),
        "always" | "always_approve" | "always-approve" | "yolo" => {
            Some(AgentMode::AlwaysApprove)
        }
        _ => None,
    }
}

pub fn build_snapshot(g: &AppStateInner) -> UiSnapshot {
    let view = match g.session.view {
        AgentsView::Home => "home",
        AgentsView::Session => "session",
    };
    let history = g.history.list().unwrap_or_default();
    let chat = g
        .session
        .chat
        .messages
        .iter()
        .map(|m| ChatDto {
            role: match m.role {
                crate::chat::ChatRole::User => "user",
                crate::chat::ChatRole::Assistant => "assistant",
                crate::chat::ChatRole::System => "system",
            }
            .into(),
            content: m.content.clone(),
            streaming: m.streaming,
        })
        .collect();
    let activity = g
        .session
        .activity
        .entries
        .iter()
        .map(|e| ActivityDto {
            id: e.id.clone(),
            title: e.title.clone(),
            status: format!("{:?}", e.status).to_lowercase(),
            tool_name: e.tool_name.clone(),
        })
        .collect();
    let diffs = g
        .session
        .diffs
        .items
        .iter()
        .map(|d| DiffDto {
            id: d.id.clone(),
            path: d.path.display().to_string(),
            summary: d.summary.clone(),
            decision: match d.decision {
                ChangeDecision::Pending => "pending",
                ChangeDecision::Accepted => "accepted",
                ChangeDecision::Rejected => "rejected",
            }
            .into(),
            inspect_preview: d.inspect_preview(),
        })
        .collect();

    let projects = list_projects(&g.opts.project_roots, 200);
    let branch = git_branch(&g.session.workspace.root);

    UiSnapshot {
        kind: "snapshot".into(),
        view: view.into(),
        layout: g.session.home_layout_snapshot(),
        history,
        chat,
        activity,
        diffs,
        status: g.session.status_line.clone(),
        plan_mode: g.session.plan_mode,
        agent_mode: g.session.agent_mode.label().to_lowercase(),
        model_label: g.session.model_label.clone(),
        workspace: g.session.workspace.root.display().to_string(),
        branch,
        agent_busy: g.session.agent_busy,
        attachments: g
            .session
            .attachments
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        projects,
        slash_commands: builtin_slash_commands(),
    }
}

/// Resolve UI static directory (crate `ui/` next to source, or CARGO_MANIFEST_DIR).
pub fn default_ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

pub fn default_server_opts(cwd: PathBuf, ui_dir: PathBuf, history_path: PathBuf) -> ServerOptions {
    ServerOptions {
        cwd,
        ui_dir,
        agent_bin: None,
        allow_simulated_runtime: true,
        agent_timeout_secs: 90,
        history_path,
        project_roots: default_project_roots(),
    }
}
