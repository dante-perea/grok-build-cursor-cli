//! Grok subscription auth status + device-code login launcher.
//!
//! Reads `~/.grok/auth.json` **without** exposing tokens. Login is delegated to
//! the installed `grok` CLI: `grok login --device-auth` (alias `--device-code`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Public auth status for the UI (never includes tokens/keys).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthStatus {
    pub logged_in: bool,
    pub needs_login: bool,
    pub email: Option<String>,
    pub auth_mode: Option<String>,
    pub expires_at: Option<String>,
    pub display_name: Option<String>,
    pub message: String,
}

impl AuthStatus {
    pub fn logged_out(message: impl Into<String>) -> Self {
        Self {
            logged_in: false,
            needs_login: true,
            email: None,
            auth_mode: None,
            expires_at: None,
            display_name: None,
            message: message.into(),
        }
    }
}

/// Resolve auth.json path (GROK_HOME or ~/.grok/auth.json).
pub fn auth_json_path() -> PathBuf {
    if let Ok(home) = std::env::var("GROK_HOME") {
        return PathBuf::from(home).join("auth.json");
    }
    dirs_home()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok/auth.json")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Read auth status without returning secrets.
pub fn read_auth_status() -> AuthStatus {
    read_auth_status_at(&auth_json_path())
}

pub fn read_auth_status_at(path: &Path) -> AuthStatus {
    if !path.exists() {
        return AuthStatus::logged_out(
            "Not signed in to Grok. Use Sign in (device) or type /device to authenticate with your subscription.",
        );
    }
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            return AuthStatus::logged_out(
                "Cannot read Grok auth store. Run `grok login --device-auth`.",
            );
        }
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return AuthStatus::logged_out("Empty Grok auth store. Sign in with /device.");
    }
    let map: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            return AuthStatus::logged_out(
                "Corrupt auth.json. Run `grok login --device-auth` to re-authenticate.",
            );
        }
    };
    let obj = match map.as_object() {
        Some(o) if !o.is_empty() => o,
        _ => {
            return AuthStatus::logged_out("No Grok credentials found. Sign in with /device.");
        }
    };

    let now = Utc::now();
    let mut best: Option<(DateTime<Utc>, AuthStatus)> = None;

    for (_scope, entry) in obj {
        let Some(e) = entry.as_object() else {
            continue;
        };
        // Never surface `key` or `refresh_token`.
        let key_ok = e
            .get("key")
            .and_then(|k| k.as_str())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if !key_ok {
            continue;
        }
        let email = e
            .get("email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let auth_mode = e
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let expires_at = e
            .get("expires_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let first = e.get("first_name").and_then(|v| v.as_str()).unwrap_or("");
        let last = e.get("last_name").and_then(|v| v.as_str()).unwrap_or("");
        let display_name = {
            let n = format!("{first} {last}").trim().to_string();
            if n.is_empty() {
                email.clone()
            } else {
                Some(n)
            }
        };

        // Expired?
        if let Some(exp) = expires_at {
            if exp <= now {
                continue;
            }
            let status = AuthStatus {
                logged_in: true,
                needs_login: false,
                email: email.clone(),
                auth_mode: auth_mode.clone(),
                expires_at: Some(exp.to_rfc3339()),
                display_name: display_name.clone(),
                message: format!(
                    "Signed in as {} (subscription / OIDC). Expires {}.",
                    email.as_deref().unwrap_or("Grok user"),
                    exp.format("%Y-%m-%d %H:%M UTC")
                ),
            };
            match &best {
                None => best = Some((exp, status)),
                Some((prev, _)) if exp > *prev => best = Some((exp, status)),
                _ => {}
            }
        } else {
            // API key style — no expires_at, treat as valid if key present
            let status = AuthStatus {
                logged_in: true,
                needs_login: false,
                email: email.clone(),
                auth_mode: auth_mode.clone(),
                expires_at: None,
                display_name: display_name.clone(),
                message: format!(
                    "Signed in as {} (API key / durable credential).",
                    email.as_deref().unwrap_or("Grok user")
                ),
            };
            if best.is_none() {
                best = Some((now + chrono::Duration::days(365), status));
            }
        }
    }

    if let Some((_, status)) = best {
        return status;
    }

    AuthStatus::logged_out(
        "Grok credentials expired or missing. Sign in with your subscription via /device (device code).",
    )
}

/// Detect auth-related agent failures (so we can force re-login).
pub fn looks_like_auth_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("401")
        || m.contains("unauthorized")
        || m.contains("unauthenticated")
        || m.contains("not authenticated")
        || m.contains("sign in")
        || m.contains("log in")
        || m.contains("login required")
        || m.contains("auth") && (m.contains("fail") || m.contains("error") || m.contains("invalid"))
        || m.contains("token") && (m.contains("expired") || m.contains("invalid") || m.contains("revok"))
        || m.contains("no credentials")
        || m.contains("not signed in")
}

/// Resolve `grok` binary for login (same order as agent driver, login-focused).
pub fn resolve_grok_cli() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GROK_AGENT_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    for name in ["grok", "xai-grok-pager"] {
        if let Some(p) = which(name) {
            return Some(p);
        }
    }
    let local = dirs_home()?.join(".local/bin/grok");
    if local.is_file() {
        return Some(local);
    }
    None
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

/// Result of starting device-code login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginStartResult {
    pub started: bool,
    pub method: String,
    pub message: String,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
}

/// In-process login job state (poll until auth.json becomes valid).
#[derive(Debug, Default)]
pub struct LoginJob {
    pub started_at: Option<Instant>,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
    pub last_message: String,
    pub finished: bool,
    pub success: bool,
}

