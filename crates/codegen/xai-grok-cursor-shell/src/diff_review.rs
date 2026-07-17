//! Diff / change-review model — Cursor-like inspect + accept/reject.
//!
//! Maps proposed agent edits (from `xai-hunk-tracker` events and edit payloads)
//! into a reviewable list with accept/reject decisions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use xai_hunk_tracker::{Hunk, HunkAction, HunkEvent, HunkId, HunkSource};

/// User decision on a proposed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDecision {
    Pending,
    Accepted,
    Rejected,
}

/// One reviewable change (file edit / hunk) in the Diff Review pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeItem {
    pub id: String,
    pub path: PathBuf,
    pub summary: String,
    pub old_text: Option<String>,
    pub new_text: String,
    pub patch: Option<String>,
    pub decision: ChangeDecision,
    /// 1-based start line in the new file when known.
    pub line_start: Option<usize>,
}

impl ChangeItem {
    /// Build a review item from a real Grok Build `Hunk`.
    pub fn from_hunk(hunk: &Hunk) -> Self {
        Self {
            id: hunk.id.as_str().to_string(),
            path: hunk.path.clone(),
            summary: hunk.summary(),
            old_text: hunk.old_text.clone(),
            new_text: hunk.new_text.clone(),
            patch: hunk.patch.clone(),
            decision: ChangeDecision::Pending,
            line_start: Some(hunk.line_info.new_start).filter(|&n| n > 0),
        }
    }

    /// Build from a raw edit payload (search_replace style).
    pub fn from_edit(
        id: impl Into<String>,
        path: impl Into<PathBuf>,
        old_text: impl Into<String>,
        new_text: impl Into<String>,
    ) -> Self {
        let old_text = old_text.into();
        let new_text = new_text.into();
        let old_lines = if old_text.is_empty() {
            0
        } else {
            old_text.lines().count()
        };
        let new_lines = if new_text.is_empty() {
            0
        } else {
            new_text.lines().count()
        };
        Self {
            id: id.into(),
            path: path.into(),
            summary: format!("+{new_lines}/-{old_lines}"),
            old_text: if old_text.is_empty() {
                None
            } else {
                Some(old_text)
            },
            new_text,
            patch: None,
            decision: ChangeDecision::Pending,
            line_start: None,
        }
    }

    /// Unified-diff style preview for inspect.
    pub fn inspect_preview(&self) -> String {
        if let Some(patch) = &self.patch {
            return patch.clone();
        }
        let mut out = format!("--- a/{}\n+++ b/{}\n", self.path.display(), self.path.display());
        if let Some(old) = &self.old_text {
            for line in old.lines() {
                out.push_str(&format!("-{line}\n"));
            }
        }
        for line in self.new_text.lines() {
            out.push_str(&format!("+{line}\n"));
        }
        out
    }
}

/// Diff review pane state (list of proposed agent changes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffReviewState {
    pub items: Vec<ChangeItem>,
    pub selected: usize,
}

