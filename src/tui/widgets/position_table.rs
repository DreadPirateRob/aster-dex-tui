// src/tui/widgets/position_table.rs
// Position table rendering with color-coded PnL columns and 3-zone layout

use crate::data::position::PositionSide;
use crate::tui::positions_app::{ManagementAction, PositionsApp, PositionsMode};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, HighlightSpacing, Paragraph, Row, Table},
    Frame,
};
use rust_decimal::Decimal;
use std::time::Duration;

/// Render the complete positions view with 3-zone vertical layout.
///
/// Zone 1 (top): Status bar showing symbol, position count, mark freshness, aggregate PnL
/// Zone 2 (middle): Scrollable position table with color-coded PnL columns
/// Zone 3 (bottom): Help bar with keybinding hints
///
/// Uses `render_stateful_widget` for the table to enable TableState-managed
/// scrolling and selection highlighting.
pub fn ui_positions(frame: &mut Frame, app: &mut PositionsApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar (bordered, 1 line content + 2 borders)
            Constraint::Min(0),   // Position table (scrollable, fills remaining space)
            Constraint::Length(1), // Help bar (single line)
        ])
        .split(frame.area());

    // Update page_size from actual table area height
    // Subtract header row (1) + header bottom_margin (1) + some border padding
    app.page_size = chunks[1].height.saturating_sub(4);

    // Zone 1: Status bar
    render_status_bar(frame, app, chunks[0]);

    // Zone 2: Position table (or empty state)
    if app.positions.is_empty() {
        render_empty_state(frame, app, chunks[1]);
    } else {
        render_position_table(frame, app, chunks[1]);
    }

    // Zone 3: Help bar
    render_help_bar(frame, app, chunks[2]);

    // Overlay rendering (on top of base layout)
    match app.mode {
        PositionsMode::PriceInput => render_price_input_overlay(frame, app),
        PositionsMode::Confirming => render_confirmation_overlay(frame, app),
        PositionsMode::Submitting => render_submitting_overlay(frame, app),
        PositionsMode::ShowingResult => render_result_overlay(frame, app),
        PositionsMode::Normal => {} // no overlay
    }
}

/// Render the status bar (Zone 1) with symbol, position count, mark freshness, and aggregate PnL.
fn render_status_bar(frame: &mut Frame, app: &PositionsApp, area: ratatui::layout::Rect) {
    let symbol_display = match &app.symbol {
        Some(s) => s.as_str(),
        None => "ALL PAIRS",
    };

    // Mark price freshness indicator
    let mark_label = match app.last_mark_update {
        Some(t) if t.elapsed() < Duration::from_secs(5) => "Mark: Live",
        Some(_) => "Mark: Stale",
        None => "Mark: --",
    };

    // Build status content parts
    let mut parts = vec![format!(
        " {}  |  {} positions  |  {}",
        symbol_display,
        app.positions.len(),
        mark_label,
    )];

    // Aggregate PnL for all-pairs mode
    if app.symbol.is_none() && !app.positions.is_empty() {
        let aggregate_pnl: Decimal = app.positions.iter().map(|p| p.unrealized_pnl()).sum();
        let pnl_sign = if aggregate_pnl >= Decimal::ZERO {
            "+"
        } else {
            ""
        };
        parts.push(format!("Total PnL: {}${:.2}", pnl_sign, aggregate_pnl));
    }

    // Debounce feedback message
    if let Some(ref msg) = app.status_message {
        parts.push(msg.clone());
    }

    let content = parts.join("  |  ");

    // Build styled spans for color-coded PnL in status bar
    let status_line = if app.symbol.is_none() && !app.positions.is_empty() {
        let aggregate_pnl: Decimal = app.positions.iter().map(|p| p.unrealized_pnl()).sum();
        let pnl_color = if aggregate_pnl >= Decimal::ZERO {
            app.theme.bullish
        } else {
            app.theme.bearish
        };
        let pnl_sign = if aggregate_pnl >= Decimal::ZERO {
            "+"
        } else {
            ""
        };

        let mut spans = vec![
            Span::styled(
                format!(
                    " {}  |  {} positions  |  {}  |  Total PnL: ",
                    symbol_display,
                    app.positions.len(),
                    mark_label,
                ),
                Style::default().fg(app.theme.text_secondary),
            ),
            Span::styled(
                format!("{}${:.2}", pnl_sign, aggregate_pnl),
                Style::default().fg(pnl_color),
            ),
        ];
        if let Some(ref msg) = app.status_message {
            spans.push(Span::styled(
                format!("  |  {}", msg),
                Style::default().fg(app.theme.text_muted),
            ));
        }
        Line::from(spans)
    } else {
        Line::from(Span::styled(
            content,
            Style::default().fg(app.theme.text_secondary),
        ))
    };

    let status = Paragraph::new(status_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Positions ")
            .border_style(Style::default().fg(app.theme.border)),
    );

    frame.render_widget(status, area);
}

