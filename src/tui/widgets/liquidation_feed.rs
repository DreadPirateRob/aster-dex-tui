// src/tui/widgets/liquidation_feed.rs
// Render widget for the liquidation feed: title bar, stats row, scrollable
// event list with color coding and flash effects, and help bar.

use crate::data::liquidation::LiquidationSide;
use crate::tui::liquidations_app::LiquidationsApp;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use rust_decimal::Decimal;

/// Widget that renders the complete liquidation feed view.
///
/// Borrows the LiquidationsApp for read-only access to feed state,
/// symbol filter, theme, and input mode.
pub struct LiquidationFeedWidget<'a> {
    app: &'a LiquidationsApp,
}

impl<'a> LiquidationFeedWidget<'a> {
    pub fn new(app: &'a LiquidationsApp) -> Self {
        Self { app }
    }
}

impl<'a> Widget for LiquidationFeedWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = &self.app.theme;

        // Vertical layout: title (1), stats (2), separator (1), feed (remaining), help (1)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Length(2), // Stats row
                Constraint::Length(1), // Separator
                Constraint::Min(0),   // Feed list
                Constraint::Length(1), // Help bar
            ])
            .split(area);

        render_title_bar(self.app, chunks[0], buf);
        render_stats_row(self.app, chunks[1], buf);
        render_separator(theme.border, chunks[2], buf);
        render_feed_list(self.app, chunks[3], buf);
        render_help_bar(self.app, chunks[4], buf);
    }
}

