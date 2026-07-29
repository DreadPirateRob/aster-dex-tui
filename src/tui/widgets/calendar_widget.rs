// src/tui/widgets/calendar_widget.rs
// Render widget for the economic calendar: title bar with countdown,
// sortable column headers, scrollable color-coded event table, and help bar.

use crate::data::economic_calendar::{Impact, SortColumn};
use crate::tui::calendar_app::CalendarApp;
use chrono::Utc;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// Widget that renders the complete economic calendar view.
///
/// Borrows the CalendarApp for read-only access to calendar state,
/// sort settings, theme, filters, and input mode.
pub struct CalendarWidget<'a> {
    app: &'a CalendarApp,
}

impl<'a> CalendarWidget<'a> {
    pub fn new(app: &'a CalendarApp) -> Self {
        Self { app }
    }
}

impl<'a> Widget for CalendarWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // 4-zone vertical layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Length(1), // Column headers
                Constraint::Min(0),    // Scrollable body
                Constraint::Length(1), // Help bar
            ])
            .split(area);

        render_title_bar(self.app, chunks[0], buf);
        render_column_headers(self.app, chunks[1], buf);
        render_table_body(self.app, chunks[2], buf);
        render_help_bar(self.app, chunks[3], buf);
    }
}

/// Column widths for the calendar table.
struct ColumnWidths {
    time: usize,
    cc: usize,
    event: usize,
    impact: usize,
    actual: usize,
    est: usize,
    prev: usize,
}

/// Compute column widths based on available terminal width.
fn compute_columns(total_width: usize) -> ColumnWidths {
    // Fixed columns
    let time_w = 19; // "YYYY-MM-DD HH:MM:SS"
    let cc_w = 4;    // 2-char country code + padding
    let impact_w = 8; // "HIGH" + padding
    let actual_w = 10;
    let est_w = 10;
    let prev_w = 10;

    // Overhead: 1 prefix + 7 column separators
    let fixed_overhead = 8;
    let fixed_cols = time_w + cc_w + impact_w + actual_w + est_w + prev_w + fixed_overhead;

    // Event column gets remaining space
    let event_w = total_width.saturating_sub(fixed_cols).max(10);

    ColumnWidths {
        time: time_w,
        cc: cc_w,
        event: event_w,
        impact: impact_w,
        actual: actual_w,
        est: est_w,
        prev: prev_w,
    }
}

/// Render the title bar with dashboard name, date range, countdown, and filter indicators.
fn render_title_bar(app: &CalendarApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    // Input mode title
    if let Some(ref input_buffer) = app.input_mode {
        let line = Line::from(vec![
            Span::styled(
                " ECONOMIC CALENDAR ",
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Country: {}_", input_buffer),
                Style::default().fg(theme.status_warning),
            ),
        ]);
        Paragraph::new(line).render(area, buf);
        return;
    }

    let mut left_spans = vec![
        Span::styled(
            " ECONOMIC CALENDAR ",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}]", app.date_range.label()),
            Style::default().fg(theme.text_secondary),
        ),
    ];

    // Countdown to next high-impact event
    let countdown = compute_countdown(app);
    if let Some(cd) = countdown {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            format!("Next HIGH: {}", cd),
            Style::default()
                .fg(theme.status_error)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Right side: filter indicators + event count
    let mut right_parts = Vec::new();

    let visible_count = app.visible_events().len();
    let total_count = app.calendar.len();
    if app.high_only || app.country_filter.is_some() {
        right_parts.push(format!("{}/{}", visible_count, total_count));
    } else {
        right_parts.push(format!("{} events", total_count));
    }

    if app.high_only {
        right_parts.push("[HIGH]".to_string());
    }
    if let Some(ref cc) = app.country_filter {
        right_parts.push(format!("[CC: {}]", cc));
    }

    let right_text = format!(" {} ", right_parts.join(" "));
    let left_len: usize = left_spans.iter().map(|s| s.width()).sum();
    let right_len = right_text.len();
    let total_len = left_len + right_len;
    if total_len < area.width as usize {
        let padding = area.width as usize - total_len;
        left_spans.push(Span::raw(" ".repeat(padding)));
    }
    left_spans.push(Span::styled(
        right_text,
        Style::default().fg(theme.text_muted),
    ));

    let line = Line::from(left_spans);
    Paragraph::new(line).render(area, buf);
}

/// Compute countdown string to next high-impact future event.
fn compute_countdown(app: &CalendarApp) -> Option<String> {
    let now = Utc::now().naive_utc();
    let visible = app.calendar.sorted_events(SortColumn::Time, true);

    for event in &visible {
        if event.impact != Impact::High {
            continue;
        }
        if let Some(tp) = event.time_parsed {
            if tp > now {
                let dur = tp.signed_duration_since(now);
                let total_mins = dur.num_minutes();
                if total_mins < 0 {
                    continue;
                }
                let days = total_mins / (24 * 60);
                let hours = (total_mins % (24 * 60)) / 60;
                let mins = total_mins % 60;

                return if days > 0 {
                    Some(format!("{}d {}h", days, hours))
                } else if hours > 0 {
                    Some(format!("{}h {}m", hours, mins))
                } else {
                    Some(format!("{}m", mins))
                };
            }
        }
    }
    None
}

