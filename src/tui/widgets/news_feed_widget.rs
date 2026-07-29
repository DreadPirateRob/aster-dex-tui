// src/tui/widgets/news_feed_widget.rs
// Render widget for the news feed: title bar, scrollable article list
// with breaking news highlighting, and help bar.

use crate::tui::news_app::NewsApp;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// Widget that renders the complete news feed view.
///
/// Borrows the NewsApp for read-only access to feed state,
/// symbol filter, theme, and input mode.
pub struct NewsFeedWidget<'a> {
    app: &'a NewsApp,
}

impl<'a> NewsFeedWidget<'a> {
    pub fn new(app: &'a NewsApp) -> Self {
        Self { app }
    }
}

impl<'a> Widget for NewsFeedWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Vertical layout: title (1), article list (remaining), help (1)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Min(0),   // Article list
                Constraint::Length(1), // Help bar
            ])
            .split(area);

        render_title_bar(self.app, chunks[0], buf);
        render_article_list(self.app, chunks[1], buf);
        render_help_bar(self.app, chunks[2], buf);
    }
}

/// Render the title bar with feed name, filter indicators, and article count.
fn render_title_bar(app: &NewsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    // Check for symbol input mode first
    if let Some(ref input_buffer) = app.symbol_input {
        let line = Line::from(vec![
            Span::styled(
                " NEWS FEED ",
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
        " NEWS FEED ",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    )];

    // Right side: counts and filters
    let mut right_parts = Vec::new();

    let visible_count = app.visible_articles().len();
    let total_count = app.feed.articles().len();

    if app.symbol_filter.is_some() {
        right_parts.push(format!("{}/{} articles", visible_count, total_count));
    } else {
        right_parts.push(format!("{} articles", total_count));
    }

    if let Some(ref filter) = app.symbol_filter {
        right_parts.push(format!("Filter: {}", filter));
    }

    if let Some(ref cat) = app.category_filter {
        right_parts.push(format!("Category: {}", cat));
    }

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

/// Render the scrollable article list with breaking news highlighting.
fn render_article_list(app: &NewsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let visible = app.visible_articles();

    if visible.is_empty() {
        let msg = if app.symbol_filter.is_some() {
            format!(
                "No articles matching '{}'",
                app.symbol_filter.as_deref().unwrap_or("")
            )
        } else {
            "Waiting for news...".to_string()
        };

        // Center the message
        let msg_line = Line::from(Span::styled(
            msg,
            Style::default().fg(theme.text_muted),
        ));

        // Place in vertical center
        let center_y = area.height / 2;
        if center_y < area.height {
            let center_area = Rect::new(area.x, area.y + center_y, area.width, 1);
            Paragraph::new(msg_line)
                .alignment(ratatui::layout::Alignment::Center)
                .render(center_area, buf);
        }
        return;
    }

    let visible_rows = area.height as usize;

    // Apply scroll offset: articles are newest-first in the Vec
    let start = app.scroll_offset;
    let end = (start + visible_rows).min(visible.len());

    for (row, idx) in (start..end).enumerate() {
        let article = visible[idx];
        let y = area.y + row as u16;
        if y >= area.y + area.height {
            break;
        }

        // Format timestamp as HH:MM
        let time_str = format_timestamp_hhmm(article.timestamp);

        // Source: left-padded/truncated to 12 chars
        let source = format_source(&article.source, 12);

        // Remaining width for title
        // Format: " HH:MM | {source:12} | {title}"
        let prefix_len = 1 + 5 + 3 + 12 + 3; // " HH:MM | source______ | "
        let title_max = (area.width as usize).saturating_sub(prefix_len);
        let title = truncate_str(&article.title, title_max);

        // Build spans with different styles per column
        let row_area = Rect::new(area.x, y, area.width, 1);

        if article.is_breaking {
            // Breaking: bold + yellow
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", time_str),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "| ",
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(
                    format!("{} ", source),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    "| ",
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(
                    title,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            Paragraph::new(line).render(row_area, buf);
        } else {
            // Normal article
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", time_str),
                    Style::default().fg(theme.text_secondary),
                ),
                Span::styled(
                    "| ",
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(
                    format!("{} ", source),
                    Style::default()
                        .fg(theme.text_muted)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    "| ",
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(
                    title,
                    Style::default().fg(theme.text_primary),
                ),
            ]);
            Paragraph::new(line).render(row_area, buf);
        }
    }
}

/// Render the help bar with hotkey hints.
fn render_help_bar(app: &NewsApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let help_text = if app.symbol_input.is_some() {
        "Enter: apply | Esc: cancel | type symbol name"
    } else {
        " j/k:scroll  g/G:top/bottom  /:filter  c:clear  q:quit"
    };

    let line = Line::from(Span::styled(
        help_text,
        Style::default().fg(theme.text_muted),
    ));
    Paragraph::new(line).render(area, buf);
}

/// Format a millisecond epoch timestamp to HH:MM.
fn format_timestamp_hhmm(timestamp_ms: i64) -> String {
    let secs = timestamp_ms / 1000;
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "??:??".to_string(),
    }
}

/// Format source name: left-pad or truncate to exact width.
fn format_source(source: &str, width: usize) -> String {
    if source.len() > width {
        source[..width].to_string()
    } else {
        format!("{:width$}", source, width = width)
    }
}

/// Truncate a string to fit within a given character width.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("{}...", &s[..max_len - 3])
    } else {
        s[..max_len].to_string()
    }
}
