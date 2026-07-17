//! Slash-command catalog + local handlers (Grok Build–style autocomplete).

use serde::{Deserialize, Serialize};

/// A slash command advertised in the Composer autocomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
    pub usage: String,
    /// When true, handled locally by the control plane (not sent as a freeform agent chat).
    pub local: bool,
}

/// Built-in slash commands aligned with Grok Build pager names.
pub fn builtin_slash_commands() -> Vec<SlashCommandInfo> {
    vec![
        cmd("help", "List available slash commands", "/help", true),
        cmd("usage", "Show usage / quota summary", "/usage", true),
        cmd("plan", "Toggle plan mode (read-only planning)", "/plan", true),
        cmd("model", "Show or set model label", "/model [name]", true),
        cmd("new", "Start a new agent session", "/new", true),
        cmd("clear", "Clear the current transcript", "/clear", true),
        cmd("status", "Session and workspace status", "/status", true),
        cmd("projects", "List known projects", "/projects", true),
        cmd("cd", "Change workspace project", "/cd <path-or-name>", true),
        cmd("attach", "Hint: use + to attach files", "/attach", true),
        cmd(
            "strings",
            "Search string literals (delegates to agent)",
            "/strings <query>",
            false,
        ),
        cmd(
            "compact",
            "Compact conversation context (delegates to agent)",
            "/compact",
            false,
        ),
        cmd(
            "export",
            "Export session (delegates to agent)",
            "/export",
            false,
        ),
        cmd("diff", "Show pending diffs in review pane", "/diff", true),
        cmd("theme", "Theme note (UI is web dark)", "/theme", true),
        cmd(
            "login",
            "Auth: run `grok login` in a terminal",
            "/login",
            true,
        ),
        cmd("logout", "Auth: run `grok logout` in a terminal", "/logout", true),
        cmd(
            "config",
            "Open config guidance",
            "/config",
            true,
        ),
        cmd(
            "mcp",
            "MCP servers (use full Grok TUI for management)",
            "/mcp",
            true,
        ),
        cmd(
            "skills",
            "Skills (use full Grok TUI or agent)",
            "/skills",
            true,
        ),
    ]
}

fn cmd(name: &str, description: &str, usage: &str, local: bool) -> SlashCommandInfo {
    SlashCommandInfo {
        name: name.into(),
        description: description.into(),
        usage: usage.into(),
        local,
    }
}

/// Filter commands by prefix (without leading `/`).
pub fn filter_slash_commands(prefix: &str) -> Vec<SlashCommandInfo> {
    let p = prefix.trim().trim_start_matches('/').to_lowercase();
    builtin_slash_commands()
        .into_iter()
        .filter(|c| p.is_empty() || c.name.starts_with(&p) || c.name.contains(&p))
        .collect()
}