/// Render the scrollable position table (Zone 2) with color-coded columns.
fn render_position_table(frame: &mut Frame, app: &mut PositionsApp, area: ratatui::layout::Rect) {
    let theme = &app.theme;
    let show_symbol = app.symbol.is_none();

    // Header row: include Symbol column in all-pairs mode
    let header_cells: Vec<&str> = if show_symbol {
        vec![
            "Symbol",
            "Side",
            "Size",
            "Entry",
            "Mark",
            "PnL ($)",
            "PnL (%)",
            "Liq. Price",
            "Liq Dist",
            "Funding",
            "Leverage",
            "Margin",
        ]
    } else {
        vec![
            "Side",
            "Size",
            "Entry",
            "Mark",
            "PnL ($)",
            "PnL (%)",
            "Liq. Price",
            "Liq Dist",
            "Funding",
            "Leverage",
            "Margin",
        ]
    };
    let header = Row::new(header_cells)
        .style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.text_secondary),
        )
        .bottom_margin(1);

    // Column widths: include Symbol column in all-pairs mode
    let widths: Vec<Constraint> = if show_symbol {
        vec![
            Constraint::Length(10), // Symbol
            Constraint::Length(5),  // Side: "LONG"/"SHORT"
            Constraint::Length(12), // Size (position_amt.abs())
            Constraint::Length(12), // Entry price
            Constraint::Length(12), // Mark price
            Constraint::Length(14), // PnL ($): "-$12345.67"
            Constraint::Length(10), // PnL (%): "-123.45%"
            Constraint::Length(12), // Liq. Price
            Constraint::Length(8),  // Liq Dist: "12.3%"
            Constraint::Length(10), // Funding: "$1.23"
            Constraint::Length(5),  // Leverage: "125x"
            Constraint::Length(12), // Margin
        ]
    } else {
        vec![
            Constraint::Length(5),  // Side: "LONG"/"SHORT"
            Constraint::Length(12), // Size (position_amt.abs())
            Constraint::Length(12), // Entry price
            Constraint::Length(12), // Mark price
            Constraint::Length(14), // PnL ($): "-$12345.67"
            Constraint::Length(10), // PnL (%): "-123.45%"
            Constraint::Length(12), // Liq. Price
            Constraint::Length(8),  // Liq Dist: "12.3%"
            Constraint::Length(10), // Funding: "$1.23"
            Constraint::Length(5),  // Leverage: "125x"
            Constraint::Length(12), // Margin
        ]
    };

    // Data rows with per-cell styling
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

            // PnL ($): color-coded
            let pnl = pos.unrealized_pnl();
            let pnl_color = if pnl >= Decimal::ZERO {
                theme.bullish
            } else {
                theme.bearish
            };
            let pnl_cell =
                Cell::new(format!("{:.2}", pnl)).style(Style::default().fg(pnl_color));

            // PnL (%): same color as PnL ($)
            let pnl_pct = pos.unrealized_pnl_pct();
            let pnl_pct_cell =
                Cell::new(format!("{:.2}%", pnl_pct)).style(Style::default().fg(pnl_color));

            // Liquidation price: "--" if zero
            let liq_str = if pos.liquidation_price.is_zero() {
                "--".to_string()
            } else {
                pos.liquidation_price.to_string()
            };

            // Liquidation distance percentage: "X.X%" or "--"
            let liq_dist_str = match pos.liquidation_distance_pct() {
                Some(pct) => format!("{:.1}%", pct),
                None => "--".to_string(),
            };

            // Estimated funding payment: color-coded "$X.XX" or "--"
            let funding_cell = match pos.estimated_funding_payment() {
                Some(payment) => {
                    let color = if payment < Decimal::ZERO {
                        theme.bullish // Negative = you receive = green
                    } else {
                        theme.bearish // Positive = you pay = red
                    };
                    Cell::new(format!("${:.2}", payment))
                        .style(Style::default().fg(color))
                }
                None => Cell::new("--"),
            };

            // Margin column: show "cross" for cross-margin, isolated_margin for isolated
            let margin_cell = Cell::new(if pos.margin_type == "cross" {
                "cross".to_string()
            } else {
                pos.isolated_margin.to_string()
            });

            // Build cells conditionally: include Symbol in all-pairs mode
            let mut cells = vec![];
            if show_symbol {
                cells.push(Cell::new(pos.symbol.as_str()));
            }
            cells.extend([
                side_cell,
                Cell::new(pos.position_amt.abs().to_string()),
                Cell::new(pos.entry_price.to_string()),
                Cell::new(pos.mark_price.to_string()),
                pnl_cell,
                pnl_pct_cell,
                Cell::new(liq_str),
                Cell::new(liq_dist_str),
                funding_cell,
                Cell::new(format!("{}x", pos.leverage)),
                margin_cell,
            ]);
            Row::new(cells)
        })
        .collect();

    // Build table
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(theme.background_highlight)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ")
        .highlight_spacing(HighlightSpacing::Always);

    // CRITICAL: render_stateful_widget (not render_widget) for TableState scroll tracking
    frame.render_stateful_widget(table, area, &mut app.table_state);
}

