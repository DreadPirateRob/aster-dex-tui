// src/tui/widgets/analytics_dashboard.rs
// Render widget for the session analytics dashboard: title bar, PnL summary,
// stats row, equity sparkline, scrollable recent fills, and help bar.

use crate::tui::analytics_app::{AnalyticsApp, AnalyticsInputMode};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::bar::NINE_LEVELS,
    text::{Line, Span},
    widgets::{Paragraph, Sparkline, Widget},
};
use rust_decimal::Decimal;

/// Widget that renders the complete session analytics dashboard.
///
/// Borrows the AnalyticsApp for read-only access to analytics data,
/// symbol filter, theme, and input mode.
pub struct AnalyticsDashboardWidget<'a> {
    app: &'a AnalyticsApp,
}

impl<'a> AnalyticsDashboardWidget<'a> {
    pub fn new(app: &'a AnalyticsApp) -> Self {
        Self { app }
    }
}

impl<'a> Widget for AnalyticsDashboardWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Length(2), // PnL summary
                Constraint::Length(1), // Separator
                Constraint::Length(2), // Stats row
                Constraint::Length(1), // Separator
                Constraint::Length(2), // Equity sparkline
                Constraint::Length(1), // Separator
                Constraint::Min(0),    // Recent fills
                Constraint::Length(1), // Help bar
            ])
            .split(area);

        render_title_bar(self.app, chunks[0], buf);
        render_pnl_summary(self.app, chunks[1], buf);
        render_separator(self.app.theme.border, chunks[2], buf);
        render_stats_row(self.app, chunks[3], buf);
        render_separator(self.app.theme.border, chunks[4], buf);
        render_equity_sparkline(self.app, chunks[5], buf);
        render_separator(self.app.theme.border, chunks[6], buf);
        render_recent_fills(self.app, chunks[7], buf);
        render_help_bar(self.app, chunks[8], buf);
    }
}

