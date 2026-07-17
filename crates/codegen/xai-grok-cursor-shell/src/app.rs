//! Interactive app loop for the Cursor-like shell.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::agent_bridge::{AgentRuntimeEvent, bind_events};
use crate::agent_driver::{AgentDriver, AgentPromptRequest, RealGrokAgentDriver, simulate_representative_turn};
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
    /// When agent binary is missing, still run representative tools/edits path
    /// through the shipped event bridge (launch probes).
    pub allow_simulated_runtime: bool,
    pub agent_bin: Option<PathBuf>,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            dump_layout: false,
            auto_prompt: None,
            allow_simulated_runtime: true,
            agent_bin: None,
        }
    }
}

/// Headless: construct session, optionally run a representative turn, dump layout.
pub fn run_headless_dump(opts: &AppOptions) -> Result<String> {
    let mut session = CursorSession::new(opts.cwd.clone());
    if let Some(prompt) = &opts.auto_prompt {
        session.reduce(CursorAction::ComposerInsertStr(prompt.clone()));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        let _ = effects;
        // Drive shipped agent event binding (representative tools/edits path).
        let events = simulate_representative_turn(prompt);
        let actions = bind_events(events);
        session.reduce_all(actions);
    }
    Ok(dump_layout_text(&session))
}

/// Interactive fullscreen multi-pane shell.
pub async fn run_interactive(opts: AppOptions) -> Result<()> {
    if opts.dump_layout {
        let dump = run_headless_dump(&opts)?;
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
    let mut driver = RealGrokAgentDriver::new(opts.cwd.clone());
    if let Some(bin) = opts.agent_bin.clone() {
        driver = driver.with_bin(bin);
    }

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentRuntimeEvent>();
    let mut agent_task: Option<tokio::task::JoinHandle<()>> = None;

    // Auto-submit if requested.
    if let Some(prompt) = opts.auto_prompt.clone() {
        session.reduce(CursorAction::ComposerInsertStr(prompt.clone()));
        let effects = session.reduce(CursorAction::ComposerSubmit);
        spawn_agent_for_effects(
            &mut session,
            &mut driver,
            &effects,
            event_tx.clone(),
            &mut agent_task,
            opts.allow_simulated_runtime,
        )
        .await;
    }

    let result = async {
        loop {
            terminal.draw(|f| draw_session(f, &session))?;

            // Drain agent events
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
                            spawn_agent_for_effects(
                                &mut session,
                                &mut driver,
                                &effects,
                                event_tx.clone(),
                                &mut agent_task,
                                opts.allow_simulated_runtime,
                            )
                            .await;
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

async fn spawn_agent_for_effects(
    session: &mut CursorSession,
    driver: &mut RealGrokAgentDriver,
    effects: &[SessionEffect],
    event_tx: mpsc::UnboundedSender<AgentRuntimeEvent>,
    agent_task: &mut Option<tokio::task::JoinHandle<()>>,
    allow_simulated: bool,
) {
    for effect in effects {
        if let SessionEffect::SubmitToAgent { prompt } = effect {
            let prompt = prompt.clone();
            let cwd = session.workspace.root.clone();
            let tx = event_tx.clone();
            let mut driver_clone = driver.clone();
            let allow_simulated = allow_simulated;

            // Cancel previous task if any (best-effort).
            if let Some(handle) = agent_task.take() {
                handle.abort();
            }

            *agent_task = Some(tokio::spawn(async move {
                let req = AgentPromptRequest {
                    prompt: prompt.clone(),
                    cwd,
                };
                match driver_clone.submit(req, tx.clone()).await {
                    Ok(_) => {}
                    Err(err) => {
                        let _ = tx.send(AgentRuntimeEvent::Status {
                            message: format!("Agent spawn failed: {err}"),
                        });
                        if allow_simulated {
                            let _ = tx.send(AgentRuntimeEvent::Status {
                                message: "Falling back to representative runtime event stream (agent binary unavailable)".into(),
                            });
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
    }
}

fn map_key(
    session: &CursorSession,
    code: KeyCode,
    mods: KeyModifiers,
) -> Vec<CursorAction> {
    // Global shortcuts
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
            KeyCode::Char(c) => vec![CursorAction::ComposerInsertChar(c)], // type to jump composer? keep simple
            _ => vec![],
        },
        FocusPane::Workspace => match code {
            KeyCode::Down | KeyCode::Char('j') => vec![CursorAction::WorkspaceSelectNext],
            KeyCode::Up | KeyCode::Char('k') => vec![CursorAction::WorkspaceSelectPrev],
            KeyCode::Enter => vec![CursorAction::WorkspaceOpenSelected],
            KeyCode::Char(c) => {
                // Typing focuses composer for quick prompts.
                vec![
                    CursorAction::Focus(FocusPane::Composer),
                    CursorAction::ComposerInsertChar(c),
                ]
            }
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
