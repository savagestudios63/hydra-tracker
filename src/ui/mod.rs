mod holdings;
mod layout;
mod pnl_tab;
mod transactions;

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::Backend, Terminal};

use crate::app::{AppState, InputMode, PnlRange, SortMode, Tab};

pub async fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    tick_rate: Duration,
) -> Result<()> {
    let mut last_tick = Instant::now();
    let mut last_auto_refresh = Instant::now();
    let auto_every = Duration::from_secs(state.cfg.general.refresh_interval.max(1));
    let auto_enabled = state.cfg.general.refresh_interval > 0;

    loop {
        terminal.draw(|f| layout::render(f, state))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if let ControlFlow::Quit = handle_key(k, state) {
                        return Ok(());
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if last_tick.elapsed() >= tick_rate { last_tick = Instant::now(); }

        state.drain_refreshes();

        if auto_enabled && last_auto_refresh.elapsed() >= auto_every {
            state.spawn_refresh();
            last_auto_refresh = Instant::now();
        }
    }
}

enum ControlFlow { Continue, Quit }

fn handle_key(key: KeyEvent, state: &mut AppState) -> ControlFlow {
    match state.ui.input_mode {
        InputMode::Normal => handle_normal(key, state),
        InputMode::Filter => handle_filter(key, state),
        InputMode::Command => handle_command(key, state),
    }
}

fn handle_normal(key: KeyEvent, state: &mut AppState) -> ControlFlow {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => return ControlFlow::Quit,
        KeyCode::Char('c') if ctrl => return ControlFlow::Quit,
        KeyCode::Char('r') => state.spawn_refresh(),
        KeyCode::Char('R') => {
            // Force-refresh AND re-load history from disk.
            let hist = crate::pnl::load_history(&state.cfg);
            state.inner().write().history = hist;
            state.spawn_refresh();
        }
        KeyCode::Tab => cycle_tab(state, 1),
        KeyCode::BackTab => cycle_tab(state, -1),
        KeyCode::Char('1') => state.ui.active_tab = Tab::Holdings,
        KeyCode::Char('2') => state.ui.active_tab = Tab::Transactions,
        KeyCode::Char('3') => state.ui.active_tab = Tab::Pnl,
        KeyCode::Char('j') | KeyCode::Down => move_cursor(state, 1),
        KeyCode::Char('k') | KeyCode::Up => move_cursor(state, -1),
        KeyCode::Char('h') | KeyCode::Left => move_wallet(state, -1),
        KeyCode::Char('l') | KeyCode::Right => move_wallet(state, 1),
        KeyCode::Char('G') => move_cursor_to_end(state),
        KeyCode::Char('g') => {
            if state.ui.last_keys.ends_with('g') {
                move_cursor_to_top(state);
                state.ui.last_keys.clear();
            } else {
                state.ui.last_keys = "g".into();
                return ControlFlow::Continue;
            }
        }
        KeyCode::Char('/') => {
            state.ui.input_mode = InputMode::Filter;
            state.ui.filter.clear();
        }
        KeyCode::Char(':') => {
            state.ui.input_mode = InputMode::Command;
            state.ui.command_buffer.clear();
        }
        KeyCode::Char('s') => cycle_sort(state),
        KeyCode::Char('w') => state.ui.selected_wallet_only = !state.ui.selected_wallet_only,
        KeyCode::Char('7') => state.ui.pnl_range = PnlRange::Week,
        KeyCode::Char('0') => state.ui.pnl_range = PnlRange::Month,
        KeyCode::Char('a') => state.ui.pnl_range = PnlRange::All,
        KeyCode::Esc => { state.ui.status_message = None; }
        _ => {}
    }
    if !matches!(key.code, KeyCode::Char('g')) {
        state.ui.last_keys.clear();
    }
    ControlFlow::Continue
}

fn handle_filter(key: KeyEvent, state: &mut AppState) -> ControlFlow {
    match key.code {
        KeyCode::Esc => {
            state.ui.input_mode = InputMode::Normal;
            state.ui.filter.clear();
        }
        KeyCode::Enter => state.ui.input_mode = InputMode::Normal,
        KeyCode::Backspace => { state.ui.filter.pop(); }
        KeyCode::Char(c) => state.ui.filter.push(c),
        _ => {}
    }
    ControlFlow::Continue
}

