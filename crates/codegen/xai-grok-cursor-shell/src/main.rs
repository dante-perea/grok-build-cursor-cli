//! `grok-build-cursor-cli` — Cursor Agents Home on Grok Build.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use clap::Parser;
use xai_grok_cursor_shell::app::{AppOptions, run_headless_dump, run_interactive};
use xai_grok_cursor_shell::history::AgentHistoryStore;
use xai_grok_cursor_shell::layout_home::HomeLayoutSnapshot;
use xai_grok_cursor_shell::projects::default_project_roots;
use xai_grok_cursor_shell::server::{ServerOptions, default_ui_dir, run_server};

#[derive(Debug, Parser)]
#[command(
    name = "grok-build-cursor-cli",
    about = "Cursor Agents Home UX on the Grok Build agent runtime",
    version
)]
struct Cli {
    /// Working directory / workspace root.
    #[arg(long, short = 'C', global = true)]
    cwd: Option<PathBuf>,

    /// Dump Agents Home layout JSON (verification / CI).
    #[arg(long)]
    dump_layout: bool,

    /// Legacy multipane ratatui shell.
    #[arg(long)]
    tui: bool,

    /// Auto-submit prompt (TUI / headless dump).
    #[arg(long)]
    prompt: Option<String>,

    /// Path to Grok Build agent binary.
    #[arg(long, env = "GROK_AGENT_BIN")]
    agent_bin: Option<PathBuf>,

    /// Require real agent binary (no simulated fallback).
    #[arg(long)]
    require_agent: bool,

    /// Max seconds for agent turn.
    #[arg(long, default_value = "90")]
    agent_timeout: u64,

    /// HTTP listen port (0 = ephemeral; default 9876).
    #[arg(long, default_value = "9876")]
    port: u16,

    /// Do not open a browser when starting the Agents Home server.
    #[arg(long)]
    no_open: bool,

    /// Directory for static UI (defaults to crate `ui/`).
    #[arg(long)]
    ui_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = cli
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    if cli.dump_layout {
        // Prefer Agents Home schema (product surface).
        if cli.prompt.is_none() && !cli.tui {
            let snap = HomeLayoutSnapshot::agents_home();
            println!("{}", snap.to_json_pretty());
            return Ok(());
        }
        let opts = AppOptions {
            cwd: cwd.clone(),
            dump_layout: true,
            auto_prompt: cli.prompt.clone(),
            allow_simulated_runtime: !cli.require_agent,
            agent_bin: cli.agent_bin.clone(),
            agent_timeout_secs: cli.agent_timeout,
        };
        let dump = run_headless_dump(&opts).await?;
        // Also print Agents Home product marker for CI.
        let home = HomeLayoutSnapshot::agents_home();
        println!("{}", home.to_json_pretty());
        eprintln!("--- session dump ---\n{dump}");
        return Ok(());
    }

    if cli.tui {
        let opts = AppOptions {
            cwd,
            dump_layout: false,
            auto_prompt: cli.prompt,
            allow_simulated_runtime: !cli.require_agent,
            agent_bin: cli.agent_bin,
            agent_timeout_secs: cli.agent_timeout,
        };
        return run_interactive(opts).await;
    }

    // Default: Cursor Agents Home web UI.
    let ui_dir = cli.ui_dir.unwrap_or_else(default_ui_dir);
    if !ui_dir.join("index.html").is_file() {
        anyhow::bail!(
            "UI not found at {} (expected index.html). Pass --ui-dir.",
            ui_dir.display()
        );
    }

    let port = cli.port;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let history_path = AgentHistoryStore::default_path();
    let opts = ServerOptions {
        cwd: cwd.clone(),
        ui_dir,
        agent_bin: cli.agent_bin,
        allow_simulated_runtime: !cli.require_agent,
        agent_timeout_secs: cli.agent_timeout,
        history_path,
        project_roots: default_project_roots(),
    };

    let url = format!("http://{addr}");
    eprintln!("grok-build-cursor-cli · Cursor Agents Home");
    eprintln!("  open {url}");
    eprintln!("  workspace {}", cwd.display());
    if !cli.no_open {
        let _ = open_browser(&url);
    }

    run_server(opts, addr)
        .await
        .with_context(|| format!("server on {addr}"))
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(url).spawn()?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Ok(())
    }
}
