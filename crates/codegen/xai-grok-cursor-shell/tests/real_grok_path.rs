//! Optional probe: drive RealGrokAgentDriver against host `grok` on PATH.
//!
//! Skipped when `grok` is not installed. When present, proves the shipped
//! driver spawns `grok agent stdio` and reads ACP stdout (turn may be short
//! if unauthenticated).

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;
use xai_grok_cursor_shell::agent_bridge::AgentRuntimeEvent;
use xai_grok_cursor_shell::agent_driver::{AgentPromptRequest, RealGrokAgentDriver};

fn find_grok() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GROK_AGENT_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    for name in ["grok", "xai-grok-pager"] {
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
    }
    // Common install location on this machine
    let local = PathBuf::from("/Users/danteperea/.local/bin/grok");
    if local.is_file() {
        return Some(local);
    }
    None
}

// Minimal which without extra crate
mod which {
    use std::path::PathBuf;
    pub fn which(name: &str) -> Result<PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        for dir in std::env::split_paths(&path) {
            let c = dir.join(name);
            if c.is_file() {
                return Ok(c);
            }
        }
        Err(())
    }
}

#[tokio::test]
async fn real_grok_on_path_spawn_and_read_stdio() {
    let Some(bin) = find_grok() else {
        eprintln!("skip: grok not on PATH");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut driver = RealGrokAgentDriver::new(dir.path())
        .with_bin(&bin)
        .with_read_timeout(Duration::from_secs(25));

    let cmdline = driver.agent_command_line(&bin);
    assert!(
        cmdline.iter().any(|a| a == "agent") && cmdline.iter().any(|a| a == "stdio"),
        "must target agent stdio: {cmdline:?}"
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    let result = driver
        .submit_prompt(
            AgentPromptRequest {
                prompt: "Reply with exactly: pong".into(),
                cwd: dir.path().to_path_buf(),
            },
            tx,
        )
        .await;

    match result {
        Ok(r) => {
            // Real process produced a turn boundary and/or stream events.
            assert!(
                r.events.iter().any(|e| matches!(
                    e,
                    AgentRuntimeEvent::TurnCompleted { .. }
                        | AgentRuntimeEvent::AgentMessageChunk { .. }
                        | AgentRuntimeEvent::Status { .. }
                        | AgentRuntimeEvent::ToolCall { .. }
                        | AgentRuntimeEvent::Error { .. }
                )),
                "expected runtime events from real grok: {:?}",
                r.events
            );
            eprintln!(
                "real grok events ({}): {:?}",
                r.events.len(),
                r.events.iter().take(8).collect::<Vec<_>>()
            );
        }
        Err(e) => {
            // Spawn itself must work if binary exists; parse failures still
            // return Ok with events. Hard spawn errors fail the test.
            panic!("RealGrokAgentDriver failed to drive host grok at {}: {e:#}", bin.display());
        }
    }
}
