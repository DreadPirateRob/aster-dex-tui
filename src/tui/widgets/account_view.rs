// src/tui/widgets/account_view.rs
// Account overview rendering with 3-zone layout: status bar, metrics + positions, help bar

use crate::data::position::PositionSide;
use crate::helpers;
use crate::tui::account_app::AccountApp;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};
use rust_decimal::Decimal;

/// Render the complete account overview with 3-zone vertical layout.
///
/// Zone 1 (top): Status bar showing connection status, last refresh, auto-refresh label
/// Zone 2 (middle): Metrics panel + position mini-table (vertical split)
/// Zone 3 (bottom): Help bar with keybinding hints
///
/// Uses `render_widget` for the table (NOT `render_stateful_widget` -- no TableState,
/// this is a non-interactive mini-table per ACCT-09).
pub fn ui_account(frame: &mut Frame, app: &AccountApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar (bordered, 1 line content + 2 borders)
            Constraint::Min(0),   // Main content (metrics panel + position mini-table)
            Constraint::Length(1), // Help bar (single line)
        ])
        .split(frame.area());

    // Zone 1: Status bar
    render_status_bar(frame, app, chunks[0]);

    // Zone 2: Main content (metrics panel + position mini-table)
    render_main_content(frame, app, chunks[1]);

    // Zone 3: Help bar
    render_help_bar(frame, app, chunks[2]);
}

/// Render the status bar (Zone 1) with connection status, last refresh, auto-refresh label.
fn render_status_bar(frame: &mut Frame, app: &AccountApp, area: ratatui::layout::Rect) {
    let theme = &app.theme;

    // Connection status indicator
    let (status_text, status_color) = if app.api_reachable {
        ("API: Connected", theme.status_connected)
    } else {
        ("API: Unreachable", theme.status_error)
    };

    // Right side: error > status_message > last refresh
    let right_text = if let Some(ref err) = app.error_message {
        Span::styled(
            format!("  Error: {}", err),
            Style::default().fg(theme.status_error),
        )
    } else if let Some(ref status) = app.status_message {
        Span::styled(
            format!("  {}  |  Auto-refresh: 10s", status),
            Style::default().fg(theme.status_warning),
        )
    } else {
        let refresh_str = match app.last_refresh {
            Some(t) => format!("Last refresh: {}s ago", t.elapsed().as_secs()),
            None => "Last refresh: --".to_string(),
        };
        Span::styled(
            format!("  {}  |  Auto-refresh: 10s", refresh_str),
            Style::default().fg(theme.text_secondary),
        )
    };

    let status_line = Line::from(vec![
        Span::styled(
            format!(" Account Overview  |  {}", status_text),
            Style::default().fg(status_color),
        ),
        Span::raw("  |  "),
        right_text,
    ]);

    let status = Paragraph::new(status_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Account ")
            .border_style(Style::default().fg(theme.border)),
    );

    frame.render_widget(status, area);
}

/// Render the main content area (Zone 2): metrics panel on top, position mini-table below.
fn render_main_content(frame: &mut Frame, app: &AccountApp, area: ratatui::layout::Rect) {
    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(14), // Metrics panel (fixed height)
            Constraint::Min(0),    // Position mini-table (fills remaining)
        ])
        .split(area);

    render_metrics_panel(frame, app, content_chunks[0]);
    render_position_mini_table(frame, app, content_chunks[1]);
}