/// Render the title bar with feed name, filter indicator, and event count.
fn render_title_bar(app: &LiquidationsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let mut spans = vec![
        Span::styled(
            " Liquidation Feed ",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // Symbol input mode or filter display
    if let Some(ref buffer) = app.symbol_input_buffer {
        spans.push(Span::styled(
            format!("Filter: {}_", buffer),
            Style::default().fg(theme.status_warning),
        ));
    } else {
        let filter_text = match &app.symbol_filter {
            Some(s) => format!("[{}]", s),
            None => "All Markets".to_string(),
        };
        spans.push(Span::styled(
            filter_text,
            Style::default().fg(theme.text_secondary),
        ));
    }

    // Right-aligned event count
    let count_text = format!(" {} events ", app.feed.event_count());
    let left_len: usize = spans.iter().map(|s| s.width()).sum();
    let padding = area.width as usize - left_len.min(area.width as usize) - count_text.len().min(area.width as usize);
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.push(Span::styled(
        count_text,
        Style::default().fg(theme.text_muted),
    ));

    let line = Line::from(spans);
    let p = Paragraph::new(line);
    p.render(area, buf);
}

/// Render the stats row: volume breakdown and ratio bar.
fn render_stats_row(app: &LiquidationsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let feed = &app.feed;

    // Line 1: Volume stats
    let elapsed = feed.session_start.elapsed();
    let hours = elapsed.as_secs() / 3600;
    let minutes = (elapsed.as_secs() % 3600) / 60;
    let session_str = if hours > 0 {
        format!("{}h{}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    };

    let total_display = format_usd_compact(feed.total_volume);
    let long_display = format_usd_compact(feed.long_liq_volume);
    let short_display = format_usd_compact(feed.short_liq_volume);

    let line1 = Line::from(vec![
        Span::styled(" Total: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${}", total_display),
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | Long Liq: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${}", long_display),
            Style::default().fg(theme.bearish),
        ),
        Span::styled(" | Short Liq: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("${}", short_display),
            Style::default().fg(theme.bullish),
        ),
        Span::styled(
            format!(" | Since {}", session_str),
            Style::default().fg(theme.text_muted),
        ),
    ]);

    // Line 2: Ratio bar
    let line2 = if feed.total_volume > Decimal::ZERO {
        let long_pct = (feed.long_liq_volume * Decimal::from(100) / feed.total_volume)
            .round_dp(0)
            .to_string()
            .parse::<u32>()
            .unwrap_or(50);
        let short_pct = 100u32.saturating_sub(long_pct);

        // 20-char bar
        let bar_width: u32 = 20;
        let long_bars = (long_pct * bar_width / 100).max(0);
        let short_bars = bar_width.saturating_sub(long_bars);

        let long_bar: String = "\u{2588}".repeat(long_bars as usize);
        let short_bar: String = "\u{2591}".repeat(short_bars as usize);

        Line::from(vec![
            Span::styled(" [", Style::default().fg(theme.text_muted)),
            Span::styled(long_bar, Style::default().fg(theme.bearish)),
            Span::styled(short_bar, Style::default().fg(theme.bullish)),
            Span::styled("] ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}% Long", long_pct),
                Style::default().fg(theme.bearish),
            ),
            Span::styled(" / ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}% Short", short_pct),
                Style::default().fg(theme.bullish),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            " Waiting for liquidation events...",
            Style::default().fg(theme.text_muted),
        )])
    };

    if area.height >= 1 {
        let top = Rect::new(area.x, area.y, area.width, 1);
        Paragraph::new(line1).render(top, buf);
    }
    if area.height >= 2 {
        let bottom = Rect::new(area.x, area.y + 1, area.width, 1);
        Paragraph::new(line2).render(bottom, buf);
    }
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

/// Render the scrollable feed list with color-coded events.
fn render_feed_list(app: &LiquidationsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let feed = &app.feed;

    if feed.events.is_empty() {
        let msg = Line::from(Span::styled(
            " No liquidation events yet. Waiting for WebSocket data...",
            Style::default().fg(theme.text_muted),
        ));
        Paragraph::new(msg).render(area, buf);
        return;
    }

    let visible_rows = area.height as usize;

    // Build flash event index set for O(1) lookup
    let flash_indices: std::collections::HashSet<usize> = feed
        .flash_events
        .iter()
        .map(|(idx, _)| *idx)
        .collect();

    // Iterate events in reverse (newest first), skip scroll_offset from end
    let total = feed.events.len();
    let start_idx = if app.feed.scroll_offset < total {
        total - 1 - app.feed.scroll_offset
    } else {
        0
    };

    for (row, display_idx) in (0..start_idx + 1).rev().take(visible_rows).enumerate() {
        if row >= visible_rows {
            break;
        }

        let event = &feed.events[display_idx];
        let y = area.y + row as u16;
        if y >= area.y + area.height {
            break;
        }

        let is_flashing = flash_indices.contains(&display_idx);
        let is_large = event.usd_value >= feed.large_threshold_usd;

        // Side color and text
        let (side_text, side_color) = match event.side {
            LiquidationSide::LongLiquidated => ("LONG LIQ ", theme.bearish),
            LiquidationSide::ShortLiquidated => ("SHORT LIQ", theme.bullish),
        };

        // Format timestamp from ms epoch
        let ts_str = format_timestamp_utc(event.timestamp);

        // Format USD value
        let usd_str = format_usd_compact(event.usd_value);

        // Large marker
        let marker = if is_large { "! " } else { "  " };

        // Build row text
        let row_text = format!(
            "{}{} | {:10} | {} | {} @ {} | ${}",
            marker,
            ts_str,
            event.symbol,
            side_text,
            format_qty(event.quantity),
            format_price(event.price),
            usd_str
        );

        // Style based on flash/large state
        let style = if is_flashing {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_large {
            Style::default()
                .fg(side_color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(side_color)
        };

        // Render row
        let row_area = Rect::new(area.x, y, area.width, 1);
        let line = Line::from(Span::styled(
            truncate_to_width(&row_text, area.width as usize),
            style,
        ));
        Paragraph::new(line).render(row_area, buf);
    }
}

/// Render the help bar with hotkey hints.
fn render_help_bar(app: &LiquidationsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let help_text = if app.symbol_input_buffer.is_some() {
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

/// Format a USD value compactly with commas (integer display).
fn format_usd_compact(value: Decimal) -> String {
    let rounded = value.round_dp(0);
    let s = rounded.to_string();
    // Remove any decimal point and trailing zeros from round_dp
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        return "0".to_string();
    }

    // Handle negative values
    let (negative, digits) = if s.starts_with('-') {
        (true, &s[1..])
    } else {
        (false, s)
    };

    // Insert commas
    let mut result = String::new();
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    let formatted: String = result.chars().rev().collect();

    if negative {
        format!("-{}", formatted)
    } else {
        formatted
    }
}

/// Format a quantity value (up to 4 decimal places).
fn format_qty(value: Decimal) -> String {
    let s = value.normalize().to_string();
    // Pad to at least 8 chars for alignment
    format!("{:>8}", s)
}

/// Format a price value (keep original precision).
fn format_price(value: Decimal) -> String {
    let s = value.normalize().to_string();
    format!("{:>10}", s)
}

/// Format a millisecond epoch timestamp to HH:MM:SS UTC.
fn format_timestamp_utc(timestamp_ms: u64) -> String {
    let secs = (timestamp_ms / 1000) as i64;
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => dt.format("%H:%M:%S").to_string(),
        None => "??:??:??".to_string(),
    }
}

/// Truncate a string to fit within a given width.
fn truncate_to_width(s: &str, width: usize) -> String {
    if s.len() <= width {
        s.to_string()
    } else {
        s[..width].to_string()
    }
}
