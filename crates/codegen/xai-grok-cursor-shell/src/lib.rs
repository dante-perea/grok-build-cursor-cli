//! Cursor-like multi-pane shell for Grok Build.
//!
//! Pure layout / session state is isolated from terminal I/O so unit tests can
//! exercise the shipped reducers, Composer submit path, activity binding, and
//! diff-review model without a full TUI frame loop. The interactive binary
//! (`grok-build-cursor-cli`) renders the same state with ratatui and drives
//! the real Grok Build agent runtime via ACP stdio.

pub mod activity;
pub mod agent_bridge;
pub mod agent_driver;
pub mod app;
pub mod chat;
pub mod composer;
pub mod diff_review;
pub mod layout;
pub mod render;
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
pub use layout::{CursorLayout, FocusPane, LayoutSnapshot};
pub use session::{CursorAction, CursorSession, SessionEffect};
pub use workspace::{WorkspaceFile, WorkspacePane};
