// src/tui/widgets/status_bar.rs
// Status bar widget displaying product info, connection status, and ticker data

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use rust_decimal::Decimal;

use crate::network::{ConnectionState, ConnectionStatus};
use crate::tui::app::{MessageType, TickerStats};
use crate::tui::theme::Theme;
use rust_decimal::prelude::ToPrimitive;

/// Status bar widget for the top of the TUI.
///
/// Displays: symbol :: price :: exchange :: status :: UTC time
/// When an error is present, displays it with red background.
pub struct StatusBar<'a> {
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: &'a str,
    /// Current connection status
    pub connection_status: &'a ConnectionStatus,
    /// Current UTC time string (formatted as HH:MM:SS)
    pub utc_time: &'a str,
    /// Most recent trade price (None if not yet received)
    pub last_price: Option<Decimal>,
    /// Status message to display with type (None if no current message)
    pub status_message: Option<(&'a str, MessageType)>,
    /// Color theme for styling
    pub theme: &'a Theme,
    /// 24h ticker statistics (None if not fetched)
    pub ticker_stats: Option<&'a TickerStats>,
    /// Whether stats are visible
    pub stats_visible: bool,
}

/// Format volume with K/M suffix for compact display.
fn format_volume(volume: Decimal) -> String {
    let vol_f64 = volume.to_f64().unwrap_or(0.0);
    if vol_f64 >= 1_000_000.0 {
        format!("{:.1}M", vol_f64 / 1_000_000.0)
    } else if vol_f64 >= 1_000.0 {
        format!("{:.1}K", vol_f64 / 1_000.0)
    } else {
        format!("{:.1}", vol_f64)
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let line = self.build_line();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border));

        let paragraph = Paragraph::new(line).block(block);
        paragraph.render(area, buf);
    }
}

