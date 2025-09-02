//! Transactions tab: reverse-chronological feed with category filter via `/`.

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table, TableState},
    Frame,
};

use crate::app::{AppState, Inner};
use crate::chains::{Transaction, TxCategory};

pub fn visible_rows<'a>(state: &AppState, g: &'a Inner) -> Vec<&'a Transaction> {
    let wallet_filter = if state.ui.selected_wallet_only {
        state.cfg.wallets.get(state.ui.wallet_cursor).map(|w| w.label.clone())
    } else { None };

    let filter_lower = state.ui.filter.to_ascii_lowercase();
    let mut rows: Vec<&Transaction> = g.transactions.iter().filter(|t| {
        if let Some(w) = &wallet_filter { if t.wallet_label != *w { return false; } }
        if !filter_lower.is_empty() {
            let hay = format!("{} {} {} {}", t.summary, t.wallet_label, t.chain.short(), category_label(t.category)).to_ascii_lowercase();
            if !hay.contains(&filter_lower) { return false; }
        }
        true
    }).collect();
    rows.sort_by_key(|t| std::cmp::Reverse(t.timestamp));
    rows
}

pub fn render(f: &mut Frame, state: &mut AppState, area: Rect) {
    let inner = state.inner();
    let g = inner.read();
    let rows = visible_rows(state, &g);

    let header = Row::new(vec![
        Cell::from("When"),
        Cell::from("Wallet"),
        Cell::from("Chain"),
        Cell::from("Type"),
        Cell::from("Summary"),
    ]).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let table_rows: Vec<Row> = rows.iter().map(|t| {
        let when = relative_time(t.timestamp);
        let cat = category_label(t.category);
        Row::new(vec![
            Cell::from(when).style(Style::default().fg(Color::DarkGray)),
            Cell::from(truncate(&t.wallet_label, 12)),
            Cell::from(t.chain.short()).style(Style::default().fg(Color::DarkGray)),
            Cell::from(cat).style(category_style(t.category)),
            Cell::from(truncate(&t.summary, 80)),
        ])
    }).collect();

    let title = format!(" Transactions — {} ", rows.len());
    let table = Table::new(table_rows, [
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(6),
        Constraint::Length(14),
        Constraint::Min(20),
    ])
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 40)));

    let mut tstate = TableState::default();
    if !rows.is_empty() {
        tstate.select(Some(state.ui.transactions_cursor.min(rows.len().saturating_sub(1))));
    }
    f.render_stateful_widget(table, area, &mut tstate);
}

fn category_label(c: TxCategory) -> &'static str {
    match c {
        TxCategory::Swap => "swap",
        TxCategory::Transfer => "transfer",
        TxCategory::Stake => "stake",
        TxCategory::Unstake => "unstake",
        TxCategory::LiquidityAdd => "lp add",
        TxCategory::LiquidityRemove => "lp remove",
        TxCategory::Unknown => "unknown",
    }
}

fn category_style(c: TxCategory) -> Style {
    match c {
        TxCategory::Swap => Style::default().fg(Color::Magenta),
        TxCategory::Transfer => Style::default().fg(Color::Blue),
        TxCategory::Stake | TxCategory::LiquidityAdd => Style::default().fg(Color::Green),
        TxCategory::Unstake | TxCategory::LiquidityRemove => Style::default().fg(Color::Yellow),
        TxCategory::Unknown => Style::default().fg(Color::DarkGray),
    }
}

fn relative_time(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    if secs < 60 { format!("{}s ago", secs) }
    else if secs < 3600 { format!("{}m ago", secs / 60) }
    else if secs < 86_400 { format!("{}h ago", secs / 3600) }
    else { format!("{}d ago", secs / 86_400) }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
