//! Composer (agent prompt input) — Cursor-style primary chat box.

use serde::{Deserialize, Serialize};

/// Outcome of handling a Composer key / action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerOutcome {
    /// No state change.
    Unchanged,
    /// Draft text changed.
    Changed,
    /// User submitted a non-empty prompt for the agent runtime.
    Submit { prompt: String },
    /// Clear draft without submit.
    Cleared,
}

/// Primary Composer surface state (Cursor Chat / Agent prompt).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerState {
    pub draft: String,
    /// Placeholder shown when draft is empty.
    pub placeholder: String,
    /// True while an agent turn is in flight (blocks double-submit by default).
    pub turn_in_flight: bool,
    /// Allow force-queue while a turn is running (Cursor multi-prompt).
    pub allow_queue_while_busy: bool,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            draft: String::new(),
            placeholder: "Plan, search, build anything…".to_string(),
            turn_in_flight: false,
            allow_queue_while_busy: true,
        }
    }
}

impl ComposerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_char(&mut self, c: char) -> ComposerOutcome {
        self.draft.push(c);
        ComposerOutcome::Changed
    }

    pub fn insert_str(&mut self, s: &str) -> ComposerOutcome {
        if s.is_empty() {
            return ComposerOutcome::Unchanged;
        }
        self.draft.push_str(s);
        ComposerOutcome::Changed
    }

    pub fn backspace(&mut self) -> ComposerOutcome {
        if self.draft.pop().is_some() {
            ComposerOutcome::Changed
        } else {
            ComposerOutcome::Unchanged
        }
    }

    pub fn set_draft(&mut self, text: impl Into<String>) -> ComposerOutcome {
        let text = text.into();
        if self.draft == text {
            return ComposerOutcome::Unchanged;
        }
        self.draft = text;
        ComposerOutcome::Changed
    }

    pub fn clear(&mut self) -> ComposerOutcome {
        if self.draft.is_empty() {
            return ComposerOutcome::Unchanged;
        }
        self.draft.clear();
        ComposerOutcome::Cleared
    }

    /// Submit path: trims draft, returns prompt for the real agent runtime.
    /// Empty / whitespace-only drafts do not submit.
    pub fn submit(&mut self) -> ComposerOutcome {
        let prompt = self.draft.trim().to_string();
        if prompt.is_empty() {
            return ComposerOutcome::Unchanged;
        }
        if self.turn_in_flight && !self.allow_queue_while_busy {
            return ComposerOutcome::Unchanged;
        }
        self.draft.clear();
        ComposerOutcome::Submit { prompt }
    }

    pub fn set_turn_in_flight(&mut self, busy: bool) {
        self.turn_in_flight = busy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_trims_and_clears_draft() {
        let mut c = ComposerState::new();
        c.set_draft("  fix the login bug  ");
        match c.submit() {
            ComposerOutcome::Submit { prompt } => {
                assert_eq!(prompt, "fix the login bug");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
        assert!(c.draft.is_empty());
    }

    #[test]
    fn empty_submit_is_noop() {
        let mut c = ComposerState::new();
        assert_eq!(c.submit(), ComposerOutcome::Unchanged);
    }
}