/// Render the empty state when no positions are found (Zone 2 alternative).
fn render_empty_state(frame: &mut Frame, app: &PositionsApp, area: ratatui::layout::Rect) {
    let message = Paragraph::new("No open positions")
        .style(Style::default().fg(app.theme.text_muted))
        .alignment(Alignment::Center);

    frame.render_widget(message, area);
}

/// Render the help bar (Zone 3) with keybinding hints.
fn render_help_bar(frame: &mut Frame, app: &PositionsApp, area: ratatui::layout::Rect) {
    let help_text = " q quit | j/k scroll | g/G top/bottom | r refresh | s SL | t TP | x close | X reverse";

    let help = Paragraph::new(Line::from(help_text)).style(
        Style::default()
            .fg(app.theme.text_secondary)
            .bg(app.theme.background_highlight),
    );

    frame.render_widget(help, area);
}

/// Compute a centered rectangle with given percentage width and fixed height.
fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let remaining_x = 100u16.saturating_sub(percent_x);
    let half_x = remaining_x / 2;

    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(half_x),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(remaining_x - half_x),
        ])
        .split(area);

    let center_col = horiz[1];

    // Vertically center the given height
    let total_height = center_col.height;
    if height >= total_height {
        return center_col;
    }
    let top_pad = (total_height - height) / 2;

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_pad),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(center_col);

    vert[1]
}

