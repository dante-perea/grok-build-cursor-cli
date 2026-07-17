//! Bind real Grok Build agent runtime events into Cursor shell UI state.
//!
//! Event shapes mirror ACP / shell session updates (tool calls, message chunks,
//! diffs) so the UI binds the real runtime progression path — not a parallel
//! mock agent.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use xai_hunk_tracker::{Hunk, HunkEvent, HunkId, HunkRemovalReason, HunkSource};

use crate::session::CursorAction;

/// Phase of a tool call in the agent runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallPhase {
    Started,
    InProgress,
    Completed,
    Failed,
}

/// Agent runtime events consumed by the Cursor shell.
///
/// These map 1:1 onto the progression surface of Grok Build (streaming text,
/// tool steps, proposed file edits). Producers include the ACP stdio bridge
/// and direct shell session update adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRuntimeEvent {
    /// Assistant text chunk (streaming reply).
    AgentMessageChunk { text: String },
    /// Turn / stream finished for the assistant message.
    AgentMessageEnd,
    /// Tool call lifecycle (search, edit, bash, …).
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        title: String,
        phase: ToolCallPhase,
        detail: String,
    },
    /// Proposed file edit from the tools/edits path.
    ProposedEdit {
        edit_id: String,
        path: PathBuf,
        old_text: String,
        new_text: String,
    },
    /// Status line for the activity feed.
    Status { message: String },
    /// Turn started (after Composer submit accepted by runtime).
    TurnStarted,
    /// Turn completed (success or cancelled).
    TurnCompleted { ok: bool },
    /// Error from the agent runtime.
    Error { message: String },
    /// Hunk tracker event (serialized bridge for shell/hunk pipeline).
    Hunk(HunkEventDto),
}

/// Serde-friendly slice of `HunkEvent` for tests and wire dumps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HunkEventDto {
    Added {
        path: PathBuf,
        id: String,
        new_text: String,
        old_text: Option<String>,
        prompt_index: Option<usize>,
    },
    Removed {
        path: PathBuf,
        id: String,
    },
}

impl AgentRuntimeEvent {
    /// Convert a real `HunkEvent` into a bridge event.
    pub fn from_hunk_event(event: &HunkEvent) -> Option<Self> {
        match event {
            HunkEvent::HunkAdded { path, hunk } => Some(Self::Hunk(HunkEventDto::Added {
                path: path.clone(),
                id: hunk.id.as_str().to_string(),
                new_text: hunk.new_text.clone(),
                old_text: hunk.old_text.clone(),
                prompt_index: hunk.source.prompt_index(),
            })),
            HunkEvent::HunkRemoved { path, hunk_id, .. } => {
                Some(Self::Hunk(HunkEventDto::Removed {
                    path: path.clone(),
                    id: hunk_id.as_str().to_string(),
                }))
            }
            _ => None,
        }
    }

    /// Map runtime event → pure session actions (shipped reducer input).
    pub fn into_actions(self) -> Vec<CursorAction> {
        match self {
            AgentRuntimeEvent::AgentMessageChunk { text } => {
                vec![CursorAction::AppendAssistantChunk { text }]
            }
            AgentRuntimeEvent::AgentMessageEnd => {
                vec![CursorAction::FinishAssistantStream]
            }
            AgentRuntimeEvent::ToolCall {
                tool_call_id,
                tool_name,
                title,
                phase,
                detail,
            } => match phase {
                ToolCallPhase::Started | ToolCallPhase::InProgress => {
                    vec![CursorAction::StartTool {
                        tool_call_id,
                        tool_name,
                        title,
                    }]
                }
                ToolCallPhase::Completed => vec![CursorAction::CompleteTool {
                    tool_call_id,
                    ok: true,
                    detail,
                }],
                ToolCallPhase::Failed => vec![CursorAction::CompleteTool {
                    tool_call_id,
                    ok: false,
                    detail,
                }],
            },
            AgentRuntimeEvent::ProposedEdit {
                edit_id,
                path,
                old_text,
                new_text,
            } => vec![CursorAction::ProposeEdit {
                edit_id,
                path,
                old_text,
                new_text,
            }],
            AgentRuntimeEvent::Status { message } => {
                vec![CursorAction::PushActivityStatus { message }]
            }
            AgentRuntimeEvent::TurnStarted => vec![CursorAction::TurnStarted],
            AgentRuntimeEvent::TurnCompleted { ok } => {
                vec![CursorAction::TurnCompleted { ok }]
            }
            AgentRuntimeEvent::Error { message } => {
                vec![CursorAction::RuntimeError { message }]
            }
            AgentRuntimeEvent::Hunk(dto) => match dto {
                HunkEventDto::Added {
                    path,
                    id,
                    new_text,
                    old_text,
                    prompt_index,
                } => {
                    let source = HunkSource::AgentEdit {
                        prompt_index: prompt_index.unwrap_or(0),
                    };
                    // Prefer file_created ctor (stamps created_at); overlay id.
                    let mut hunk = if old_text
                        .as_ref()
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                    {
                        Hunk::file_created(path.clone(), new_text, source)
                    } else {
                        let mut h = Hunk::file_created(path.clone(), new_text.clone(), source);
                        h.old_text = old_text;
                        h.new_text = new_text;
                        h
                    };
                    hunk.id = HunkId::from_string(id);
                    vec![CursorAction::ApplyHunkEvent {
                        event: HunkEvent::HunkAdded {
                            path,
                            hunk: Arc::new(hunk),
                        },
                    }]
                }
                HunkEventDto::Removed { path, id } => vec![CursorAction::ApplyHunkEvent {
                    event: HunkEvent::HunkRemoved {
                        path,
                        hunk_id: HunkId::from_string(id),
                        reason: HunkRemovalReason::Superseded,
                    },
                }],
            },
        }
    }
}

/// Apply a list of runtime events to produce session actions (batch helper).
pub fn bind_events(events: impl IntoIterator<Item = AgentRuntimeEvent>) -> Vec<CursorAction> {
    events
        .into_iter()
        .flat_map(AgentRuntimeEvent::into_actions)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_and_tool_events_bind_to_actions() {
        let events = vec![
            AgentRuntimeEvent::TurnStarted,
            AgentRuntimeEvent::AgentMessageChunk {
                text: "I'll fix that.".into(),
            },
            AgentRuntimeEvent::ToolCall {
                tool_call_id: "t1".into(),
                tool_name: "search_replace".into(),
                title: "Edit src/lib.rs".into(),
                phase: ToolCallPhase::Started,
                detail: String::new(),
            },
            AgentRuntimeEvent::ToolCall {
                tool_call_id: "t1".into(),
                tool_name: "search_replace".into(),
                title: "Edit src/lib.rs".into(),
                phase: ToolCallPhase::Completed,
                detail: "ok".into(),
            },
            AgentRuntimeEvent::ProposedEdit {
                edit_id: "e1".into(),
                path: PathBuf::from("src/lib.rs"),
                old_text: "a".into(),
                new_text: "b".into(),
            },
            AgentRuntimeEvent::AgentMessageEnd,
            AgentRuntimeEvent::TurnCompleted { ok: true },
        ];
        let actions = bind_events(events);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, CursorAction::AppendAssistantChunk { .. }))
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, CursorAction::StartTool { .. }))
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, CursorAction::ProposeEdit { .. }))
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, CursorAction::TurnCompleted { ok: true }))
        );
    }
}