/// Render the title bar with session info.
fn render_title_bar(app: &AnalyticsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    // Check if in symbol input mode
    if let AnalyticsInputMode::SymbolInput = app.input_mode {
        let buffer_text = app.input_buffer.as_deref().unwrap_or("");
        let line = Line::from(vec![
            Span::styled(
                " Session Analytics ",
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Filter: {}_", buffer_text),
                Style::default().fg(theme.status_warning),
            ),
        ]);
        Paragraph::new(line).render(area, buf);
        return;
    }

    let mut spans = vec![
        Span::styled(
            " Session Analytics ",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // Symbol filter
    if let Some(ref sym) = app.symbol_filter {
        spans.push(Span::styled(
            format!("| {} ", sym),
            Style::default().fg(theme.text_secondary),
        ));
    }

    // Session start time
    let start_time = app.analytics.session_start_utc.format("%H:%M UTC");
    spans.push(Span::styled(
        format!("| Since {} ", start_time),
        Style::default().fg(theme.text_muted),
    ));

    // Duration
    let duration = app.analytics.session_duration();
    let hours = duration.as_secs() / 3600;
    let mins = (duration.as_secs() % 3600) / 60;
    let secs = duration.as_secs() % 60;
    let duration_str = if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    };
    spans.push(Span::styled(
        format!("| Duration: {} ", duration_str),
        Style::default().fg(theme.text_muted),
    ));

    let line = Line::from(spans);
    Paragraph::new(line).render(area, buf);
}

/// Render the PnL summary (2 lines).
fn render_pnl_summary(app: &AnalyticsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let analytics = &app.analytics;

    let net = analytics.net_pnl();
    let net_color = pnl_color(net, theme);

    // Line 1: Net PnL | Realized | Unrealized
    let line1 = Line::from(vec![
        Span::styled(" PnL: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format_pnl(net),
            Style::default().fg(net_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | Realized: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format_pnl(analytics.total_realized_pnl),
            Style::default().fg(pnl_color(analytics.total_realized_pnl, theme)),
        ),
        Span::styled(" | Unrealized: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format_pnl(analytics.current_unrealized_pnl),
            Style::default().fg(pnl_color(analytics.current_unrealized_pnl, theme)),
        ),
    ]);

    // Line 2: Fees | Net (repeat for emphasis)
    let line2 = Line::from(vec![
        Span::styled(" Fees: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("-${:.2}", analytics.total_commissions),
            Style::default().fg(theme.bearish),
        ),
        Span::styled(" | Balance: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${:.2}", analytics.starting_wallet_balance),
            Style::default().fg(theme.text_secondary),
        ),
    ]);

    if area.height >= 1 {
        let top = Rect::new(area.x, area.y, area.width, 1);
        Paragraph::new(line1).render(top, buf);
    }
    if area.height >= 2 {
        let bottom = Rect::new(area.x, area.y + 1, area.width, 1);
        Paragraph::new(line2).render(bottom, buf);
    }
}

/// Render the stats row (2 lines).
fn render_stats_row(app: &AnalyticsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let analytics = &app.analytics;

    let win_rate = analytics.win_rate();
    let win_rate_color = if win_rate >= 50.0 {
        theme.bullish
    } else if analytics.trade_count > 0 {
        theme.bearish
    } else {
        theme.text_muted
    };

    // Line 1: Win Rate | Avg Win | Avg Loss
    let line1 = Line::from(vec![
        Span::styled(" Win Rate: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("{:.0}%", win_rate),
            Style::default().fg(win_rate_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({}W/{}L)", analytics.winning_trades, analytics.losing_trades),
            Style::default().fg(theme.text_secondary),
        ),
        Span::styled(" | Avg Win: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${:.2}", analytics.avg_win()),
            Style::default().fg(theme.bullish),
        ),
        Span::styled(" | Avg Loss: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${:.2}", analytics.avg_loss()),
            Style::default().fg(theme.bearish),
        ),
    ]);

    // Line 2: Max DD | Current DD | Trades
    let line2 = Line::from(vec![
        Span::styled(" Max DD: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${:.2}", analytics.max_drawdown),
            Style::default().fg(theme.bearish),
        ),
        Span::styled(" | Current DD: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${:.2}", analytics.current_drawdown),
            Style::default().fg(if analytics.current_drawdown > Decimal::ZERO {
                theme.bearish
            } else {
                theme.text_muted
            }),
        ),
        Span::styled(" | Trades: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("{}", analytics.trade_count),
            Style::default().fg(theme.text_primary),
        ),
    ]);

    if area.height >= 1 {
        let top = Rect::new(area.x, area.y, area.width, 1);
        Paragraph::new(line1).render(top, buf);
    }
    if area.height >= 2 {
        let bottom = Rect::new(area.x, area.y + 1, area.width, 1);
        Paragraph::new(line2).render(bottom, buf);
    }
}

/// Render the equity sparkline (2 rows).
fn render_equity_sparkline(app: &AnalyticsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let data = app.analytics.equity_as_sparkline_data();

    if data.is_empty() {
        let msg = Line::from(Span::styled(
            " Collecting data...",
            Style::default().fg(theme.text_muted),
        ));
        Paragraph::new(msg).render(area, buf);
        return;
    }

    let sparkline_color = if app.analytics.net_pnl() >= Decimal::ZERO {
        theme.bullish
    } else {
        theme.bearish
    };

    let sparkline = Sparkline::default()
        .data(&data)
        .bar_set(NINE_LEVELS)
        .style(Style::default().fg(sparkline_color));
    Widget::render(sparkline, area, buf);
}

/// Render a horizontal separator line.
fn render_separator(border_color: Color, area: Rect, buf: &mut Buffer) {
    let sep = "\u{2500}".repeat(area.width as usize);
    let line = Line::from(Span::styled(
        sep,
        Style::default().fg(border_color),
    ));
    Paragraph::new(line).render(area, buf);
}

