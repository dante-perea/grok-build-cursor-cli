//! Agent chat transcript (center pane).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: ChatRole,
    pub content: String,
    /// True while assistant stream is still open.
    pub streaming: bool,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: ChatRole::User,
            content: content.into(),
            streaming: false,
        }
    }

    pub fn assistant_stream_start() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: ChatRole::Assistant,
            content: String::new(),
            streaming: true,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: ChatRole::System,
            content: content.into(),
            streaming: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTranscript {
    pub messages: Vec<ChatMessage>,
    /// Id of the open streaming assistant message, if any.
    pub streaming_id: Option<String>,
}

impl ChatTranscript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_user(&mut self, content: impl Into<String>) -> &ChatMessage {
        self.messages.push(ChatMessage::user(content));
        self.messages.last().expect("just pushed")
    }

    pub fn begin_assistant_stream(&mut self) -> &ChatMessage {
        let msg = ChatMessage::assistant_stream_start();
        self.streaming_id = Some(msg.id.clone());
        self.messages.push(msg);
        self.messages.last().expect("just pushed")
    }

    /// Append a streaming chunk to the open assistant message (or start one).
    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if let Some(id) = self.streaming_id.clone() {
            if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                msg.content.push_str(chunk);
                msg.streaming = true;
                return;
            }
        }
        self.begin_assistant_stream();
        self.append_assistant_chunk(chunk);
    }

    pub fn finish_assistant_stream(&mut self) {
        if let Some(id) = self.streaming_id.take() {
            if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                msg.streaming = false;
            }
        }
    }

    pub fn push_system(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage::system(content));
    }
}
