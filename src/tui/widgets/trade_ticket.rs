// src/tui/widgets/trade_ticket.rs
// Trade ticket widget rendering with 4-zone layout, form fields, info row,
// reduce-only toggle, TIF selector, recent orders footer, and overlays.

use crate::data::order::{OrderSide, OrderStatus};
use crate::data::position::PositionSide;
use crate::tui::trade_app::{AppMode, BracketState, Focus, LegResult, TradeApp};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};
use rust_decimal::Decimal;

/// Render the complete trade ticket view with 4-zone vertical layout.
///
/// Zone 1 (top): Header showing symbol, order type, and mark price
/// Zone 2 (middle): Interactive form with info row, side, order type, quantity,
///   price, stop price, reduce-only, time-in-force, and submit button
/// Zone 3: Recent orders footer (last 5 orders for the symbol)
/// Zone 4 (bottom): Help bar with keybinding hints per current mode
///
/// When AppMode is Confirming, ShowingResult, or Submitting, an overlay is drawn
/// on top of the form area.
pub fn ui_trade(frame: &mut Frame, app: &mut TradeApp) {
    let orders_height = if app.show_orders { 7 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),            // Zone 1: Header
            Constraint::Min(0),               // Zone 2: Form area
            Constraint::Length(orders_height), // Zone 3: Recent orders (0 = hidden)
            Constraint::Length(1),            // Zone 4: Help bar
        ])
        .split(frame.area());

    // Zone 1: Header
    render_header(frame, app, chunks[0]);

    // Zone 2: Form
    render_form(frame, app, chunks[1]);

    // Zone 3: Recent orders footer (only if visible)
    if app.show_orders {
        render_recent_orders(frame, app, chunks[2]);
    }

    // Zone 4: Help bar
    render_help_bar(frame, app, chunks[3]);

    // Overlays (drawn last, on top of everything)
    match app.mode {
        AppMode::Confirming => render_confirmation_overlay(frame, app),
        AppMode::ShowingResult => render_result_overlay(frame, app),
        AppMode::Submitting => render_submitting_overlay(frame),
        AppMode::BracketWaiting => render_bracket_waiting_overlay(frame, app),
        AppMode::ShowingBracketResult => render_bracket_result_overlay(frame, app),
        AppMode::Editing => {}
    }
}

/// Render the header bar (Zone 1) with symbol, order type label, and mark price.
fn render_header(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let mark_str = match app.mark_price {
        Some(price) => {
            let precision = app.symbol_info.price_precision as usize;
            format!("Mark: {:.prec$}", price, prec = precision)
        }
        None => "Mark: --".to_string(),
    };

    let content = Line::from(vec![
        Span::styled(
            format!(" {} ", app.symbol),
            Style::default()
                .fg(app.theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(app.theme.text_muted)),
        Span::styled(
            app.order_type.label(),
            Style::default().fg(app.theme.text_secondary),
        ),
        Span::styled(" | ", Style::default().fg(app.theme.text_muted)),
        Span::styled(mark_str, Style::default().fg(Color::Yellow)),
    ]);

    let header = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Trade Ticket ")
            .border_style(Style::default().fg(app.theme.border)),
    );

    frame.render_widget(header, area);
}

