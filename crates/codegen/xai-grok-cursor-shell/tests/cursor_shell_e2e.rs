//! Integration tests for the shipped Cursor shell (layout, submit, activity, diffs).

use std::path::PathBuf;
use std::process::Command;

use xai_grok_cursor_shell::agent_bridge::{AgentRuntimeEvent, ToolCallPhase, bind_events};
use xai_grok_cursor_shell::agent_driver::{map_agent_line, simulate_representative_turn};
use xai_grok_cursor_shell::app::{AppOptions, run_headless_dump};
use xai_grok_cursor_shell::diff_review::ChangeDecision;
use xai_grok_cursor_shell::layout::FocusPane;
use xai_grok_cursor_shell::session::{CursorAction, CursorSession, SessionEffect};
use xai_hunk_tracker::{Hunk, HunkEvent, HunkSource};

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
    assert_eq!(session.chat.messages[0].content, "add logging to the auth module");
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
    assert!(preview.contains("-fn a() {}") || preview.contains("fn a()"));

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
fn headless_dump_and_representative_turn() {
    let opts = AppOptions {
        cwd: std::env::temp_dir(),
        dump_layout: true,
        auto_prompt: Some("smoke prompt for cursor shell".into()),
        allow_simulated_runtime: true,
        agent_bin: None,
    };
    let dump = run_headless_dump(&opts).expect("dump");
    assert!(dump.contains("multi_pane: true"), "{dump}");
    assert!(dump.contains("Composer"), "{dump}");
    assert!(dump.contains("Activity"), "{dump}");
    assert!(dump.contains("Diff Review"), "{dump}");
    assert!(
        dump.contains("diff_items:") && !dump.contains("diff_items: 0"),
        "expected diffs from representative turn:\n{dump}"
    );
    assert!(
        dump.contains("activity_entries:") && !dump.contains("activity_entries: 0"),
        "{dump}"
    );

    // Also exercise the representative event factory directly.
    let events = simulate_representative_turn("x");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentRuntimeEvent::ProposedEdit { .. }))
    );
}

#[test]
fn binary_dump_layout_entrypoint_when_built() {
    // Prefer cargo-built binary from this package if present in PATH-like target.
    // This test is a structural/process check: if the binary isn't built yet,
    // we only assert the library dump path above. When CARGO_BIN_EXE is set
    // (cargo test), drive the real entrypoint.
    let bin = option_env!("CARGO_BIN_EXE_grok-build-cursor-cli");
    let Some(bin) = bin else {
        return;
    };
    let output = Command::new(bin)
        .args(["--dump-layout", "--prompt", "entry smoke"])
        .output()
        .expect("run binary");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("multi_pane: true"), "{stdout}");
    assert!(stdout.contains("Composer"), "{stdout}");
}