static LOGIN_JOB: Mutex<LoginJob> = Mutex::new(LoginJob {
    started_at: None,
    verification_url: None,
    user_code: None,
    last_message: String::new(),
    finished: false,
    success: false,
});

/// Start device-code login via `grok login --device-auth` and open Terminal on macOS.
pub fn start_device_login() -> LoginStartResult {
    if read_auth_status().logged_in {
        return LoginStartResult {
            started: false,
            method: "already".into(),
            message: "Already signed in to Grok.".into(),
            verification_url: None,
            user_code: None,
        };
    }

    let Some(bin) = resolve_grok_cli() else {
        return LoginStartResult {
            started: false,
            method: "missing".into(),
            message: "Could not find `grok` CLI. Install Grok Build, then run: grok login --device-auth".into(),
            verification_url: None,
            user_code: None,
        };
    };

    // Reset job
    {
        let mut job = LOGIN_JOB.lock().unwrap_or_else(|e| e.into_inner());
        *job = LoginJob {
            started_at: Some(Instant::now()),
            verification_url: None,
            user_code: None,
            last_message: "Starting device-code login…".into(),
            finished: false,
            success: false,
        };
    }

    // Prefer interactive Terminal so user completes the browser/device flow.
    let started_terminal = open_login_in_terminal(&bin);

    // Also spawn a background poller that watches auth.json for up to 15 minutes.
    std::thread::spawn(|| {
        poll_auth_until_logged_in(Duration::from_secs(15 * 60));
    });

    let msg = if started_terminal {
        "Opened Terminal for `grok login --device-auth`. Complete the device code flow in the browser, then return here — this app will detect your subscription automatically.".to_string()
    } else {
        format!(
            "Run this in a terminal to sign in with your Grok subscription:\n\n  {} login --device-auth\n\nThis app will detect login automatically once auth.json updates.",
            bin.display()
        )
    };

    LoginStartResult {
        started: true,
        method: if started_terminal {
            "terminal+device-auth".into()
        } else {
            "device-auth-instructions".into()
        },
        message: msg,
        verification_url: None,
        user_code: None,
    }
}

fn open_login_in_terminal(bin: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        // Open a new Terminal window running device-auth login.
        let cmd = format!(
            "{} login --device-auth; echo; echo 'Press Enter to close…'; read",
            shell_escape(&bin.display().to_string())
        );
        let script = format!(
            "tell application \"Terminal\"\n  activate\n  do script \"{}\"\nend tell",
            cmd.replace('\\', "\\\\").replace('"', "\\\"")
        );
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bin;
        // Best-effort: spawn detached login process
        Command::new(bin)
            .args(["login", "--device-auth"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn poll_auth_until_logged_in(max: Duration) {
    let start = Instant::now();
    while start.elapsed() < max {
        if read_auth_status().logged_in {
            let mut job = LOGIN_JOB.lock().unwrap_or_else(|e| e.into_inner());
            job.finished = true;
            job.success = true;
            job.last_message = "Signed in successfully.".into();
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let mut job = LOGIN_JOB.lock().unwrap_or_else(|e| e.into_inner());
    if !job.success {
        job.finished = true;
        job.last_message =
            "Login timed out waiting for credentials. Run `/device` again or `grok login --device-auth`."
                .into();
    }
}

/// Poll login job + current auth status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginPollResult {
    pub auth: AuthStatus,
    pub login_in_progress: bool,
    pub login_message: String,
    pub login_finished: bool,
    pub login_success: bool,
}

pub fn poll_login() -> LoginPollResult {
    let auth = read_auth_status();
    let job = LOGIN_JOB.lock().unwrap_or_else(|e| e.into_inner());
    let in_progress = job.started_at.is_some() && !job.finished && !auth.logged_in;
    LoginPollResult {
        auth: auth.clone(),
        login_in_progress: in_progress,
        login_message: if auth.logged_in {
            auth.message.clone()
        } else {
            job.last_message.clone()
        },
        login_finished: job.finished || auth.logged_in,
        login_success: job.success || auth.logged_in,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn empty_auth_needs_login() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("auth.json");
        let s = read_auth_status_at(&p);
        assert!(!s.logged_in);
        assert!(s.needs_login);
    }

    #[test]
    fn valid_oidc_auth_is_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("auth.json");
        let exp = (Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        let json = format!(
            r#"{{"https://auth.x.ai::test":{{"key":"secret-token-not-leaked","auth_mode":"oidc","create_time":"2026-01-01T00:00:00Z","user_id":"u1","email":"user@example.com","expires_at":"{exp}"}}}}"#
        );
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        let s = read_auth_status_at(&p);
        assert!(s.logged_in, "{s:?}");
        assert_eq!(s.email.as_deref(), Some("user@example.com"));
        // Must not leak key into message
        assert!(!s.message.contains("secret-token"));
    }

    #[test]
    fn expired_auth_needs_login() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("auth.json");
        let exp = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let json = format!(
            r#"{{"scope":{{"key":"x","auth_mode":"oidc","create_time":"2026-01-01T00:00:00Z","user_id":"u1","email":"a@b.c","expires_at":"{exp}"}}}}"#
        );
        fs::write(&p, json).unwrap();
        let s = read_auth_status_at(&p);
        assert!(!s.logged_in);
        assert!(s.needs_login);
    }

    #[test]
    fn auth_error_detection() {
        assert!(looks_like_auth_error("HTTP 401 Unauthorized"));
        assert!(looks_like_auth_error("token expired"));
        assert!(!looks_like_auth_error("file not found"));
    }
}
