//! Interactive app loop for the Cursor-like shell.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use xai_hunk_tracker::HunkAction;

use crate::agent_bridge::{AgentRuntimeEvent, bind_events};
use crate::agent_driver::{
    AgentPromptRequest, RealGrokAgentDriver, apply_change_to_disk, simulate_representative_turn,
};
use crate::layout::FocusPane;
use crate::render::{draw_session, dump_layout_text};
use crate::session::{CursorAction, CursorSession, SessionEffect};

/// CLI / launch options.
#[derive(Debug, Clone)]
pub struct AppOptions {
    pub cwd: PathBuf,
    /// If true, run one headless layout dump and exit (for verification).
    pub dump_layout: bool,
    /// Optional prompt to auto-submit once (demo / smoke).
    pub auto_prompt: Option<String>,
    /// When agent binary is missing, allow representative fallback events.
    /// When false (`--require-agent`), failure to resolve/spawn the real agent is an error.
    pub allow_simulated_runtime: bool,
    pub agent_bin: Option<PathBuf>,
    /// Max seconds for headless agent turn (default 90).
    pub agent_timeout_secs: u64,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            dump_layout: false,
            auto_prompt: None,
            allow_simulated_runtime: true,
            agent_bin: None,
            agent_timeout_secs: 90,
        }
    }
}

/// Headless: construct session, optionally drive **RealGrokAgentDriver**, dump layout.
///
/// When `auto_prompt` is set, Composer submit runs and the real agent driver is
/// invoked (ACP stdio). Simulated events are only used if the binary cannot be
/// resolved **and** `allow_simulated_runtime` is true. With `--require-agent`,
/// missing/failed agent is a hard error (no fake activity/diffs).
pub async fn run_headless_dump(opts: &AppOptions) -> Result<String> {
    let mut session = CursorSession::new(opts.cwd.clone());
    if let Some(prompt) = &opts.auto_prompt {
        session.reduce(CursorAction::ComposerInsertStr(prompt.clone()));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        apply_effects_headless(&mut session, opts, &effects).await?;
    }
    Ok(dump_layout_text(&session))
}

/// Sync wrapper for callers that already run inside a tokio runtime or want
/// `block_on` themselves. Prefer [`run_headless_dump`].
pub fn run_headless_dump_blocking(opts: &AppOptions) -> Result<String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    rt.block_on(run_headless_dump(opts))
}

async fn apply_effects_headless(
    session: &mut CursorSession,
    opts: &AppOptions,
    effects: &[SessionEffect],
) -> Result<()> {
    for effect in effects {
        match effect {
            SessionEffect::SubmitToAgent { prompt } => {
                let events = drive_real_agent(session, opts, prompt).await?;
                let actions = bind_events(events);
                session.reduce_all(actions);
            }
            SessionEffect::ApplyHunkAction { hunk_id, action } => {
                apply_hunk_effect(session, hunk_id, *action)?;
            }
            SessionEffect::Quit | SessionEffect::Redraw => {}
        }
    }
    Ok(())
}