/// Render the price input overlay for SL or TP price entry.
fn render_price_input_overlay(frame: &mut Frame, app: &PositionsApp) {
    let overlay_area = centered_rect(50, 8, frame.area());
    frame.render_widget(Clear, overlay_area);

    let title = match app.current_action {
        Some(ManagementAction::StopLoss) => " Set Stop-Loss Price ",
        Some(ManagementAction::TakeProfit) => " Set Take-Profit Price ",
        _ => " Set Price ",
    };

    // Build lines
    let mut lines = Vec::new();

    // Mark price reference
    if let Some(ref snap) = app.position_snapshot {
        lines.push(Line::from(vec![
            Span::raw("  Mark: "),
            Span::styled(
                snap.mark_price.to_string(),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    lines.push(Line::from(""));

    // Input value
    lines.push(Line::from(vec![
        Span::raw("  > "),
        Span::styled(
            app.price_input.value(),
            Style::default().fg(Color::White),
        ),
    ]));

    // Error message (if any)
    if let Some(ref err) = app.error_message {
        lines.push(Line::from(Span::styled(
            format!("  {}", err),
            Style::default().fg(Color::Red),
        )));
    } else {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter to confirm, Esc to cancel",
        Style::default().fg(app.theme.text_muted),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(app.theme.border));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, overlay_area);

    // Set cursor position in the input field
    let cursor_x = overlay_area.x + 4 + app.price_input.cursor() as u16; // "  > " = 4 chars
    let cursor_y = overlay_area.y + 3; // title border + mark line + blank line + input line
    if cursor_x < overlay_area.x + overlay_area.width.saturating_sub(1) {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

/// Render the confirmation overlay for all 4 management actions.
fn render_confirmation_overlay(frame: &mut Frame, app: &PositionsApp) {
    // Height 14 for ReversePosition (extra warning lines), 12 for others
    let height = match app.current_action {
        Some(ManagementAction::ReversePosition) => 14,
        _ => 12,
    };
    let overlay_area = centered_rect(50, height, frame.area());
    frame.render_widget(Clear, overlay_area);

    let title = match app.current_action {
        Some(ManagementAction::StopLoss) => " Confirm Stop-Loss ",
        Some(ManagementAction::TakeProfit) => " Confirm Take-Profit ",
        Some(ManagementAction::ClosePosition) => " Close Position at Market ",
        Some(ManagementAction::ReversePosition) => " Reverse Position ",
        None => " Confirm ",
    };

    let mut lines = Vec::new();

    if let Some(ref snap) = app.position_snapshot {
        // Symbol (bold)
        lines.push(Line::from(vec![
            Span::raw("  Symbol:   "),
            Span::styled(
                snap.symbol.as_str(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));

        // Side (color-coded)
        let side_span = match snap.side {
            PositionSide::Long => Span::styled(
                "LONG",
                Style::default()
                    .fg(app.theme.bullish)
                    .add_modifier(Modifier::BOLD),
            ),
            PositionSide::Short => Span::styled(
                "SHORT",
                Style::default()
                    .fg(app.theme.bearish)
                    .add_modifier(Modifier::BOLD),
            ),
        };
        lines.push(Line::from(vec![Span::raw("  Side:     "), side_span]));

        // Quantity
        lines.push(Line::from(vec![
            Span::raw("  Quantity: "),
            Span::raw(snap.quantity.to_string()),
        ]));

        // Action-specific details
        match app.current_action {
            Some(ManagementAction::StopLoss) | Some(ManagementAction::TakeProfit) => {
                if let Some(price) = app.price_input.as_decimal() {
                    lines.push(Line::from(vec![
                        Span::raw("  Price:    "),
                        Span::styled(
                            price.to_string(),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]));
                }
            }
            Some(ManagementAction::ClosePosition) => {
                lines.push(Line::from(Span::styled(
                    "  Close at Market",
                    Style::default().fg(Color::Yellow),
                )));
            }
            Some(ManagementAction::ReversePosition) => {
                lines.push(Line::from(Span::styled(
                    "  Close and open opposite at Market",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(Span::styled(
                    "  WARNING: This is a two-step operation (close + open).",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(Span::styled(
                    "  If the open leg fails, you will be left flat.",
                    Style::default().fg(Color::Yellow),
                )));
            }
            None => {}
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press Y to confirm, N to cancel",
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the submitting overlay while waiting for order placement.
fn render_submitting_overlay(frame: &mut Frame, app: &PositionsApp) {
    let overlay_area = centered_rect(50, 5, frame.area());
    frame.render_widget(Clear, overlay_area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Please wait...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Submitting Order... ")
        .border_style(Style::default().fg(app.theme.border));

    let para = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the result overlay showing order success or failure.
fn render_result_overlay(frame: &mut Frame, app: &PositionsApp) {
    let overlay_area = centered_rect(50, 10, frame.area());
    frame.render_widget(Clear, overlay_area);

    // Check for non-API error messages first
    if let Some(ref err_msg) = app.error_message {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", err_msg),
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press any key to dismiss",
                Style::default().fg(app.theme.text_muted),
            )),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " Error ",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::Red));

        let para = Paragraph::new(lines).block(block);
        frame.render_widget(para, overlay_area);
        return;
    }

    match &app.order_result {
        Some(Ok(response)) => {
            let mut lines = vec![
                Line::from(vec![
                    Span::raw("  Order ID:      "),
                    Span::raw(response.order_id.to_string()),
                ]),
                Line::from(vec![
                    Span::raw("  Status:        "),
                    Span::styled(
                        &response.status,
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  Side:          "),
                    Span::raw(&response.side),
                ]),
                Line::from(vec![
                    Span::raw("  Quantity:      "),
                    Span::raw(response.executed_qty.to_string()),
                ]),
            ];

            if !response.avg_price.is_zero() {
                lines.push(Line::from(vec![
                    Span::raw("  Avg Price:     "),
                    Span::raw(response.avg_price.to_string()),
                ]));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Press any key to dismiss",
                Style::default().fg(app.theme.text_muted),
            )));

            let block = Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " Order Placed ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(Color::Green));

            let para = Paragraph::new(lines).block(block);
            frame.render_widget(para, overlay_area);
        }
        Some(Err(err)) => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", err),
                    Style::default().fg(Color::Red),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Press any key to dismiss",
                    Style::default().fg(app.theme.text_muted),
                )),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " Order Failed ",
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(Color::Red));

            let para = Paragraph::new(lines).block(block);
            frame.render_widget(para, overlay_area);
        }
        None => {}
    }
}
