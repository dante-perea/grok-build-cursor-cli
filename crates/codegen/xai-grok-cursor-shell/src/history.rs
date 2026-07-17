//! Local agent session history for the Agents Home sidebar.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HistorySource {
    #[default]
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub workspace: PathBuf,
    pub updated_at: DateTime<Utc>,
    pub source: HistorySource,
}

impl SessionMeta {
    pub fn new(title: impl Into<String>, workspace: impl Into<PathBuf>) -> Self {
        let title = truncate_title(title.into());
        Self {
            id: Uuid::new_v4().to_string(),
            title,
            workspace: workspace.into(),
            updated_at: Utc::now(),
            source: HistorySource::Local,
        }
    }
}

fn truncate_title(s: String) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "New Agent".into();
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= 60 {
        t.to_string()
    } else {
        chars.into_iter().take(57).collect::<String>() + "…"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HistoryFile {
    sessions: Vec<SessionMeta>,
}

/// File-backed sidebar history (`~/.grok/cursor-cli/sessions.json` by default).
#[derive(Debug, Clone)]
pub struct AgentHistoryStore {
    path: PathBuf,
}

impl AgentHistoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Default path under grok home / cursor-cli.
    pub fn default_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".grok/cursor-cli/sessions.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> std::io::Result<Vec<SessionMeta>> {
        let file = self.load()?;
        Ok(file.sessions)
    }

    pub fn add(&mut self, meta: SessionMeta) -> std::io::Result<SessionMeta> {
        let mut file = self.load()?;
        file.sessions.insert(0, meta.clone());
        // Cap history
        if file.sessions.len() > 100 {
            file.sessions.truncate(100);
        }
        self.save(&file)?;
        Ok(meta)
    }

    pub fn touch(&mut self, id: &str) -> std::io::Result<()> {
        let mut file = self.load()?;
        if let Some(s) = file.sessions.iter_mut().find(|s| s.id == id) {
            s.updated_at = Utc::now();
        }
        // Move to front
        if let Some(pos) = file.sessions.iter().position(|s| s.id == id) {
            let s = file.sessions.remove(pos);
            file.sessions.insert(0, s);
        }
        self.save(&file)
    }

    fn load(&self) -> std::io::Result<HistoryFile> {
        if !self.path.exists() {
            return Ok(HistoryFile::default());
        }
        let raw = fs::read_to_string(&self.path)?;
        if raw.trim().is_empty() {
            return Ok(HistoryFile::default());
        }
        serde_json::from_str(&raw).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })
    }

    fn save(&self, file: &HistoryFile) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&self.path, raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_and_persist_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let mut store = AgentHistoryStore::new(&path);
        let meta = SessionMeta::new("fix the login bug please", dir.path());
        let id = meta.id.clone();
        store.add(meta).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].title.contains("login"));

        // Reload
        let store2 = AgentHistoryStore::new(&path);
        let listed2 = store2.list().unwrap();
        assert_eq!(listed2.len(), 1);
        assert_eq!(listed2[0].id, id);
    }

    #[test]
    fn empty_title_becomes_new_agent() {
        let m = SessionMeta::new("   ", "/tmp");
        assert_eq!(m.title, "New Agent");
    }
}