/// Render the info row showing notional value, margin required, and available balance.
fn render_info_row(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let exceeds_cap = app.notional_exceeds_cap();
    let notional_str = match (app.notional_value(), app.notional_cap()) {
        (Some(v), Some(cap)) if exceeds_cap => format!("{:.2} (max {})", v, cap),
        (Some(v), _) => format!("{:.2}", v),
        _ => "--".to_string(),
    };
    let notional_color = if exceeds_cap { Color::Red } else { Color::Yellow };
    let margin_str = match app.margin_required() {
        Some(v) => format!("{:.2}", v),
        None => "--".to_string(),
    };
    let balance_str = match app.available_balance {
        Some(v) => format!("{:.2}", v),
        None => "Loading...".to_string(),
    };

    let leverage_str = if app.leverage_editing {
        // Show inline editing indicator with current input
        let input = app.leverage_input.value();
        if input.is_empty() {
            "?x".to_string()
        } else {
            format!("{}x_", input)
        }
    } else {
        match (app.leverage, app.max_leverage) {
            (Some(lev), Some(max)) => format!("{}x / {}x", lev, max),
            (Some(lev), None) => format!("{}x", lev),
            _ => "--".to_string(),
        }
    };

    let lev_color = if app.leverage_editing { Color::Yellow } else { Color::Magenta };

    let content = Line::from(vec![
        Span::styled(" Lev: ", Style::default().fg(app.theme.text_muted)),
        Span::styled(&leverage_str, Style::default().fg(lev_color)),
        Span::styled(" | ", Style::default().fg(app.theme.text_muted)),
        Span::styled("Notional: ", Style::default().fg(app.theme.text_muted)),
        Span::styled(&notional_str, Style::default().fg(notional_color)),
        Span::styled(" | ", Style::default().fg(app.theme.text_muted)),
        Span::styled("Margin: ", Style::default().fg(app.theme.text_muted)),
        Span::styled(&margin_str, Style::default().fg(Color::Cyan)),
        Span::styled(" | ", Style::default().fg(app.theme.text_muted)),
        Span::styled("Balance: ", Style::default().fg(app.theme.text_muted)),
        Span::styled(&balance_str, Style::default().fg(Color::Green)),
    ]);

    let para = Paragraph::new(content);
    frame.render_widget(para, area);
}

