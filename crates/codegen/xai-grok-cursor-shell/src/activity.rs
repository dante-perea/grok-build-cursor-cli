//! Agent activity feed — tool steps / status (Cursor activity surface).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Status,
    ToolCall,
    ToolResult,
    Thinking,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub id: String,
    pub kind: ActivityKind,
    pub title: String,
    pub detail: String,
    pub status: ActivityStatus,
    /// Optional tool name when kind is ToolCall / ToolResult.
    pub tool_name: Option<String>,
}

impl ActivityEntry {
    pub fn status(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            kind: ActivityKind::Status,
            title: title.into(),
            detail: String::new(),
            status: ActivityStatus::Running,
            tool_name: None,
        }
    }

    pub fn tool_call(tool_name: impl Into<String>, title: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        Self {
            id: Uuid::new_v4().to_string(),
            kind: ActivityKind::ToolCall,
            title: title.into(),
            detail: String::new(),
            status: ActivityStatus::Running,
            tool_name: Some(tool_name),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityFeed {
    pub entries: Vec<ActivityEntry>,
}

impl ActivityFeed {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: ActivityEntry) {
        self.entries.push(entry);
    }

    pub fn push_status(&mut self, title: impl Into<String>) -> String {
        let e = ActivityEntry::status(title);
        let id = e.id.clone();
        self.entries.push(e);
        id
    }

    pub fn start_tool(
        &mut self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        title: impl Into<String>,
    ) {
        let id = tool_call_id.into();
        let tool_name = tool_name.into();
        // Update existing row with same id if present.
        if let Some(existing) = self.entries.iter_mut().find(|e| e.id == id) {
            existing.kind = ActivityKind::ToolCall;
            existing.title = title.into();
            existing.status = ActivityStatus::Running;
            existing.tool_name = Some(tool_name);
            return;
        }
        self.entries.push(ActivityEntry {
            id,
            kind: ActivityKind::ToolCall,
            title: title.into(),
            detail: String::new(),
            status: ActivityStatus::Running,
            tool_name: Some(tool_name),
        });
    }

    pub fn complete_tool(&mut self, tool_call_id: &str, ok: bool, detail: impl Into<String>) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == tool_call_id) {
            e.status = if ok {
                ActivityStatus::Completed
            } else {
                ActivityStatus::Failed
            };
            e.kind = ActivityKind::ToolResult;
            e.detail = detail.into();
        }
    }

    pub fn running_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == ActivityStatus::Running)
            .count()
    }
}
