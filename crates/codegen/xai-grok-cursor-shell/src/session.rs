//! Cursor session — state machine for Agents Home + active session.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use xai_hunk_tracker::{HunkAction, HunkEvent};

use crate::activity::ActivityFeed;
use crate::chat::ChatTranscript;
use crate::composer::{ComposerOutcome, ComposerState};
use crate::diff_review::{ChangeItem, DiffReviewState};
use crate::layout::{CursorLayout, FocusPane, LayoutSnapshot};
use crate::layout_home::{AgentsView, HomeLayoutSnapshot};
use crate::workspace::WorkspacePane;

/// Composer interaction mode (Grok Build Shift+Tab cycle: Normal / Plan / Always-approve).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Normal agent — tools + edits allowed.
    #[default]
    Agent,
    /// Plan mode — planning first (prompt prefixed).
    Plan,
    /// Always-approve style (auto-approve tools when agent supports it).
    AlwaysApprove,
}

impl AgentMode {
    pub fn label(self) -> &'static str {
        match self {
            AgentMode::Agent => "Agent",
            AgentMode::Plan => "Plan",
            AgentMode::AlwaysApprove => "Always",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            AgentMode::Agent => AgentMode::Plan,
            AgentMode::Plan => AgentMode::AlwaysApprove,
            AgentMode::AlwaysApprove => AgentMode::Agent,
        }
    }

    pub fn is_plan(self) -> bool {
        matches!(self, AgentMode::Plan)
    }
}

/// Side effects requested by the session reducer (executed by the app / driver).
#[derive(Debug, Clone)]
pub enum SessionEffect {
    /// Submit prompt to the real Grok Build agent runtime.
    SubmitToAgent { prompt: String },
    /// Apply accept/reject through the hunk tracker pipeline.
    ApplyHunkAction { hunk_id: String, action: HunkAction },
    /// Record a sidebar history entry (first prompt of a turn).
    RecordHistory { title: String },
    /// Quit the interactive shell.
    Quit,
    /// Redraw needed (always implicit; listed for clarity in tests).
    Redraw,
}

/// User / runtime actions reduced by [`CursorSession`].
#[derive(Debug, Clone)]
pub enum CursorAction {
    Focus(FocusPane),
    CycleFocusForward,
    CycleFocusBackward,
    ToggleWorkspace,
    ToggleSide,
    ComposerInsertChar(char),
    ComposerInsertStr(String),
    ComposerBackspace,
    /// Primary Composer submit — drives the real agent when an effect runner is wired.
    ComposerSubmit,
    AppendAssistantChunk { text: String },
    FinishAssistantStream,
    StartTool {
        tool_call_id: String,
        tool_name: String,
        title: String,
    },
    CompleteTool {
        tool_call_id: String,
        ok: bool,
        detail: String,
    },
    ProposeEdit {
        edit_id: String,
        path: PathBuf,
        old_text: String,
        new_text: String,
    },
    PushActivityStatus { message: String },
    TurnStarted,
    TurnCompleted { ok: bool },
    RuntimeError { message: String },
    ApplyHunkEvent { event: HunkEvent },
    AcceptSelectedChange,
    RejectSelectedChange,
    DiffSelectNext,
    DiffSelectPrev,
    WorkspaceSelectNext,
    WorkspaceSelectPrev,
    WorkspaceOpenSelected,
    /// Toggle Plan mode chip (Cursor Agents home).
    SetPlanMode(bool),
    TogglePlanMode,
    /// Set / cycle Agent | Plan | Always modes.
    SetAgentMode(AgentMode),
    CycleAgentMode,
    SetModelLabel(String),
    /// Attach a file path for the next prompt.
    AttachFile { path: PathBuf },
    ClearAttachments,
    RemoveAttachment { path: PathBuf },
    /// Switch workspace root (project picker).
    SetWorkspace(PathBuf),
    /// Return to Agents Home empty canvas.
    NewAgent,
    ClearTranscript,
    Quit,
}

/// Full session state (Agents Home + active agent turn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorSession {
    pub layout: CursorLayout,
    pub workspace: WorkspacePane,
    pub chat: ChatTranscript,
    pub composer: ComposerState,
    pub activity: ActivityFeed,
    pub diffs: DiffReviewState,
    pub status_line: String,
    pub agent_busy: bool,
    /// Agents Home vs active session transcript.
    pub view: AgentsView,
    /// Plan mode chip (prefixes prompt when submitting). Kept in sync with `agent_mode`.
    pub plan_mode: bool,
    /// Agent | Plan | Always (Shift+Tab style).
    pub agent_mode: AgentMode,
    /// Display model label (static for v1).
    pub model_label: String,
    /// Files attached for the next submit.
    pub attachments: Vec<PathBuf>,
}