/// Render the position strip showing current position context (side, size, entry, PnL).
///
/// Uses app.mark_price (live from watch channel) for PnL computation instead of
/// position.mark_price (stale from last REST/WS update). Shows "No position" when
/// current_position is None.
fn render_position_strip(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let pos = match &app.current_position {
        Some(p) => p,
        None => {
            let empty = Paragraph::new(Line::from(Span::styled(
                " No position",
                Style::default().fg(app.theme.text_muted),
            )));
            frame.render_widget(empty, area);
            return;
        }
    };

    // Use live mark price for PnL computation; fall back to position's REST mark
    let live_mark = app.mark_price.unwrap_or(pos.mark_price);
    let pnl = pos.position_amt * (live_mark - pos.entry_price);
    let pnl_pct = {
        let entry_notional = pos.position_amt.abs() * pos.entry_price;
        if entry_notional.is_zero() {
            Decimal::ZERO
        } else {
            (pnl / entry_notional) * Decimal::from(pos.leverage) * Decimal::from(100)
        }
    };
    let pnl_color = if pnl >= Decimal::ZERO {
        Color::Green
    } else {
        Color::Red
    };

    let side_str = match pos.position_side {
        PositionSide::Long => "LONG",
        PositionSide::Short => "SHORT",
    };
    let side_color = match pos.position_side {
        PositionSide::Long => Color::Green,
        PositionSide::Short => Color::Red,
    };

    let price_prec = app.symbol_info.price_precision as usize;
    let qty_prec = app.symbol_info.quantity_precision as usize;

    let content = Line::from(vec![
        Span::styled(" Pos: ", Style::default().fg(app.theme.text_muted)),
        Span::styled(
            side_str,
            Style::default()
                .fg(side_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {:.prec$}", pos.position_amt.abs(), prec = qty_prec),
            Style::default().fg(app.theme.text_primary),
        ),
        Span::styled(
            format!(" @ {:.prec$}", pos.entry_price, prec = price_prec),
            Style::default().fg(app.theme.text_secondary),
        ),
        Span::styled(" | PnL: ", Style::default().fg(app.theme.text_muted)),
        Span::styled(
            format!("{:.2} ({:+.1}%)", pnl, pnl_pct),
            Style::default().fg(pnl_color),
        ),
    ]);

    frame.render_widget(Paragraph::new(content), area);
}

/// Render the form area (Zone 2) with info row and all interactive fields.
fn render_form(frame: &mut Frame, app: &mut TradeApp, area: Rect) {
    // Build dynamic constraints based on which fields are visible
    let needs_price = app.order_type.needs_price();
    let needs_stop = app.order_type.needs_stop_price();
    let needs_callback = app.order_type.needs_callback_rate();
    let needs_activation = app.order_type.needs_activation_price();
    let has_error = app.error_message.is_some();

    let mut constraints = vec![
        Constraint::Length(1), // Info row (notional/margin/balance)
        Constraint::Length(1), // Position strip
        Constraint::Length(3), // Side
        Constraint::Length(3), // Order Type
        Constraint::Length(3), // Quantity
    ];

    if needs_price {
        constraints.push(Constraint::Length(3)); // Price
    }
    if needs_stop {
        constraints.push(Constraint::Length(3)); // Stop Price
    }
    if needs_callback {
        constraints.push(Constraint::Length(3)); // Callback Rate
    }
    if needs_activation {
        constraints.push(Constraint::Length(3)); // Activation Price
    }

    // Bracket fields (conditional on bracket_enabled)
    constraints.push(Constraint::Length(1)); // Bracket toggle indicator (always shown)
    if app.bracket_enabled {
        constraints.push(Constraint::Length(3)); // Bracket SL Price
        constraints.push(Constraint::Length(3)); // Bracket TP Price
    }

    constraints.push(Constraint::Length(3)); // ReduceOnly

    if needs_price {
        constraints.push(Constraint::Length(3)); // TimeInForce
    }

    constraints.push(Constraint::Length(3)); // Submit

    if has_error {
        constraints.push(Constraint::Length(1)); // Error message
    }

    constraints.push(Constraint::Min(0)); // Remaining space

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut row_idx: usize = 0;

    // Info row (notional, margin, balance)
    render_info_row(frame, app, rows[row_idx]);
    row_idx += 1;

    // Position strip (current position context)
    render_position_strip(frame, app, rows[row_idx]);
    row_idx += 1;

    // Side selector
    render_side_field(frame, app, rows[row_idx]);
    row_idx += 1;

    // Order type selector
    render_order_type_field(frame, app, rows[row_idx]);
    row_idx += 1;

    // Quantity input
    render_text_input_field(frame, app, rows[row_idx], "Qty", Focus::Quantity);
    row_idx += 1;

    // Price input (conditional)
    if needs_price {
        render_text_input_field(frame, app, rows[row_idx], "Price", Focus::Price);
        row_idx += 1;
    }

    // Stop price input (conditional)
    if needs_stop {
        render_text_input_field(frame, app, rows[row_idx], "Stop Price", Focus::StopPrice);
        row_idx += 1;
    }

    // Callback rate input (conditional -- TrailingStop only)
    if needs_callback {
        render_text_input_field(frame, app, rows[row_idx], "Callback %", Focus::CallbackRate);
        row_idx += 1;
    }

    // Activation price input (conditional -- TrailingStop only)
    if needs_activation {
        render_text_input_field(frame, app, rows[row_idx], "Act. Price", Focus::ActivationPrice);
        row_idx += 1;
    }

    // Bracket toggle indicator (always shown so user knows the feature exists)
    render_bracket_toggle(frame, app, rows[row_idx]);
    row_idx += 1;

    // Bracket SL/TP price inputs (conditional on bracket_enabled)
    if app.bracket_enabled {
        render_text_input_field(frame, app, rows[row_idx], "SL Price", Focus::BracketSLPrice);
        row_idx += 1;
        render_text_input_field(frame, app, rows[row_idx], "TP Price", Focus::BracketTPPrice);
        row_idx += 1;
    }

    // Reduce-only toggle
    render_reduce_only_field(frame, app, rows[row_idx]);
    row_idx += 1;

    // Time-in-force selector (conditional, same as Price)
    if needs_price {
        render_time_in_force_field(frame, app, rows[row_idx]);
        row_idx += 1;
    }

    // Submit button
    render_submit_button(frame, app, rows[row_idx]);
    row_idx += 1;

    // Error message (if present)
    if has_error {
        if let Some(ref msg) = app.error_message {
            let error = Paragraph::new(Line::from(Span::styled(
                format!(" {}", msg),
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )));
            frame.render_widget(error, rows[row_idx]);
        }
    }
}

/// Render the Side selector field (BUY / SELL with color coding).
fn render_side_field(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let is_focused = app.focus == Focus::Side;
    let border_color = if is_focused {
        app.theme.text_primary
    } else {
        Color::DarkGray
    };

    let buy_style = if app.side == OrderSide::Buy {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(Color::Green)
    };

    let sell_style = if app.side == OrderSide::Sell {
        Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(Color::Red)
    };

    let content = Line::from(vec![
        Span::raw("  "),
        Span::styled(" BUY ", buy_style),
        Span::raw("   "),
        Span::styled(" SELL ", sell_style),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Side ")
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(content).block(block);
    frame.render_widget(para, area);
}

/// Render the Order Type selector field with left/right arrows.
fn render_order_type_field(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let is_focused = app.focus == Focus::OrderType;
    let border_color = if is_focused {
        app.theme.text_primary
    } else {
        Color::DarkGray
    };

    let label = app.order_type.label();
    let content = Line::from(vec![
        Span::styled(" < ", Style::default().fg(app.theme.text_muted)),
        Span::styled(
            label,
            Style::default()
                .fg(app.theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" > ", Style::default().fg(app.theme.text_muted)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Type ")
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(content).alignment(Alignment::Center).block(block);
    frame.render_widget(para, area);
}

/// Render a text input field (Quantity, Price, or StopPrice) with cursor support.
fn render_text_input_field(
    frame: &mut Frame,
    app: &TradeApp,
    area: Rect,
    title: &str,
    focus_variant: Focus,
) {
    let is_focused = app.focus == focus_variant;
    let border_color = if is_focused {
        app.theme.text_primary
    } else {
        Color::DarkGray
    };

    let (value, cursor_pos) = match focus_variant {
        Focus::Quantity => (app.quantity_input.value(), app.quantity_input.cursor()),
        Focus::Price => (app.price_input.value(), app.price_input.cursor()),
        Focus::StopPrice => (app.stop_price_input.value(), app.stop_price_input.cursor()),
        Focus::CallbackRate => (app.callback_rate_input.value(), app.callback_rate_input.cursor()),
        Focus::ActivationPrice => (app.activation_price_input.value(), app.activation_price_input.cursor()),
        Focus::BracketSLPrice => (app.bracket_sl_price.value(), app.bracket_sl_price.cursor()),
        Focus::BracketTPPrice => (app.bracket_tp_price.value(), app.bracket_tp_price.cursor()),
        _ => ("", 0),
    };

    let content = Paragraph::new(Line::from(Span::styled(
        value,
        Style::default().fg(app.theme.text_primary),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", title))
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(content, area);

    // Set cursor position when this field is focused
    if is_focused && app.mode == AppMode::Editing {
        // Position cursor inside the bordered block: +1 for left border
        let cursor_x = area.x + cursor_pos as u16 + 1;
        let cursor_y = area.y + 1; // +1 for top border
        if cursor_x < area.x + area.width.saturating_sub(1) {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }
}

/// Render the Reduce Only toggle field (OFF / ON with visual indicator).
fn render_reduce_only_field(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let is_focused = app.focus == Focus::ReduceOnly;
    let border_color = if is_focused {
        app.theme.text_primary
    } else {
        Color::DarkGray
    };

    let off_style = if !app.reduce_only {
        Style::default()
            .fg(app.theme.text_primary)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(app.theme.text_secondary)
    };

    let on_style = if app.reduce_only {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(app.theme.text_secondary)
    };

    let content = Line::from(vec![
        Span::raw("  "),
        Span::styled(" OFF ", off_style),
        Span::raw("   "),
        Span::styled(" ON ", on_style),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Reduce Only ")
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(content).block(block);
    frame.render_widget(para, area);
}

/// Render the bracket toggle indicator row showing [B] BRACKET ON/OFF.
fn render_bracket_toggle(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let (status_text, status_style) = if app.bracket_enabled {
        (
            "ON",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("OFF", Style::default().fg(Color::DarkGray))
    };

    let content = Line::from(vec![
        Span::styled(" [B] ", Style::default().fg(app.theme.text_muted)),
        Span::styled("BRACKET ", Style::default().fg(app.theme.text_secondary)),
        Span::styled(status_text, status_style),
    ]);

    let para = Paragraph::new(content);
    frame.render_widget(para, area);
}

/// Render the Time in Force selector field with left/right arrows.
fn render_time_in_force_field(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let is_focused = app.focus == Focus::TimeInForce;
    let border_color = if is_focused {
        app.theme.text_primary
    } else {
        Color::DarkGray
    };

    let label = app.time_in_force.label();
    let content = Line::from(vec![
        Span::styled(" < ", Style::default().fg(app.theme.text_muted)),
        Span::styled(
            label,
            Style::default()
                .fg(app.theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" > ", Style::default().fg(app.theme.text_muted)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Time in Force ")
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(content).alignment(Alignment::Center).block(block);
    frame.render_widget(para, area);
}

/// Render the Submit button.
fn render_submit_button(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let is_focused = app.focus == Focus::Submit;
    let border_color = if is_focused {
        app.theme.text_primary
    } else {
        Color::DarkGray
    };

    let style = if is_focused {
        Style::default()
            .fg(app.theme.text_primary)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(app.theme.text_secondary)
    };

    let content = Paragraph::new(Line::from(Span::styled("[ SUBMIT ORDER ]", style)))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );

    frame.render_widget(content, area);
}

/// Render the help bar (Zone 4) with keybinding hints per current mode.
///
/// Shows F1-F4 quick-size hints when focused on Quantity in Editing mode.
fn render_help_bar(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let help_text = match app.mode {
        AppMode::Editing => {
            if app.leverage_editing {
                match app.max_leverage {
                    Some(max) => format!(" Type leverage (1-{}), Enter to set, Esc to cancel", max),
                    None => " Type leverage, Enter to set, Esc to cancel".to_string(),
                }
            } else {
                let base = " Tab/S-Tab: nav | b/s: side | o: type | r: reduce | l: leverage | B: bracket | O: orders | q: quit";
                if app.focus == Focus::Quantity {
                    // Append quick-size hints when on Quantity
                    format!("{} | F1:25% F2:50% F3:75% F4:100%", base)
                } else {
                    base.to_string()
                }
            }
        }
        AppMode::Confirming => " Y: confirm | N: cancel".to_string(),
        AppMode::Submitting => " Submitting order...".to_string(),
        AppMode::ShowingResult => " Press any key to continue".to_string(),
        AppMode::BracketWaiting => " Waiting for entry fill... | Esc/n: cancel bracket".to_string(),
        AppMode::ShowingBracketResult => " Press any key to continue".to_string(),
    };

    let help = Paragraph::new(Line::from(help_text)).style(
        Style::default()
            .fg(app.theme.text_secondary)
            .bg(app.theme.background_highlight),
    );

    frame.render_widget(help, area);
}

/// Render the recent orders footer (Zone 3).
///
/// Shows a compact table of the last 5 orders for the current symbol,
/// or a "No recent orders" message if the list is empty.
fn render_recent_orders(frame: &mut Frame, app: &TradeApp, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .title(" Recent Orders ")
        .border_style(Style::default().fg(app.theme.border));

    if app.recent_orders.is_empty() {
        let empty_msg = Paragraph::new(Line::from(Span::styled(
            " No recent orders",
            Style::default().fg(app.theme.text_muted),
        )))
        .block(block);
        frame.render_widget(empty_msg, area);
        return;
    }

    let header_style = Style::default()
        .fg(app.theme.text_secondary)
        .add_modifier(Modifier::BOLD);

    let header = Row::new(vec![
        Cell::from("Time").style(header_style),
        Cell::from("Side").style(header_style),
        Cell::from("Type").style(header_style),
        Cell::from("Qty").style(header_style),
        Cell::from("Price").style(header_style),
        Cell::from("Status").style(header_style),
    ]);

    let rows: Vec<Row> = app
        .recent_orders
        .iter()
        .take(5)
        .map(|order| {
            // Format time as HH:MM:SS from millisecond timestamp
            let time_str = chrono::DateTime::from_timestamp_millis(order.time as i64)
                .map(|dt| dt.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "---".to_string());

            // Side with color
            let side_cell = match order.side {
                OrderSide::Buy => Cell::from("BUY").style(Style::default().fg(Color::Green)),
                OrderSide::Sell => Cell::from("SELL").style(Style::default().fg(Color::Red)),
            };

            // Order type
            let type_str = format!("{}", order.order_type);

            // Quantity and price
            let qty_str = format!("{}", order.orig_qty);
            let price_str = format!("{}", order.price);

            // Status with color coding (match order_table.rs labels)
            let (status_str, status_style) = match order.status {
                OrderStatus::New | OrderStatus::PartiallyFilled => {
                    ("OPEN", Style::default().fg(Color::Yellow))
                }
                OrderStatus::Filled => ("FILLED", Style::default().fg(Color::Green)),
                OrderStatus::Canceled => ("CANCELLED", Style::default().fg(Color::DarkGray)),
                _ => ("--", Style::default().fg(Color::White)),
            };

            Row::new(vec![
                Cell::from(time_str),
                side_cell,
                Cell::from(type_str),
                Cell::from(qty_str),
                Cell::from(price_str),
                Cell::from(status_str).style(status_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(4),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block);

    frame.render_widget(table, area);
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

/// Render the confirmation overlay when AppMode is Confirming.
fn render_confirmation_overlay(frame: &mut Frame, app: &TradeApp) {
    let snapshot = match &app.confirm_snapshot {
        Some(s) => s,
        None => return,
    };

    let price_prec = app.symbol_info.price_precision as usize;
    let qty_prec = app.symbol_info.quantity_precision as usize;

    // Compute overlay height dynamically: base (symbol, side, type, qty, mark, blank, prompt
    // + 2 border + 2 padding) = 11, plus conditional fields
    let mut overlay_height: u16 = 11;
    if snapshot.price.is_some() {
        overlay_height += 1;
    }
    if snapshot.stop_price.is_some() {
        overlay_height += 1;
    }
    if snapshot.callback_rate.is_some() {
        overlay_height += 1;
    }
    if snapshot.activation_price.is_some() {
        overlay_height += 1;
    }
    // Bracket SL/TP: separator line + 1 for each present price
    let has_bracket = snapshot.bracket_sl_price.is_some() || snapshot.bracket_tp_price.is_some();
    if has_bracket {
        overlay_height += 1; // separator "── Bracket ──"
    }
    if snapshot.bracket_sl_price.is_some() {
        overlay_height += 1;
    }
    if snapshot.bracket_tp_price.is_some() {
        overlay_height += 1;
    }
    let overlay_area = centered_rect(50, overlay_height, frame.area());

    // Clear underlying content
    frame.render_widget(Clear, overlay_area);

    let side_span = match snapshot.side {
        OrderSide::Buy => Span::styled(
            "BUY",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        OrderSide::Sell => Span::styled(
            "SELL",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let mut lines = vec![
        Line::from(vec![
            Span::raw("  Symbol:     "),
            Span::styled(&snapshot.symbol, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![Span::raw("  Side:       "), side_span]),
        Line::from(vec![
            Span::raw("  Type:       "),
            Span::raw(snapshot.order_type.label()),
        ]),
        Line::from(vec![
            Span::raw("  Quantity:   "),
            Span::raw(format!("{:.prec$}", snapshot.quantity, prec = qty_prec)),
        ]),
    ];

    if let Some(price) = snapshot.price {
        lines.push(Line::from(vec![
            Span::raw("  Price:      "),
            Span::raw(format!("{:.prec$}", price, prec = price_prec)),
        ]));
    }

    if let Some(stop_price) = snapshot.stop_price {
        lines.push(Line::from(vec![
            Span::raw("  Stop Price: "),
            Span::raw(format!("{:.prec$}", stop_price, prec = price_prec)),
        ]));
    }

    if let Some(cr) = snapshot.callback_rate {
        lines.push(Line::from(vec![
            Span::raw("  Callback %: "),
            Span::styled(
                format!("{}%", cr),
                Style::default().fg(Color::Cyan),
            ),
        ]));
    }

    if let Some(ap) = snapshot.activation_price {
        lines.push(Line::from(vec![
            Span::raw("  Act. Price: "),
            Span::raw(format!("{:.prec$}", ap, prec = price_prec)),
        ]));
    }

    // Bracket SL/TP prices (if present)
    let has_bracket = snapshot.bracket_sl_price.is_some() || snapshot.bracket_tp_price.is_some();
    if has_bracket {
        lines.push(Line::from(Span::styled(
            "  ── Bracket ──",
            Style::default().fg(app.theme.text_muted),
        )));
        if let Some(sl) = snapshot.bracket_sl_price {
            lines.push(Line::from(vec![
                Span::raw("  Stop Loss:   "),
                Span::styled(
                    format!("{:.prec$}", sl, prec = price_prec),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
        if let Some(tp) = snapshot.bracket_tp_price {
            lines.push(Line::from(vec![
                Span::raw("  Take Profit: "),
                Span::styled(
                    format!("{:.prec$}", tp, prec = price_prec),
                    Style::default().fg(Color::Green),
                ),
            ]));
        }
    }

    // USER LOCKED DECISION: Use live mark price that updates in real-time,
    // not the frozen snapshot value. Fall back to snapshot if live unavailable.
    let mark_str = match app.mark_price {
        Some(mp) => format!("{:.prec$}", mp, prec = price_prec),
        None => match snapshot.mark_price {
            Some(mp) => format!("{:.prec$}", mp, prec = price_prec),
            None => "--".to_string(),
        },
    };
    lines.push(Line::from(vec![
        Span::raw("  Mark Price: "),
        Span::styled(mark_str, Style::default().fg(Color::Yellow)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press Y to confirm, N to cancel",
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let confirm_title = if has_bracket {
        " Confirm Bracket Order "
    } else {
        " Confirm Order "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(confirm_title)
        .border_style(Style::default().fg(app.theme.border));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the result overlay when AppMode is ShowingResult.
fn render_result_overlay(frame: &mut Frame, app: &TradeApp) {
    let overlay_area = centered_rect(50, 12, frame.area());

    // Clear underlying content
    frame.render_widget(Clear, overlay_area);

    match &app.order_result {
        Some(Ok(response)) => {
            let mut lines = vec![
                Line::from(vec![
                    Span::raw("  Status:        "),
                    Span::styled(
                        &response.status,
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  Order ID:      "),
                    Span::raw(response.order_id.to_string()),
                ]),
                Line::from(vec![
                    Span::raw("  Executed Qty:  "),
                    Span::raw(response.executed_qty.to_string()),
                ]),
            ];

            if !response.avg_price.is_zero() {
                lines.push(Line::from(vec![
                    Span::raw("  Avg Price:     "),
                    Span::raw(response.avg_price.to_string()),
                ]));
            }

            lines.push(Line::from(vec![
                Span::raw("  Cum Quote:     "),
                Span::raw(response.cum_quote.to_string()),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Press any key to continue",
                Style::default().fg(app.theme.text_muted),
            )));

            let block = Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " Order Submitted ",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
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
                    "  Press any key to continue",
                    Style::default().fg(app.theme.text_muted),
                )),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " Order Failed ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(Color::Red));

            let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(block);
            frame.render_widget(para, overlay_area);
        }
        None => {}
    }
}

/// Render a small centered overlay showing "Submitting order..." while waiting.
fn render_submitting_overlay(frame: &mut Frame) {
    let overlay_area = centered_rect(40, 5, frame.area());

    frame.render_widget(Clear, overlay_area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Submitting order...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(lines).alignment(Alignment::Center).block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the bracket waiting overlay when AppMode is BracketWaiting.
///
/// Shows "Waiting for entry fill..." with the entry order ID if available,
/// and an Esc/n cancel instruction. Yellow border matches the Submitting overlay.
fn render_bracket_waiting_overlay(frame: &mut Frame, app: &TradeApp) {
    let overlay_area = centered_rect(50, 7, frame.area());
    frame.render_widget(Clear, overlay_area);

    let mut lines = vec![Line::from("")];

    // Show entry order ID if available from bracket state
    if let Some(BracketState::WaitingForFill { entry_order_id }) = &app.bracket_state {
        lines.push(Line::from(Span::styled(
            format!("  Waiting for entry fill...  (ID: {})", entry_order_id),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Waiting for entry fill...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press Esc to cancel bracket",
        Style::default().fg(app.theme.text_muted),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Bracket Order ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, overlay_area);
}

/// Render the bracket result overlay when AppMode is ShowingBracketResult.
///
/// Shows per-leg status for entry, SL, and TP with color coding:
/// - Success = green with order ID
/// - Failed = red with error message on next line
/// - Skipped = dimmed gray
/// - Pending = yellow dots
///
/// If any leg failed, shows a warning about unprotected position.
/// NEVER auto-dismisses (financial safety requirement).
fn render_bracket_result_overlay(frame: &mut Frame, app: &TradeApp) {
    let (entry, sl, tp) = match &app.bracket_state {
        Some(BracketState::Complete { entry, sl, tp }) => (entry, sl, tp),
        _ => return, // Not in Complete state, nothing to render
    };

    // Determine if any leg failed for color/warning decisions
    let any_failed = matches!(entry, LegResult::Failed { .. })
        || matches!(sl, LegResult::Failed { .. })
        || matches!(tp, LegResult::Failed { .. });

    // Compute dynamic overlay height:
    // base 10: 2 border + 1 blank top + 3 leg lines + 1 blank + 1 prompt + 1 warning placeholder + 1 blank bottom
    let mut height: u16 = 10;
    // Add 1 for each failed leg (error detail line)
    if matches!(entry, LegResult::Failed { .. }) {
        height += 1;
    }
    if matches!(sl, LegResult::Failed { .. }) {
        height += 1;
    }
    if matches!(tp, LegResult::Failed { .. }) {
        height += 1;
    }
    // If no failures, reclaim the warning placeholder line
    if !any_failed {
        height -= 1;
    }

    let overlay_area = centered_rect(60, height, frame.area());
    frame.render_widget(Clear, overlay_area);

    let mut lines: Vec<Line> = vec![Line::from("")];

    // Entry leg -- show fill details if available from order_result
    let entry_line = match entry {
        LegResult::Success { order_id } => {
            // Try to show fill details from order_result
            if let Some(Ok(response)) = &app.order_result {
                let qty_str = response.executed_qty.to_string();
                let price_str = if !response.avg_price.is_zero() {
                    response.avg_price.to_string()
                } else {
                    "--".to_string()
                };
                Line::from(Span::styled(
                    format!(
                        "  Entry: FILLED @ {} (qty: {}, ID: {})",
                        price_str, qty_str, order_id
                    ),
                    Style::default().fg(Color::Green),
                ))
            } else {
                Line::from(Span::styled(
                    format!("  Entry: FILLED  (ID: {})", order_id),
                    Style::default().fg(Color::Green),
                ))
            }
        }
        LegResult::Failed { error } => {
            lines.push(Line::from(Span::styled(
                "  Entry: FAILED",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!("         {}", error),
                Style::default().fg(Color::Red),
            )));
            // Return early for this leg since we pushed two lines
            Line::from("") // placeholder, won't be used
        }
        LegResult::Skipped => Line::from(Span::styled(
            "  Entry: --",
            Style::default().fg(Color::DarkGray),
        )),
        LegResult::_Pending => Line::from(Span::styled(
            "  Entry: ...",
            Style::default().fg(Color::Yellow),
        )),
    };

    // For Failed entry, lines were already pushed inline above
    if !matches!(entry, LegResult::Failed { .. }) {
        lines.push(entry_line);
    }

    // SL leg
    render_leg_lines(&mut lines, "SL", sl);

    // TP leg
    render_leg_lines(&mut lines, "TP", tp);

    // Warning if any leg failed
    if any_failed {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ! Position may lack protection!",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press any key to dismiss",
        Style::default().fg(app.theme.text_muted),
    )));

    let (title_color, border_color) = if any_failed {
        (Color::Yellow, Color::Yellow)
    } else {
        (Color::Green, Color::Green)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Bracket Result ",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, overlay_area);
}

/// Push leg status lines for a bracket leg (SL or TP) into the lines vector.
fn render_leg_lines<'a>(lines: &mut Vec<Line<'a>>, label: &str, leg: &LegResult) {
    // Pad label to align with "Entry:" (5 chars)
    let padded_label = format!("{:<5}", label);
    match leg {
        LegResult::Success { order_id } => {
            lines.push(Line::from(Span::styled(
                format!("  {}: PLACED  (ID: {})", padded_label.trim_end(), order_id),
                Style::default().fg(Color::Green),
            )));
        }
        LegResult::Failed { error } => {
            lines.push(Line::from(Span::styled(
                format!("  {}: FAILED", padded_label.trim_end()),
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!("         {}", error),
                Style::default().fg(Color::Red),
            )));
        }
        LegResult::Skipped => {
            lines.push(Line::from(Span::styled(
                format!("  {}: --", padded_label.trim_end()),
                Style::default().fg(Color::DarkGray),
            )));
        }
        LegResult::_Pending => {
            lines.push(Line::from(Span::styled(
                format!("  {}: ...", padded_label.trim_end()),
                Style::default().fg(Color::Yellow),
            )));
        }
    }
}