/// Render column headers with sort indicators.
fn render_column_headers(app: &CalendarApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let cols = compute_columns(area.width as usize);

    let sort_arrow = if app.sort_ascending { "^" } else { "v" };

    let mut spans = vec![Span::raw(" ")];

    // Time
    spans.push(header_span(
        "Time",
        cols.time,
        app.sort_column == SortColumn::Time,
        sort_arrow,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    // CC (country)
    spans.push(header_span(
        "CC",
        cols.cc,
        app.sort_column == SortColumn::Country,
        sort_arrow,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    // Event
    spans.push(header_span(
        "Event",
        cols.event,
        app.sort_column == SortColumn::Event,
        sort_arrow,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    // Impact
    spans.push(header_span(
        "Impact",
        cols.impact,
        app.sort_column == SortColumn::Impact,
        sort_arrow,
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    // Actual
    spans.push(header_span(
        "Actual",
        cols.actual,
        false,
        "",
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    // Est
    spans.push(header_span(
        "Est",
        cols.est,
        false,
        "",
        theme.text_secondary,
    ));
    spans.push(Span::raw(" "));

    // Prev
    spans.push(header_span(
        "Prev",
        cols.prev,
        false,
        "",
        theme.text_secondary,
    ));

    let line = Line::from(spans);
    Paragraph::new(line).render(area, buf);
}

/// Create a header span with optional sort indicator.
fn header_span(
    label: &str,
    width: usize,
    is_sorted: bool,
    arrow: &str,
    color: Color,
) -> Span<'static> {
    let text = if is_sorted {
        format!("{:<width$}", format!("{}*{}", label, arrow), width = width)
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

/// Render the scrollable table body with color-coded impact levels.
fn render_table_body(app: &CalendarApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let visible = app.visible_events();

    if visible.is_empty() {
        let msg = if app.high_only {
            "No high-impact events in this range".to_string()
        } else if app.country_filter.is_some() {
            format!(
                "No events matching country '{}'",
                app.country_filter.as_deref().unwrap_or("")
            )
        } else {
            "Waiting for economic calendar data...".to_string()
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

    let start = app.scroll_offset;
    let end = (start + visible_rows).min(visible.len());

    for (row, idx) in (start..end).enumerate() {
        let event = visible[idx];
        let y = area.y + row as u16;
        if y >= area.y + area.height {
            break;
        }

        let row_area = Rect::new(area.x, y, area.width, 1);
        let is_selected = idx == app.scroll_offset;

        // Impact color
        let (impact_color, impact_modifier) = match event.impact {
            Impact::High => (theme.status_error, Modifier::BOLD),
            Impact::Medium => (theme.status_warning, Modifier::empty()),
            Impact::Low | Impact::Unknown => (theme.text_muted, Modifier::empty()),
        };

        // Row background for selected row
        let row_bg = if is_selected {
            Color::DarkGray
        } else {
            Color::Reset
        };

        // Format time
        let time_str = if let Some(tp) = event.time_parsed {
            format!("{:<width$}", tp.format("%Y-%m-%d %H:%M"), width = cols.time)
        } else {
            format!("{:<width$}", &event.time_raw, width = cols.time)
        };

        // Format country
        let cc_str = format!("{:<width$}", &event.country, width = cols.cc);

        // Format event name (truncate to available width)
        let event_name = if event.event.len() > cols.event {
            format!("{}...", &event.event[..cols.event.saturating_sub(3)])
        } else {
            format!("{:<width$}", &event.event, width = cols.event)
        };

        // Format impact
        let impact_str = format!("{:<width$}", event.impact.label(), width = cols.impact);

        // Format numeric values
        let actual_str = format_value(event.actual, &event.unit, cols.actual);
        let est_str = format_value(event.estimate, &event.unit, cols.est);
        let prev_str = format_value(event.prev, &event.unit, cols.prev);

        // Determine if actual beats/misses estimate for bold highlight
        let actual_modifier = if let (Some(act), Some(est)) = (event.actual, event.estimate) {
            if (act - est).abs() > f64::EPSILON {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }
        } else {
            Modifier::empty()
        };

        let mut spans = vec![Span::raw(" ")];

        // Time
        spans.push(Span::styled(
            time_str,
            Style::default().fg(theme.text_primary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // CC
        spans.push(Span::styled(
            cc_str,
            Style::default().fg(theme.text_muted).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Event
        spans.push(Span::styled(
            event_name,
            Style::default().fg(theme.text_primary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Impact (color-coded)
        spans.push(Span::styled(
            impact_str,
            Style::default()
                .fg(impact_color)
                .bg(row_bg)
                .add_modifier(impact_modifier),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Actual (bold if different from estimate)
        spans.push(Span::styled(
            actual_str,
            Style::default()
                .fg(theme.text_primary)
                .bg(row_bg)
                .add_modifier(actual_modifier),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Est
        spans.push(Span::styled(
            est_str,
            Style::default().fg(theme.text_secondary).bg(row_bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Prev
        spans.push(Span::styled(
            prev_str,
            Style::default().fg(theme.text_muted).bg(row_bg),
        ));

        let line = Line::from(spans);
        Paragraph::new(line).render(row_area, buf);
    }
}

/// Format a numeric value with unit for display.
fn format_value(value: Option<f64>, unit: &str, width: usize) -> String {
    match value {
        Some(v) => {
            let formatted = if unit.is_empty() {
                format!("{:.2}", v)
            } else {
                format!("{:.2}{}", v, unit)
            };
            format!("{:>width$}", formatted, width = width)
        }
        None => format!("{:>width$}", "-", width = width),
    }
}

/// Render the help bar with keybinding hints.
fn render_help_bar(app: &CalendarApp, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;

    let help_text = if app.input_mode.is_some() {
        "Enter: apply | Esc: cancel | type country code"
    } else {
        " j/k:scroll  g/G:top/btm  Tab/s:sort  r:reverse  d/D:range  h:high-only  /:filter  c:clear  q:quit"
    };

    let line = Line::from(Span::styled(
        help_text,
        Style::default().fg(theme.text_muted),
    ));
    Paragraph::new(line).render(area, buf);
}