fn handle_command(key: KeyEvent, state: &mut AppState) -> ControlFlow {
    match key.code {
        KeyCode::Esc => {
            state.ui.input_mode = InputMode::Normal;
            state.ui.command_buffer.clear();
        }
        KeyCode::Enter => {
            let cmd = state.ui.command_buffer.trim().to_string();
            state.ui.command_buffer.clear();
            state.ui.input_mode = InputMode::Normal;
            if cmd == "q" || cmd == "quit" { return ControlFlow::Quit; }
            run_command(&cmd, state);
        }
        KeyCode::Backspace => { state.ui.command_buffer.pop(); }
        KeyCode::Char(c) => state.ui.command_buffer.push(c),
        _ => {}
    }
    ControlFlow::Continue
}

fn run_command(cmd: &str, state: &mut AppState) {
    // Supported: `export csv [path]`, `add wallet <label> <chain> <address>`,
    // `refresh`. Anything else => unknown.
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.as_slice() {
        ["refresh"] => state.spawn_refresh(),
        ["export", "csv", rest @ ..] => {
            let path = rest.first().map(|s| s.to_string()).unwrap_or_else(|| "holdings.csv".into());
            match state.export_csv(std::path::Path::new(&path)) {
                Ok(_) => state.ui.status_message = Some(format!("wrote {}", path)),
                Err(e) => state.ui.status_message = Some(format!("csv export failed: {}", e)),
            }
        }
        ["add", "wallet", label, chain, addr] => {
            let chain_parsed = match *chain {
                "solana" => Some(crate::config::Chain::Solana),
                "ethereum" => Some(crate::config::Chain::Ethereum),
                "base" => Some(crate::config::Chain::Base),
                "arbitrum" => Some(crate::config::Chain::Arbitrum),
                _ => None,
            };
            match chain_parsed {
                Some(c) => {
                    state.cfg.wallets.push(crate::config::Wallet {
                        label: (*label).to_string(),
                        chain: c,
                        address: (*addr).to_string(),
                    });
                    state.spawn_refresh();
                    state.ui.status_message = Some(format!("added wallet {}", label));
                }
                None => state.ui.status_message = Some(format!("unknown chain: {}", chain)),
            }
        }
        _ => state.ui.status_message = Some(format!("unknown command: {}", cmd)),
    }
}

fn cycle_tab(state: &mut AppState, dir: i32) {
    let tabs = [Tab::Holdings, Tab::Transactions, Tab::Pnl];
    let idx = tabs.iter().position(|t| *t == state.ui.active_tab).unwrap_or(0) as i32;
    let n = tabs.len() as i32;
    let next = ((idx + dir).rem_euclid(n)) as usize;
    state.ui.active_tab = tabs[next];
}

fn cycle_sort(state: &mut AppState) {
    state.ui.sort_mode = match state.ui.sort_mode {
        SortMode::Value => SortMode::Change24h,
        SortMode::Change24h => SortMode::Pnl,
        SortMode::Pnl => SortMode::Balance,
        SortMode::Balance => SortMode::Symbol,
        SortMode::Symbol => SortMode::Value,
    };
}

fn move_cursor(state: &mut AppState, dir: i32) {
    let inner = state.inner();
    let g = inner.read();
    match state.ui.active_tab {
        Tab::Holdings => {
            let n = holdings::visible_rows(state, &g).len();
            adjust(&mut state.ui.holdings_cursor, dir, n);
        }
        Tab::Transactions => {
            let n = transactions::visible_rows(state, &g).len();
            adjust(&mut state.ui.transactions_cursor, dir, n);
        }
        Tab::Pnl => {}
    }
}

fn adjust(cursor: &mut usize, dir: i32, n: usize) {
    if n == 0 { *cursor = 0; return; }
    let cur = *cursor as i32 + dir;
    let clamped = cur.clamp(0, (n - 1) as i32);
    *cursor = clamped as usize;
}

fn move_cursor_to_top(state: &mut AppState) {
    match state.ui.active_tab {
        Tab::Holdings => state.ui.holdings_cursor = 0,
        Tab::Transactions => state.ui.transactions_cursor = 0,
        _ => {}
    }
}

fn move_cursor_to_end(state: &mut AppState) {
    let inner = state.inner();
    let g = inner.read();
    match state.ui.active_tab {
        Tab::Holdings => {
            let n = holdings::visible_rows(state, &g).len();
            if n > 0 { state.ui.holdings_cursor = n - 1; }
        }
        Tab::Transactions => {
            let n = transactions::visible_rows(state, &g).len();
            if n > 0 { state.ui.transactions_cursor = n - 1; }
        }
        _ => {}
    }
}

fn move_wallet(state: &mut AppState, dir: i32) {
    let n = state.cfg.wallets.len();
    if n == 0 { return; }
    let cur = state.ui.wallet_cursor as i32 + dir;
    state.ui.wallet_cursor = cur.rem_euclid(n as i32) as usize;
}
