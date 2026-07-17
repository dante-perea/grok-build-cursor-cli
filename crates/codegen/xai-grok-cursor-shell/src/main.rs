//! `grok-build-cursor-cli` — Cursor-like multi-pane shell on Grok Build.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use xai_grok_cursor_shell::app::{AppOptions, run_headless_dump, run_interactive};

#[derive(Debug, Parser)]
#[command(
    name = "grok-build-cursor-cli",
    about = "Cursor-like multi-pane UX on the Grok Build agent runtime",
    version
)]
struct Cli {
    /// Working directory / workspace root.
    #[arg(long, short = 'C', global = true)]
    cwd: Option<PathBuf>,

    /// Dump multi-pane layout structure to stdout and exit (verification / CI).
    #[arg(long)]
    dump_layout: bool,

    /// Auto-submit this prompt once on launch (smoke / demo).
    #[arg(long)]
    prompt: Option<String>,

    /// Path to Grok Build agent binary (`xai-grok-pager` or `grok`).
    #[arg(long, env = "GROK_AGENT_BIN")]
    agent_bin: Option<PathBuf>,

    /// Disable simulated fallback; require a real agent binary (hard error if missing).
    #[arg(long)]
    require_agent: bool,

    /// Max seconds for headless agent turn (default 90).
    #[arg(long, default_value = "90")]
    agent_timeout: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = cli
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let opts = AppOptions {
        cwd,
        dump_layout: cli.dump_layout,
        auto_prompt: cli.prompt,
        allow_simulated_runtime: !cli.require_agent,
        agent_bin: cli.agent_bin,
        agent_timeout_secs: cli.agent_timeout,
    };

    if opts.dump_layout {
        let dump = run_headless_dump(&opts).await?;
        print!("{dump}");
        return Ok(());
    }

    run_interactive(opts).await
}