impl CursorSession {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let root = workspace_root.into();
        let mut workspace = WorkspacePane::new(root.clone());
        let _ = workspace.refresh_listing();
        let mut composer = ComposerState::new();
        composer.placeholder = "Plan and design before coding…".into();
        Self {
            layout: CursorLayout::default(),
            workspace,
            chat: ChatTranscript::new(),
            composer,
            activity: ActivityFeed::new(),
            diffs: DiffReviewState::new(),
            status_line: format!("grok-build-cursor-cli · {}", root.display()),
            agent_busy: false,
            view: AgentsView::Home,
            plan_mode: false,
            agent_mode: AgentMode::Agent,
            model_label: "Grok".into(),
            attachments: Vec::new(),
        }
    }

    pub fn layout_snapshot(&self) -> LayoutSnapshot {
        self.layout.snapshot()
    }

    /// Cursor Agents Home dump (default product surface).
    pub fn home_layout_snapshot(&self) -> HomeLayoutSnapshot {
        HomeLayoutSnapshot::agents_home().with_view(self.view)
    }

    /// Reduce one action; returns effects for the app loop / agent driver.
    pub fn reduce(&mut self, action: CursorAction) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        match action {
            CursorAction::Focus(pane) => self.layout.focus_pane(pane),
            CursorAction::CycleFocusForward => self.layout.cycle_focus_forward(),
            CursorAction::CycleFocusBackward => self.layout.cycle_focus_backward(),
            CursorAction::ToggleWorkspace => self.layout.toggle_workspace(),
            CursorAction::ToggleSide => self.layout.toggle_side(),
            CursorAction::ComposerInsertChar(c) => {
                let _ = self.composer.insert_char(c);
            }
            CursorAction::ComposerInsertStr(s) => {
                let _ = self.composer.insert_str(&s);
            }
            CursorAction::ComposerBackspace => {
                let _ = self.composer.backspace();
            }
            CursorAction::SetPlanMode(v) => {
                self.plan_mode = v;
                self.agent_mode = if v {
                    AgentMode::Plan
                } else if self.agent_mode.is_plan() {
                    AgentMode::Agent
                } else {
                    self.agent_mode
                };
            }
            CursorAction::TogglePlanMode => {
                if self.agent_mode.is_plan() {
                    self.agent_mode = AgentMode::Agent;
                    self.plan_mode = false;
                } else {
                    self.agent_mode = AgentMode::Plan;
                    self.plan_mode = true;
                }
            }
            CursorAction::SetAgentMode(m) => {
                self.agent_mode = m;
                self.plan_mode = m.is_plan();
            }
            CursorAction::CycleAgentMode => {
                self.agent_mode = self.agent_mode.cycle();
                self.plan_mode = self.agent_mode.is_plan();
            }
            CursorAction::SetModelLabel(s) => {
                if !s.trim().is_empty() {
                    self.model_label = s.trim().to_string();
                }
            }
            CursorAction::AttachFile { path } => {
                if !self.attachments.iter().any(|p| p == &path) {
                    self.attachments.push(path);
                }
            }
            CursorAction::ClearAttachments => {
                self.attachments.clear();
            }
            CursorAction::RemoveAttachment { path } => {
                self.attachments.retain(|p| p != &path);
            }
            CursorAction::SetWorkspace(root) => {
                let mut workspace = WorkspacePane::new(root.clone());
                let _ = workspace.refresh_listing();
                self.workspace = workspace;
                self.status_line = format!("Project · {}", root.display());
            }
            CursorAction::ClearTranscript => {
                self.chat = ChatTranscript::new();
                self.activity = ActivityFeed::new();
            }
            CursorAction::NewAgent => {
                self.view = AgentsView::Home;
                self.chat = ChatTranscript::new();
                self.activity = ActivityFeed::new();
                self.diffs = DiffReviewState::new();
                self.attachments.clear();
                self.agent_busy = false;
                self.composer.set_turn_in_flight(false);
                let _ = self.composer.clear();
                self.status_line = "New Agent".into();
            }
            CursorAction::ComposerSubmit => {
                match self.composer.submit() {
                    ComposerOutcome::Submit { prompt } => {
                        let title = prompt.clone();
                        let mut agent_prompt = match self.agent_mode {
                            AgentMode::Plan => format!("[plan mode] {prompt}"),
                            AgentMode::AlwaysApprove => {
                                format!("[always-approve] {prompt}")
                            }
                            AgentMode::Agent => prompt.clone(),
                        };
                        if !self.attachments.is_empty() {
                            let list = self
                                .attachments
                                .iter()
                                .map(|p| format!("- {}", p.display()))
                                .collect::<Vec<_>>()
                                .join("\n");
                            agent_prompt = format!(
                                "{agent_prompt}\n\nAttached files:\n{list}"
                            );
                        }
                        let display = if self.attachments.is_empty() {
                            prompt.clone()
                        } else {
                            format!(
                                "{prompt}\n\n(attached {} file{})",
                                self.attachments.len(),
                                if self.attachments.len() == 1 { "" } else { "s" }
                            )
                        };
                        self.view = AgentsView::Session;
                        self.chat.push_user(&display);
                        self.activity
                            .push_status("Submitting to Grok Build agent…");
                        self.chat.begin_assistant_stream();
                        self.agent_busy = true;
                        self.composer.set_turn_in_flight(true);
                        self.status_line = "Agent turn in progress…".into();
                        self.attachments.clear();
                        effects.push(SessionEffect::RecordHistory { title });
                        effects.push(SessionEffect::SubmitToAgent {
                            prompt: agent_prompt,
                        });
                    }
                    _ => {}
                }
            }
            CursorAction::AppendAssistantChunk { text } => {
                self.chat.append_assistant_chunk(&text);
            }
            CursorAction::FinishAssistantStream => {
                self.chat.finish_assistant_stream();
            }
            CursorAction::StartTool {
                tool_call_id,
                tool_name,
                title,
            } => {
                self.activity
                    .start_tool(tool_call_id, tool_name, title);
            }
            CursorAction::CompleteTool {
                tool_call_id,
                ok,
                detail,
            } => {
                self.activity.complete_tool(&tool_call_id, ok, detail);
            }
            CursorAction::ProposeEdit {
                edit_id,
                path,
                old_text,
                new_text,
            } => {
                self.diffs.upsert(ChangeItem::from_edit(
                    edit_id, path, old_text, new_text,
                ));
                if !self.layout.show_diff_review {
                    self.layout.show_diff_review = true;
                }
            }
            CursorAction::PushActivityStatus { message } => {
                self.activity.push_status(message);
            }
            CursorAction::TurnStarted => {
                self.agent_busy = true;
                self.composer.set_turn_in_flight(true);
                self.status_line = "Agent turn in progress…".into();
            }
            CursorAction::TurnCompleted { ok } => {
                self.agent_busy = false;
                self.composer.set_turn_in_flight(false);
                self.chat.finish_assistant_stream();
                self.status_line = if ok {
                    "Ready".into()
                } else {
                    "Turn failed".into()
                };
            }
            CursorAction::RuntimeError { message } => {
                self.activity.push_status(format!("error: {message}"));
                self.chat.push_system(format!("Error: {message}"));
                self.agent_busy = false;
                self.composer.set_turn_in_flight(false);
                self.status_line = "Error".into();
            }
            CursorAction::ApplyHunkEvent { event } => {
                self.diffs.apply_hunk_event(&event);
            }
            CursorAction::AcceptSelectedChange => {
                if let Some((id, action)) = self.diffs.decide_selected(true) {
                    effects.push(SessionEffect::ApplyHunkAction {
                        hunk_id: id,
                        action,
                    });
                }
            }
            CursorAction::RejectSelectedChange => {
                if let Some((id, action)) = self.diffs.decide_selected(false) {
                    effects.push(SessionEffect::ApplyHunkAction {
                        hunk_id: id,
                        action,
                    });
                }
            }
            CursorAction::DiffSelectNext => self.diffs.select_next(),
            CursorAction::DiffSelectPrev => self.diffs.select_prev(),
            CursorAction::WorkspaceSelectNext => self.workspace.select_next(),
            CursorAction::WorkspaceSelectPrev => self.workspace.select_prev(),
            CursorAction::WorkspaceOpenSelected => {
                let _ = self.workspace.open_selected();
            }
            CursorAction::Quit => effects.push(SessionEffect::Quit),
        }
        effects.push(SessionEffect::Redraw);
        effects
    }

    /// Apply many actions (e.g. from `agent_bridge::bind_events`).
    pub fn reduce_all(&mut self, actions: impl IntoIterator<Item = CursorAction>) -> Vec<SessionEffect> {
        let mut all = Vec::new();
        for a in actions {
            all.extend(self.reduce(a));
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_bridge::{AgentRuntimeEvent, ToolCallPhase, bind_events};
    use crate::layout_home::AgentsView;
    use crate::session::AgentMode;
    use std::env;

    #[test]
    fn composer_submit_emits_submit_to_agent_effect() {
        let mut session = CursorSession::new(env::temp_dir());
        assert_eq!(session.view, AgentsView::Home);
        session.plan_mode = false;
        session
            .reduce(CursorAction::ComposerInsertStr("refactor auth".into()));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                SessionEffect::SubmitToAgent { prompt } if prompt == "refactor auth"
            )),
            "effects: {effects:?}"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, SessionEffect::RecordHistory { title } if title == "refactor auth"))
        );
        assert_eq!(session.chat.messages[0].content, "refactor auth");
        assert!(session.agent_busy);
        assert_eq!(session.view, AgentsView::Session);
        assert!(session.home_layout_snapshot().is_cursor_agents_home());
    }

    #[test]
    fn plan_mode_prefixes_agent_prompt() {
        let mut session = CursorSession::new(env::temp_dir());
        session.reduce(CursorAction::SetAgentMode(AgentMode::Plan));
        session.reduce(CursorAction::ComposerInsertStr("design auth".into()));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                SessionEffect::SubmitToAgent { prompt } if prompt.starts_with("[plan mode]")
            )),
            "{effects:?}"
        );
    }

    #[test]
    fn cycle_modes_agent_plan_always() {
        let mut session = CursorSession::new(env::temp_dir());
        assert_eq!(session.agent_mode, AgentMode::Agent);
        session.reduce(CursorAction::CycleAgentMode);
        assert_eq!(session.agent_mode, AgentMode::Plan);
        assert!(session.plan_mode);
        session.reduce(CursorAction::CycleAgentMode);
        assert_eq!(session.agent_mode, AgentMode::AlwaysApprove);
        session.reduce(CursorAction::CycleAgentMode);
        assert_eq!(session.agent_mode, AgentMode::Agent);
    }

    #[test]
    fn attachments_included_on_submit() {
        let mut session = CursorSession::new(env::temp_dir());
        session.plan_mode = false;
        session.agent_mode = AgentMode::Agent;
        session.reduce(CursorAction::AttachFile {
            path: PathBuf::from("/tmp/a.rs"),
        });
        session.reduce(CursorAction::ComposerInsertStr("review this".into()));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                SessionEffect::SubmitToAgent { prompt } if prompt.contains("Attached files:") && prompt.contains("a.rs")
            )),
            "{effects:?}"
        );
        assert!(session.attachments.is_empty());
    }

    #[test]
    fn new_agent_returns_home() {
        let mut session = CursorSession::new(env::temp_dir());
        session.view = AgentsView::Session;
        session.reduce(CursorAction::NewAgent);
        assert_eq!(session.view, AgentsView::Home);
        assert!(session.chat.messages.is_empty());
    }

    #[test]
    fn runtime_events_drive_activity_chat_and_diffs() {
        let mut session = CursorSession::new(env::temp_dir());
        session.reduce(CursorAction::ComposerInsertStr("edit file".into()));
        let _ = session.reduce(CursorAction::ComposerSubmit);

        let actions = bind_events(vec![
            AgentRuntimeEvent::TurnStarted,
            AgentRuntimeEvent::AgentMessageChunk {
                text: "Applying edit…".into(),
            },
            AgentRuntimeEvent::ToolCall {
                tool_call_id: "tool-1".into(),
                tool_name: "search_replace".into(),
                title: "Edit foo.rs".into(),
                phase: ToolCallPhase::Started,
                detail: String::new(),
            },
            AgentRuntimeEvent::ProposedEdit {
                edit_id: "edit-1".into(),
                path: PathBuf::from("foo.rs"),
                old_text: "a".into(),
                new_text: "b".into(),
            },
            AgentRuntimeEvent::ToolCall {
                tool_call_id: "tool-1".into(),
                tool_name: "search_replace".into(),
                title: "Edit foo.rs".into(),
                phase: ToolCallPhase::Completed,
                detail: "applied".into(),
            },
            AgentRuntimeEvent::AgentMessageEnd,
            AgentRuntimeEvent::TurnCompleted { ok: true },
        ]);
        session.reduce_all(actions);

        assert!(
            session
                .chat
                .messages
                .iter()
                .any(|m| m.content.contains("Applying edit"))
        );
        assert!(session.activity.entries.iter().any(|e| e.id == "tool-1"));
        assert_eq!(session.diffs.pending_count(), 1);
        assert!(!session.agent_busy);
    }
}
