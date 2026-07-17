//! Cursor Agents Home shell for Grok Build.
//!
//! Default product surface is a local web UI matching Cursor Agents home
//! (sidebar + floating Composer). Agent runtime is Grok Build via ACP stdio.
//! Legacy multipane TUI is available via `--tui`.

pub mod activity;
pub mod agent_bridge;
pub mod agent_driver;
pub mod app;
pub mod chat;
pub mod composer;
pub mod diff_review;
pub mod history;
pub mod layout;
pub mod layout_home;
pub mod render;
pub mod server;
pub mod session;
pub mod workspace;

pub use activity::{ActivityEntry, ActivityKind, ActivityStatus};
pub use agent_bridge::{AgentRuntimeEvent, ToolCallPhase};
pub use agent_driver::{
    AgentDriver, AgentPromptRequest, AgentPromptResult, RealGrokAgentDriver, apply_change_to_disk,
    map_agent_line, map_agent_line_all,
};
pub use app::{AppOptions, run_headless_dump, run_headless_dump_blocking};
pub use chat::{ChatMessage, ChatRole, ChatTranscript};
pub use composer::{ComposerOutcome, ComposerState};
pub use diff_review::{ChangeDecision, ChangeItem, DiffReviewState};
pub use history::{AgentHistoryStore, SessionMeta};
pub use layout::{CursorLayout, FocusPane, LayoutSnapshot};
pub use layout_home::{AgentsView, HomeLayoutSnapshot};
pub use session::{CursorAction, CursorSession, SessionEffect};
pub use workspace::{WorkspaceFile, WorkspacePane};
