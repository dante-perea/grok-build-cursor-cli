//! HTTP control-plane tests for Cursor Agents Home.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::{Mutex, broadcast};
use tower::ServiceExt;
use xai_grok_cursor_shell::history::AgentHistoryStore;
use xai_grok_cursor_shell::server::{
    AppStateInner, ServerOptions, build_router, default_ui_dir,
};
use xai_grok_cursor_shell::session::CursorSession;

fn fixture_agent() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-grok-agent.sh")
}

fn test_state(tmp: &tempfile::TempDir, require_agent: bool) -> Arc<Mutex<AppStateInner>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture_agent();
        if f.is_file() {
            let mut p = std::fs::metadata(&f).unwrap().permissions();
            p.set_mode(0o755);
            let _ = std::fs::set_permissions(&f, p);
        }
    }
    let (tx, _) = broadcast::channel(16);
    let opts = ServerOptions {
        cwd: tmp.path().to_path_buf(),
        ui_dir: default_ui_dir(),
        agent_bin: Some(fixture_agent()),
        allow_simulated_runtime: !require_agent,
        agent_timeout_secs: 15,
        history_path: tmp.path().join("sessions.json"),
    };
    Arc::new(Mutex::new(AppStateInner {
        session: CursorSession::new(tmp.path()),
        history: AgentHistoryStore::new(tmp.path().join("sessions.json")),
        opts,
        tx,
    }))
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({}))
}

#[tokio::test]
async fn health_and_index() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(&tmp, false);
    let app = build_router(state, default_ui_dir());

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["product"], "cursor-agents-home");

    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&bytes);
    assert!(
        html.contains("data-testid=\"floating_composer\"")
            || html.contains("floating_composer")
            || html.contains("Plan and design"),
        "index must be Agents Home UI, got len={}",
        html.len()
    );
    assert!(
        !html.contains("file_tree_primary"),
        "must not be file-tree IDE primary"
    );
}

#[tokio::test]
async fn snapshot_is_agents_home() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(&tmp, false);
    let app = build_router(state, default_ui_dir());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["view"], "home");
    assert_eq!(v["layout"]["product"], "cursor-agents-home");
    assert_eq!(v["layout"]["show_file_tree_primary"], false);
}

#[tokio::test]
async fn prompt_drives_real_fixture_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(&tmp, true);
    let app = build_router(state, default_ui_dir());

    let body = serde_json::json!({
        "prompt": "http e2e edit",
        "plan_mode": false
    });
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/prompt")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["view"], "session");
    assert!(
        v["chat"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "chat: {v}"
    );
    assert!(
        v["diffs"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "diffs from fixture agent: {v}"
    );
    assert!(
        v["activity"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "activity: {v}"
    );
    // History recorded
    assert!(
        v["history"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "history: {v}"
    );
}

#[tokio::test]
async fn diff_reject_restores_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("edit_me.rs");
    std::fs::write(&path, "NEW").unwrap();

    let state = test_state(&tmp, false);
    // Seed a proposed edit via session
    {
        use xai_grok_cursor_shell::agent_bridge::{AgentRuntimeEvent, bind_events};
        use xai_grok_cursor_shell::session::CursorAction;
        let mut g = state.lock().await;
        g.session.reduce(CursorAction::ComposerInsertStr("x".into()));
        let _ = g.session.reduce(CursorAction::ComposerSubmit);
        g.session.reduce_all(bind_events(vec![AgentRuntimeEvent::ProposedEdit {
            edit_id: "disk-e".into(),
            path: PathBuf::from("edit_me.rs"),
            old_text: "OLD".into(),
            new_text: "NEW".into(),
        }]));
    }

    let app = build_router(state, default_ui_dir());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/diff/disk-e/reject")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Allow FS settle
    tokio::time::sleep(Duration::from_millis(20)).await;
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "OLD");
}
