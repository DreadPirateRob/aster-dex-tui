// src/tui/widgets/order_table.rs
// Order table rendering with color-coded columns and 3-zone layout

use crate::data::order::{Order, OrderSide, OrderStatus};
use crate::network::types::ConnectionState;
use crate::tui::orders_app::{OrdersApp, OrdersMode};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, HighlightSpacing, Paragraph, Row, Table},
    Frame,
};
use rust_decimal::Decimal;

/// Render the complete orders view with 3-zone vertical layout.
///
/// Zone 1 (top): Status bar showing symbol, connection status, order count
/// Zone 2 (middle): Scrollable order table with color-coded columns
/// Zone 3 (bottom): Help bar with keybinding hints
///
/// Uses `render_stateful_widget` for the table to enable TableState-managed
/// scrolling and selection highlighting.
pub fn ui_orders(frame: &mut Frame, app: &mut OrdersApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar (bordered, 1 line content + 2 borders)
            Constraint::Min(0),   // Order table (scrollable, fills remaining space)
            Constraint::Length(1), // Help bar (single line)
        ])
        .split(frame.area());

    // Update page_size from actual table area height
    // Subtract header row (1) + header bottom_margin (1) + some border padding
    app.page_size = chunks[1].height.saturating_sub(4);

    // Zone 1: Status bar
    render_status_bar(frame, app, chunks[0]);

    // Zone 2: Order table (or empty state)
    let filtered_count = app.orders.iter()
        .filter(|o| app.status_filter.matches(&o.status))
        .filter(|o| app.side_filter.matches(&o.side))
        .count();

    if app.orders.is_empty() {
        render_empty_state(frame, app, chunks[1], false);
    } else if filtered_count == 0 {
        render_empty_state(frame, app, chunks[1], true);
    } else {
        render_order_table(frame, app, chunks[1]);
    }

    // Zone 3: Help bar
    render_help_bar(frame, app, chunks[2]);

    // Overlay rendering (on top of base layout)
    match app.mode {
        OrdersMode::Confirming => render_cancel_confirmation_overlay(frame, app),
        OrdersMode::Submitting => render_cancel_submitting_overlay(frame, app),
        OrdersMode::ShowingResult => render_cancel_result_overlay(frame, app),
        OrdersMode::Normal => {} // no overlay
    }
}

/// Render the status bar (Zone 1) with symbol, connection status, and order count.
fn render_status_bar(frame: &mut Frame, app: &OrdersApp, area: ratatui::layout::Rect) {
    let filtered_count = app.orders.iter()
        .filter(|o| app.status_filter.matches(&o.status))
        .filter(|o| app.side_filter.matches(&o.side))
        .count();

    // Determine connection label from WebSocket status
    let ws_label = match &app.connection_status.state {
        ConnectionState::Connected => "WS: Connected".to_string(),
        ConnectionState::Connecting => "WS: Connecting...".to_string(),
        ConnectionState::Reconnecting { attempt, max_attempts, .. } => {
            format!("WS: Reconnecting ({}/{})", attempt, max_attempts)
        }
        ConnectionState::Disconnected => "WS: Disconnected".to_string(),
    };

    let symbol_display = match &app.symbol {
        Some(s) => s.as_str(),
        None => "ALL PAIRS",
    };

    let content = format!(
        " {}  |  {}  |  {} of {} orders  |  Status: {}  |  Side: {}",
        symbol_display,
        ws_label,
        filtered_count,
        app.orders.len(),
        app.status_filter.label(),
        app.side_filter.label(),
    );

    let status = Paragraph::new(Line::from(content))
        .style(Style::default().fg(app.theme.text_secondary))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Orders ")
                .border_style(Style::default().fg(app.theme.border)),
        );

    frame.render_widget(status, area);
}

