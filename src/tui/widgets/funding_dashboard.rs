// src/tui/widgets/funding_dashboard.rs
// Render widget for the funding rate dashboard: title bar, sortable
// table with color-coded funding rates and countdown timer, and help bar.

use crate::data::funding_rates::SortColumn;
use crate::tui::funding_app::FundingApp;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Widget that renders the complete funding rate dashboard view.
///
/// Borrows the FundingApp for read-only access to table state,
/// sort settings, theme, and input mode.
pub struct FundingDashboardWidget<'a> {
    app: &'a FundingApp,
}

impl<'a> FundingDashboardWidget<'a> {
    pub fn new(app: &'a FundingApp) -> Self {
        Self { app }
    }
}

impl<'a> Widget for FundingDashboardWidget<'a> {
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

/// Render the title bar with dashboard name, filter/sort indicators, and pair count.
fn render_title_bar(app: &FundingApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    // Check for symbol input mode first
    if let Some(ref input_buffer) = app.symbol_input {
        let line = Line::from(vec![
            Span::styled(
                " FUNDING RATES ",
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
        " FUNDING RATES ",
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
        right_parts.push(format!("Filter: {}", filter));
    }

    // Sort indicator
    let direction = if app.sort_ascending { "ASC" } else { "DESC" };
    right_parts.push(format!("Sort: {} {}", app.sort_column.label(), direction));

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
fn render_table_header(app: &FundingApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let cols = compute_columns(area.width as usize);

    let mut spans = vec![Span::raw(" ")];

    // Symbol column
    spans.push(header_span(
        "Symbol",
        cols.symbol,
        app.sort_column == SortColumn::Symbol,
        theme.text_secondary,
    ));

    spans.push(Span::styled(" ", Style::default()));

    // Mark Price column
    spans.push(header_span(
        "Mark Price",
        cols.mark_price,
        app.sort_column == SortColumn::MarkPrice,
        theme.text_secondary,
    ));

    spans.push(Span::styled(" ", Style::default()));

    // Funding Rate column
    spans.push(header_span(
        "Funding (%)",
        cols.funding_rate,
        app.sort_column == SortColumn::FundingRate,
        theme.text_secondary,
    ));

    spans.push(Span::styled(" ", Style::default()));

    // Countdown column
    spans.push(header_span(
        "Countdown",
        cols.countdown,
        false,
        theme.text_secondary,
    ));

    spans.push(Span::styled(" ", Style::default()));

    // Annual Rate column
    spans.push(header_span(
        "Annual (%)",
        cols.annual_rate,
        app.sort_column == SortColumn::AnnualRate,
        theme.text_secondary,
    ));

    let line = Line::from(spans);
    Paragraph::new(line).render(area, buf);
}

/// Create a header span with optional sort indicator.
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

/// Render the scrollable table body.
fn render_table_body(app: &FundingApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let visible = app.visible_entries();

    if visible.is_empty() {
        let msg = if app.symbol_filter.is_some() {
            format!(
                "No pairs matching '{}'",
                app.symbol_filter.as_deref().unwrap_or("")
            )
        } else {
            "Waiting for funding rate data...".to_string()
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

    // Current time for countdown computation
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let start = app.scroll_offset;
    let end = (start + visible_rows).min(visible.len());

    // Thresholds for highlighting (absolute rate values)
    let high_threshold = Decimal::from_str("0.001").unwrap_or(Decimal::ZERO); // 0.1%
    let extreme_threshold = Decimal::from_str("0.005").unwrap_or(Decimal::ZERO); // 0.5%

    for (row, idx) in (start..end).enumerate() {
        let entry = visible[idx];
        let y = area.y + row as u16;
        if y >= area.y + area.height {
            break;
        }

        let row_area = Rect::new(area.x, y, area.width, 1);
        let abs_rate = entry.funding_rate.abs();

        // Determine rate color: green for negative (shorts pay), red for positive (longs pay)
        let rate_color = if entry.funding_rate < Decimal::ZERO {
            Color::Green
        } else if entry.funding_rate > Decimal::ZERO {
            Color::Red
        } else {
            theme.text_primary
        };

        // Determine modifier for high/extreme rates
        let rate_modifier = if abs_rate >= extreme_threshold {
            Modifier::BOLD
        } else if abs_rate >= high_threshold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };

        // Extreme rates get brighter color
        let rate_display_color = if abs_rate >= extreme_threshold {
            if entry.funding_rate < Decimal::ZERO {
                Color::LightGreen
            } else {
                Color::LightRed
            }
        } else {
            rate_color
        };

        // Format columns
        let symbol_str = format!("{:<width$}", entry.symbol, width = cols.symbol);
        let mark_price_str =
            format!("{:>width$}", entry.mark_price, width = cols.mark_price);

        // Funding rate as percentage (rate * 100)
        let funding_pct = entry.funding_rate * Decimal::from(100);
        let funding_str = format!(
            "{:>width$.4}",
            funding_pct,
            width = cols.funding_rate
        );

        // Countdown to next funding
        let countdown_str = format_countdown(entry.next_funding_time, now_ms, cols.countdown);

        // Annual rate: funding_rate * 3 * 365 * 100 (percentage)
        let annual_pct = entry.funding_rate * Decimal::from(3) * Decimal::from(365) * Decimal::from(100);
        let annual_str = format!("{:>width$.2}", annual_pct, width = cols.annual_rate);

        let mut spans = vec![Span::raw(" ")];

        // Symbol
        spans.push(Span::styled(
            symbol_str,
            Style::default().fg(theme.text_primary),
        ));
        spans.push(Span::raw(" "));

        // Mark price
        spans.push(Span::styled(
            mark_price_str,
            Style::default().fg(theme.text_secondary),
        ));
        spans.push(Span::raw(" "));

        // Funding rate (color-coded)
        spans.push(Span::styled(
            funding_str,
            Style::default()
                .fg(rate_display_color)
                .add_modifier(rate_modifier),
        ));
        spans.push(Span::raw(" "));

        // Countdown
        spans.push(Span::styled(
            countdown_str,
            Style::default().fg(theme.text_muted),
        ));
        spans.push(Span::raw(" "));

        // Annual rate (same color as funding rate)
        spans.push(Span::styled(
            annual_str,
            Style::default()
                .fg(rate_display_color)
                .add_modifier(rate_modifier),
        ));

        let line = Line::from(spans);
        Paragraph::new(line).render(row_area, buf);
    }
}

/// Render the help bar with hotkey hints.
fn render_help_bar(app: &FundingApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let help_text = if app.symbol_input.is_some() {
        "Enter: apply | Esc: cancel | type symbol name"
    } else {
        " j/k:scroll  g/G:top/bottom  Tab:sort  r:reverse  s/f/a/p:sort col  /:filter  c:clear  q:quit"
    };

    let line = Line::from(Span::styled(
        help_text,
        Style::default().fg(theme.text_muted),
    ));
    Paragraph::new(line).render(area, buf);
}

/// Format countdown as HH:MM:SS from remaining milliseconds.
fn format_countdown(next_funding_ms: u64, now_ms: u64, width: usize) -> String {
    let remaining_secs = next_funding_ms.saturating_sub(now_ms) / 1000;

    if remaining_secs == 0 {
        return format!("{:>width$}", "FUNDED", width = width);
    }

    let h = remaining_secs / 3600;
    let m = (remaining_secs % 3600) / 60;
    let s = remaining_secs % 60;
    let countdown = format!("{:02}:{:02}:{:02}", h, m, s);
    format!("{:>width$}", countdown, width = width)
}

/// Column widths for the table layout.
struct ColumnWidths {
    symbol: usize,
    mark_price: usize,
    funding_rate: usize,
    countdown: usize,
    annual_rate: usize,
}

/// Compute column widths based on available terminal width.
fn compute_columns(total_width: usize) -> ColumnWidths {
    // Fixed: " " prefix (1) + 4 column separators (4) = 5 overhead
    // Symbol: 14, Countdown: 10, fixed
    // Remaining: split between mark_price, funding_rate, annual_rate

    let fixed_overhead = 6; // 1 prefix + 5 separators
    let symbol_w = 14;
    let countdown_w = 10;
    let fixed_cols = symbol_w + countdown_w + fixed_overhead;

    let remaining = total_width.saturating_sub(fixed_cols);

    // Split remaining roughly: mark_price 40%, funding_rate 30%, annual_rate 30%
    let mark_price_w = (remaining * 40 / 100).max(12);
    let funding_rate_w = (remaining * 30 / 100).max(11);
    let annual_rate_w = remaining.saturating_sub(mark_price_w).saturating_sub(funding_rate_w).max(10);

    ColumnWidths {
        symbol: symbol_w,
        mark_price: mark_price_w,
        funding_rate: funding_rate_w,
        countdown: countdown_w,
        annual_rate: annual_rate_w,
    }
}
