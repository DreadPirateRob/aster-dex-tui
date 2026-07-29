// src/tui/widgets/trades_list.rs
// Live trades list widget with color-coded buy/sell and flash effects for large trades

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget},
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::data::types::{Trade, TradeSide};
use crate::tui::theme::Theme;

/// Flash duration for large trades (300ms).
const FLASH_DURATION: Duration = Duration::from_millis(300);

/// Default capacity for trades display buffer.
pub const DEFAULT_TRADES_CAPACITY: usize = 200;

/// Threshold in seconds for switching from relative to absolute time (1 hour).
const RELATIVE_TIME_THRESHOLD_SECS: u64 = 3600;

/// A trade with arrival timestamp for flash effect tracking.
#[derive(Debug, Clone)]
pub struct DisplayTrade {
    /// The underlying trade data.
    pub trade: Trade,
    /// When this trade was added to the display buffer.
    pub arrived_at: Instant,
}

impl DisplayTrade {
    /// Create a new display trade with current timestamp.
    pub fn new(trade: Trade) -> Self {
        Self {
            trade,
            arrived_at: Instant::now(),
        }
    }

    /// Create a display trade for historical data (no flash effect).
    ///
    /// Sets arrived_at to a time far enough in the past to avoid the flash effect.
    pub fn new_historical(trade: Trade) -> Self {
        Self {
            trade,
            // Set arrived_at to past time to avoid flash effect
            arrived_at: Instant::now() - Duration::from_secs(60),
        }
    }

    /// Check if this trade should show the flash effect.
    ///
    /// Returns true if the trade arrived within FLASH_DURATION.
    pub fn is_flashing(&self) -> bool {
        self.arrived_at.elapsed() < FLASH_DURATION
    }
}

/// Bounded container for display trades with newest-first ordering.
///
/// Maintains a fixed capacity, evicting oldest trades when full.
/// New trades are prepended so iteration yields newest first.
#[derive(Debug)]
pub struct TradesDisplay {
    trades: VecDeque<DisplayTrade>,
    capacity: usize,
}

impl TradesDisplay {
    /// Create a new trades display with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            trades: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push a new trade to the display.
    ///
    /// The trade is prepended (newest first). If at capacity,
    /// the oldest trade is evicted.
    pub fn push(&mut self, trade: Trade) {
        // Evict oldest if at capacity
        while self.trades.len() >= self.capacity {
            self.trades.pop_back();
        }
        // Prepend newest at front
        self.trades.push_front(DisplayTrade::new(trade));
    }

    /// Load historical trades in bulk, preserving chronological order (newest first).
    ///
    /// Historical trades are prepended at the front, maintaining capacity limits.
    /// Trades should be passed in oldest-first order (as returned by REST API).
    /// Historical trades do not trigger flash effects.
    pub fn load_historical(&mut self, trades: Vec<Trade>) {
        // Iterate normally (oldest to newest) and push_front
        // This results in newest ending up at front
        // e.g., [1, 2, 3] -> push_front(1) -> [1]
        //                 -> push_front(2) -> [2, 1]
        //                 -> push_front(3) -> [3, 2, 1] (newest first)
        for trade in trades {
            while self.trades.len() >= self.capacity {
                self.trades.pop_back();
            }
            self.trades.push_front(DisplayTrade::new_historical(trade));
        }
    }

    /// Iterate over trades, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &DisplayTrade> {
        self.trades.iter()
    }

    /// Get the number of trades in the display.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.trades.len()
    }

    /// Check if the display is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.trades.is_empty()
    }
}

impl Default for TradesDisplay {
    fn default() -> Self {
        Self::new(DEFAULT_TRADES_CAPACITY)
    }
}

/// Format a volume value in compact K/M notation.
fn format_compact_volume(vol: Decimal) -> String {
    let v = vol.to_f64().unwrap_or(0.0);
    if v.abs() >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if v.abs() >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{:.1}", v)
    }
}

