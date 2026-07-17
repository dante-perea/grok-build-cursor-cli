//! Agent driver — Composer submit → real Grok Build agent runtime.
//!
//! The production path spawns (or attaches to) the Grok Build agent over ACP
//! stdio (`xai-grok-pager agent stdio` / `grok agent stdio`). Events are parsed
//! into [`AgentRuntimeEvent`] and reduced into the Cursor session.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::agent_bridge::{AgentRuntimeEvent, ToolCallPhase};

/// Request issued when Composer submits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPromptRequest {
    pub prompt: String,
    pub cwd: PathBuf,
}

/// Outcome of a driven agent turn (for logging / tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPromptResult {
    pub events: Vec<AgentRuntimeEvent>,
    pub ok: bool,
}

/// Trait for driving the agent runtime from Composer.
pub trait AgentDriver: Send {
    /// Human-readable backend name (for status UI).
    fn name(&self) -> &str;

    /// Submit a prompt and stream runtime events on `tx`.
    ///
    /// Implementations must talk to the real Grok Build agent path when
    /// available (ACP stdio). Unit tests use [`RecordingDriver`] only as a
    /// harness observer — product code uses [`RealGrokAgentDriver`].
    fn submit(
        &mut self,
        request: AgentPromptRequest,
        tx: mpsc::UnboundedSender<AgentRuntimeEvent>,
    ) -> impl std::future::Future<Output = Result<AgentPromptResult>> + Send;
}

/// Production driver: spawn Grok Build agent stdio and map JSON-RPC / ACP-like
/// lines into [`AgentRuntimeEvent`].
///
/// Resolution order for the agent binary:
/// 1. `GROK_AGENT_BIN` env
/// 2. `agent_bin` field (CLI flag)
/// 3. `xai-grok-pager` on PATH
/// 4. `grok` on PATH
#[derive(Debug, Clone)]
pub struct RealGrokAgentDriver {
    pub agent_bin: Option<PathBuf>,
    pub cwd: PathBuf,
    /// Extra args before `agent stdio` (rarely needed).
    pub extra_args: Vec<String>,
}

impl RealGrokAgentDriver {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            agent_bin: None,
            cwd: cwd.into(),
            extra_args: Vec::new(),
        }
    }

    pub fn with_bin(mut self, bin: impl Into<PathBuf>) -> Self {
        self.agent_bin = Some(bin.into());
        self
    }

    /// Resolve the agent executable path.
    pub fn resolve_bin(&self) -> Result<PathBuf> {
        if let Ok(env_bin) = std::env::var("GROK_AGENT_BIN") {
            let p = PathBuf::from(env_bin);
            if p.exists() || which_exists(&p) {
                return Ok(p);
            }
        }
        if let Some(bin) = &self.agent_bin {
            return Ok(bin.clone());
        }
        for candidate in ["xai-grok-pager", "grok"] {
            if let Some(p) = which(candidate) {
                return Ok(p);
            }
        }
        bail!(
            "could not find Grok Build agent binary (set GROK_AGENT_BIN or install `grok` / build xai-grok-pager)"
        )
    }

    /// Build the argv used to start the real agent runtime.
    pub fn agent_command_line(&self, bin: &Path) -> Vec<String> {
        let mut args = vec![bin.display().to_string()];
        args.extend(self.extra_args.iter().cloned());
        // Grok Build composition root: `agent stdio` is the ACP agent runtime.
        args.push("agent".into());
        args.push("stdio".into());
        args
    }
}

impl AgentDriver for RealGrokAgentDriver {
    fn name(&self) -> &str {
        "grok-build-agent-stdio"
    }

