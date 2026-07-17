//! Cursor Agents Home layout snapshot (pure — no terminal I/O).
//!
//! Default product surface is Agent Home (sidebar + floating composer),
//! not a 3-column file-explorer IDE TUI.

use serde::{Deserialize, Serialize};

/// View mode for the Cursor-clone product surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentsView {
    #[default]
    Home,
    Session,
}

/// Structural dump of the Agents Home chrome for CI / `--dump-layout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeLayoutSnapshot {
    pub product: String,
    pub view: AgentsView,
    pub regions: Vec<String>,
    pub not_regions: Vec<String>,
    /// Primary surface must not be a file tree (Cursor Agents home).
    pub show_file_tree_primary: bool,
}

impl Default for HomeLayoutSnapshot {
    fn default() -> Self {
        Self::agents_home()
    }
}

impl HomeLayoutSnapshot {
    /// Canonical Cursor Agents Home structure.
    pub fn agents_home() -> Self {
        Self {
            product: "cursor-agents-home".into(),
            view: AgentsView::Home,
            regions: vec![
                "sidebar_new_agent".into(),
                "sidebar_history".into(),
                "floating_composer".into(),
                "plan_chip".into(),
                "model_chip".into(),
                "project_context".into(),
            ],
            not_regions: vec![
                "file_tree_primary".into(),
                "three_column_ide".into(),
            ],
            show_file_tree_primary: false,
        }
    }

    pub fn with_view(mut self, view: AgentsView) -> Self {
        self.view = view;
        if view == AgentsView::Session {
            if !self.regions.iter().any(|r| r == "session_transcript") {
                self.regions.push("session_transcript".into());
            }
            if !self.regions.iter().any(|r| r == "diff_review") {
                self.regions.push("diff_review".into());
            }
            if !self.regions.iter().any(|r| r == "activity_feed") {
                self.regions.push("activity_feed".into());
            }
        }
        self
    }

    /// True when this is Cursor Agents Home chrome (not multipane IDE).
    pub fn is_cursor_agents_home(&self) -> bool {
        self.product == "cursor-agents-home"
            && !self.show_file_tree_primary
            && self.regions.iter().any(|r| r == "floating_composer")
            && self.regions.iter().any(|r| r == "sidebar_new_agent")
            && self
                .not_regions
                .iter()
                .any(|r| r == "file_tree_primary")
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_home_snapshot_is_cursor_agents_home() {
        let snap = HomeLayoutSnapshot::default();
        assert!(snap.is_cursor_agents_home(), "{snap:?}");
        assert_eq!(snap.product, "cursor-agents-home");
        assert!(!snap.show_file_tree_primary);
        assert!(snap.regions.contains(&"floating_composer".into()));
        assert!(snap.regions.contains(&"sidebar_new_agent".into()));
        assert!(snap.regions.contains(&"plan_chip".into()));
        assert!(snap.not_regions.contains(&"file_tree_primary".into()));
        assert!(snap.not_regions.contains(&"three_column_ide".into()));
    }

    #[test]
    fn session_view_adds_transcript_regions() {
        let snap = HomeLayoutSnapshot::agents_home().with_view(AgentsView::Session);
        assert_eq!(snap.view, AgentsView::Session);
        assert!(snap.regions.contains(&"session_transcript".into()));
        assert!(snap.regions.contains(&"diff_review".into()));
    }
}