impl DiffReviewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.decision == ChangeDecision::Pending)
            .count()
    }

    pub fn upsert(&mut self, item: ChangeItem) {
        if let Some(existing) = self.items.iter_mut().find(|i| i.id == item.id) {
            *existing = item;
        } else {
            self.items.push(item);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|i| i.id != id);
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
    }

    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.items.len() - 1);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn selected_item(&self) -> Option<&ChangeItem> {
        self.items.get(self.selected)
    }

    /// Apply accept/reject to the selected item. Returns the hunk action for
    /// the real hunk-tracker apply path when the id is a real `HunkId`.
    pub fn decide_selected(&mut self, accept: bool) -> Option<(String, HunkAction)> {
        let item = self.items.get_mut(self.selected)?;
        if item.decision != ChangeDecision::Pending {
            return None;
        }
        item.decision = if accept {
            ChangeDecision::Accepted
        } else {
            ChangeDecision::Rejected
        };
        let action = if accept {
            HunkAction::Accept
        } else {
            HunkAction::Reject
        };
        Some((item.id.clone(), action))
    }

    pub fn decide_by_id(&mut self, id: &str, accept: bool) -> Option<HunkAction> {
        let item = self.items.iter_mut().find(|i| i.id == id)?;
        if item.decision != ChangeDecision::Pending {
            return None;
        }
        item.decision = if accept {
            ChangeDecision::Accepted
        } else {
            ChangeDecision::Rejected
        };
        Some(if accept {
            HunkAction::Accept
        } else {
            HunkAction::Reject
        })
    }

    /// Bind a real `HunkEvent` from the Grok Build hunk tracker into review UI.
    pub fn apply_hunk_event(&mut self, event: &HunkEvent) {
        match event {
            HunkEvent::HunkAdded { hunk, .. } | HunkEvent::HunkContentChanged { hunk, .. } => {
                // Only agent-related edits surface in the primary review list.
                if hunk.source.is_agent_tracked() || matches!(hunk.source, HunkSource::External) {
                    self.upsert(ChangeItem::from_hunk(hunk));
                }
            }
            HunkEvent::HunkRemoved { hunk_id, .. } => {
                self.remove(hunk_id.as_str());
            }
            HunkEvent::HunkMoved {
                hunk_id,
                new_line_info,
                path,
            } => {
                if let Some(item) = self.items.iter_mut().find(|i| i.id == hunk_id.as_str()) {
                    item.path = path.clone();
                    item.line_start = Some(new_line_info.new_start).filter(|&n| n > 0);
                }
            }
            HunkEvent::FileAdded { .. }
            | HunkEvent::FileRemoved { .. }
            | HunkEvent::BaselineUpdated { .. } => {}
        }
    }

    /// Convenience for tests: record an agent write as a file-created hunk style item.
    pub fn record_agent_file_write(
        &mut self,
        path: impl Into<PathBuf>,
        content: impl Into<String>,
        prompt_index: usize,
    ) -> String {
        let path = path.into();
        let content = content.into();
        let hunk = Hunk::file_created(
            path,
            content,
            HunkSource::AgentEdit { prompt_index },
        );
        let id = hunk.id.as_str().to_string();
        self.upsert(ChangeItem::from_hunk(&hunk));
        id
    }
}

/// Parse a hunk id string into the tracker type (for apply path).
pub fn hunk_id_from_str(id: &str) -> HunkId {
    HunkId::from_string(id.to_string())
}

/// Re-export Arc for callers binding events.
pub type SharedHunk = Arc<Hunk>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_edit_and_accept_reject() {
        let mut review = DiffReviewState::new();
        review.upsert(ChangeItem::from_edit(
            "c1",
            "src/main.rs",
            "fn old()",
            "fn new()",
        ));
        assert_eq!(review.pending_count(), 1);
        let (id, action) = review.decide_selected(true).expect("decide");
        assert_eq!(id, "c1");
        assert_eq!(action, HunkAction::Accept);
        assert_eq!(review.items[0].decision, ChangeDecision::Accepted);
        assert_eq!(review.pending_count(), 0);
    }

    #[test]
    fn inspect_preview_shows_diff_markers() {
        let item = ChangeItem::from_edit("x", "a.rs", "old", "new");
        let preview = item.inspect_preview();
        assert!(preview.contains("-old"));
        assert!(preview.contains("+new"));
    }

    #[test]
    fn hunk_event_bind_adds_agent_hunk() {
        let mut review = DiffReviewState::new();
        let hunk = Hunk::file_created(
            PathBuf::from("foo.rs"),
            "pub fn x() {}".into(),
            HunkSource::AgentEdit { prompt_index: 0 },
        );
        let event = HunkEvent::HunkAdded {
            path: PathBuf::from("foo.rs"),
            hunk: Arc::new(hunk),
        };
        review.apply_hunk_event(&event);
        assert_eq!(review.items.len(), 1);
        assert_eq!(review.items[0].path, PathBuf::from("foo.rs"));
    }
}
