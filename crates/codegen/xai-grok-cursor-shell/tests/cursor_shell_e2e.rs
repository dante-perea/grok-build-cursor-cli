//! Integration tests for the shipped Cursor shell (layout, submit, activity, diffs).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tokio::sync::mpsc;
use xai_grok_cursor_shell::agent_bridge::{AgentRuntimeEvent, ToolCallPhase, bind_events};
use xai_grok_cursor_shell::agent_driver::{
    AgentPromptRequest, RealGrokAgentDriver, apply_change_to_disk, map_agent_line,
    map_agent_line_all,
};
use xai_grok_cursor_shell::app::{AppOptions, run_headless_dump};
use xai_grok_cursor_shell::diff_review::ChangeDecision;
use xai_grok_cursor_shell::layout::FocusPane;
use xai_grok_cursor_shell::session::{CursorAction, CursorSession, SessionEffect};
use xai_hunk_tracker::{Hunk, HunkAction, HunkEvent, HunkSource};

fn fixture_agent() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-grok-agent.sh")
}

#[test]
fn multi_pane_layout_regions_are_cursor_like() {
    let session = CursorSession::new(std::env::temp_dir());
    let snap = session.layout_snapshot();
    assert!(snap.is_multi_pane(), "snapshot: {:?}", snap.regions);
    for required in [
        FocusPane::Workspace,
        FocusPane::Chat,
        FocusPane::Composer,
        FocusPane::Activity,
        FocusPane::DiffReview,
    ] {
        assert!(
            snap.regions.contains(&required),
            "missing region {required:?}"
        );
    }
    let dump = snap.dump();
    assert!(dump.contains("Composer"));
    assert!(dump.contains("Activity"));
    assert!(dump.contains("Diff Review"));
    assert!(dump.contains("multi_pane: true"));
}

#[test]
fn composer_submit_path_emits_real_agent_effect() {
    let mut session = CursorSession::new(std::env::temp_dir());
    session.reduce(CursorAction::ComposerInsertStr(
        "add logging to the auth module".into(),
    ));
    let effects = session.reduce(CursorAction::ComposerSubmit);
    let prompt = effects.iter().find_map(|e| match e {
        SessionEffect::SubmitToAgent { prompt } => Some(prompt.as_str()),
        _ => None,
    });
    assert_eq!(
        prompt,
        Some("add logging to the auth module"),
        "effects={effects:?}"
    );
    assert!(session.agent_busy);
    assert_eq!(
        session.chat.messages[0].content,
        "add logging to the auth module"
    );
}

#[test]
fn activity_and_streaming_bind_from_runtime_events() {
    let mut session = CursorSession::new(std::env::temp_dir());
    session.reduce(CursorAction::ComposerInsertStr("go".into()));
    let _ = session.reduce(CursorAction::ComposerSubmit);

    let events = vec![
        AgentRuntimeEvent::AgentMessageChunk {
            text: "Working on it".into(),
        },
        AgentRuntimeEvent::ToolCall {
            tool_call_id: "t-bash".into(),
            tool_name: "bash".into(),
            title: "Run tests".into(),
            phase: ToolCallPhase::Started,
            detail: String::new(),
        },
        AgentRuntimeEvent::ToolCall {
            tool_call_id: "t-bash".into(),
            tool_name: "bash".into(),
            title: "Run tests".into(),
            phase: ToolCallPhase::Completed,
            detail: "exit 0".into(),
        },
        AgentRuntimeEvent::AgentMessageEnd,
        AgentRuntimeEvent::TurnCompleted { ok: true },
    ];
    session.reduce_all(bind_events(events));

    assert!(
        session
            .chat
            .messages
            .iter()
            .any(|m| m.content.contains("Working on it"))
    );
    let tool = session
        .activity
        .entries
        .iter()
        .find(|e| e.id == "t-bash")
        .expect("tool activity");
    assert_eq!(
        tool.status,
        xai_grok_cursor_shell::ActivityStatus::Completed
    );
    assert!(!session.agent_busy);
}