/// Render the scrollable recent fills list.
fn render_recent_fills(app: &AnalyticsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let fills = &app.analytics.recent_fills;

    // Header line
    let header_str = if app.symbol_filter.is_some() {
        " Recent Fills (filtered)"
    } else {
        " Recent Fills"
    };

    if area.height == 0 {
        return;
    }

    // Render header
    let header = Line::from(vec![
        Span::styled(
            header_str,
            Style::default()
                .fg(theme.text_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  [{} total]", fills.len()),
            Style::default().fg(theme.text_muted),
        ),
    ]);
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    Paragraph::new(header).render(header_area, buf);

    if area.height <= 1 {
        return;
    }

    let list_area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);

    if fills.is_empty() {
        let msg = Line::from(Span::styled(
            " No fills yet. Waiting for trades...",
            Style::default().fg(theme.text_muted),
        ));
        Paragraph::new(msg).render(list_area, buf);
        return;
    }

    let visible_rows = list_area.height as usize;

    // Filter fills by symbol if active (display-level filter only)
    let filtered_fills: Vec<(usize, &crate::data::SessionFill)> = fills
        .iter()
        .enumerate()
        .filter(|(_, fill)| {
            if let Some(ref filter) = app.symbol_filter {
                fill.symbol.eq_ignore_ascii_case(filter)
            } else {
                true
            }
        })
        .collect();

    // Iterate in reverse (newest first), apply scroll offset
    let total = filtered_fills.len();
    let start = app.scroll_offset.min(total.saturating_sub(1));

    for (row, display_idx) in (0..total)
        .rev()
        .skip(start)
        .take(visible_rows)
        .enumerate()
    {
        let (_, fill) = filtered_fills[display_idx];
        let y = list_area.y + row as u16;
        if y >= list_area.y + list_area.height {
            break;
        }

        // Format timestamp
        let ts_str = format_timestamp_utc(fill.time);

        // Side styling
        let side_color = if fill.side == "BUY" {
            theme.bullish
        } else {
            theme.bearish
        };

        // PnL display
        let (pnl_text, pnl_color) = if fill.realized_pnl.is_zero() {
            ("ENTRY".to_string(), theme.text_muted)
        } else {
            (format_pnl(fill.realized_pnl), pnl_color(fill.realized_pnl, theme))
        };

        // Build row
        let row_line = Line::from(vec![
            Span::styled(
                format!(" {} ", ts_str),
                Style::default().fg(theme.text_muted),
            ),
            Span::styled(
                format!("{:10} ", fill.symbol),
                Style::default().fg(theme.text_primary),
            ),
            Span::styled(
                format!("{:4} ", fill.side),
                Style::default().fg(side_color),
            ),
            Span::styled(
                format!("{} ", fill.quantity.normalize()),
                Style::default().fg(theme.text_secondary),
            ),
            Span::styled(
                format!("@ {} ", fill.price.normalize()),
                Style::default().fg(theme.text_secondary),
            ),
            Span::styled(
                pnl_text,
                Style::default().fg(pnl_color),
            ),
        ]);

        let row_area = Rect::new(list_area.x, y, list_area.width, 1);
        Paragraph::new(row_line).render(row_area, buf);
    }
}

/// Render the help bar with hotkey hints.
fn render_help_bar(app: &AnalyticsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let help_text = if matches!(app.input_mode, AnalyticsInputMode::SymbolInput) {
        "Enter: apply | Esc: cancel | type symbol name"
    } else {
        "j/k: scroll | g/G: top/bottom | /: filter | c: clear filter | q: quit"
    };

    let line = Line::from(Span::styled(
        format!(" {}", help_text),
        Style::default().fg(theme.text_muted),
    ));
    Paragraph::new(line).render(area, buf);
}

/// Format a PnL value as $X.XX with sign.
fn format_pnl(value: Decimal) -> String {
    if value >= Decimal::ZERO {
        format!("${:.2}", value)
    } else {
        format!("-${:.2}", value.abs())
    }
}

/// Choose green/red based on PnL sign.
fn pnl_color(value: Decimal, theme: &crate::tui::theme::Theme) -> Color {
    if value >= Decimal::ZERO {
        theme.bullish
    } else {
        theme.bearish
    }
}

/// Format a millisecond epoch timestamp to HH:MM UTC.
fn format_timestamp_utc(timestamp_ms: u64) -> String {
    let secs = (timestamp_ms / 1000) as i64;
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "??:??".to_string(),
    }
}
