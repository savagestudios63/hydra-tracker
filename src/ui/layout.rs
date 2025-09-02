//! Screen layout + shared widgets (top bar, wallet panel, footer).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
    Frame,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::app::{AppState, InputMode, Tab};
use crate::chains::PriceKey;

pub fn render(f: &mut Frame, state: &mut AppState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // top bar
            Constraint::Min(10),     // body
            Constraint::Length(2),   // footer (keys + status)
        ])
        .split(area);

    render_top_bar(f, state, chunks[0]);
    render_body(f, state, chunks[1]);
    render_footer(f, state, chunks[2]);
}

fn render_top_bar(f: &mut Frame, state: &AppState, area: Rect) {
    let g = state.inner().read();
    let total: Decimal = g.holdings.iter()
        .filter_map(|h| {
            g.prices.get(&PriceKey {
                chain: h.chain,
                identifier: h.identifier.clone(),
                symbol: h.symbol.clone(),
            }).map(|p| h.amount * p.usd)
        })
        .sum();
    let total_f = total.to_f64().unwrap_or(0.0);

    let change_24h = portfolio_change_24h(&g);

    let status: String = if g.refreshing {
        "refreshing…".into()
    } else if let Some(t) = g.last_refresh {
        let secs = (chrono::Utc::now() - t).num_seconds();
        format!("updated {}s ago", secs)
    } else {
        "idle".into()
    };

    let title = Span::styled(
        " hydra ",
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
    );
    let total_span = Span::styled(
        format!("  ${:>12.2}  ", total_f),
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    );
    let change_span = match change_24h {
        Some(p) if p >= 0.0 => Span::styled(format!("▲ {:.2}%", p), Style::default().fg(Color::Green)),
        Some(p) => Span::styled(format!("▼ {:.2}%", p.abs()), Style::default().fg(Color::Red)),
        None => Span::styled("— 24h", Style::default().fg(Color::DarkGray)),
    };
    let status_span = Span::styled(
        format!("   {}", status),
        Style::default().fg(if g.refreshing { Color::Yellow } else { Color::DarkGray }),
    );
    let err_span = if let Some(e) = &g.last_error {
        Span::styled(format!("   ! {}", truncate(e, 60)), Style::default().fg(Color::Red))
    } else {
        Span::raw("")
    };
    let line = Line::from(vec![title, total_span, change_span, status_span, err_span]);
    let p = Paragraph::new(line).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(p, area);
}

fn portfolio_change_24h(g: &crate::app::Inner) -> Option<f64> {
    // Weighted by current USD value.
    let mut total_val = 0.0;
    let mut total_prev = 0.0;
    for h in &g.holdings {
        let Some(price) = g.prices.get(&PriceKey {
            chain: h.chain,
            identifier: h.identifier.clone(),
            symbol: h.symbol.clone(),
        }) else { continue };
        let val = (h.amount * price.usd).to_f64().unwrap_or(0.0);
        total_val += val;
        if let Some(pct) = price.change_24h_pct {
            let prev = val / (1.0 + pct / 100.0);
            total_prev += prev;
        } else {
            total_prev += val; // treat unknown as flat
        }
    }
    if total_prev == 0.0 || total_val == 0.0 { return None; }
    Some((total_val - total_prev) / total_prev * 100.0)
}

fn render_body(f: &mut Frame, state: &mut AppState, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(40)])
        .split(area);

    render_wallets(f, state, cols[0]);
    render_main(f, state, cols[1]);
}

fn render_wallets(f: &mut Frame, state: &mut AppState, area: Rect) {
    let g = state.inner().read();
    let items: Vec<ListItem> = state.cfg.wallets.iter().enumerate().map(|(i, w)| {
        let total: Decimal = g.holdings.iter()
            .filter(|h| h.wallet_label == w.label)
            .filter_map(|h| g.prices.get(&h.price_key()).map(|p| h.amount * p.usd))
            .sum();
        let selected = i == state.ui.wallet_cursor;
        let total_f = total.to_f64().unwrap_or(0.0);
        let line = Line::from(vec![
            Span::styled(
                format!("{:<14}", truncate(&w.label, 14)),
                Style::default().fg(if selected { Color::Cyan } else { Color::White }).add_modifier(
                    if selected { Modifier::BOLD } else { Modifier::empty() }
                ),
            ),
            Span::styled(format!(" {:>4}", w.chain.short()), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("  ${:>9.2}", total_f), Style::default().fg(Color::White)),
        ]);
        ListItem::new(line)
    }).collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" wallets "))
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut lstate = ListState::default();
    lstate.select(Some(state.ui.wallet_cursor));
    f.render_stateful_widget(list, area, &mut lstate);
}

fn render_main(f: &mut Frame, state: &mut AppState, area: Rect) {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);

    let titles: Vec<Line> = ["Holdings", "Transactions", "PnL"].iter()
        .map(|t| Line::from(Span::styled(*t, Style::default().fg(Color::White)))).collect();
    let selected = match state.ui.active_tab { Tab::Holdings => 0, Tab::Transactions => 1, Tab::Pnl => 2 };
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL))
        .select(selected)
        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .divider("│");
    f.render_widget(tabs, v[0]);

    match state.ui.active_tab {
        Tab::Holdings => super::holdings::render(f, state, v[1]),
        Tab::Transactions => super::transactions::render(f, state, v[1]),
        Tab::Pnl => super::pnl_tab::render(f, state, v[1]),
    }
}

fn render_footer(f: &mut Frame, state: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    // Command / filter / hint line.
    let bottom = match state.ui.input_mode {
        InputMode::Filter => Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Yellow)),
            Span::raw(&state.ui.filter),
        ]),
        InputMode::Command => Line::from(vec![
            Span::styled(":", Style::default().fg(Color::Yellow)),
            Span::raw(&state.ui.command_buffer),
        ]),
        InputMode::Normal => {
            let hints = "j/k move  h/l wallet  tab switch  s sort  / filter  : cmd  r refresh  q quit";
            Line::from(Span::styled(hints, Style::default().fg(Color::DarkGray)))
        }
    };
    f.render_widget(Paragraph::new(bottom), chunks[1]);

    // Status line (pinned above hints).
    let status = state.ui.status_message.clone().unwrap_or_default();
    let status_line = Line::from(Span::styled(status, Style::default().fg(Color::Yellow)));
    f.render_widget(Paragraph::new(status_line), chunks[0]);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