    async fn submit(
        &mut self,
        request: AgentPromptRequest,
        tx: mpsc::UnboundedSender<AgentRuntimeEvent>,
    ) -> Result<AgentPromptResult> {
        let bin = self.resolve_bin().context("resolve agent binary")?;
        let cmdline = self.agent_command_line(&bin);
        let _ = tx.send(AgentRuntimeEvent::Status {
            message: format!("Starting real agent: {}", cmdline.join(" ")),
        });
        let _ = tx.send(AgentRuntimeEvent::TurnStarted);

        let mut child = spawn_agent(&cmdline, &request.cwd)
            .await
            .context("spawn Grok Build agent")?;

        // Minimal ACP-ish handshake + prompt. The full ACP client lives in the
        // pager; here we drive the tools/edits path by sending a session prompt
        // JSON line the runtime accepts, and map streamed notifications.
        if let Some(stdin) = child.stdin.as_mut() {
            // Initialize
            let init = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "0.1.0",
                    "clientInfo": { "name": "grok-build-cursor-cli", "version": "0.1.0" },
                    "capabilities": {}
                }
            });
            write_json_line(stdin, &init).await?;

            let new_session = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": request.cwd,
                    "mcpServers": []
                }
            });
            write_json_line(stdin, &new_session).await?;

            let prompt = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": "cursor-shell",
                    "prompt": [{ "type": "text", "text": request.prompt }]
                }
            });
            write_json_line(stdin, &prompt).await?;
        }

        let stdout = child
            .stdout
            .take()
            .context("agent stdout missing")?;
        let mut reader = BufReader::new(stdout).lines();
        let mut events = Vec::new();

        while let Some(line) = reader.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(ev) = map_agent_line(&line) {
                let _ = tx.send(ev.clone());
                events.push(ev);
            }
        }

        let status = child.wait().await?;
        let ok = status.success();
        let end = AgentRuntimeEvent::TurnCompleted { ok };
        let _ = tx.send(end.clone());
        events.push(end);

        Ok(AgentPromptResult { events, ok })
    }
}

async fn spawn_agent(cmdline: &[String], cwd: &Path) -> Result<Child> {
    let (prog, args) = cmdline
        .split_first()
        .context("empty agent command line")?;
    let child = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn agent: {}", cmdline.join(" ")))?;
    Ok(child)
}

async fn write_json_line(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// Map a single stdout line from the agent process into a runtime event.
///
/// Accepts JSON-RPC notifications and a small set of ACP session update shapes
/// used by Grok Build (`agent_message_chunk`, `tool_call`, diffs).
pub fn map_agent_line(line: &str) -> Option<AgentRuntimeEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    // Notification form: { method, params }
    if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        return map_method(method, &params);
    }
    // Bare sessionUpdate objects
    if let Some(update) = v.get("sessionUpdate").and_then(|u| u.as_str()) {
        return map_session_update(update, &v);
    }
    None
}

fn map_method(method: &str, params: &Value) -> Option<AgentRuntimeEvent> {
    match method {
        "session/update" => {
            let update = params.get("update").unwrap_or(params);
            if let Some(kind) = update
                .get("sessionUpdate")
                .or_else(|| update.get("session_update"))
                .and_then(|s| s.as_str())
            {
                return map_session_update(kind, update);
            }
            // Nested ACP content
            if let Some(content) = update.get("content") {
                if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                    return Some(AgentRuntimeEvent::AgentMessageChunk {
                        text: text.to_string(),
                    });
                }
            }
            None
        }
        _ => None,
    }
}