impl StatusBar<'_> {
    /// Build the status bar line with all sections.
    fn build_line(&self) -> Line<'static> {
        let (conn_text, conn_color) = self.connection_display();

        let mut spans = vec![
            Span::raw(" "),
            Span::styled(
                self.symbol.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" :: "),
            self.format_price(),
            Span::raw(" :: "),
        ];

        // Add stats section if visible (includes trailing separator)
        spans.extend(self.format_stats());

        // Continue with exchange, connection, time
        spans.extend(vec![
            Span::raw("AsterDEX"),
            Span::raw(" :: "),
            Span::styled(conn_text, Style::default().fg(conn_color)),
            Span::raw(" :: "),
            Span::raw(self.utc_time.to_string()),
            Span::raw(" "),
        ]);

        // Insert status message after first space if present
        if let Some((msg, msg_type)) = self.status_message {
            let (prefix, bg_color) = match msg_type {
                MessageType::Error => ("ERROR: ", self.theme.status_error),
                MessageType::Info => ("", Color::Blue),
            };
            spans.insert(
                1,
                Span::styled(
                    format!(" {}{} ", prefix, msg),
                    Style::default()
                        .fg(self.theme.text_primary)
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
            );
            spans.insert(2, Span::raw(" :: "));
        }

        Line::from(spans)
    }

    /// Get connection status display text and color.
    fn connection_display(&self) -> (String, Color) {
        match &self.connection_status.state {
            ConnectionState::Connected => ("Connected \u{2022}".to_string(), self.theme.status_connected),
            ConnectionState::Connecting => ("Connecting \u{2026}".to_string(), self.theme.status_warning),
            ConnectionState::Reconnecting {
                attempt,
                max_attempts,
                ..
            } => (
                format!("Retry {}/{} \u{21BB}", attempt, max_attempts),
                self.theme.status_warning,
            ),
            ConnectionState::Disconnected => ("Offline \u{2717}".to_string(), self.theme.status_error),
        }
    }

    /// Format price for display (bold + theme price_line color for visual prominence).
    fn format_price(&self) -> Span<'static> {
        match self.last_price {
            Some(price) => Span::styled(
                format!("${:.2}", price),
                Style::default()
                    .fg(self.theme.price_line)
                    .add_modifier(Modifier::BOLD),
            ),
            None => Span::styled("$--", Style::default().fg(self.theme.text_muted)),
        }
    }

    /// Format stats section for status bar.
    /// Returns spans only if stats are visible and available.
    fn format_stats(&self) -> Vec<Span<'static>> {
        if !self.stats_visible {
            return vec![];
        }

        let Some(stats) = self.ticker_stats else {
            return vec![];
        };

        let change_color = if stats.price_change_percent.is_sign_positive() {
            self.theme.bullish
        } else {
            self.theme.bearish
        };

        let change_sign = if stats.price_change_percent.is_sign_positive() { "+" } else { "" };

        vec![
            Span::styled(
                format!("{}${:.2} ({}{}%)",
                    change_sign,
                    stats.price_change.abs(),
                    change_sign,
                    stats.price_change_percent.abs()
                ),
                Style::default().fg(change_color),
            ),
            Span::raw(" :: "),
            Span::styled(
                format!("H:${:.2} L:${:.2}", stats.high_price, stats.low_price),
                Style::default().fg(self.theme.text_secondary),
            ),
            Span::raw(" :: "),
            Span::styled(
                format!("V:{}", format_volume(stats.volume)),
                Style::default().fg(self.theme.text_secondary),
            ),
            Span::raw(" :: "),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;

    #[test]
    fn test_connection_display_states() {
        let theme = Theme::dark();
        let status_connected = ConnectionStatus {
            state: ConnectionState::Connected,
            _last_error: None,
            retry_count: 0,
        };
        let status_connecting = ConnectionStatus {
            state: ConnectionState::Connecting,
            _last_error: None,
            retry_count: 0,
        };
        let status_reconnecting = ConnectionStatus {
            state: ConnectionState::Reconnecting {
                attempt: 2,
                max_attempts: 5,
                _next_retry_at: None,
            },
            _last_error: None,
            retry_count: 2,
        };
        let status_disconnected = ConnectionStatus {
            state: ConnectionState::Disconnected,
            _last_error: None,
            retry_count: 0,
        };

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status_connected,
            utc_time: "00:00:00",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };
        assert_eq!(
            bar.connection_display(),
            ("Connected \u{2022}".to_string(), theme.status_connected)
        );

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status_connecting,
            utc_time: "00:00:00",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };
        assert_eq!(
            bar.connection_display(),
            ("Connecting \u{2026}".to_string(), theme.status_warning)
        );

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status_reconnecting,
            utc_time: "00:00:00",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };
        assert_eq!(
            bar.connection_display(),
            ("Retry 2/5 \u{21BB}".to_string(), theme.status_warning)
        );

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status_disconnected,
            utc_time: "00:00:00",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };
        assert_eq!(
            bar.connection_display(),
            ("Offline \u{2717}".to_string(), theme.status_error)
        );
    }

    #[test]
    fn test_build_line_with_error() {
        let theme = Theme::dark();
        let status = ConnectionStatus {
            state: ConnectionState::Connected,
            _last_error: None,
            retry_count: 0,
        };

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status,
            utc_time: "00:00:00",
            last_price: None,
            status_message: Some(("Connection lost", MessageType::Error)),
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };

        let line = bar.build_line();
        // Find the error span in the line
        let has_error_span = line.spans.iter().any(|span| {
            span.content.contains("ERROR: Connection lost")
                && span.style.bg == Some(theme.status_error)
                && span.style.fg == Some(theme.text_primary)
        });
        assert!(
            has_error_span,
            "Error span with theme error background should be present"
        );
    }

    #[test]
    fn test_build_line_without_error() {
        let theme = Theme::dark();
        let status = ConnectionStatus {
            state: ConnectionState::Connected,
            _last_error: None,
            retry_count: 0,
        };

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status,
            utc_time: "00:00:00",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };

        let line = bar.build_line();
        // Should not have any error spans
        let has_error_span = line.spans.iter().any(|span| span.content.contains("ERROR:"));
        assert!(
            !has_error_span,
            "Error span should not be present when no error"
        );
    }

    #[test]
    fn test_utc_time_displayed() {
        let theme = Theme::dark();
        let status = ConnectionStatus {
            state: ConnectionState::Connected,
            _last_error: None,
            retry_count: 0,
        };

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status,
            utc_time: "14:32:45",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };

        let line = bar.build_line();
        // UTC time should appear in the built line
        let has_utc_time = line
            .spans
            .iter()
            .any(|span| span.content.contains("14:32:45"));
        assert!(has_utc_time, "UTC time should be displayed in status bar");
    }

    #[test]
    fn test_exchange_name_hardcoded_asterdex() {
        let theme = Theme::dark();
        let status = ConnectionStatus {
            state: ConnectionState::Connected,
            _last_error: None,
            retry_count: 0,
        };

        let bar = StatusBar {
            symbol: "BTCUSDT",
            connection_status: &status,
            utc_time: "00:00:00",
            last_price: None,
            status_message: None,
            theme: &theme,
            ticker_stats: None,
            stats_visible: true,
        };

        // Verify "AsterDEX" appears in the built line
        let line = bar.build_line();
        let has_asterdex = line.spans.iter().any(|span| span.content.contains("AsterDEX"));
        assert!(has_asterdex, "AsterDEX should appear in status bar");
    }
}
