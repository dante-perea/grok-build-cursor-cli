//! Discover local projects for the Agents Home project picker.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_git: bool,
}

/// Default roots scanned for projects (existing dirs only).
pub fn default_project_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for rel in ["projects", "Projects", "Developer", "dev", "code", "src"] {
            let p = home.join(rel);
            if p.is_dir() {
                roots.push(p);
            }
        }
        // Always include home as last-resort single folder listing is not wanted;
        // but include common TAIC path if present.
        let taic = home.join("Documents/Obsidian");
        if taic.is_dir() {
            roots.push(taic);
        }
    }
    roots
}

/// List projects under the given roots (one level deep).
pub fn list_projects(roots: &[PathBuf], limit: usize) -> Vec<ProjectEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let Ok(rd) = fs::read_dir(root) else {
            continue;
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if !path.is_dir() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(canon.clone()) {
                continue;
            }
            let is_git = path.join(".git").exists();
            out.push(ProjectEntry {
                name,
                path: canon,
                is_git,
            });
            if out.len() >= limit {
                out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                return out;
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Resolve a project name or path to an absolute directory.
pub fn resolve_project(query: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    let as_path = PathBuf::from(q);
    if as_path.is_dir() {
        return Some(as_path.canonicalize().unwrap_or(as_path));
    }
    // Expand ~
    if let Some(rest) = q.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home).join(rest);
            if p.is_dir() {
                return Some(p.canonicalize().unwrap_or(p));
            }
        }
    }
    let projects = list_projects(roots, 500);
    // Exact name
    if let Some(p) = projects.iter().find(|p| p.name == q) {
        return Some(p.path.clone());
    }
    // Case-insensitive
    let ql = q.to_lowercase();
    if let Some(p) = projects.iter().find(|p| p.name.to_lowercase() == ql) {
        return Some(p.path.clone());
    }
    // Prefix
    projects
        .into_iter()
        .find(|p| p.name.to_lowercase().starts_with(&ql))
        .map(|p| p.path)
}

/// Read git branch name if available.
pub fn git_branch(path: &Path) -> Option<String> {
    let head = path.join(".git/HEAD");
    let raw = fs::read_to_string(head).ok()?;
    let raw = raw.trim();
    if let Some(branch) = raw.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_string());
    }
    if raw.len() >= 7 {
        return Some(raw[..7].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("alpha-app");
        let b = dir.path().join("beta-app");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::create_dir(a.join(".git")).unwrap();
        let roots = vec![dir.path().to_path_buf()];
        let list = list_projects(&roots, 50);
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|p| p.name == "alpha-app" && p.is_git));
        let resolved = resolve_project("alpha-app", &roots).unwrap();
        assert_eq!(resolved, a.canonicalize().unwrap());
    }
}