/// Render the metrics panel with key-value account data.
fn render_metrics_panel(frame: &mut Frame, app: &AccountApp, area: ratatui::layout::Rect) {
    let theme = &app.theme;

    let lines: Vec<Line<'static>> = match &app.summary {
        None => {
            vec![
                Line::from(""),
                Line::from(""),
                Line::from(""),
                Line::from(""),
                Line::from(Span::styled(
                    "Loading...".to_string(),
                    Style::default().fg(theme.text_muted),
                )),
            ]
        }
        Some(s) => {
            // Margin ratio color coding: green <50%, yellow 50-80%, red >=80%
            let margin_color = margin_ratio_color(app.margin_ratio, theme);

            // PnL color coding
            let unrealized_pnl_color = pnl_color(s.total_unrealized_pnl, theme);
            let daily_pnl_color = pnl_color(s.daily_realized_pnl, theme);

            vec![
                metric_line("Wallet Balance:", &format!("{:.2} USDT", s.wallet_balance), theme.text_primary, theme),
                metric_line("Available Balance:", &format!("{:.2} USDT", s.available_balance), theme.text_primary, theme),
                metric_line("Margin Ratio:", &format!("{:.2}%", app.margin_ratio), margin_color, theme),
                metric_line("Margin Mode:", &app.margin_mode, theme.text_primary, theme),
                Line::from(String::new()),
                metric_line("Unrealized PnL:", &format!("{:.2}", s.total_unrealized_pnl), unrealized_pnl_color, theme),
                metric_line("Daily PnL (Gross, UTC):", &format!("{:.2}", s.daily_realized_pnl), daily_pnl_color, theme),
                Line::from(String::new()),
                metric_line("Open Positions:", &s.open_position_count.to_string(), theme.text_primary, theme),
                metric_line("Total Notional:", &format!("{:.2} USDT", s.total_notional), theme.text_primary, theme),
                metric_line(
                    "Long Exposure:",
                    &format!("{:.2} USDT ({} pos, PnL: {:.2})", s.long_exposure.notional, s.long_exposure.count, s.long_exposure.unrealized_pnl),
                    theme.text_primary,
                    theme,
                ),
                metric_line(
                    "Short Exposure:",
                    &format!("{:.2} USDT ({} pos, PnL: {:.2})", s.short_exposure.notional, s.short_exposure.count, s.short_exposure.unrealized_pnl),
                    theme.text_primary,
                    theme,
                ),
            ]
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Account Metrics ")
        .border_style(Style::default().fg(app.theme.border));

    let para = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(block);

    frame.render_widget(para, area);
}

/// Render the position mini-table (non-interactive, no TableState).
fn render_position_mini_table(
    frame: &mut Frame,
    app: &AccountApp,
    area: ratatui::layout::Rect,
) {
    let theme = &app.theme;

    if app.positions.is_empty() {
        // Empty state: show centered message in bordered block
        let message = Paragraph::new("No open positions")
            .style(Style::default().fg(theme.text_muted))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Positions ")
                    .border_style(Style::default().fg(theme.border)),
            );
        frame.render_widget(message, area);
        return;
    }

    // Header row
    let header = Row::new(vec!["Symbol", "Side", "Size", "Entry", "Mark", "PnL", "Margin"])
        .style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.text_secondary),
        )
        .bottom_margin(1);

    // Column widths
    let widths = [
        Constraint::Length(15), // Symbol
        Constraint::Length(6),  // Side
        Constraint::Length(12), // Size
        Constraint::Length(12), // Entry
        Constraint::Length(12), // Mark
        Constraint::Length(12), // PnL
        Constraint::Length(8),  // Margin
    ];

    // Data rows
    let rows: Vec<Row> = app
        .positions
        .iter()
        .map(|pos| {
            // Side column: color-coded
            let side_cell = match pos.position_side {
                PositionSide::Long => {
                    Cell::new("LONG").style(Style::default().fg(theme.bullish))
                }
                PositionSide::Short => {
                    Cell::new("SHORT").style(Style::default().fg(theme.bearish))
                }
            };

            // PnL: color-coded
            let pnl = pos.unrealized_pnl();
            let pnl_c = pnl_color(pnl, theme);
            let pnl_cell =
                Cell::new(format!("{:.2}", pnl)).style(Style::default().fg(pnl_c));

            // Margin type: capitalize first letter
            let margin_str = helpers::capitalize_first(&pos.margin_type);

            Row::new(vec![
                Cell::new(pos.symbol.as_str()),
                side_cell,
                Cell::new(pos.position_amt.abs().to_string()),
                Cell::new(pos.entry_price.to_string()),
                Cell::new(pos.mark_price.to_string()),
                pnl_cell,
                Cell::new(margin_str),
            ])
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Positions ")
                .border_style(Style::default().fg(theme.border)),
        );

    // CRITICAL: render_widget (NOT render_stateful_widget) -- no TableState, non-interactive
    frame.render_widget(table, area);
}

/// Render the help bar (Zone 3).
fn render_help_bar(frame: &mut Frame, app: &AccountApp, area: ratatui::layout::Rect) {
    let help = Paragraph::new(Line::from(" q quit | r refresh | Auto-refresh: 10s")).style(
        Style::default()
            .fg(app.theme.text_secondary)
            .bg(app.theme.background_highlight),
    );

    frame.render_widget(help, area);
}

/// Build a metric line with label (muted) + value (colored).
fn metric_line(label: &str, value: &str, value_color: Color, theme: &crate::tui::theme::Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {:<22}", label),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(value.to_string(), Style::default().fg(value_color)),
    ])
}

/// Get the color for margin ratio: green <50%, yellow 50-80%, red >=80%.
fn margin_ratio_color(ratio: Decimal, theme: &crate::tui::theme::Theme) -> Color {
    let fifty = Decimal::from(50);
    let eighty = Decimal::from(80);

    if ratio < fifty {
        theme.status_connected // green
    } else if ratio < eighty {
        theme.status_warning // yellow, from theme (not hardcoded)
    } else {
        theme.status_error // red
    }
}

/// Get bullish/bearish color for a PnL value.
fn pnl_color(value: Decimal, theme: &crate::tui::theme::Theme) -> Color {
    if value >= Decimal::ZERO {
        theme.bullish
    } else {
        theme.bearish
    }
}