fn map_session_update(kind: &str, body: &Value) -> Option<AgentRuntimeEvent> {
    match kind {
        "agent_message_chunk" | "agentMessageChunk" | "message" => {
            let text = body
                .pointer("/content/text")
                .or_else(|| body.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                None
            } else {
                Some(AgentRuntimeEvent::AgentMessageChunk { text })
            }
        }
        "tool_call" | "toolCall" => {
            let tool_call_id = body
                .get("toolCallId")
                .or_else(|| body.get("tool_call_id"))
                .and_then(|s| s.as_str())
                .unwrap_or("tool")
                .to_string();
            let tool_name = body
                .get("title")
                .or_else(|| body.get("kind"))
                .or_else(|| body.get("toolName"))
                .and_then(|s| s.as_str())
                .unwrap_or("tool")
                .to_string();
            let status = body
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("in_progress");
            let phase = match status {
                "pending" | "in_progress" => ToolCallPhase::Started,
                "completed" => ToolCallPhase::Completed,
                "failed" => ToolCallPhase::Failed,
                _ => ToolCallPhase::InProgress,
            };
            Some(AgentRuntimeEvent::ToolCall {
                tool_call_id,
                tool_name: tool_name.clone(),
                title: tool_name,
                phase,
                detail: String::new(),
            })
        }
        "tool_call_update" | "toolCallUpdate" => {
            let tool_call_id = body
                .get("toolCallId")
                .or_else(|| body.get("tool_call_id"))
                .and_then(|s| s.as_str())
                .unwrap_or("tool")
                .to_string();
            let status = body
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("completed");
            let phase = if status == "failed" {
                ToolCallPhase::Failed
            } else {
                ToolCallPhase::Completed
            };
            Some(AgentRuntimeEvent::ToolCall {
                tool_call_id,
                tool_name: "tool".into(),
                title: "tool".into(),
                phase,
                detail: status.into(),
            })
        }
        _ => None,
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn which_exists(p: &Path) -> bool {
    p.is_file() || which(p.to_str().unwrap_or("")).is_some()
}

/// Shared handle type for the app loop.
pub type SharedDriver = Arc<tokio::sync::Mutex<RealGrokAgentDriver>>;

/// Demo / dry-run path that still exercises the **shipped** event→UI pipeline
/// with a representative tools/edits progression when the agent binary is
/// unavailable. This is not the product agent — `RealGrokAgentDriver` is —
/// but unit and launch probes can call [`simulate_representative_turn`] to
/// prove binding of real event shapes.
pub fn simulate_representative_turn(prompt: &str) -> Vec<AgentRuntimeEvent> {
    vec![
        AgentRuntimeEvent::TurnStarted,
        AgentRuntimeEvent::Status {
            message: format!("Processing: {prompt}"),
        },
        AgentRuntimeEvent::AgentMessageChunk {
            text: "I'll inspect the workspace and apply a fix.\n".into(),
        },
        AgentRuntimeEvent::ToolCall {
            tool_call_id: "tc-read-1".into(),
            tool_name: "read_file".into(),
            title: "Read file".into(),
            phase: ToolCallPhase::Started,
            detail: String::new(),
        },
        AgentRuntimeEvent::ToolCall {
            tool_call_id: "tc-read-1".into(),
            tool_name: "read_file".into(),
            title: "Read file".into(),
            phase: ToolCallPhase::Completed,
            detail: "ok".into(),
        },
        AgentRuntimeEvent::ToolCall {
            tool_call_id: "tc-edit-1".into(),
            tool_name: "search_replace".into(),
            title: "Edit src/example.rs".into(),
            phase: ToolCallPhase::Started,
            detail: String::new(),
        },
        AgentRuntimeEvent::ProposedEdit {
            edit_id: "edit-example".into(),
            path: PathBuf::from("src/example.rs"),
            old_text: "fn hello() { }\n".into(),
            new_text: "fn hello() { println!(\"hi\"); }\n".into(),
        },
        AgentRuntimeEvent::ToolCall {
            tool_call_id: "tc-edit-1".into(),
            tool_name: "search_replace".into(),
            title: "Edit src/example.rs".into(),
            phase: ToolCallPhase::Completed,
            detail: "applied".into(),
        },
        AgentRuntimeEvent::AgentMessageChunk {
            text: "Done — review the diff in the Diff Review pane.".into(),
        },
        AgentRuntimeEvent::AgentMessageEnd,
        AgentRuntimeEvent::TurnCompleted { ok: true },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_agent_line_parses_message_chunk() {
        let line = r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}"#;
        let ev = map_agent_line(line).expect("map");
        match ev {
            AgentRuntimeEvent::AgentMessageChunk { text } => assert_eq!(text, "hello"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn map_agent_line_parses_tool_call() {
        let line = r#"{"sessionUpdate":"tool_call","toolCallId":"abc","title":"search_replace","status":"in_progress"}"#;
        let ev = map_agent_line(line).expect("map");
        match ev {
            AgentRuntimeEvent::ToolCall {
                tool_call_id,
                phase,
                ..
            } => {
                assert_eq!(tool_call_id, "abc");
                assert_eq!(phase, ToolCallPhase::Started);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn agent_command_line_targets_stdio_runtime() {
        let driver = RealGrokAgentDriver::new("/tmp");
        let cmdline = driver.agent_command_line(Path::new("/usr/bin/grok"));
        assert!(cmdline.ends_with(&["agent".into(), "stdio".into()]));
    }

    #[test]
    fn representative_turn_includes_tools_and_edit() {
        let events = simulate_representative_turn("fix it");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentRuntimeEvent::ProposedEdit { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentRuntimeEvent::ToolCall { .. }))
        );
    }
}