/// Render the scrollable order table (Zone 2) with color-coded columns.
fn render_order_table(frame: &mut Frame, app: &mut OrdersApp, area: ratatui::layout::Rect) {
    let theme = &app.theme;
    let show_symbol = app.symbol.is_none();

    // Header row: include Symbol column in all-pairs mode
    let header_cells: Vec<&str> = if show_symbol {
        vec!["Time", "Symbol", "Side", "Type", "Price", "Qty", "Filled", "Avg Price", "Cost", "Status"]
    } else {
        vec!["Time", "Side", "Type", "Price", "Qty", "Filled", "Avg Price", "Cost", "Status"]
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
            Constraint::Length(19), // Time: "2026-02-09 14:30:00"
            Constraint::Length(10), // Symbol
            Constraint::Length(4),  // Side: "BUY"/"SELL"
            Constraint::Length(12), // Type: "STOP_MARKET" longest
            Constraint::Length(12), // Price
            Constraint::Length(12), // Qty (orig_qty)
            Constraint::Length(12), // Filled (executed_qty)
            Constraint::Length(12), // Avg Price
            Constraint::Length(12), // Cost (cum_quote)
            Constraint::Length(10), // Status: "CANCELLED"
        ]
    } else {
        vec![
            Constraint::Length(19), // Time: "2026-02-09 14:30:00"
            Constraint::Length(4),  // Side: "BUY"/"SELL"
            Constraint::Length(12), // Type: "STOP_MARKET" longest
            Constraint::Length(12), // Price
            Constraint::Length(12), // Qty (orig_qty)
            Constraint::Length(12), // Filled (executed_qty)
            Constraint::Length(12), // Avg Price
            Constraint::Length(12), // Cost (cum_quote)
            Constraint::Length(10), // Status: "CANCELLED"
        ]
    };

    // Filter orders by active status and side filters
    let filtered: Vec<&Order> = app.orders.iter()
        .filter(|o| app.status_filter.matches(&o.status))
        .filter(|o| app.side_filter.matches(&o.side))
        .collect();

    // Data rows with per-cell styling
    let rows: Vec<Row> = filtered
        .iter()
        .map(|order| {
            // Time column: format epoch ms to human-readable
            let time_str = chrono::DateTime::from_timestamp_millis(order.time as i64)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "---".to_string());

            // Side column: color-coded
            let side_cell = match order.side {
                OrderSide::Buy => {
                    Cell::new("BUY").style(Style::default().fg(theme.bullish))
                }
                OrderSide::Sell => {
                    Cell::new("SELL").style(Style::default().fg(theme.bearish))
                }
            };

            // Status column: mapped display labels with color coding
            let status_cell = match order.status {
                OrderStatus::New => {
                    Cell::new("OPEN").style(Style::default().fg(Color::Yellow))
                }
                OrderStatus::PartiallyFilled => {
                    Cell::new("OPEN").style(Style::default().fg(Color::Yellow))
                }
                OrderStatus::Filled => {
                    Cell::new("FILLED").style(Style::default().fg(theme.bullish))
                }
                OrderStatus::Canceled => {
                    Cell::new("CANCELLED").style(Style::default().fg(Color::DarkGray))
                }
                OrderStatus::Rejected => {
                    Cell::new("FAILED").style(Style::default().fg(theme.status_error))
                }
                OrderStatus::Expired => {
                    Cell::new("FAILED").style(Style::default().fg(theme.status_error))
                }
            };

            // Build cells conditionally: include Symbol in all-pairs mode
            let mut cells = vec![Cell::new(time_str)];
            if show_symbol {
                cells.push(Cell::new(order.symbol.as_str()));
            }
            cells.extend([
                side_cell,
                Cell::new(order.order_type.to_string()),
                Cell::new(order.price.to_string()),
                Cell::new(order.orig_qty.to_string()),
                Cell::new(order.executed_qty.to_string()),
                Cell::new(order.avg_price.to_string()),
                Cell::new(order.cum_quote.to_string()),
                status_cell,
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

/// Render the empty state when no orders are found (Zone 2 alternative).
///
/// If `filtered` is true, shows "No matching orders (filters active)" to indicate
/// that orders exist but are hidden by the current filter settings. Otherwise
/// shows "No orders found" for a truly empty order list.
fn render_empty_state(frame: &mut Frame, app: &OrdersApp, area: ratatui::layout::Rect, filtered: bool) {
    let text = if filtered {
        "No matching orders (filters active)"
    } else {
        "No orders found"
    };

    let message = Paragraph::new(text)
        .style(Style::default().fg(app.theme.text_muted))
        .alignment(Alignment::Center);

    frame.render_widget(message, area);
}

/// Render the help bar (Zone 3) with keybinding hints.
fn render_help_bar(frame: &mut Frame, app: &OrdersApp, area: ratatui::layout::Rect) {
    let help_text = " q quit | j/k scroll | PgUp/PgDn page | g/G top/bottom | f status | s side | r refresh | d cancel";

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

/// Render the confirmation overlay for cancel (symbol, side, type, price, qty, executed).
fn render_cancel_confirmation_overlay(frame: &mut Frame, app: &OrdersApp) {
    let overlay_area = centered_rect(50, 10, frame.area());
    frame.render_widget(Clear, overlay_area);

    let mut lines = Vec::new();

    if let Some(ref snap) = app.order_snapshot {
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
            OrderSide::Buy => Span::styled(
                "BUY",
                Style::default()
                    .fg(app.theme.bullish)
                    .add_modifier(Modifier::BOLD),
            ),
            OrderSide::Sell => Span::styled(
                "SELL",
                Style::default()
                    .fg(app.theme.bearish)
                    .add_modifier(Modifier::BOLD),
            ),
        };
        lines.push(Line::from(vec![Span::raw("  Side:     "), side_span]));

        // Type
        lines.push(Line::from(vec![
            Span::raw("  Type:     "),
            Span::raw(snap.order_type.to_string()),
        ]));

        // Price (show "MARKET" if zero)
        if snap.price.is_zero() {
            lines.push(Line::from(vec![
                Span::raw("  Price:    "),
                Span::styled("MARKET", Style::default().fg(Color::Yellow)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  Price:    "),
                Span::raw(snap.price.to_string()),
            ]));
        }

        // Quantity
        lines.push(Line::from(vec![
            Span::raw("  Quantity: "),
            Span::raw(snap.orig_qty.to_string()),
        ]));

        // Executed (if > 0, show as warning)
        if snap.executed_qty > Decimal::ZERO {
            lines.push(Line::from(vec![
                Span::raw("  Executed: "),
                Span::styled(
                    format!("{} already filled", snap.executed_qty),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press Y to confirm, N to cancel",
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Cancel Order ")
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the submitting overlay while waiting for cancel API response.
fn render_cancel_submitting_overlay(frame: &mut Frame, app: &OrdersApp) {
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
        .title(" Cancelling Order... ")
        .border_style(Style::default().fg(app.theme.border));

    let para = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the result overlay showing cancel success or failure.
fn render_cancel_result_overlay(frame: &mut Frame, app: &OrdersApp) {
    let overlay_area = centered_rect(50, 8, frame.area());
    frame.render_widget(Clear, overlay_area);

    // Check for non-API error messages first (e.g., "Only open orders can be cancelled")
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

    match &app.cancel_result {
        Some(Ok(msg)) => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", msg),
                    Style::default().fg(Color::Green),
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
                    " Order Cancelled ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(Color::Green));

            let para = Paragraph::new(lines).block(block);
            frame.render_widget(para, overlay_area);
        }
        Some(Err(msg)) => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", msg),
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
                    " Cancel Failed ",
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(Color::Red));

            let para = Paragraph::new(lines).block(block);
            frame.render_widget(para, overlay_area);
        }
        None => {} // no-op
    }
}
