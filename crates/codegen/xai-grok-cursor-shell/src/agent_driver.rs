//! Agent driver — Composer submit → real Grok Build agent runtime.
//!
//! The production path spawns (or attaches to) the Grok Build agent over ACP
//! stdio (`xai-grok-pager agent stdio` / `grok agent stdio`). Events are parsed
//! into [`AgentRuntimeEvent`] and reduced into the Cursor session.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

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
    /// Product path: spawn real Grok Build agent stdio and map ACP lines.
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
    /// Max time to wait for agent stdout before treating the turn as complete.
    pub read_timeout: Duration,
}

impl RealGrokAgentDriver {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            agent_bin: None,
            cwd: cwd.into(),
            extra_args: Vec::new(),
            read_timeout: Duration::from_secs(120),
        }
    }

    pub fn with_bin(mut self, bin: impl Into<PathBuf>) -> Self {
        self.agent_bin = Some(bin.into());
        self
    }

    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
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
            if bin.exists() || which_exists(bin) {
                return Ok(bin.clone());
            }
            // Still return configured path so spawn error is explicit.
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
        // Fixture agents (scripts named `fake-grok-agent*`) speak ACP on stdio
        // without the `agent stdio` subcommand — detect by basename.
        let base = bin
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if base.contains("fake-grok-agent") || base.ends_with(".fixture") {
            return args;
        }
        args.push("agent".into());
        args.push("stdio".into());
        args
    }

    /// Core submit implementation shared by the trait and direct callers.
    pub async fn submit_prompt(
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

        // Minimal ACP handshake + prompt.
        if let Some(stdin) = child.stdin.as_mut() {
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

            // session/new response may include sessionId; fixtures accept any id.
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
            // Close stdin so agents that exit on EOF can finish.
            // Drop write half by not holding after this block ends — we need
            // to take stdin and drop it explicitly.
        }
        // Drop stdin so the child sees EOF after requests.
        drop(child.stdin.take());

        let stdout = child.stdout.take().context("agent stdout missing")?;
        let mut reader = BufReader::new(stdout).lines();
        let mut events = Vec::new();

        let deadline = tokio::time::Instant::now() + self.read_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let _ = tx.send(AgentRuntimeEvent::Status {
                    message: "agent read timeout — closing turn".into(),
                });
                let _ = child.kill().await;
                break;
            }
            match tokio::time::timeout(remaining, reader.next_line()).await {
                Ok(Ok(Some(line))) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    for ev in map_agent_line_all(&line) {
                        let _ = tx.send(ev.clone());
                        events.push(ev);
                    }
                }
                Ok(Ok(None)) => break, // EOF
                Ok(Err(e)) => {
                    let _ = tx.send(AgentRuntimeEvent::Error {
                        message: format!("agent stdout read error: {e}"),
                    });
                    break;
                }
                Err(_) => {
                    let _ = tx.send(AgentRuntimeEvent::Status {
                        message: "agent read timeout — closing turn".into(),
                    });
                    let _ = child.kill().await;
                    break;
                }
            }
        }

        let status = child.wait().await;
        let ok = status.map(|s| s.success()).unwrap_or(false);
        // Ensure we always finish the stream for the UI.
        if !events
            .iter()
            .any(|e| matches!(e, AgentRuntimeEvent::AgentMessageEnd))
        {
            let end_msg = AgentRuntimeEvent::AgentMessageEnd;
            let _ = tx.send(end_msg.clone());
            events.push(end_msg);
        }
        let end = AgentRuntimeEvent::TurnCompleted { ok };
        let _ = tx.send(end.clone());
        events.push(end);

        Ok(AgentPromptResult { events, ok })
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
        self.submit_prompt(request, tx).await
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

/// Map a single stdout line from the agent process into zero or more events.
///
/// Accepts JSON-RPC notifications and ACP session update shapes used by Grok
/// Build (`agent_message_chunk`, `tool_call`, tool content diffs).
pub fn map_agent_line_all(line: &str) -> Vec<AgentRuntimeEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    // Notification form: { method, params }
    if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        return map_method(method, &params);
    }
    // Bare sessionUpdate objects
    if let Some(update) = v.get("sessionUpdate").and_then(|u| u.as_str()) {
        return map_session_update(update, &v);
    }
    Vec::new()
}

/// Convenience: first event only (back-compat for simple tests).
pub fn map_agent_line(line: &str) -> Option<AgentRuntimeEvent> {
    map_agent_line_all(line).into_iter().next()
}