#[test]
fn diff_review_maps_proposed_edits_and_hunk_events() {
    let mut session = CursorSession::new(std::env::temp_dir());

    session.reduce_all(bind_events(vec![AgentRuntimeEvent::ProposedEdit {
        edit_id: "e1".into(),
        path: PathBuf::from("src/lib.rs"),
        old_text: "fn a() {}".into(),
        new_text: "fn a() { todo!() }".into(),
    }]));
    assert_eq!(session.diffs.pending_count(), 1);
    let preview = session
        .diffs
        .selected_item()
        .expect("item")
        .inspect_preview();
    assert!(preview.contains("fn a()"));

    let hunk = Hunk::file_created(
        PathBuf::from("src/new.rs"),
        "pub struct X;".into(),
        HunkSource::AgentEdit { prompt_index: 1 },
    );
    let event = HunkEvent::HunkAdded {
        path: PathBuf::from("src/new.rs"),
        hunk: std::sync::Arc::new(hunk),
    };
    session.reduce(CursorAction::ApplyHunkEvent { event });
    assert!(session.diffs.items.len() >= 2);

    let effects = session.reduce(CursorAction::AcceptSelectedChange);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SessionEffect::ApplyHunkAction { .. }))
    );
    assert_eq!(
        session.diffs.items[session.diffs.selected].decision,
        ChangeDecision::Accepted
    );
}

#[test]
fn map_agent_line_is_shipped_parser_for_stdio_runtime() {
    let line = r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"stream"}}}}"#;
    let ev = map_agent_line(line).expect("parse shipped mapper");
    match ev {
        AgentRuntimeEvent::AgentMessageChunk { text } => assert_eq!(text, "stream"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn map_agent_line_emits_proposed_edit_from_acp_diff() {
    let line = r#"{"method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"e1","title":"search_replace","status":"completed","content":[{"type":"diff","path":"src/x.rs","oldText":"a","newText":"b"}]}}}"#;
    let events = map_agent_line_all(line);
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentRuntimeEvent::ProposedEdit {
                path,
                old_text,
                new_text,
                ..
            } if path == Path::new("src/x.rs") && old_text == "a" && new_text == "b"
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn real_driver_submit_against_fixture_agent_streams_tools_and_edits() {
    let fixture = fixture_agent();
    assert!(
        fixture.is_file(),
        "missing fixture agent at {}",
        fixture.display()
    );
    // Ensure executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fixture).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fixture, perms).unwrap();
    }

    let dir = tempfile::tempdir().unwrap();
    let mut driver = RealGrokAgentDriver::new(dir.path())
        .with_bin(&fixture)
        .with_read_timeout(Duration::from_secs(10));

    let (tx, _rx) = mpsc::unbounded_channel();
    let result = driver
        .submit_prompt(
            AgentPromptRequest {
                prompt: "fixture turn".into(),
                cwd: dir.path().to_path_buf(),
            },
            tx,
        )
        .await
        .expect("RealGrokAgentDriver.submit_prompt must succeed against fixture");

    assert!(
        result.events.iter().any(|e| matches!(
            e,
            AgentRuntimeEvent::AgentMessageChunk { text } if text.contains("search_replace") || text.contains("Applying")
        )),
        "streaming text missing: {:?}",
        result.events
    );
    assert!(
        result
            .events
            .iter()
            .any(|e| matches!(e, AgentRuntimeEvent::ToolCall { .. })),
        "tool call missing: {:?}",
        result.events
    );
    assert!(
        result.events.iter().any(|e| matches!(
            e,
            AgentRuntimeEvent::ProposedEdit {
                path,
                ..
            } if path.ends_with("fixture_edit.rs")
        )),
        "ProposedEdit missing from real driver path: {:?}",
        result.events
    );
    assert!(
        result
            .events
            .iter()
            .any(|e| matches!(e, AgentRuntimeEvent::TurnCompleted { .. })),
        "{:?}",
        result.events
    );
}

#[tokio::test]
async fn headless_dump_drives_real_driver_not_hardcoded_only() {
    let fixture = fixture_agent();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fixture).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fixture, perms).unwrap();
    }

    let opts = AppOptions {
        cwd: std::env::temp_dir(),
        dump_layout: true,
        auto_prompt: Some("headless real driver smoke".into()),
        allow_simulated_runtime: false, // require real driver (fixture)
        agent_bin: Some(fixture),
        agent_timeout_secs: 15,
    };
    let dump = run_headless_dump(&opts)
        .await
        .expect("headless dump with real driver");
    assert!(dump.contains("multi_pane: true"), "{dump}");
    assert!(dump.contains("Composer"), "{dump}");
    assert!(
        dump.contains("activity_entries:") && !dump.contains("activity_entries: 0"),
        "activity must come from real driver events:\n{dump}"
    );
    assert!(
        dump.contains("diff_items:") && !dump.contains("diff_items: 0"),
        "diffs must come from real driver ACP mapping:\n{dump}"
    );
    // Status should reflect agent completion, not only mock
    assert!(
        dump.contains("chat_messages:") && !dump.contains("chat_messages: 0"),
        "{dump}"
    );
}

