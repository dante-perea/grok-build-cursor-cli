//! Multi-pane Cursor-like layout state (pure, no terminal I/O).

use serde::{Deserialize, Serialize};

/// Which surface holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusPane {
    /// File explorer / open buffers (left).
    Workspace,
    /// Agent chat transcript (center).
    Chat,
    /// Composer prompt input (center bottom).
    Composer,
    /// Tool / status activity feed (right or bottom-right).
    Activity,
    /// Diff / change review list.
    DiffReview,
}

impl FocusPane {
    /// Stable display label for chrome / tests.
    pub fn label(self) -> &'static str {
        match self {
            FocusPane::Workspace => "Workspace",
            FocusPane::Chat => "Agent Chat",
            FocusPane::Composer => "Composer",
            FocusPane::Activity => "Activity",
            FocusPane::DiffReview => "Diff Review",
        }
    }

    /// Cycle focus in Cursor-like order (Workspace → Chat → Composer → Activity → Diff → …).
    pub fn next(self) -> Self {
        match self {
            FocusPane::Workspace => FocusPane::Chat,
            FocusPane::Chat => FocusPane::Composer,
            FocusPane::Composer => FocusPane::Activity,
            FocusPane::Activity => FocusPane::DiffReview,
            FocusPane::DiffReview => FocusPane::Workspace,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            FocusPane::Workspace => FocusPane::DiffReview,
            FocusPane::Chat => FocusPane::Workspace,
            FocusPane::Composer => FocusPane::Chat,
            FocusPane::Activity => FocusPane::Composer,
            FocusPane::DiffReview => FocusPane::Activity,
        }
    }
}

/// Relative column widths as percent of terminal width (must sum ≤ 100).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnSplits {
    /// Left workspace column.
    pub workspace_pct: u16,
    /// Center chat + composer column.
    pub chat_pct: u16,
    /// Right activity + diff column.
    pub side_pct: u16,
}

impl Default for ColumnSplits {
    fn default() -> Self {
        Self {
            workspace_pct: 22,
            chat_pct: 48,
            side_pct: 30,
        }
    }
}

/// Pure multi-pane layout for the Cursor shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorLayout {
    pub focus: FocusPane,
    pub splits: ColumnSplits,
    /// Whether the workspace column is visible.
    pub show_workspace: bool,
    /// Whether the activity / diff side column is visible.
    pub show_side: bool,
    /// Whether the dedicated diff-review subpane is expanded.
    pub show_diff_review: bool,
}

impl Default for CursorLayout {
    fn default() -> Self {
        Self {
            focus: FocusPane::Composer,
            splits: ColumnSplits::default(),
            show_workspace: true,
            show_side: true,
            show_diff_review: true,
        }
    }
}

impl CursorLayout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focus_pane(&mut self, pane: FocusPane) {
        self.focus = pane;
    }

    pub fn cycle_focus_forward(&mut self) {
        self.focus = self.focus.next();
    }

    pub fn cycle_focus_backward(&mut self) {
        self.focus = self.focus.prev();
    }

    pub fn toggle_workspace(&mut self) {
        self.show_workspace = !self.show_workspace;
        if !self.show_workspace && self.focus == FocusPane::Workspace {
            self.focus = FocusPane::Composer;
        }
    }

    pub fn toggle_side(&mut self) {
        self.show_side = !self.show_side;
        if !self.show_side && matches!(self.focus, FocusPane::Activity | FocusPane::DiffReview) {
            self.focus = FocusPane::Composer;
        }
    }

    /// Structural snapshot used by tests and headless layout dumps.
    pub fn snapshot(&self) -> LayoutSnapshot {
        let mut regions = Vec::new();
        if self.show_workspace {
            regions.push(FocusPane::Workspace);
        }
        regions.push(FocusPane::Chat);
        regions.push(FocusPane::Composer);
        if self.show_side {
            regions.push(FocusPane::Activity);
            if self.show_diff_review {
                regions.push(FocusPane::DiffReview);
            }
        }
        LayoutSnapshot {
            focus: self.focus,
            regions,
            show_workspace: self.show_workspace,
            show_side: self.show_side,
            show_diff_review: self.show_diff_review,
            splits: self.splits.clone(),
        }
    }
}

/// Observable multi-pane structure (no pixels — for verification / dumps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    pub focus: FocusPane,
    pub regions: Vec<FocusPane>,
    pub show_workspace: bool,
    pub show_side: bool,
    pub show_diff_review: bool,
    pub splits: ColumnSplits,
}

impl LayoutSnapshot {
    /// True when the shell is multi-pane Cursor-like (not single-column only).
    pub fn is_multi_pane(&self) -> bool {
        self.regions.len() >= 3
            && self.regions.contains(&FocusPane::Composer)
            && (self.regions.contains(&FocusPane::Workspace)
                || self.regions.contains(&FocusPane::Activity))
    }

    /// Human-readable layout dump for evidence files.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str("grok-build-cursor-cli layout\n");
        out.push_str(&format!("focus: {}\n", self.focus.label()));
        out.push_str(&format!(
            "workspace: {} | side: {} | diff_review: {}\n",
            self.show_workspace, self.show_side, self.show_diff_review
        ));
        out.push_str(&format!(
            "splits: workspace={} chat={} side={}\n",
            self.splits.workspace_pct, self.splits.chat_pct, self.splits.side_pct
        ));
        out.push_str("regions:\n");
        for r in &self.regions {
            let mark = if *r == self.focus { "*" } else { " " };
            out.push_str(&format!("  {mark} {}\n", r.label()));
        }
        out.push_str(&format!("multi_pane: {}\n", self.is_multi_pane()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_multi_pane_cursor_shell() {
        let snap = CursorLayout::default().snapshot();
        assert!(snap.is_multi_pane());
        assert!(snap.regions.contains(&FocusPane::Workspace));
        assert!(snap.regions.contains(&FocusPane::Composer));
        assert!(snap.regions.contains(&FocusPane::Activity));
        assert!(snap.regions.contains(&FocusPane::DiffReview));
        assert!(snap.regions.contains(&FocusPane::Chat));
    }

    #[test]
    fn focus_cycle_visits_all_panes() {
        let mut layout = CursorLayout::default();
        let start = layout.focus;
        let mut seen = vec![start];
        for _ in 0..4 {
            layout.cycle_focus_forward();
            seen.push(layout.focus);
        }
        layout.cycle_focus_forward();
        assert_eq!(layout.focus, start);
        assert_eq!(seen.len(), 5);
    }
}
