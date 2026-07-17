//! Cursor session — pure multi-pane state machine + Composer submit path.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use xai_hunk_tracker::{HunkAction, HunkEvent};

use crate::activity::ActivityFeed;
use crate::chat::ChatTranscript;
use crate::composer::{ComposerOutcome, ComposerState};
use crate::diff_review::{ChangeItem, DiffReviewState};
use crate::layout::{CursorLayout, FocusPane, LayoutSnapshot};
use crate::workspace::WorkspacePane;

/// Side effects requested by the session reducer (executed by the app / driver).
#[derive(Debug, Clone)]
pub enum SessionEffect {
    /// Submit prompt to the real Grok Build agent runtime.
    SubmitToAgent { prompt: String },
    /// Apply accept/reject through the hunk tracker pipeline.
    ApplyHunkAction { hunk_id: String, action: HunkAction },
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
    Quit,
}

/// Full Cursor-like multi-pane session state.
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
}

impl CursorSession {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let root = workspace_root.into();
        let mut workspace = WorkspacePane::new(root.clone());
        let _ = workspace.refresh_listing();
        Self {
            layout: CursorLayout::default(),
            workspace,
            chat: ChatTranscript::new(),
            composer: ComposerState::new(),
            activity: ActivityFeed::new(),
            diffs: DiffReviewState::new(),
            status_line: format!("grok-build-cursor-cli · {}", root.display()),
            agent_busy: false,
        }
    }

    pub fn layout_snapshot(&self) -> LayoutSnapshot {
        self.layout.snapshot()
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
            CursorAction::ComposerSubmit => {
                match self.composer.submit() {
                    ComposerOutcome::Submit { prompt } => {
                        self.chat.push_user(&prompt);
                        self.activity
                            .push_status(format!("Submitting to Grok Build agent…"));
                        self.chat.begin_assistant_stream();
                        self.agent_busy = true;
                        self.composer.set_turn_in_flight(true);
                        self.status_line = "Agent turn in progress…".into();
                        effects.push(SessionEffect::SubmitToAgent { prompt });
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
    use std::env;

    #[test]
    fn composer_submit_emits_submit_to_agent_effect() {
        let mut session = CursorSession::new(env::temp_dir());
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
        assert_eq!(session.chat.messages[0].content, "refactor auth");
        assert!(session.agent_busy);
        assert!(session.layout_snapshot().is_multi_pane());
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