/// Spawn RealGrokAgentDriver and collect streamed events.
async fn drive_real_agent(
    session: &CursorSession,
    opts: &AppOptions,
    prompt: &str,
) -> Result<Vec<AgentRuntimeEvent>> {
    let mut driver = RealGrokAgentDriver::new(session.workspace.root.clone())
        .with_read_timeout(Duration::from_secs(opts.agent_timeout_secs));
    if let Some(bin) = &opts.agent_bin {
        driver = driver.with_bin(bin.clone());
    }

    // Fail fast when agent is required and binary cannot be resolved.
    if !opts.allow_simulated_runtime {
        driver
            .resolve_bin()
            .context("require-agent: Grok Build agent binary not found")?;
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let req = AgentPromptRequest {
        prompt: prompt.to_string(),
        cwd: session.workspace.root.clone(),
    };

    match driver.submit_prompt(req, tx).await {
        Ok(result) => {
            // Drain any residual (already included in result.events via collect).
            while rx.try_recv().is_ok() {}
            Ok(result.events)
        }
        Err(err) => {
            if opts.allow_simulated_runtime {
                let mut events = vec![
                    AgentRuntimeEvent::Status {
                        message: format!("Agent spawn failed ({err}); using representative fallback"),
                    },
                ];
                events.extend(simulate_representative_turn(prompt));
                Ok(events)
            } else {
                bail!("require-agent: real agent failed: {err}");
            }
        }
    }
}

fn apply_hunk_effect(
    session: &mut CursorSession,
    hunk_id: &str,
    action: HunkAction,
) -> Result<()> {
    let accept = matches!(action, HunkAction::Accept);
    let item = session
        .diffs
        .items
        .iter()
        .find(|i| i.id == hunk_id)
        .cloned();
    let Some(item) = item else {
        session
            .activity
            .push_status(format!("hunk apply: id {hunk_id} not found in review list"));
        return Ok(());
    };
    let path = if item.path.is_absolute() {
        item.path.clone()
    } else {
        session.workspace.root.join(&item.path)
    };
    apply_change_to_disk(
        &path,
        item.old_text.as_deref(),
        &item.new_text,
        accept,
    )
    .with_context(|| format!("apply {:?} to {}", action, path.display()))?;
    session.activity.push_status(format!(
        "{} {} ({})",
        if accept { "accepted" } else { "rejected" },
        path.display(),
        item.summary
    ));
    // Refresh editor buffer if this file is open.
    if session.workspace.open_path.as_ref() == Some(&path) {
        let _ = session.workspace.open_path(path);
    }
    Ok(())
}

/// Interactive fullscreen multi-pane shell.
pub async fn run_interactive(opts: AppOptions) -> Result<()> {
    if opts.dump_layout {
        let dump = run_headless_dump(&opts).await?;
        print!("{dump}");
        io::stdout().flush()?;
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut session = CursorSession::new(opts.cwd.clone());
    let mut driver = RealGrokAgentDriver::new(opts.cwd.clone())
        .with_read_timeout(Duration::from_secs(opts.agent_timeout_secs));
    if let Some(bin) = opts.agent_bin.clone() {
        driver = driver.with_bin(bin);
    }

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentRuntimeEvent>();
    let mut agent_task: Option<tokio::task::JoinHandle<()>> = None;

    if let Some(prompt) = opts.auto_prompt.clone() {
        session.reduce(CursorAction::ComposerInsertStr(prompt));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        dispatch_effects(
            &mut session,
            &mut driver,
            &effects,
            event_tx.clone(),
            &mut agent_task,
            opts.allow_simulated_runtime,
        )
        .await?;
    }

    let result = async {
        loop {
            terminal.draw(|f| draw_session(f, &session))?;

            while let Ok(ev) = event_rx.try_recv() {
                let actions = bind_events(std::iter::once(ev));
                session.reduce_all(actions);
            }

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
                        {
                            break;
                        }
                        let actions = map_key(&session, key.code, key.modifiers);
                        let mut quit = false;
                        for action in actions {
                            let effects = session.reduce(action);
                            if effects.iter().any(|e| matches!(e, SessionEffect::Quit)) {
                                quit = true;
                            }
                            dispatch_effects(
                                &mut session,
                                &mut driver,
                                &effects,
                                event_tx.clone(),
                                &mut agent_task,
                                opts.allow_simulated_runtime,
                            )
                            .await?;
                        }
                        if quit {
                            break;
                        }
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn dispatch_effects(
    session: &mut CursorSession,
    driver: &mut RealGrokAgentDriver,
    effects: &[SessionEffect],
    event_tx: mpsc::UnboundedSender<AgentRuntimeEvent>,
    agent_task: &mut Option<tokio::task::JoinHandle<()>>,
    allow_simulated: bool,
) -> Result<()> {
    for effect in effects {
        match effect {
            SessionEffect::SubmitToAgent { prompt } => {
                let prompt = prompt.clone();
                let cwd = session.workspace.root.clone();
                let tx = event_tx.clone();
                let mut driver_clone = driver.clone();
                if let Some(handle) = agent_task.take() {
                    handle.abort();
                }
                *agent_task = Some(tokio::spawn(async move {
                    let req = AgentPromptRequest {
                        prompt: prompt.clone(),
                        cwd,
                    };
                    match driver_clone.submit_prompt(req, tx.clone()).await {
                        Ok(_) => {}
                        Err(err) => {
                            let _ = tx.send(AgentRuntimeEvent::Status {
                                message: format!("Agent spawn failed: {err}"),
                            });
                            if allow_simulated {
                                for ev in simulate_representative_turn(&prompt) {
                                    let _ = tx.send(ev);
                                }
                            } else {
                                let _ = tx.send(AgentRuntimeEvent::Error {
                                    message: err.to_string(),
                                });
                                let _ = tx.send(AgentRuntimeEvent::TurnCompleted { ok: false });
                            }
                        }
                    }
                }));
            }
            SessionEffect::ApplyHunkAction { hunk_id, action } => {
                apply_hunk_effect(session, hunk_id, *action)?;
            }
            SessionEffect::Quit | SessionEffect::Redraw => {}
        }
    }
    Ok(())
}

fn map_key(
    session: &CursorSession,
    code: KeyCode,
    mods: KeyModifiers,
) -> Vec<CursorAction> {
    match code {
        KeyCode::Tab if !mods.contains(KeyModifiers::SHIFT) => {
            return vec![CursorAction::CycleFocusForward];
        }
        KeyCode::BackTab | KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => {
            return vec![CursorAction::CycleFocusBackward];
        }
        KeyCode::Esc => return vec![CursorAction::Quit],
        _ => {}
    }

    match session.layout.focus {
        FocusPane::Composer => match code {
            KeyCode::Enter => vec![CursorAction::ComposerSubmit],
            KeyCode::Backspace => vec![CursorAction::ComposerBackspace],
            KeyCode::Char(c) => vec![CursorAction::ComposerInsertChar(c)],
            _ => vec![],
        },
        FocusPane::DiffReview => match code {
            KeyCode::Char('a') | KeyCode::Char('y') => vec![CursorAction::AcceptSelectedChange],
            KeyCode::Char('r') | KeyCode::Char('n') => vec![CursorAction::RejectSelectedChange],
            KeyCode::Down | KeyCode::Char('j') => vec![CursorAction::DiffSelectNext],
            KeyCode::Up | KeyCode::Char('k') => vec![CursorAction::DiffSelectPrev],
            _ => vec![],
        },
        FocusPane::Workspace => match code {
            KeyCode::Down | KeyCode::Char('j') => vec![CursorAction::WorkspaceSelectNext],
            KeyCode::Up | KeyCode::Char('k') => vec![CursorAction::WorkspaceSelectPrev],
            KeyCode::Enter => vec![CursorAction::WorkspaceOpenSelected],
            KeyCode::Char(c) => vec![
                CursorAction::Focus(FocusPane::Composer),
                CursorAction::ComposerInsertChar(c),
            ],
            _ => vec![],
        },
        FocusPane::Chat | FocusPane::Activity => match code {
            KeyCode::Char(c) => vec![
                CursorAction::Focus(FocusPane::Composer),
                CursorAction::ComposerInsertChar(c),
            ],
            KeyCode::Enter => vec![CursorAction::Focus(FocusPane::Composer)],
            _ => vec![],
        },
    }
}