/// Widget for displaying a list of trades with stats and size filter.
///
/// Renders a vertical stack:
/// 1. Stats row (1 row) - shows B:/S:/D:/L: summary
/// 2. Trade table (remaining) - color-coded buy/sell with optional size filter
pub struct TradesList<'a> {
    /// Reference to the trades display buffer.
    pub trades: &'a TradesDisplay,
    /// Threshold for large trade highlighting (trade value = price * quantity).
    pub large_trade_threshold: Decimal,
    /// Color theme for styling.
    pub theme: &'a Theme,
    /// Buy volume in rolling window (from TradeStats).
    pub buy_volume: Decimal,
    /// Sell volume in rolling window (from TradeStats).
    pub sell_volume: Decimal,
    /// Large trade count in rolling window (from TradeStats).
    pub large_trade_count: u32,
    /// Whether size filter is active.
    pub size_filter_active: bool,
    /// Size filter threshold (trade value = price * quantity).
    pub size_filter_threshold: Decimal,
}

impl Widget for TradesList<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Split into 2 vertical sections: stats, trade table
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Stats header row
                Constraint::Min(0),   // Trade table (remaining)
            ])
            .split(area);

        // === Stats row (chunks[0]) ===
        let net = self.buy_volume - self.sell_volume;
        let net_color = if net >= Decimal::ZERO {
            self.theme.bullish
        } else {
            self.theme.bearish
        };
        let net_prefix = if net >= Decimal::ZERO { "+" } else { "" };
        let stats_text = Line::from(vec![
            Span::styled(
                format!("B:{}", format_compact_volume(self.buy_volume)),
                Style::default().fg(self.theme.bullish),
            ),
            Span::raw(" "),
            Span::styled(
                format!("S:{}", format_compact_volume(self.sell_volume)),
                Style::default().fg(self.theme.bearish),
            ),
            Span::raw(" "),
            Span::styled(
                format!("D:{}{}", net_prefix, format_compact_volume(net.abs())),
                Style::default().fg(net_color),
            ),
            Span::raw(format!(" L:{}", self.large_trade_count)),
        ]);
        let stats_paragraph = Paragraph::new(stats_text);
        Widget::render(stats_paragraph, chunks[0], buf);

        // === Trade table (chunks[1]) ===
        let table_height = chunks[1].height.saturating_sub(2) as usize; // subtract 2 for borders

        // Column widths: Price(12) + Qty(12) + Time(10) = 34 + spacing
        let widths = [
            Constraint::Length(12), // Price: right-aligned, color-coded by side
            Constraint::Length(12), // Qty: right-aligned decimals
            Constraint::Length(10), // Time: "HH:MM:SS" format
        ];

        // Build rows with optional size filter
        let rows: Vec<Row> = self
            .trades
            .iter()
            .filter(|dt| {
                if self.size_filter_active {
                    trade_value(&dt.trade) >= self.size_filter_threshold
                } else {
                    true
                }
            })
            .take(table_height)
            .map(|dt| {
                let is_large = trade_value(&dt.trade) >= self.large_trade_threshold;
                let is_flashing = is_large && dt.is_flashing();
                trade_to_row(&dt.trade, is_large, is_flashing, self.theme)
            })
            .collect();

        let title = if self.size_filter_active {
            " Trades [F] "
        } else {
            " Trades "
        };
        let table = Table::new(rows, widths)
            .block(Block::default().borders(Borders::ALL).title(title));

        Widget::render(table, chunks[1], buf);
    }
}

/// Calculate trade value (price * quantity).
fn trade_value(trade: &Trade) -> Decimal {
    trade.price * trade.quantity
}

/// Format quantity with 4 decimal places.
fn format_quantity(qty: &Decimal) -> String {
    format!("{:.4}", qty)
}

/// Format price with 2 decimal places.
fn format_price(price: &Decimal) -> String {
    format!("{:.2}", price)
}

/// Format trade timestamp as relative time.
///
/// Returns compact relative format for recent trades:
/// - "Xs" for trades less than 60 seconds old
/// - "Xm" for trades less than 60 minutes old
/// - "Xh" for trades less than RELATIVE_TIME_THRESHOLD old
/// - "HH:MM:SS" absolute format for older trades
fn format_relative_time(trade_timestamp_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let elapsed_secs = now_ms.saturating_sub(trade_timestamp_ms) / 1000;

    match elapsed_secs {
        0..=59 => format!("{}s", elapsed_secs),
        60..=3599 => format!("{}m", elapsed_secs / 60),
        _ if elapsed_secs < RELATIVE_TIME_THRESHOLD_SECS => {
            format!("{}h", elapsed_secs / 3600)
        }
        _ => {
            // Absolute time HH:MM:SS (UTC)
            let total_secs = trade_timestamp_ms / 1000;
            let hours = (total_secs / 3600) % 24;
            let mins = (total_secs / 60) % 60;
            let secs = total_secs % 60;
            format!("{:02}:{:02}:{:02}", hours, mins, secs)
        }
    }
}

