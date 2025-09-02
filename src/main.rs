use std::{io, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tracing_subscriber::EnvFilter;

mod app;
mod chains;
mod config;
mod pnl;
mod pricing;
mod ui;

#[derive(Parser, Debug)]
#[command(name = "hydra", version, about = "Cross-chain portfolio tracker for your terminal")]
struct Cli {
    /// Override path to config.toml
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// One-shot: print portfolio once and exit (no TUI)
    #[arg(long)]
    once: bool,

    /// Export holdings to CSV and exit
    #[arg(long)]
    export_csv: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;
    let cli = Cli::parse();

    let cfg = config::load(cli.config.as_deref()).context("loading config")?;
    let mut state = app::AppState::new(cfg).await?;

    if cli.once {
        state.refresh_now().await?;
        state.print_summary();
        return Ok(());
    }

    if let Some(path) = cli.export_csv {
        state.refresh_now().await?;
        state.export_csv(&path)?;
        println!("wrote {}", path.display());
        return Ok(());
    }

    run_tui(state).await
}

fn init_tracing() -> Result<()> {
    let dir = directories::ProjectDirs::from("dev", "hydra", "hydra-tracker")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).ok();
    let file = tracing_appender::rolling::daily(&dir, "hydra.log");
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(file)
        .with_ansi(false)
        .init();
    Ok(())
}

async fn run_tui(mut state: app::AppState) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Kick an immediate background refresh.
    state.spawn_refresh();

    let res = ui::event_loop(&mut terminal, &mut state, Duration::from_millis(100)).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    res
}
