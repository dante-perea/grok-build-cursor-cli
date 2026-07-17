//! Workspace / editor surface (left pane).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePane {
    pub root: PathBuf,
    pub files: Vec<WorkspaceFile>,
    pub selected: usize,
    /// Open buffer path (editor surface).
    pub open_path: Option<PathBuf>,
    /// In-memory buffer content for the open file.
    pub buffer: String,
}

impl WorkspacePane {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            files: Vec::new(),
            selected: 0,
            open_path: None,
            buffer: String::new(),
        }
    }

    /// Load shallow listing of `root` (files + dirs, non-recursive).
    pub fn refresh_listing(&mut self) -> std::io::Result<()> {
        let mut files = Vec::new();
        let rd = std::fs::read_dir(&self.root)?;
        for ent in rd.flatten() {
            let path = ent.path();
            let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
            // Skip hidden / target noise for the explorer surface.
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.starts_with('.') || name == "target" {
                    continue;
                }
            }
            files.push(WorkspaceFile { path, is_dir });
        }
        files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        });
        self.files = files;
        if self.selected >= self.files.len() {
            self.selected = self.files.len().saturating_sub(1);
        }
        Ok(())
    }

    pub fn select_next(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.files.len() - 1);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn open_selected(&mut self) -> std::io::Result<bool> {
        let Some(file) = self.files.get(self.selected) else {
            return Ok(false);
        };
        if file.is_dir {
            return Ok(false);
        }
        let content = std::fs::read_to_string(&file.path)?;
        self.open_path = Some(file.path.clone());
        self.buffer = content;
        Ok(true)
    }

    pub fn open_path(&mut self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        self.open_path = Some(path.to_path_buf());
        self.buffer = content;
        Ok(())
    }

    pub fn set_buffer(&mut self, content: impl Into<String>) {
        self.buffer = content.into();
    }
}