/// Convert a trade to a styled table row.
/// Renders 3 columns: Price (color-coded by side), Quantity, Time.
fn trade_to_row(trade: &Trade, is_large: bool, is_flashing: bool, theme: &Theme) -> Row<'static> {
    let side_color = match trade.side {
        TradeSide::Buy => theme.bullish,
        TradeSide::Sell => theme.bearish,
    };

    let mut style = Style::default().fg(side_color);

    if is_large {
        style = style.add_modifier(Modifier::BOLD);
    }

    if is_flashing {
        // Inverse colors for flash effect
        style = style.add_modifier(Modifier::REVERSED);
    }

    Row::new(vec![
        Cell::new(Line::from(format_price(&trade.price)).alignment(Alignment::Right)),
        Cell::new(Line::from(format_quantity(&trade.quantity)).alignment(Alignment::Right)),
        Cell::new(format_relative_time(trade.timestamp)),
    ])
    .style(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    /// Create a test trade with specified parameters.
    fn make_trade(id: u64, side: TradeSide, price: Decimal, qty: Decimal) -> Trade {
        Trade {
            id,
            price,
            quantity: qty,
            timestamp: 0,
            side,
            is_buyer_maker: false,
        }
    }

    #[test]
    fn test_trades_display_push_order() {
        let mut display = TradesDisplay::new(10);

        display.push(make_trade(1, TradeSide::Buy, dec!(100), dec!(1)));
        display.push(make_trade(2, TradeSide::Sell, dec!(101), dec!(2)));
        display.push(make_trade(3, TradeSide::Buy, dec!(102), dec!(3)));

        let trades: Vec<_> = display.iter().collect();
        assert_eq!(trades.len(), 3);
        // Newest first
        assert_eq!(trades[0].trade.id, 3);
        assert_eq!(trades[1].trade.id, 2);
        assert_eq!(trades[2].trade.id, 1);
    }

    #[test]
    fn test_trades_display_capacity() {
        let mut display = TradesDisplay::new(3);

        // Push more than capacity
        display.push(make_trade(1, TradeSide::Buy, dec!(100), dec!(1)));
        display.push(make_trade(2, TradeSide::Sell, dec!(101), dec!(2)));
        display.push(make_trade(3, TradeSide::Buy, dec!(102), dec!(3)));
        display.push(make_trade(4, TradeSide::Sell, dec!(103), dec!(4)));

        assert_eq!(display.len(), 3);

        let trades: Vec<_> = display.iter().collect();
        // Oldest (id=1) should be evicted
        assert_eq!(trades[0].trade.id, 4);
        assert_eq!(trades[1].trade.id, 3);
        assert_eq!(trades[2].trade.id, 2);
    }

    #[test]
    fn test_trades_display_empty() {
        let display = TradesDisplay::new(10);
        assert!(display.is_empty());
        assert_eq!(display.len(), 0);
    }

    #[test]
    fn test_trade_value() {
        let trade = make_trade(1, TradeSide::Buy, dec!(100), dec!(5));
        assert_eq!(trade_value(&trade), dec!(500));

        let trade2 = make_trade(2, TradeSide::Sell, dec!(50000), dec!(0.5));
        assert_eq!(trade_value(&trade2), dec!(25000));
    }

    #[test]
    fn test_format_price() {
        // Decimal format truncates to specified precision
        assert_eq!(format_price(&dec!(12345.6789)), "12345.67");
        assert_eq!(format_price(&dec!(100)), "100.00");
        assert_eq!(format_price(&dec!(0.1)), "0.10");
    }

    #[test]
    fn test_format_quantity() {
        // Decimal format truncates to specified precision
        assert_eq!(format_quantity(&dec!(1.23456789)), "1.2345");
        assert_eq!(format_quantity(&dec!(100)), "100.0000");
        assert_eq!(format_quantity(&dec!(0.0001)), "0.0001");
    }

    #[test]
    fn test_display_trade_flashing() {
        let trade = make_trade(1, TradeSide::Buy, dec!(100), dec!(1));
        let display_trade = DisplayTrade::new(trade);

        // Immediately after creation, should be flashing
        assert!(display_trade.is_flashing());
    }

    #[test]
    fn test_default_capacity() {
        let mut display = TradesDisplay::default();

        // Push 250 trades (more than default capacity of 200)
        for i in 0..250 {
            display.push(make_trade(i as u64, TradeSide::Buy, dec!(100), dec!(1)));
        }

        // Should be capped at 200
        assert_eq!(display.len(), 200);

        // Newest should be id=249, oldest should be id=50 (0-49 evicted)
        let trades: Vec<_> = display.iter().collect();
        assert_eq!(trades[0].trade.id, 249);
        assert_eq!(trades[199].trade.id, 50);
    }

    #[test]
    fn test_display_trade_new_historical_not_flashing() {
        let trade = make_trade(1, TradeSide::Buy, dec!(100), dec!(1));
        let display_trade = DisplayTrade::new_historical(trade);

        // Historical trades should NOT be flashing
        assert!(!display_trade.is_flashing());
    }

    #[test]
    fn test_load_historical_maintains_newest_first_order() {
        let mut display = TradesDisplay::new(10);

        // Historical trades are provided oldest-first (REST API order)
        let trades = vec![
            make_trade(1, TradeSide::Buy, dec!(100), dec!(1)),
            make_trade(2, TradeSide::Sell, dec!(101), dec!(2)),
            make_trade(3, TradeSide::Buy, dec!(102), dec!(3)),
        ];

        display.load_historical(trades);

        // After loading, should be newest-first
        let loaded: Vec<_> = display.iter().collect();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].trade.id, 3); // Newest first
        assert_eq!(loaded[1].trade.id, 2);
        assert_eq!(loaded[2].trade.id, 1); // Oldest last
    }

    #[test]
    fn test_load_historical_respects_capacity() {
        let mut display = TradesDisplay::new(3);

        // Load 5 trades into a display with capacity 3
        let trades = vec![
            make_trade(1, TradeSide::Buy, dec!(100), dec!(1)),
            make_trade(2, TradeSide::Sell, dec!(101), dec!(2)),
            make_trade(3, TradeSide::Buy, dec!(102), dec!(3)),
            make_trade(4, TradeSide::Sell, dec!(103), dec!(4)),
            make_trade(5, TradeSide::Buy, dec!(104), dec!(5)),
        ];

        display.load_historical(trades);

        // Should only keep newest 3
        assert_eq!(display.len(), 3);
        let loaded: Vec<_> = display.iter().collect();
        assert_eq!(loaded[0].trade.id, 5);
        assert_eq!(loaded[1].trade.id, 4);
        assert_eq!(loaded[2].trade.id, 3);
    }

    #[test]
    fn test_load_historical_trades_do_not_flash() {
        let mut display = TradesDisplay::new(10);

        let trades = vec![
            make_trade(1, TradeSide::Buy, dec!(100), dec!(1)),
            make_trade(2, TradeSide::Sell, dec!(101), dec!(2)),
        ];

        display.load_historical(trades);

        // None of the historical trades should be flashing
        for dt in display.iter() {
            assert!(!dt.is_flashing(), "Historical trade {} should not flash", dt.trade.id);
        }
    }

    #[test]
    fn test_format_relative_time_seconds() {
        // Get current time in milliseconds
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Test 0 seconds ago
        assert_eq!(format_relative_time(now_ms), "0s");

        // Test 30 seconds ago
        assert_eq!(format_relative_time(now_ms - 30_000), "30s");

        // Test 59 seconds ago
        assert_eq!(format_relative_time(now_ms - 59_000), "59s");
    }

    #[test]
    fn test_format_relative_time_minutes() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Test 1 minute ago
        assert_eq!(format_relative_time(now_ms - 60_000), "1m");

        // Test 30 minutes ago
        assert_eq!(format_relative_time(now_ms - 30 * 60_000), "30m");

        // Test 59 minutes ago
        assert_eq!(format_relative_time(now_ms - 59 * 60_000), "59m");
    }

    #[test]
    fn test_format_relative_time_absolute() {
        // Timestamp for exactly 01:23:45 UTC on epoch day
        // 1 hour 23 minutes 45 seconds = 5025 seconds
        let timestamp_ms = 5025 * 1000_u64;

        let result = format_relative_time(timestamp_ms);
        assert_eq!(result, "01:23:45");

        // Timestamp for 14:30:00 UTC
        let timestamp_ms_afternoon = (14 * 3600 + 30 * 60) * 1000_u64;
        let result_afternoon = format_relative_time(timestamp_ms_afternoon);
        assert_eq!(result_afternoon, "14:30:00");
    }

}