fn map_method(method: &str, params: &Value) -> Vec<AgentRuntimeEvent> {
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
                    if !text.is_empty() {
                        return vec![AgentRuntimeEvent::AgentMessageChunk {
                            text: text.to_string(),
                        }];
                    }
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn map_session_update(kind: &str, body: &Value) -> Vec<AgentRuntimeEvent> {
    match kind {
        "agent_message_chunk" | "agentMessageChunk" | "message" => {
            let text = body
                .pointer("/content/text")
                .or_else(|| body.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![AgentRuntimeEvent::AgentMessageChunk { text }]
            }
        }
        "agent_thought_chunk" | "agentThoughtChunk" => {
            let text = body
                .pointer("/content/text")
                .or_else(|| body.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![AgentRuntimeEvent::Status {
                    message: format!("thinking: {text}"),
                }]
            }
        }
        "tool_call" | "toolCall" => map_tool_call(body, false),
        "tool_call_update" | "toolCallUpdate" => map_tool_call(body, true),
        "diff_review" | "diffReview" => extract_diffs_from_value(body, "diff-review"),
        _ => Vec::new(),
    }
}

fn map_tool_call(body: &Value, is_update: bool) -> Vec<AgentRuntimeEvent> {
    let mut out = Vec::new();
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
        .or_else(|| body.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or("tool")
        .to_string();
    let status = body
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or(if is_update {
            "completed"
        } else {
            "in_progress"
        });
    let phase = match status {
        "pending" | "in_progress" => {
            if is_update {
                ToolCallPhase::InProgress
            } else {
                ToolCallPhase::Started
            }
        }
        "completed" => ToolCallPhase::Completed,
        "failed" => ToolCallPhase::Failed,
        _ => ToolCallPhase::InProgress,
    };
    out.push(AgentRuntimeEvent::ToolCall {
        tool_call_id: tool_call_id.clone(),
        tool_name: tool_name.clone(),
        title: tool_name,
        phase,
        detail: if is_update {
            status.into()
        } else {
            String::new()
        },
    });

    // Extract ProposedEdit from ACP tool content / rawInput / rawOutput.
    out.extend(extract_diffs_from_tool_body(body, &tool_call_id));
    out
}

/// Pull file diffs from ACP `content[]` entries and common tool argument shapes.
fn extract_diffs_from_tool_body(body: &Value, tool_call_id: &str) -> Vec<AgentRuntimeEvent> {
    let mut out = Vec::new();

    // content: [ { type: "diff", path, oldText, newText }, ... ]
    if let Some(content) = body.get("content").and_then(|c| c.as_array()) {
        for (i, item) in content.iter().enumerate() {
            if let Some(edit) = parse_diff_content(item, &format!("{tool_call_id}-{i}")) {
                out.push(edit);
            }
        }
    }

    // rawInput / arguments shapes used by search_replace / write tools
    for key in ["rawInput", "raw_input", "arguments", "input"] {
        if let Some(input) = body.get(key) {
            out.extend(extract_edits_from_tool_input(input, tool_call_id));
        }
    }

    // Nested under toolCall / update
    if let Some(nested) = body.get("toolCall").or_else(|| body.get("update")) {
        out.extend(extract_diffs_from_tool_body(nested, tool_call_id));
    }

    out
}

fn extract_diffs_from_value(body: &Value, prefix: &str) -> Vec<AgentRuntimeEvent> {
    let mut out = Vec::new();
    if let Some(content) = body.get("content").and_then(|c| c.as_array()) {
        for (i, item) in content.iter().enumerate() {
            if let Some(edit) = parse_diff_content(item, &format!("{prefix}-{i}")) {
                out.push(edit);
            }
        }
    }
    if let Some(edit) = parse_diff_content(body, prefix) {
        out.push(edit);
    }
    out
}

fn parse_diff_content(item: &Value, edit_id: &str) -> Option<AgentRuntimeEvent> {
    let ty = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // Accept type:"diff" or objects that look like diffs (path + old/new text).
    let path = item
        .get("path")
        .or_else(|| item.get("filePath"))
        .or_else(|| item.get("file_path"))
        .and_then(|p| p.as_str())?;
    let old_text = item
        .get("oldText")
        .or_else(|| item.get("old_text"))
        .or_else(|| item.get("oldString"))
        .or_else(|| item.get("old_string"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let new_text = item
        .get("newText")
        .or_else(|| item.get("new_text"))
        .or_else(|| item.get("newString"))
        .or_else(|| item.get("new_string"))
        .and_then(|t| t.as_str())?;

    if ty != "diff" && ty != "Diff" && old_text.is_empty() && new_text.is_empty() {
        // Not a diff-shaped object (e.g. content type "text").
        if item.get("oldText").is_none()
            && item.get("old_text").is_none()
            && item.get("newText").is_none()
            && item.get("new_text").is_none()
        {
            return None;
        }
    }

    Some(AgentRuntimeEvent::ProposedEdit {
        edit_id: edit_id.to_string(),
        path: PathBuf::from(path),
        old_text,
        new_text: new_text.to_string(),
    })
}

fn extract_edits_from_tool_input(input: &Value, tool_call_id: &str) -> Vec<AgentRuntimeEvent> {
    let mut out = Vec::new();
    // search_replace style
    if let (Some(path), Some(old), Some(new)) = (
        input
            .get("path")
            .or_else(|| input.get("file_path"))
            .and_then(|p| p.as_str()),
        input
            .get("old_string")
            .or_else(|| input.get("oldString"))
            .or_else(|| input.get("old_text"))
            .and_then(|t| t.as_str()),
        input
            .get("new_string")
            .or_else(|| input.get("newString"))
            .or_else(|| input.get("new_text"))
            .and_then(|t| t.as_str()),
    ) {
        out.push(AgentRuntimeEvent::ProposedEdit {
            edit_id: format!("{tool_call_id}-sr"),
            path: PathBuf::from(path),
            old_text: old.to_string(),
            new_text: new.to_string(),
        });
    }
    // write / create file style
    if let (Some(path), Some(contents)) = (
        input
            .get("path")
            .or_else(|| input.get("file_path"))
            .and_then(|p| p.as_str()),
        input
            .get("contents")
            .or_else(|| input.get("content"))
            .and_then(|t| t.as_str()),
    ) {
        // Only if we didn't already add a search_replace edit for this path.
        if !out.iter().any(|e| {
            matches!(
                e,
                AgentRuntimeEvent::ProposedEdit { path: p, .. } if p == Path::new(path)
            )
        }) {
            out.push(AgentRuntimeEvent::ProposedEdit {
                edit_id: format!("{tool_call_id}-write"),
                path: PathBuf::from(path),
                old_text: String::new(),
                new_text: contents.to_string(),
            });
        }
    }
    out
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

/// Apply accept/reject for a reviewable change onto the workspace filesystem.
///
/// - **Accept**: ensure `new_text` is written to `path` (agent may already have).
/// - **Reject**: restore `old_text` when present; delete file if old was empty/new file.
pub fn apply_change_to_disk(
    path: &Path,
    old_text: Option<&str>,
    new_text: &str,
    accept: bool,
) -> Result<()> {
    if accept {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, new_text)?;
    } else {
        match old_text {
            Some(old) if !old.is_empty() => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, old)?;
            }
            _ => {
                // New file with no baseline — reject removes it.
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
        }
    }
    Ok(())
}

/// Demo-only stream used when the agent binary cannot be resolved **and**
/// `allow_simulated_runtime` is true. Product dump-layout prefers the real
/// driver; this is never used by `RealGrokAgentDriver` itself.
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
    fn map_agent_line_extracts_proposed_edit_from_diff_content() {
        let line = r#"{"sessionUpdate":"tool_call","toolCallId":"edit1","title":"search_replace","status":"completed","content":[{"type":"diff","path":"src/lib.rs","oldText":"fn a(){}","newText":"fn a(){todo!()}"}]}"#;
        let events = map_agent_line_all(line);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentRuntimeEvent::ToolCall { .. })),
            "{events:?}"
        );
        let edit = events.iter().find_map(|e| match e {
            AgentRuntimeEvent::ProposedEdit {
                path,
                old_text,
                new_text,
                ..
            } => Some((path.clone(), old_text.clone(), new_text.clone())),
            _ => None,
        });
        let (path, old, new) = edit.expect("ProposedEdit from ACP diff content");
        assert_eq!(path, PathBuf::from("src/lib.rs"));
        assert_eq!(old, "fn a(){}");
        assert_eq!(new, "fn a(){todo!()}");
    }

    #[test]
    fn map_agent_line_extracts_edit_from_raw_input() {
        let line = r#"{"sessionUpdate":"tool_call","toolCallId":"sr1","title":"search_replace","status":"in_progress","rawInput":{"path":"a.rs","old_string":"x","new_string":"y"}}"#;
        let events = map_agent_line_all(line);
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentRuntimeEvent::ProposedEdit {
                    path,
                    old_text,
                    new_text,
                    ..
                } if path == Path::new("a.rs") && old_text == "x" && new_text == "y"
            )),
            "{events:?}"
        );
    }

    #[test]
    fn agent_command_line_targets_stdio_runtime() {
        let driver = RealGrokAgentDriver::new("/tmp");
        let cmdline = driver.agent_command_line(Path::new("/usr/bin/grok"));
        assert!(cmdline.ends_with(&["agent".into(), "stdio".into()]));
    }

    #[test]
    fn apply_change_reject_restores_old_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "NEW").unwrap();
        apply_change_to_disk(&path, Some("OLD"), "NEW", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "OLD");
    }

    #[test]
    fn apply_change_accept_writes_new_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        apply_change_to_disk(&path, Some("OLD"), "NEW", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "NEW");
    }
}