#[tokio::test]
async fn require_agent_fails_without_binary() {
    let opts = AppOptions {
        cwd: std::env::temp_dir(),
        dump_layout: true,
        auto_prompt: Some("should fail".into()),
        allow_simulated_runtime: false,
        agent_bin: Some(PathBuf::from("/nonexistent/grok-agent-binary-xyz")),
        agent_timeout_secs: 5,
    };
    let err = run_headless_dump(&opts).await.expect_err("must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("require-agent") || msg.contains("not found") || msg.contains("failed"),
        "{msg}"
    );
}

#[test]
fn accept_reject_apply_writes_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apply_me.rs");
    std::fs::write(&path, "NEW").unwrap();
    apply_change_to_disk(&path, Some("OLD"), "NEW", false).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "OLD");
    apply_change_to_disk(&path, Some("OLD"), "NEW", true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "NEW");
}

#[tokio::test]
async fn accept_selected_change_dispatches_apply_hunk_effect_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let rel = PathBuf::from("src/disk_edit.rs");
    let abs = dir.path().join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, "fn old() {}").unwrap();

    let mut session = CursorSession::new(dir.path());
    session.reduce_all(bind_events(vec![AgentRuntimeEvent::ProposedEdit {
        edit_id: "disk-1".into(),
        path: rel,
        old_text: "fn old() {}".into(),
        new_text: "fn new() {}".into(),
    }]));
    // Agent already wrote new text
    std::fs::write(&abs, "fn new() {}").unwrap();

    let effects = session.reduce(CursorAction::RejectSelectedChange);
    assert!(
        effects.iter().any(|e| matches!(
            e,
            SessionEffect::ApplyHunkAction {
                action: HunkAction::Reject,
                ..
            }
        )),
        "{effects:?}"
    );
    // Simulate app loop apply path
    for e in &effects {
        if let SessionEffect::ApplyHunkAction { hunk_id, action } = e {
            let item = session
                .diffs
                .items
                .iter()
                .find(|i| i.id == *hunk_id)
                .unwrap();
            let path = dir.path().join(&item.path);
            apply_change_to_disk(
                &path,
                item.old_text.as_deref(),
                &item.new_text,
                matches!(action, HunkAction::Accept),
            )
            .unwrap();
        }
    }
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "fn old() {}");
}

#[test]
fn binary_dump_layout_entrypoint_when_built() {
    let bin = option_env!("CARGO_BIN_EXE_grok-build-cursor-cli");
    let Some(bin) = bin else {
        return;
    };
    let fixture = fixture_agent();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fixture).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&fixture, perms);
    }
    let output = Command::new(bin)
        .args([
            "--dump-layout",
            "--require-agent",
            "--agent-timeout",
            "15",
            "--prompt",
            "entry smoke",
            "--agent-bin",
        ])
        .arg(&fixture)
        .output()
        .expect("run binary");
    assert!(
        output.status.success(),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("multi_pane: true"), "{stdout}");
    assert!(stdout.contains("Composer"), "{stdout}");
    assert!(
        !stdout.contains("diff_items: 0"),
        "binary must surface diffs from real driver:\n{stdout}"
    );
}
