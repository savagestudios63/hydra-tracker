//! PnL tab: realized + unrealized summary plus a sparkline of portfolio
//! value over the selected time range (7d / 30d / all).

use chrono::Duration;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Sparkline},
    Frame,
};
use rust_decimal::prelude::ToPrimitive;

use crate::app::{AppState, PnlRange};

pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(6)])
        .split(area);

    render_summary(f, state, rows[0]);
    render_chart(f, state, rows[1]);
}

fn render_summary(f: &mut Frame, state: &AppState, area: Rect) {
    let g = state.inner().read();
    let realized = g.pnl.total_realized.to_f64().unwrap_or(0.0);
    let unrealized = g.pnl.total_unrealized.to_f64().unwrap_or(0.0);
    let total = realized + unrealized;

    let line = |label: &str, v: f64| -> Line {
        let color = if v >= 0.0 { Color::Green } else { Color::Red };
        Line::from(vec![
            Span::styled(format!("  {:<14}", label), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:+.2}", v), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ])
    };

    let lots_tracked = g.pnl.lots.len();
    let body = vec![
        line("realized", realized),
        line("unrealized", unrealized),
        line("total pnl", total),
        Line::from(Span::styled(
            format!("  {} lots tracked from tx history", lots_tracked),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let p = Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(" PnL "));
    f.render_widget(p, area);
}

fn render_chart(f: &mut Frame, state: &AppState, area: Rect) {
    let g = state.inner().read();
    let cutoff = match state.ui.pnl_range {
        PnlRange::Week => Some(chrono::Utc::now() - Duration::days(7)),
        PnlRange::Month => Some(chrono::Utc::now() - Duration::days(30)),
        PnlRange::All => None,
    };
    let samples: Vec<u64> = g.history.iter()
        .filter(|s| cutoff.map(|c| s.ts >= c).unwrap_or(true))
        .map(|s| s.total_usd.max(0.0) as u64)
        .collect();

    let range_label = match state.ui.pnl_range {
        PnlRange::Week => "7d",
        PnlRange::Month => "30d",
        PnlRange::All => "all",
    };
    let title = if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
        let delta = *last as i64 - *first as i64;
        format!(" Portfolio value ({})   first ${}  last ${}  Δ{:+}  ",
            range_label, first, last, delta)
    } else {
        format!(" Portfolio value ({})   (no history yet — snapshots saved each refresh) ", range_label)
    };

    let spark = Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(title))
        .data(&samples)
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(spark, area);
}