/// Parse a line that starts with `/` into (name, args).
pub fn parse_slash_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if !t.starts_with('/') {
        return None;
    }
    let rest = t[1..].trim();
    if rest.is_empty() {
        return Some((String::new(), String::new()));
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").to_lowercase();
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((name, args))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashExecResult {
    pub handled: bool,
    /// System message to show in chat (if any).
    pub message: Option<String>,
    /// Optional side effect key for the server.
    pub action: Option<String>,
    pub action_arg: Option<String>,
}

/// Local execution for known commands. Non-local → not handled (forward to agent).
pub fn exec_local_slash(name: &str, args: &str, ctx: &SlashExecCtx<'_>) -> SlashExecResult {
    match name {
        "help" => {
            let list = builtin_slash_commands()
                .iter()
                .map(|c| format!("/{} — {}", c.name, c.description))
                .collect::<Vec<_>>()
                .join("\n");
            SlashExecResult {
                handled: true,
                message: Some(format!("Slash commands:\n{list}")),
                action: None,
                action_arg: None,
            }
        }
        "usage" => SlashExecResult {
            handled: true,
            message: Some(format!(
                "Usage (cursor-cli local)\n\
                 · Workspace: {}\n\
                 · Mode: {}\n\
                 · Model: {}\n\
                 · Plan mode: {}\n\
                 · Agent binary: set GROK_AGENT_BIN or use `grok` on PATH\n\
                 · Full quota UI: run `grok` TUI and `/usage`",
                ctx.workspace,
                ctx.mode,
                ctx.model,
                ctx.plan_mode
            )),
            action: None,
            action_arg: None,
        },
        "plan" => SlashExecResult {
            handled: true,
            message: Some("Toggled plan mode.".into()),
            action: Some("toggle_plan".into()),
            action_arg: None,
        },
        "model" => {
            if args.is_empty() {
                SlashExecResult {
                    handled: true,
                    message: Some(format!("Current model label: {}", ctx.model)),
                    action: None,
                    action_arg: None,
                }
            } else {
                SlashExecResult {
                    handled: true,
                    message: Some(format!("Model label set to: {args}")),
                    action: Some("set_model".into()),
                    action_arg: Some(args.to_string()),
                }
            }
        }
        "new" => SlashExecResult {
            handled: true,
            message: Some("Started a new agent.".into()),
            action: Some("new_agent".into()),
            action_arg: None,
        },
        "clear" => SlashExecResult {
            handled: true,
            message: Some("Transcript cleared.".into()),
            action: Some("clear".into()),
            action_arg: None,
        },
        "status" => SlashExecResult {
            handled: true,
            message: Some(format!(
                "Status\n· view/workspace: {}\n· mode: {}\n· busy: {}",
                ctx.workspace, ctx.mode, ctx.busy
            )),
            action: None,
            action_arg: None,
        },
        "projects" => SlashExecResult {
            handled: true,
            message: Some("Listing projects…".into()),
            action: Some("list_projects".into()),
            action_arg: None,
        },
        "cd" => {
            if args.is_empty() {
                SlashExecResult {
                    handled: true,
                    message: Some("Usage: /cd <project-name-or-path>".into()),
                    action: None,
                    action_arg: None,
                }
            } else {
                SlashExecResult {
                    handled: true,
                    message: Some(format!("Switching project to: {args}")),
                    action: Some("set_project".into()),
                    action_arg: Some(args.to_string()),
                }
            }
        }
        "attach" => SlashExecResult {
            handled: true,
            message: Some("Use the + button in the composer to attach files.".into()),
            action: None,
            action_arg: None,
        },
        "diff" => SlashExecResult {
            handled: true,
            message: Some("Open Diff Review in the session side panel.".into()),
            action: Some("focus_diff".into()),
            action_arg: None,
        },
        "theme" => SlashExecResult {
            handled: true,
            message: Some("Web UI uses Cursor-style dark theme. Full themes: Grok TUI `/theme`.".into()),
            action: None,
            action_arg: None,
        },
        "login" | "logout" => SlashExecResult {
            handled: true,
            message: Some(format!(
                "Run `{name}` in a terminal with the `grok` CLI for full auth."
            )),
            action: None,
            action_arg: None,
        },
        "config" | "mcp" | "skills" => SlashExecResult {
            handled: true,
            message: Some(format!(
                "`/{name}` is fully supported in the Grok Build TUI. Here: prefer Composer + attach + project picker."
            )),
            action: None,
            action_arg: None,
        },
        _ => {
            // Non-local or unknown: not handled
            let known = builtin_slash_commands()
                .iter()
                .any(|c| c.name == name && !c.local);
            if known {
                SlashExecResult {
                    handled: false,
                    message: None,
                    action: None,
                    action_arg: None,
                }
            } else {
                SlashExecResult {
                    handled: true,
                    message: Some(format!(
                        "Unknown command `/{name}`. Type `/help` for the list."
                    )),
                    action: None,
                    action_arg: None,
                }
            }
        }
    }
}

pub struct SlashExecCtx<'a> {
    pub workspace: &'a str,
    pub mode: &'a str,
    pub model: &'a str,
    pub plan_mode: bool,
    pub busy: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_usage_and_strings() {
        let u = filter_slash_commands("us");
        assert!(u.iter().any(|c| c.name == "usage"));
        let s = filter_slash_commands("str");
        assert!(s.iter().any(|c| c.name == "strings"));
    }

    #[test]
    fn parse_slash() {
        assert_eq!(
            parse_slash_line("/usage"),
            Some(("usage".into(), String::new()))
        );
        assert_eq!(
            parse_slash_line("/cd my-app"),
            Some(("cd".into(), "my-app".into()))
        );
    }

    #[test]
    fn help_is_local() {
        let ctx = SlashExecCtx {
            workspace: "/tmp",
            mode: "agent",
            model: "Grok",
            plan_mode: false,
            busy: false,
        };
        let r = exec_local_slash("help", "", &ctx);
        assert!(r.handled);
        assert!(r.message.unwrap().contains("/usage"));
    }
}
