// src/tui/widgets/overview_dashboard.rs
// Render widget for the market overview dashboard: title bar, column headers,
// sortable table with color-coded 24h change and human-readable volumes,
// cursor highlighting, and help bar.

use crate::data::ticker::TickerSortColumn;
use crate::tui::overview_app::OverviewApp;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use rust_decimal::Decimal;

/// Widget that renders the complete market overview dashboard view.
///
/// Borrows the OverviewApp for read-only access to table state,
/// sort settings, theme, cursor position, and input mode.
pub struct OverviewDashboardWidget<'a> {
    app: &'a OverviewApp,
}

impl<'a> OverviewDashboardWidget<'a> {
    pub fn new(app: &'a OverviewApp) -> Self {
        Self { app }
    }
}

impl<'a> Widget for OverviewDashboardWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Vertical layout: title (1), header (1), table (remaining), help (1)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Length(1), // Table header
                Constraint::Min(0),    // Table body
                Constraint::Length(1), // Help bar
            ])
            .split(area);

        render_title_bar(self.app, chunks[0], buf);
        render_table_header(self.app, chunks[1], buf);
        render_table_body(self.app, chunks[2], buf);
        render_help_bar(self.app, chunks[3], buf);
    }
}

/// Render the title bar with dashboard name, pair count, filter/sort indicators.
fn render_title_bar(app: &OverviewApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    // Check for symbol input mode first
    if let Some(ref input_buffer) = app.symbol_input {
        let line = Line::from(vec![
            Span::styled(
                " MARKET OVERVIEW ",
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Symbol: {}_", input_buffer),
                Style::default().fg(theme.status_warning),
            ),
        ]);
        Paragraph::new(line).render(area, buf);
        return;
    }

    let mut spans = vec![Span::styled(
        " MARKET OVERVIEW ",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    )];

    // Right side: counts, filter, sort indicator
    let mut right_parts = Vec::new();

    let visible_count = app.visible_entries().len();
    let total_count = app.table.len();

    if app.symbol_filter.is_some() {
        right_parts.push(format!("{}/{} pairs", visible_count, total_count));
    } else {
        right_parts.push(format!("{} pairs", total_count));
    }

    if let Some(ref filter) = app.symbol_filter {
        right_parts.push(format!("[{}]", filter));
    }

    // Sort indicator
    let direction_arrow = if app.sort_ascending { "^" } else { "v" };
    right_parts.push(format!("{} {}", app.sort_column.label(), direction_arrow));

    let right_text = format!(" {} ", right_parts.join(" | "));
    let left_len: usize = spans.iter().map(|s| s.width()).sum();
    let right_len = right_text.len();
    let total_len = left_len + right_len;
    if total_len < area.width as usize {
        let padding = area.width as usize - total_len;
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.push(Span::styled(
        right_text,
        Style::default().fg(theme.text_muted),
    ));

    let line = Line::from(spans);
    Paragraph::new(line).render(area, buf);
}

/// Render the table header row.
fn render_table_header(app: &OverviewApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let cols = compute_columns(area.width as usize);

    let mut spans = vec![Span::raw(" ")];

    spans.push(header_span(
        "Symbol",
        cols.symbol,
        app.sort_column == TickerSortColumn::Symbol,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    spans.push(header_span(
        "Price",
        cols.price,
        app.sort_column == TickerSortColumn::Price,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    spans.push(header_span(
        "24h %",
        cols.change,
        app.sort_column == TickerSortColumn::Change,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    spans.push(header_span(
        "High",
        cols.high,
        false,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    spans.push(header_span(
        "Low",
        cols.low,
        false,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    spans.push(header_span(
        "Volume",
        cols.volume,
        app.sort_column == TickerSortColumn::Volume,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    spans.push(header_span(
        "Trades",
        cols.trades,
        app.sort_column == TickerSortColumn::Trades,
        theme.text_secondary,
    ));

    let line = Line::from(spans);
    Paragraph::new(line).render(area, buf);
}

/// Create a header span with optional sort indicator (*).
fn header_span(label: &str, width: usize, is_sorted: bool, color: Color) -> Span<'static> {
    let text = if is_sorted {
        format!("{:<width$}", format!("{} *", label), width = width)
    } else {
        format!("{:<width$}", label, width = width)
    };

    Span::styled(
        text,
        Style::default()
            .fg(color)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::DIM),
    )
}

/// Render the scrollable table body with cursor highlighting.
fn render_table_body(app: &OverviewApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let visible = app.visible_entries();

    if visible.is_empty() {
        let msg = if app.symbol_filter.is_some() {
            format!(
                "No pairs matching '{}'",
                app.symbol_filter.as_deref().unwrap_or("")
            )
        } else {
            "No ticker data available".to_string()
        };

        let center_y = area.height / 2;
        if center_y < area.height {
            let center_area = Rect::new(area.x, area.y + center_y, area.width, 1);
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(theme.text_muted),
            )))
            .alignment(ratatui::layout::Alignment::Center)
            .render(center_area, buf);
        }
        return;
    }

    let visible_rows = area.height as usize;
    let cols = compute_columns(area.width as usize);

    // Adjust scroll_offset to keep selected_index visible
    // (We read but don't mutate app, so this is computed locally for rendering)
    let mut effective_scroll = app.scroll_offset;
    if app.selected_index < effective_scroll {
        effective_scroll = app.selected_index;
    } else if app.selected_index >= effective_scroll + visible_rows {
        effective_scroll = app.selected_index.saturating_sub(visible_rows - 1);
    }

    let start = effective_scroll;
    let end = (start + visible_rows).min(visible.len());

    for (row, idx) in (start..end).enumerate() {
        let entry = visible[idx];
        let y = area.y + row as u16;
        if y >= area.y + area.height {
            break;
        }

        let row_area = Rect::new(area.x, y, area.width, 1);
        let is_selected = idx == app.selected_index;

        // Row background for selected cursor
        let row_bg = if is_selected {
            Color::DarkGray
        } else {
            Color::Reset
        };

        // Determine 24h% color: green for positive, red for negative
        let change_color = if entry.price_change_percent > Decimal::ZERO {
            theme.bullish
        } else if entry.price_change_percent < Decimal::ZERO {
            theme.bearish
        } else {
            theme.text_primary
        };

        // Format columns
        let symbol_str = format!("{:<width$}", entry.symbol, width = cols.symbol);
        let price_str = format!("{:>width$}", entry.last_price, width = cols.price);

        // 24h % column: format with sign and 2 decimal places
        let change_str = if entry.price_change_percent > Decimal::ZERO {
            format!("{:>width$}", format!("+{:.2}%", entry.price_change_percent), width = cols.change)
        } else {
            format!("{:>width$}", format!("{:.2}%", entry.price_change_percent), width = cols.change)
        };

        let high_str = format!("{:>width$}", entry.high_price, width = cols.high);
        let low_str = format!("{:>width$}", entry.low_price, width = cols.low);
        let volume_str = format!("{:>width$}", format_volume(entry.quote_volume), width = cols.volume);
        let trades_str = format!("{:>width$}", format_trades(entry.trade_count), width = cols.trades);

        let sym_style = if is_selected {
            Style::default().fg(theme.text_primary).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };

        let mut spans = vec![
            Span::styled(" ", Style::default().bg(row_bg)),
        ];

        // Symbol
        spans.push(Span::styled(symbol_str, sym_style));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Price
        spans.push(Span::styled(
            price_str,
            Style::default().fg(theme.text_primary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // 24h % (color-coded)
        spans.push(Span::styled(
            change_str,
            Style::default().fg(change_color).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // High
        spans.push(Span::styled(
            high_str,
            Style::default().fg(theme.text_secondary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Low
        spans.push(Span::styled(
            low_str,
            Style::default().fg(theme.text_secondary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Volume
        spans.push(Span::styled(
            volume_str,
            Style::default().fg(theme.text_secondary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Trades
        spans.push(Span::styled(
            trades_str,
            Style::default().fg(theme.text_muted).bg(row_bg),
        ));

        // Fill remaining width with row background
        let used_width: usize = spans.iter().map(|s| s.width()).sum();
        if used_width < area.width as usize {
            let remaining = area.width as usize - used_width;
            spans.push(Span::styled(
                " ".repeat(remaining),
                Style::default().bg(row_bg),
            ));
        }

        let line = Line::from(spans);
        Paragraph::new(line).render(row_area, buf);
    }
}

/// Render the help bar with hotkey hints.
fn render_help_bar(app: &OverviewApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let help_text = if app.symbol_input.is_some() {
        "Enter: apply | Esc: cancel | type symbol name"
    } else {
        " j/k:scroll  g/G:top/bot  Tab:sort  r:reverse  s/p/d/v/n:col  /:filter  c:clear  Enter:select  q:quit"
    };

    let line = Line::from(Span::styled(
        help_text,
        Style::default().fg(theme.text_muted),
    ));
    Paragraph::new(line).render(area, buf);
}

/// Format volume with human-readable suffixes (K, M, B).
fn format_volume(vol: Decimal) -> String {
    let billion = Decimal::from(1_000_000_000);
    let million = Decimal::from(1_000_000);
    let thousand = Decimal::from(1_000);

    if vol >= billion {
        format!("{:.2}B", vol / billion)
    } else if vol >= million {
        format!("{:.1}M", vol / million)
    } else if vol >= thousand {
        format!("{:.1}K", vol / thousand)
    } else {
        format!("{:.0}", vol)
    }
}

/// Format trade count with comma separators.
fn format_trades(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        format!("{}", count)
    }
}

/// Column widths for the table layout.
struct ColumnWidths {
    symbol: usize,
    price: usize,
    change: usize,
    high: usize,
    low: usize,
    volume: usize,
    trades: usize,
}

/// Compute column widths based on available terminal width.
fn compute_columns(total_width: usize) -> ColumnWidths {
    // Fixed: " " prefix (1) + 7 column separators (7) = 8 overhead
    // Symbol: fixed 14 chars
    // Remaining: distributed proportionally across Price, 24h%, High, Low, Volume, Trades
    let fixed_overhead = 8;
    let symbol_w = 14;
    let fixed_cols = symbol_w + fixed_overhead;

    let remaining = total_width.saturating_sub(fixed_cols);

    // Proportions: Price 20%, Change 12%, High 18%, Low 18%, Volume 18%, Trades 14%
    let price_w = (remaining * 20 / 100).max(8);
    let change_w = (remaining * 12 / 100).max(8);
    let high_w = (remaining * 18 / 100).max(8);
    let low_w = (remaining * 18 / 100).max(8);
    let volume_w = (remaining * 18 / 100).max(8);
    let trades_w = remaining
        .saturating_sub(price_w)
        .saturating_sub(change_w)
        .saturating_sub(high_w)
        .saturating_sub(low_w)
        .saturating_sub(volume_w)
        .max(6);

    ColumnWidths {
        symbol: symbol_w,
        price: price_w,
        change: change_w,
        high: high_w,
        low: low_w,
        volume: volume_w,
        trades: trades_w,
    }
}
