//! Holdings tab: sortable table of (symbol, chain, balance, value, 24h%, pnl).

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Table, TableState},
    Frame,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::app::{AppState, Inner, SortMode};
use crate::chains::Holding;

pub struct Row2 {
    pub holding: Holding,
    pub price_usd: Option<Decimal>,
    pub value_usd: Option<Decimal>,
    pub change_24h: Option<f64>,
    pub unrealized_pnl: Option<Decimal>,
}

pub fn visible_rows(state: &AppState, g: &Inner) -> Vec<Row2> {
    let wallet_filter = if state.ui.selected_wallet_only {
        state.cfg.wallets.get(state.ui.wallet_cursor).map(|w| w.label.clone())
    } else { None };

    let filter_lower = state.ui.filter.to_ascii_lowercase();

    let mut rows: Vec<Row2> = g.holdings.iter().filter(|h| {
        if let Some(w) = &wallet_filter { if h.wallet_label != *w { return false; } }
        if !filter_lower.is_empty() {
            let hay = format!("{} {} {} {}", h.symbol, h.name, h.wallet_label, h.chain.short()).to_ascii_lowercase();
            if !hay.contains(&filter_lower) { return false; }
        }
        true
    }).map(|h| {
        let price = g.prices.get(&h.price_key());
        let price_usd = price.map(|p| p.usd);
        let value_usd = price_usd.map(|p| p * h.amount);
        let change_24h = price.and_then(|p| p.change_24h_pct);

        let lot_key = format!("{}|{}|{}", h.wallet_label, h.chain.short(), h.symbol);
        let unrealized = g.pnl.lots.get(&lot_key).and_then(|lot| {
            price_usd.map(|px| (px - lot.avg_cost_usd) * h.amount.min(lot.qty))
        });

        Row2 {
            holding: h.clone(),
            price_usd,
            value_usd,
            change_24h,
            unrealized_pnl: unrealized,
        }
    }).collect();

    // Dust filter.
    let dust = Decimal::from_f64_retain(state.cfg.general.dust_threshold).unwrap_or(Decimal::ZERO);
    if dust > Decimal::ZERO {
        rows.retain(|r| r.value_usd.unwrap_or(Decimal::ZERO) >= dust);
    }

    sort_rows(&mut rows, state.ui.sort_mode);
    rows
}

fn sort_rows(rows: &mut [Row2], mode: SortMode) {
    use std::cmp::Ordering::*;
    rows.sort_by(|a, b| match mode {
        SortMode::Value => b.value_usd.unwrap_or_default().cmp(&a.value_usd.unwrap_or_default()),
        SortMode::Balance => b.holding.amount.cmp(&a.holding.amount),
        SortMode::Change24h => match (b.change_24h, a.change_24h) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Equal),
            (Some(_), None) => Less,
            (None, Some(_)) => Greater,
            _ => Equal,
        },
        SortMode::Pnl => b.unrealized_pnl.unwrap_or_default().cmp(&a.unrealized_pnl.unwrap_or_default()),
        SortMode::Symbol => a.holding.symbol.cmp(&b.holding.symbol),
    });
}

pub fn render(f: &mut Frame, state: &mut AppState, area: Rect) {
    let inner = state.inner();
    let g = inner.read();
    let rows = visible_rows(state, &g);

    let header = Row::new(vec![
        Cell::from("Token"),
        Cell::from("Chain"),
        Cell::from("Wallet"),
        Cell::from("Balance").style(Style::default().add_modifier(Modifier::DIM)),
        Cell::from("Price"),
        Cell::from("Value"),
        Cell::from("24h %"),
        Cell::from("PnL (u)"),
    ]).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let table_rows: Vec<Row> = rows.iter().map(|r| {
        let bal = format_amount(r.holding.amount);
        let price = r.price_usd.map(|p| format!("${}", format_price(p))).unwrap_or_else(|| "—".into());
        let val = r.value_usd.map(|v| format!("${:>.2}", v.to_f64().unwrap_or(0.0))).unwrap_or_else(|| "—".into());
        let ch = r.change_24h.map(|p| format!("{:+.2}%", p));
        let ch_cell = match r.change_24h {
            Some(p) if p >= 0.0 => Cell::from(ch.clone().unwrap_or_default()).style(Style::default().fg(Color::Green)),
            Some(_) => Cell::from(ch.clone().unwrap_or_default()).style(Style::default().fg(Color::Red)),
            None => Cell::from("—").style(Style::default().fg(Color::DarkGray)),
        };
        let pnl_str = r.unrealized_pnl.map(|v| format!("{:+.2}", v.to_f64().unwrap_or(0.0)));
        let pnl_cell = match r.unrealized_pnl {
            Some(v) if v >= Decimal::ZERO => Cell::from(pnl_str.clone().unwrap_or_default()).style(Style::default().fg(Color::Green)),
            Some(_) => Cell::from(pnl_str.clone().unwrap_or_default()).style(Style::default().fg(Color::Red)),
            None => Cell::from("—").style(Style::default().fg(Color::DarkGray)),
        };

        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::styled(truncate(&r.holding.symbol, 8), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(truncate(&r.holding.name, 16), Style::default().fg(Color::DarkGray)),
            ])),
            Cell::from(r.holding.chain.short()).style(Style::default().fg(Color::DarkGray)),
            Cell::from(truncate(&r.holding.wallet_label, 12)),
            Cell::from(bal),
            Cell::from(price),
            Cell::from(val).style(Style::default().add_modifier(Modifier::BOLD)),
            ch_cell,
            pnl_cell,
        ])
    }).collect();

    let title = format!(" Holdings — {} tokens   (sort: {})  ",
        rows.len(), sort_label(state.ui.sort_mode));
    let table = Table::new(table_rows, [
        Constraint::Length(26),
        Constraint::Length(6),
        Constraint::Length(14),
        Constraint::Length(16),
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(12),
    ])
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 40)));

    let mut tstate = TableState::default();
    if !rows.is_empty() {
        tstate.select(Some(state.ui.holdings_cursor.min(rows.len().saturating_sub(1))));
    }
    f.render_stateful_widget(table, area, &mut tstate);
}

fn sort_label(m: SortMode) -> &'static str {
    match m {
        SortMode::Value => "value ↓",
        SortMode::Change24h => "24h % ↓",
        SortMode::Pnl => "pnl ↓",
        SortMode::Balance => "balance ↓",
        SortMode::Symbol => "symbol ↑",
    }
}

fn format_amount(d: Decimal) -> String {
    let f = d.to_f64().unwrap_or(0.0);
    if f >= 1000.0 { format!("{:.2}", f) }
    else if f >= 1.0 { format!("{:.4}", f) }
    else if f >= 0.0001 { format!("{:.6}", f) }
    else { format!("{:.2e}", f) }
}

fn format_price(d: Decimal) -> String {
    let f = d.to_f64().unwrap_or(0.0);
    if f >= 1.0 { format!("{:.2}", f) }
    else if f >= 0.01 { format!("{:.4}", f) }
    else if f >= 0.000001 { format!("{:.8}", f) }
    else { format!("{:.2e}", f) }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
