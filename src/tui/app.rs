// src/tui/app.rs
// Application state for the TUI

use crate::config::{CandleMode, Config, Resolution, RESOLUTION_CYCLE, TICK_SIZE_CYCLE};
use crate::data::candle::{CandleStore, PartialCandle, TradeCountBarBuilder};
use crate::data::types::{Candle, Trade};
use crate::data::TimeBarBuilder;
use crate::data::TradeStats;
use crate::data::TradeVelocityTracker;
use crate::network::ConnectionStatus;
use crate::tui::theme::{Theme, ThemeName};
use crate::tui::widgets::TradesDisplay;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rust_decimal::Decimal;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Duration to display status messages before auto-dismiss.
const STATUS_DISPLAY_DURATION: Duration = Duration::from_secs(5);

/// Message type for status bar display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Errors (red background)
    Error,
    /// Informational messages (blue/neutral background)
    Info,
}

/// 24h ticker statistics for display.
#[derive(Debug, Clone)]
pub struct TickerStats {
    /// Absolute price change in 24h
    pub price_change: Decimal,
    /// Percentage price change in 24h
    pub price_change_percent: Decimal,
    /// 24h high price
    pub high_price: Decimal,
    /// 24h low price
    pub low_price: Decimal,
    /// 24h trading volume (base asset)
    pub volume: Decimal,
}

/// Application state container.
///
/// Holds all state needed for rendering and event handling.
/// Fields are updated by the event loop and read by the render function.
pub struct App {
    /// Configuration from CLI args
    pub config: Config,
    /// Candle building mode (TickBased or TimeBased)
    pub mode: CandleMode,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Current connection status (cached for rendering)
    pub connection_status: ConnectionStatus,
    /// Terminal dimensions (updated on resize)
    pub terminal_size: (u16, u16),
    /// Most recent trade price (None until first trade received)
    pub last_price: Option<Decimal>,
    /// 24h volume (None until REST integration)
    pub _volume_24h: Option<Decimal>,
    /// Display buffer for trades with arrival timestamps
    pub trades_display: TradesDisplay,
    /// Store for completed candles
    pub candle_store: CandleStore,
    /// Builder for accumulating trades into candles
    pub candle_builder: TradeCountBarBuilder,
    /// Builder for time-based candle aggregation
    pub time_builder: Option<TimeBarBuilder>,
    /// Status message with type and timestamp for auto-dismiss
    pub status_message: Option<(String, MessageType, Instant)>,
    /// Trade velocity tracker for activity indicator
    pub velocity_tracker: TradeVelocityTracker,
    /// Current color theme
    pub theme: Theme,
    /// Current theme name for cycling
    pub theme_name: ThemeName,
    /// Whether the trades widget is visible
    pub trades_visible: bool,
    /// Whether the volume histogram is visible
    pub volume_visible: bool,
    /// 24h ticker statistics (None until fetched)
    pub ticker_stats: Option<TickerStats>,
    /// Whether ticker stats are visible in status bar
    pub stats_visible: bool,
    /// Track if we were previously disconnected (for backfill trigger)
    pub was_disconnected: bool,
    /// Scroll offset for historical viewing (0 = live mode, >0 = viewing history)
    /// Represents how many candles back from live edge we're viewing
    pub view_offset: usize,
    /// Monotonic counter incremented on each resolution switch (for stale response rejection)
    pub resolution_generation: u64,
    /// Flag set to true when resolution changes, read and cleared by event loop
    pub resolution_switched: bool,
    /// Monotonic counter incremented on each tick size switch (for stale response rejection)
    pub tick_size_generation: u64,
    /// Flag set to true when tick size changes, read and cleared by event loop
    pub tick_size_switched: bool,
    /// Whether high-precision Braille chart rendering is active
    pub high_precision_chart: bool,
    /// Buffer for symbol input mode (Some = input mode active, None = normal)
    pub symbol_input_buffer: Option<String>,
    /// Flag consumed by event loop to trigger pair switch
    pub pair_switch_requested: Option<String>,
    /// Rolling-window trade statistics accumulator (CVD, buy/sell volume, large count)
    pub trade_stats: TradeStats,
    /// Whether the size filter is active (hides small trades at render time)
    pub size_filter_active: bool,
    /// Size filter threshold (trade value = price * quantity)
    pub size_filter_threshold: Decimal,
}

impl App {
    /// Create new App with configuration.
    pub fn new(config: Config) -> Self {
        let mode = config.mode; // Copy from config (CandleMode is Copy)
        let candle_builder = TradeCountBarBuilder::new(config.tick_size);

        // Initialize time builder only in time-based mode
        let time_builder = match mode {
            CandleMode::TimeBased => {
                let resolution = config.resolution.expect("TimeBased mode requires resolution");
                Some(TimeBarBuilder::new(resolution))
            }
            CandleMode::TickBased => None,
        };

        let theme = config.theme.to_theme();
        let theme_name = config.theme;
        let large_trade_threshold = config.large_trade_threshold;
        Self {
            config,
            mode,
            should_quit: false,
            connection_status: ConnectionStatus::default(),
            terminal_size: (80, 24), // Default, updated on first render
            last_price: None,
            _volume_24h: None,
            trades_display: TradesDisplay::new(200), // 200 trade capacity
            candle_store: CandleStore::new(500),     // Store up to 500 candles
            candle_builder,
            time_builder,
            status_message: None,
            velocity_tracker: TradeVelocityTracker::new(),
            theme,
            theme_name,
            trades_visible: true,
            volume_visible: true,
            ticker_stats: None,
            stats_visible: true,
            was_disconnected: false,
            view_offset: 0,
            resolution_generation: 0,
            resolution_switched: false,
            tick_size_generation: 0,
            tick_size_switched: false,
            high_precision_chart: false,
            symbol_input_buffer: None,
            pair_switch_requested: None,
            trade_stats: TradeStats::new(large_trade_threshold),
            size_filter_active: false,
            size_filter_threshold: large_trade_threshold,
        }
    }

    /// Set an error message to display in the status bar.
    ///
    /// The message will auto-dismiss after STATUS_DISPLAY_DURATION (5 seconds).
    pub fn set_error(&mut self, message: String) {
        self.status_message = Some((message, MessageType::Error, Instant::now()));
    }

    /// Set an info message to display in the status bar.
    ///
    /// The message will auto-dismiss after STATUS_DISPLAY_DURATION (5 seconds).
    pub fn set_info(&mut self, message: String) {
        self.status_message = Some((message, MessageType::Info, Instant::now()));
    }

    /// Get the current status message and type if still within display duration.
    ///
    /// Returns None if no message or if the message has expired.
    pub fn current_status(&self) -> Option<(&str, MessageType)> {
        self.status_message.as_ref().and_then(|(msg, msg_type, when)| {
            if when.elapsed() < STATUS_DISPLAY_DURATION {
                Some((msg.as_str(), *msg_type))
            } else {
                None
            }
        })
    }

    /// Clear the status message if it has expired.
    ///
    /// Should be called on tick to clean up stale status state.
    pub fn clear_stale_status(&mut self) {
        if let Some((_, _, when)) = &self.status_message {
            if when.elapsed() >= STATUS_DISPLAY_DURATION {
                self.status_message = None;
            }
        }
    }

    /// Get current UTC time formatted as HH:MM:SS.
    pub fn utc_time_str(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        let hours = (secs / 3600) % 24;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    }

    /// Add a trade to the display buffer, update last price, and build candles.
    pub fn add_trade(&mut self, trade: Trade) {
        self.last_price = Some(trade.price);
        self.trades_display.push(trade.clone());
        self.velocity_tracker.record_trade();
        self.trade_stats.record_trade(trade.price, trade.quantity, trade.side);

        // Feed trade to candle builder
        if let Some(candle) = self.candle_builder.add_trade(&trade) {
            self.candle_store.push(candle);
        }
    }

    /// Add a trade in time-based mode.
    ///
    /// Routes trades through TimeBarBuilder instead of TradeCountBarBuilder.
    /// Finalized candles are pushed to candle_store when bucket transitions.
    pub fn add_trade_time_based(&mut self, trade: Trade) {
        self.last_price = Some(trade.price);
        self.trades_display.push(trade.clone());
        self.velocity_tracker.record_trade();
        self.trade_stats.record_trade(trade.price, trade.quantity, trade.side);

        // Feed to time-based builder
        if let Some(ref mut builder) = self.time_builder {
            if let Some(candle) = builder.add_trade(&trade) {
                self.candle_store.push(candle);
            }
        }
    }

    /// Get current bucket start timestamp (for reconciliation filtering).
    ///
    /// Returns None if not in time-based mode or no trades received yet.
    pub fn current_time_bucket(&self) -> Option<u64> {
        self.time_builder.as_ref().and_then(|b| b.current_bucket_start())
    }

    /// Get the current partial candle (in-progress).
    pub fn partial_candle(&self) -> Option<PartialCandle> {
        // In time-based mode, the partial candle comes from time_builder
        if let Some(ref builder) = self.time_builder {
            return builder.current_partial();
        }
        // In tick-based mode, use the trade count builder
        self.candle_builder.current_partial()
    }

    /// Cycle to the next larger tick size in TICK_SIZE_CYCLE.
    /// Only works in TickBased mode -- returns None in TimeBased mode.
    /// Clears candle store, replaces builder, resets view offset,
    /// increments generation counter, and sets tick_size_switched flag.
    pub fn cycle_tick_size_forward(&mut self) -> Option<u32> {
        if self.mode != CandleMode::TickBased {
            return None;
        }
        let current = self.config.tick_size;
        let new_size = TICK_SIZE_CYCLE.iter()
            .find(|&&s| s > current)
            .copied()
            .unwrap_or(TICK_SIZE_CYCLE[0]); // wrap to smallest

        if new_size != current {
            self.config.tick_size = new_size;
            self.candle_builder = TradeCountBarBuilder::new(new_size);
            self.candle_store = CandleStore::new(500);
            self.view_offset = 0;
            self.tick_size_generation += 1;
            self.tick_size_switched = true;
            tracing::info!(old_size = current, new_size, "Tick size changed");
        }
        Some(new_size)
    }

    /// Cycle to the next smaller tick size in TICK_SIZE_CYCLE.
    /// Only works in TickBased mode -- returns None in TimeBased mode.
    /// Clears candle store, replaces builder, resets view offset,
    /// increments generation counter, and sets tick_size_switched flag.
    pub fn cycle_tick_size_backward(&mut self) -> Option<u32> {
        if self.mode != CandleMode::TickBased {
            return None;
        }
        let current = self.config.tick_size;
        let new_size = TICK_SIZE_CYCLE.iter()
            .rev()
            .find(|&&s| s < current)
            .copied()
            .unwrap_or(*TICK_SIZE_CYCLE.last().unwrap()); // wrap to largest

        if new_size != current {
            self.config.tick_size = new_size;
            self.candle_builder = TradeCountBarBuilder::new(new_size);
            self.candle_store = CandleStore::new(500);
            self.view_offset = 0;
            self.tick_size_generation += 1;
            self.tick_size_switched = true;
            tracing::info!(old_size = current, new_size, "Tick size changed");
        }
        Some(new_size)
    }

    /// Cycle to the next theme in rotation.
    ///
    /// Order: Dark -> HighContrast -> Light -> Dark
    pub fn cycle_theme(&mut self) {
        self.theme_name = match self.theme_name {
            ThemeName::Dark => ThemeName::HighContrast,
            ThemeName::HighContrast => ThemeName::Light,
            ThemeName::Light => ThemeName::Dark,
        };
        self.theme = self.theme_name.to_theme();
    }

    /// Toggle trades widget visibility.
    pub fn toggle_trades(&mut self) {
        self.trades_visible = !self.trades_visible;
    }

    /// Toggle volume histogram visibility.
    pub fn toggle_volume(&mut self) {
        self.volume_visible = !self.volume_visible;
    }

    /// Toggle ticker stats visibility.
    pub fn toggle_stats(&mut self) {
        self.stats_visible = !self.stats_visible;
    }

    /// Toggle high-precision Braille chart rendering.
    pub fn toggle_high_precision(&mut self) {
        self.high_precision_chart = !self.high_precision_chart;
    }

    /// Toggle trade size filter on/off.
    pub fn toggle_size_filter(&mut self) {
        self.size_filter_active = !self.size_filter_active;
    }

    /// Reset all symbol-dependent state for a pair switch, preserving user preferences.
    ///
    /// Fields RESET: config.symbol, candle_store, candle_builder, time_builder,
    /// last_price, trades_display, velocity_tracker, ticker_stats, view_offset,
    /// was_disconnected, status_message.
    /// Generation counters are INCREMENTED (not reset) to invalidate stale responses.
    /// Fields PRESERVED: config.mode, config.tick_size, config.resolution,
    /// config.large_trade_threshold, config.theme, theme, theme_name,
    /// trades_visible, volume_visible, stats_visible, high_precision_chart,
    /// should_quit, terminal_size, mode.
    pub fn switch_symbol(&mut self, new_symbol: String) {
        self.config.symbol = new_symbol;
        self.candle_store = CandleStore::new(500);
        self.candle_builder = TradeCountBarBuilder::new(self.config.tick_size);
        self.time_builder = match self.mode {
            CandleMode::TimeBased => {
                let resolution = self.config.resolution.expect("TimeBased mode requires resolution");
                Some(TimeBarBuilder::new(resolution))
            }
            CandleMode::TickBased => None,
        };
        self.last_price = None;
        self.trades_display = TradesDisplay::new(200);
        self.velocity_tracker = TradeVelocityTracker::new();
        self.trade_stats = TradeStats::new(self.config.large_trade_threshold);
        self.size_filter_active = false;
        self.ticker_stats = None;
        self.view_offset = 0;
        self.was_disconnected = false;
        self.resolution_generation += 1;
        self.tick_size_generation += 1;
        self.status_message = None;
    }

    /// Returns the symbol input buffer contents for UI rendering.
    ///
    /// Returns Some(&str) when in symbol input mode, None otherwise.
    pub fn symbol_input_buffer(&self) -> Option<&str> {
        self.symbol_input_buffer.as_deref()
    }

    /// Cycle to the next resolution in RESOLUTION_CYCLE.
    ///
    /// Only works in TimeBased mode — returns None (no-op) in TickBased mode.
    /// Clears candle store, replaces time builder, resets view offset,
    /// increments generation counter, and sets resolution_switched flag.
    pub fn cycle_resolution_forward(&mut self) -> Option<Resolution> {
        if self.mode != CandleMode::TimeBased {
            return None;
        }

        let current = self.config.resolution.unwrap();
        let idx = RESOLUTION_CYCLE
            .iter()
            .position(|r| *r == current)
            .unwrap_or(0);
        let next_idx = (idx + 1) % RESOLUTION_CYCLE.len();
        let new_res = RESOLUTION_CYCLE[next_idx];

        self.config.resolution = Some(new_res);
        self.time_builder = Some(TimeBarBuilder::new(new_res));
        self.candle_store = CandleStore::new(500);
        self.view_offset = 0;
        self.resolution_generation += 1;
        self.resolution_switched = true;

        Some(new_res)
    }

    /// Cycle resolution backward (previous in RESOLUTION_CYCLE).
    /// Only works in TimeBased mode — returns None (no-op) in TickBased mode.
    pub fn cycle_resolution_backward(&mut self) -> Option<Resolution> {
        if self.mode != CandleMode::TimeBased {
            return None;
        }

        let current = self.config.resolution.unwrap();
        let idx = RESOLUTION_CYCLE
            .iter()
            .position(|r| *r == current)
            .unwrap_or(0);
        let next_idx = if idx == 0 {
            RESOLUTION_CYCLE.len() - 1
        } else {
            idx - 1
        };
        let new_res = RESOLUTION_CYCLE[next_idx];

        self.config.resolution = Some(new_res);
        self.time_builder = Some(TimeBarBuilder::new(new_res));
        self.candle_store = CandleStore::new(500);
        self.view_offset = 0;
        self.resolution_generation += 1;
        self.resolution_switched = true;

        Some(new_res)
    }

    /// Scroll chart one candle into history (left arrow).
    /// Returns true if scroll happened, false if at boundary.
    pub fn scroll_left(&mut self) -> bool {
        // Maximum offset is candle_store.len() - 1 (oldest visible candle)
        // But we need at least 1 candle visible, so max is len - 1
        let max_offset = self.candle_store.len().saturating_sub(1);
        if self.view_offset < max_offset {
            self.view_offset += 1;
            true
        } else {
            false // At boundary, silent no-op
        }
    }

    /// Scroll chart one candle toward present (right arrow).
    /// Returns true if scroll happened, false if already at live edge.
    pub fn scroll_right(&mut self) -> bool {
        if self.view_offset > 0 {
            self.view_offset -= 1;
            true
        } else {
            false // Already live, silent no-op
        }
    }

    /// Return to live mode (offset = 0).
    /// Returns true if was viewing history, false if already live.
    pub fn return_to_live(&mut self) -> bool {
        if self.view_offset > 0 {
            self.view_offset = 0;
            true
        } else {
            false // Already live, silent no-op
        }
    }

    /// Check if WebSocket is currently disconnected.
    ///
    /// Returns true if status is Disconnected or Reconnecting.
    pub fn is_disconnected(&self) -> bool {
        use crate::network::types::ConnectionState;
        matches!(
            self.connection_status.state,
            ConnectionState::Disconnected | ConnectionState::Reconnecting { .. }
        )
    }

    /// Request graceful shutdown.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Check if app should exit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Update terminal size (called on resize event).
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.terminal_size = (width, height);
    }

    /// Load historical trades and build initial candles.
    ///
    /// Returns the highest trade ID seen (for deduplication with WebSocket).
    /// Trades should be in ascending order (oldest first).
    /// Also populates trades_display with historical trades (no flash effect).
    pub fn load_historical_trades(&mut self, trades: Vec<Trade>) -> u64 {
        let mut last_id = 0u64;

        // Clone trades for display before consuming for candles
        let display_trades: Vec<Trade> = trades.to_vec();

        for trade in trades {
            last_id = trade.id;
            self.last_price = Some(trade.price);
            self.trade_stats.record_trade(trade.price, trade.quantity, trade.side);
            // Feed to candle builder
            if let Some(candle) = self.candle_builder.add_trade(&trade) {
                self.candle_store.push(candle);
            }
        }

        // Populate trades display with historical trades (no flash effect)
        self.trades_display.load_historical(display_trades);

        last_id
    }

    /// Load historical candles directly into the candle store.
    ///
    /// Used for time-based mode where candles come from kline API.
    /// Unlike load_historical_trades, this bypasses the TradeCountBarBuilder
    /// because time-based candles come fully formed from the API.
    ///
    /// # Arguments
    /// * `candles` - Pre-built candles from load_asterdex_historical_klines()
    pub fn load_historical_candles(&mut self, candles: Vec<Candle>) {
        tracing::info!(count = candles.len(), "Loading historical candles");

        // Set last_price from the most recent candle's close
        if let Some(last_candle) = candles.last() {
            self.last_price = Some(last_candle.close);
        }

        for candle in candles {
            self.candle_store.push(candle);
        }
    }

    /// Handle keyboard input.
    ///
    /// Returns true if the key was handled, false otherwise.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Symbol input mode intercepts ALL keys — must be checked first
        // to prevent accidental quit, volume toggle, etc. while typing
        if let Some(ref mut buffer) = self.symbol_input_buffer {
            match key.code {
                KeyCode::Char(c) => {
                    if buffer.len() < 20 {
                        buffer.push(c.to_ascii_uppercase());
                    }
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Enter => {
                    if !buffer.is_empty() {
                        self.pair_switch_requested = Some(buffer.clone());
                    }
                    self.symbol_input_buffer = None;
                }
                KeyCode::Esc => {
                    self.symbol_input_buffer = None;
                }
                _ => {}
            }
            return true; // consumed — block all other handlers
        }

        match (key.code, key.modifiers) {
            // Ctrl+C - graceful shutdown
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.quit();
                true
            }
            // 'q' - quit
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.quit();
                true
            }
            // Escape - quit
            (KeyCode::Esc, _) => {
                self.quit();
                true
            }
            // Left arrow - scroll into history
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.scroll_left();
                true
            }
            // Right arrow - scroll toward present
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.scroll_right();
                true
            }
            // 'l' - return to live mode
            (KeyCode::Char('l'), KeyModifiers::NONE) => {
                if self.return_to_live() {
                    self.set_info("Live".to_string());
                }
                true
            }
            // Tick size increase: + or = or Up arrow
            (KeyCode::Char('+'), _)
            | (KeyCode::Char('='), _)
            | (KeyCode::Up, KeyModifiers::NONE) => {
                if let Some(new_size) = self.cycle_tick_size_forward() {
                    self.set_info(format!("Tick size: {}", new_size));
                }
                true
            }
            // Tick size decrease: - or Down arrow
            (KeyCode::Char('-'), _) | (KeyCode::Down, KeyModifiers::NONE) => {
                if let Some(new_size) = self.cycle_tick_size_backward() {
                    self.set_info(format!("Tick size: {}", new_size));
                }
                true
            }
            // Precision toggle: 'p' (hi-res Braille rendering)
            (KeyCode::Char('p'), KeyModifiers::NONE) => {
                self.toggle_high_precision();
                let state = if self.high_precision_chart { "on" } else { "off" };
                self.set_info(format!("HI-RES: {}", state));
                true
            }
            // Toggle trades widget visibility: 't'
            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                self.toggle_trades();
                true
            }
            // Toggle volume histogram visibility: 'v'
            (KeyCode::Char('v'), KeyModifiers::NONE) => {
                self.toggle_volume();
                let state = if self.volume_visible { "on" } else { "off" };
                self.set_info(format!("Volume: {}", state));
                true
            }
            // Toggle stats visibility: 'i' (info)
            (KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.toggle_stats();
                let state = if self.stats_visible { "on" } else { "off" };
                self.set_info(format!("Stats: {}", state));
                true
            }
            // Theme cycling: Shift+P (palette)
            (KeyCode::Char('P'), KeyModifiers::SHIFT) => {
                self.cycle_theme();
                self.set_info(format!("Theme: {:?}", self.theme_name));
                true
            }
            // Resolution cycling: 'r' forward, 'R' (Shift+R) backward (time-based mode only)
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                if let Some(new_res) = self.cycle_resolution_forward() {
                    self.set_info(format!("Resolution: {}", new_res));
                }
                true
            }
            (KeyCode::Char('R'), KeyModifiers::SHIFT) => {
                if let Some(new_res) = self.cycle_resolution_backward() {
                    self.set_info(format!("Resolution: {}", new_res));
                }
                true
            }
            // '/' - enter symbol input mode for pair switching
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.symbol_input_buffer = Some(String::new());
                self.set_info("Enter symbol: ".to_string());
                true
            }
            // 'f' - toggle trade size filter
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                self.toggle_size_filter();
                let state = if self.size_filter_active { "on" } else { "off" };
                self.set_info(format!("Filter: {}", state));
                true
            }
            // Other keys not handled yet
            _ => false,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CandleMode;
    use crate::tui::theme::ThemeName;
    use rust_decimal_macros::dec;

    fn test_config() -> Config {
        Config {
            symbol: "BTCUSDT".to_string(),
            mode: CandleMode::TickBased,
            tick_size: 100,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: None,
        }
    }

    #[test]
    fn test_app_quit() {
        let mut app = App::new(test_config());
        assert!(!app.should_quit());
        app.quit();
        assert!(app.should_quit());
    }

    #[test]
    fn test_handle_key_quit() {
        let mut app = App::new(test_config());

        // 'q' should quit
        let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(key_q));
        assert!(app.should_quit());
    }

    #[test]
    fn test_handle_key_ctrl_c() {
        let mut app = App::new(test_config());

        // Ctrl+C should quit
        let key_ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.handle_key(key_ctrl_c));
        assert!(app.should_quit());
    }

    #[test]
    fn test_handle_key_escape() {
        let mut app = App::new(test_config());

        // Escape should quit
        let key_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.handle_key(key_esc));
        assert!(app.should_quit());
    }

    #[test]
    fn test_handle_key_unhandled() {
        let mut app = App::new(test_config());

        // 'a' should not quit
        let key_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(!app.handle_key(key_a));
        assert!(!app.should_quit());
    }

    #[test]
    fn test_resize() {
        let mut app = App::new(test_config());
        assert_eq!(app.terminal_size, (80, 24));
        app.set_size(120, 40);
        assert_eq!(app.terminal_size, (120, 40));
    }

    #[test]
    fn test_utc_time_str_format() {
        let app = App::new(test_config());
        let time_str = app.utc_time_str();
        // Should be HH:MM:SS format (8 characters total)
        assert_eq!(time_str.len(), 8);
        assert_eq!(&time_str[2..3], ":");
        assert_eq!(&time_str[5..6], ":");
    }

    #[test]
    fn test_add_trade() {
        use crate::data::types::{Trade, TradeSide};

        let mut app = App::new(test_config());
        assert!(app.last_price.is_none());
        assert_eq!(app.trades_display.len(), 0);

        let trade = Trade {
            id: 1,
            price: dec!(50000),
            quantity: dec!(1.5),
            timestamp: 1000,
            side: TradeSide::Buy,
            is_buyer_maker: false,
        };
        app.add_trade(trade);

        assert_eq!(app.last_price, Some(dec!(50000)));
        assert_eq!(app.trades_display.len(), 1);
    }

    #[test]
    fn test_load_historical_trades() {
        use crate::data::types::{Trade, TradeSide};

        // Use tick_size of 3 so we can complete candles with few trades
        let config = Config {
            symbol: "BTCUSDT".to_string(),

            mode: CandleMode::TickBased,
            tick_size: 3, // 3 trades per candle
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: None,

        };
        let mut app = App::new(config);

        // Create 7 trades (should produce 2 complete candles, 1 partial)
        let trades: Vec<Trade> = (1..=7)
            .map(|i| Trade {
                id: i as u64,
                price: dec!(50000) + Decimal::from(i * 100),
                quantity: dec!(0.1),
                timestamp: 1000 + i as u64,
                side: TradeSide::Buy,
                is_buyer_maker: false,
            })
            .collect();

        let last_id = app.load_historical_trades(trades);

        // Should return highest trade ID
        assert_eq!(last_id, 7);

        // Should update last_price to the last trade's price
        assert_eq!(app.last_price, Some(dec!(50700)));

        // Should populate trades_display with historical trades
        assert_eq!(app.trades_display.len(), 7);

        // Trades should be newest-first (id 7 at front)
        let first_display_trade = app.trades_display.iter().next().unwrap();
        assert_eq!(first_display_trade.trade.id, 7);

        // Historical trades should NOT be flashing
        assert!(!first_display_trade.is_flashing());

        // Should build candles (7 trades / 3 per candle = 2 complete candles)
        assert_eq!(app.candle_store.len(), 2);

        // Verify first candle OHLC (trades 1-3)
        let candles: Vec<_> = app.candle_store.iter().collect();
        assert_eq!(candles[0].open, dec!(50100));
        assert_eq!(candles[0].close, dec!(50300));
        assert_eq!(candles[0].trade_count, 3);

        // Verify second candle (trades 4-6)
        assert_eq!(candles[1].open, dec!(50400));
        assert_eq!(candles[1].close, dec!(50600));
        assert_eq!(candles[1].trade_count, 3);

        // Partial candle (trade 7) should be in the builder, not store
        let partial = app.partial_candle();
        assert!(partial.is_some());
        assert_eq!(partial.unwrap().trade_count, 1);
    }

    #[test]
    fn test_set_error() {
        let mut app = App::new(test_config());
        assert!(app.status_message.is_none());

        app.set_error("Test error".to_string());

        assert!(app.status_message.is_some());
        let (msg, msg_type, _when) = app.status_message.as_ref().unwrap();
        assert_eq!(msg, "Test error");
        assert_eq!(*msg_type, MessageType::Error);
    }

    #[test]
    fn test_set_info() {
        let mut app = App::new(test_config());
        assert!(app.status_message.is_none());

        app.set_info("Test info".to_string());

        assert!(app.status_message.is_some());
        let (msg, msg_type, _when) = app.status_message.as_ref().unwrap();
        assert_eq!(msg, "Test info");
        assert_eq!(*msg_type, MessageType::Info);
    }

    #[test]
    fn test_current_status_returns_fresh_message() {
        let mut app = App::new(test_config());

        app.set_error("Fresh error".to_string());

        // Should return the message immediately (within display duration)
        let status = app.current_status();
        assert!(status.is_some());
        let (msg, msg_type) = status.unwrap();
        assert_eq!(msg, "Fresh error");
        assert_eq!(msg_type, MessageType::Error);
    }

    #[test]
    fn test_current_status_returns_none_when_no_message() {
        let app = App::new(test_config());

        assert!(app.current_status().is_none());
    }

    #[test]
    fn test_clear_stale_status_clears_expired() {
        let mut app = App::new(test_config());

        // Set an error with a timestamp in the past (beyond display duration)
        app.status_message = Some((
            "Stale error".to_string(),
            MessageType::Error,
            Instant::now() - Duration::from_secs(10), // 10 seconds ago, beyond 5s threshold
        ));

        // Should be expired
        assert!(app.current_status().is_none());

        // Clear should remove the stale message
        app.clear_stale_status();
        assert!(app.status_message.is_none());
    }

    #[test]
    fn test_clear_stale_status_keeps_fresh() {
        let mut app = App::new(test_config());

        app.set_error("Fresh error".to_string());

        // Clear should not remove a fresh message
        app.clear_stale_status();
        assert!(app.status_message.is_some());
    }

    #[test]
    fn test_cycle_tick_size_forward_cycles() {
        let mut app = App::new(test_config());
        assert_eq!(app.config.tick_size, 100);

        let result = app.cycle_tick_size_forward();
        assert_eq!(result, Some(200));
        assert_eq!(app.config.tick_size, 200);
    }

    #[test]
    fn test_cycle_tick_size_backward_cycles() {
        let mut app = App::new(test_config());
        assert_eq!(app.config.tick_size, 100);

        let result = app.cycle_tick_size_backward();
        assert_eq!(result, Some(50));
        assert_eq!(app.config.tick_size, 50);
    }

    #[test]
    fn test_cycle_tick_size_forward_wraps() {
        let config = Config {
            symbol: "BTCUSDT".to_string(),
            mode: CandleMode::TickBased,
            tick_size: 5000,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: None,
        };
        let mut app = App::new(config);

        // At max (5000), cycling forward should wrap to 10 (first preset)
        let result = app.cycle_tick_size_forward();
        assert_eq!(result, Some(10));
        assert_eq!(app.config.tick_size, 10);
    }

    #[test]
    fn test_cycle_tick_size_backward_wraps() {
        let config = Config {
            symbol: "BTCUSDT".to_string(),
            mode: CandleMode::TickBased,
            tick_size: 10,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: None,
        };
        let mut app = App::new(config);

        // At min (10), cycling backward should wrap to 5000 (last preset)
        let result = app.cycle_tick_size_backward();
        assert_eq!(result, Some(5000));
        assert_eq!(app.config.tick_size, 5000);
    }

    #[test]
    fn test_cycle_tick_size_forward_clears_candles() {
        use crate::data::types::{Trade, TradeSide};

        let config = Config {
            symbol: "BTCUSDT".to_string(),
            mode: CandleMode::TickBased,
            tick_size: 50,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: None,
        };
        let mut app = App::new(config);

        // Add enough trades to create a candle (tick_size=50, so need 50+ trades)
        // Actually let's just push directly to candle store for simplicity
        for i in 1..=3 {
            app.add_trade(Trade {
                id: i,
                price: dec!(50000),
                quantity: dec!(0.1),
                timestamp: 1000 + i,
                side: TradeSide::Buy,
                is_buyer_maker: false,
            });
        }

        // Manually add a candle to verify clearing
        app.candle_store.push(crate::data::types::Candle {
            open: dec!(50000),
            high: dec!(50000),
            low: dec!(50000),
            close: dec!(50000),
            volume: dec!(1),
            buy_volume: Decimal::ZERO,
            sell_volume: Decimal::ZERO,
            trade_count: 50,
            open_time: 0,
            close_time: 100,
        });

        assert!(app.candle_store.len() > 0);

        // Cycle tick size forward
        app.cycle_tick_size_forward();

        // Candle store should be cleared
        assert_eq!(app.candle_store.len(), 0);
    }

    #[test]
    fn test_handle_key_tick_increase() {
        let mut app = App::new(test_config());
        assert_eq!(app.config.tick_size, 100);

        // '+' should increase tick size to next preset (200)
        let key_plus = KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE);
        assert!(app.handle_key(key_plus));
        assert_eq!(app.config.tick_size, 200);
        // Should show confirmation message
        let status = app.current_status();
        assert!(status.is_some());
        assert!(status.unwrap().0.contains("Tick size: 200"));
    }

    #[test]
    fn test_handle_key_tick_decrease() {
        let mut app = App::new(test_config());
        assert_eq!(app.config.tick_size, 100);

        // '-' should decrease tick size to previous preset (50)
        let key_minus = KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE);
        assert!(app.handle_key(key_minus));
        assert_eq!(app.config.tick_size, 50);
        // Should show confirmation message
        let status = app.current_status();
        assert!(status.is_some());
        assert!(status.unwrap().0.contains("Tick size: 50"));
    }

    #[test]
    fn test_handle_key_up_arrow_increases_tick() {
        let mut app = App::new(test_config());
        assert_eq!(app.config.tick_size, 100);

        let key_up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert!(app.handle_key(key_up));
        assert_eq!(app.config.tick_size, 200);
    }

    #[test]
    fn test_handle_key_down_arrow_decreases_tick() {
        let mut app = App::new(test_config());
        assert_eq!(app.config.tick_size, 100);

        let key_down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(app.handle_key(key_down));
        assert_eq!(app.config.tick_size, 50);
    }

    #[test]
    fn test_cycle_tick_size_forward_time_mode_noop() {
        let mut app = App::new(time_based_config());
        let original_tick_size = app.config.tick_size;

        let result = app.cycle_tick_size_forward();

        assert_eq!(result, None);
        assert_eq!(app.config.tick_size, original_tick_size);
        assert_eq!(app.tick_size_generation, 0);
        assert!(!app.tick_size_switched);
    }

    #[test]
    fn test_cycle_tick_size_forward_sets_flag() {
        let mut app = App::new(test_config());
        assert_eq!(app.tick_size_generation, 0);
        assert!(!app.tick_size_switched);

        app.cycle_tick_size_forward();
        assert_eq!(app.tick_size_generation, 1);
        assert!(app.tick_size_switched);

        // Second cycle
        app.tick_size_switched = false; // simulate event loop clearing
        app.cycle_tick_size_forward();
        assert_eq!(app.tick_size_generation, 2);
        assert!(app.tick_size_switched);
    }

    #[test]
    fn test_cycle_theme() {
        let mut app = App::new(test_config());
        assert_eq!(app.theme_name, ThemeName::Dark);

        app.cycle_theme();
        assert_eq!(app.theme_name, ThemeName::HighContrast);

        app.cycle_theme();
        assert_eq!(app.theme_name, ThemeName::Light);

        app.cycle_theme();
        assert_eq!(app.theme_name, ThemeName::Dark);
    }

    #[test]
    fn test_handle_key_theme_toggle() {
        let mut app = App::new(test_config());
        assert_eq!(app.theme_name, ThemeName::Dark);

        // Shift+P should cycle theme (palette hotkey, reassigned from 'p')
        let key_shift_p = KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT);
        assert!(app.handle_key(key_shift_p));
        assert_eq!(app.theme_name, ThemeName::HighContrast);
    }

    #[test]
    fn test_handle_key_t_toggles_trades() {
        let mut app = App::new(test_config());
        assert_eq!(app.theme_name, ThemeName::Dark);
        assert!(app.trades_visible); // Default visible

        // 't' should toggle trades (not theme)
        let key_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        assert!(app.handle_key(key_t)); // Returns true (handled)
        assert_eq!(app.theme_name, ThemeName::Dark); // Theme unchanged
        assert!(!app.trades_visible); // Trades now hidden
    }

    #[test]
    fn test_toggle_trades() {
        let mut app = App::new(test_config());
        assert!(app.trades_visible);

        app.toggle_trades();
        assert!(!app.trades_visible);

        app.toggle_trades();
        assert!(app.trades_visible);
    }

    #[test]
    fn test_toggle_volume() {
        let mut app = App::new(test_config());
        assert!(app.volume_visible);

        app.toggle_volume();
        assert!(!app.volume_visible);

        app.toggle_volume();
        assert!(app.volume_visible);
    }

    #[test]
    fn test_handle_key_v_toggles_volume() {
        let mut app = App::new(test_config());
        assert!(app.volume_visible);

        let key_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        assert!(app.handle_key(key_v));
        assert!(!app.volume_visible);
    }

    #[test]
    fn test_toggle_stats() {
        let mut app = App::new(test_config());
        assert!(app.stats_visible);

        app.toggle_stats();
        assert!(!app.stats_visible);

        app.toggle_stats();
        assert!(app.stats_visible);
    }

    #[test]
    fn test_handle_key_i_toggles_stats() {
        let mut app = App::new(test_config());
        assert!(app.stats_visible);

        let key_i = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        assert!(app.handle_key(key_i));
        assert!(!app.stats_visible);
    }

    #[test]
    fn test_app_has_mode_from_config() {
        use crate::config::Resolution;

        // Test that mode is correctly copied from Config to App
        let mut config = test_config();
        config.mode = CandleMode::TimeBased;
        config.resolution = Some(Resolution::OneMinute); // Required for TimeBased mode

        let app = App::new(config);

        assert_eq!(app.mode, CandleMode::TimeBased);
    }

    #[test]
    fn test_app_time_based_mode_has_builder() {
        use crate::config::Resolution;

        let config = Config {
            symbol: "BTCUSDT".to_string(),

            mode: CandleMode::TimeBased,
            tick_size: 100, // Ignored in time-based
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: Some(Resolution::OneMinute),

        };

        let app = App::new(config);

        assert!(app.time_builder.is_some());
        assert_eq!(app.mode, CandleMode::TimeBased);
    }

    #[test]
    fn test_app_tick_based_mode_no_time_builder() {
        let config = test_config(); // tick-based

        let app = App::new(config);

        assert!(app.time_builder.is_none());
        assert_eq!(app.mode, CandleMode::TickBased);
    }

    #[test]
    fn test_add_trade_time_based_updates_price() {
        use crate::config::Resolution;
        use crate::data::types::TradeSide;

        let config = Config {
            symbol: "BTCUSDT".to_string(),

            mode: CandleMode::TimeBased,
            tick_size: 100,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: Some(Resolution::OneMinute),

        };

        let mut app = App::new(config);
        assert!(app.last_price.is_none());

        let trade = Trade {
            id: 1,
            price: dec!(50000),
            quantity: dec!(1.5),
            timestamp: 60000, // 00:01:00
            side: TradeSide::Buy,
            is_buyer_maker: false,
        };
        app.add_trade_time_based(trade);

        assert_eq!(app.last_price, Some(dec!(50000)));
        assert_eq!(app.trades_display.len(), 1);
    }

    #[test]
    fn test_add_trade_time_based_finalizes_candle() {
        use crate::config::Resolution;
        use crate::data::types::TradeSide;

        let config = Config {
            symbol: "BTCUSDT".to_string(),

            mode: CandleMode::TimeBased,
            tick_size: 100,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: Some(Resolution::OneMinute),

        };

        let mut app = App::new(config);

        // Trade in bucket 00:00
        let trade1 = Trade {
            id: 1,
            price: dec!(50000),
            quantity: dec!(1.0),
            timestamp: 30000, // 00:00:30
            side: TradeSide::Buy,
            is_buyer_maker: false,
        };
        app.add_trade_time_based(trade1);
        assert_eq!(app.candle_store.len(), 0); // Not finalized yet

        // Trade in bucket 00:01 (finalizes 00:00)
        let trade2 = Trade {
            id: 2,
            price: dec!(51000),
            quantity: dec!(1.0),
            timestamp: 60000, // 00:01:00
            side: TradeSide::Buy,
            is_buyer_maker: false,
        };
        app.add_trade_time_based(trade2);
        assert_eq!(app.candle_store.len(), 1); // 00:00 candle finalized

        // Verify candle values
        let candle = app.candle_store.iter().next().unwrap();
        assert_eq!(candle.open_time, 0); // Bucket start for 00:00
        assert_eq!(candle.close, dec!(50000));
    }

    #[test]
    fn test_current_time_bucket() {
        use crate::config::Resolution;
        use crate::data::types::TradeSide;

        let config = Config {
            symbol: "BTCUSDT".to_string(),

            mode: CandleMode::TimeBased,
            tick_size: 100,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: Some(Resolution::OneMinute),

        };

        let mut app = App::new(config);

        // Before any trades, bucket is None
        assert!(app.current_time_bucket().is_none());

        // Add trade
        let trade = Trade {
            id: 1,
            price: dec!(50000),
            quantity: dec!(1.0),
            timestamp: 90000, // 00:01:30
            side: TradeSide::Buy,
            is_buyer_maker: false,
        };
        app.add_trade_time_based(trade);

        // Bucket should be 60000 (00:01:00, aligned to minute)
        assert_eq!(app.current_time_bucket(), Some(60000));
    }

    #[test]
    fn test_current_time_bucket_none_in_tick_mode() {
        let config = test_config(); // tick-based
        let app = App::new(config);

        // No time_builder in tick mode, so bucket is None
        assert!(app.current_time_bucket().is_none());
    }

    #[test]
    fn test_scroll_left_increases_offset() {
        use crate::data::types::Candle;

        let mut app = App::new(test_config());
        assert_eq!(app.view_offset, 0);

        // Add some candles to the store
        for i in 0..10 {
            app.candle_store.push(Candle {
                open: dec!(100),
                high: dec!(110),
                low: dec!(90),
                close: dec!(105),
                volume: dec!(1000),
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                trade_count: 100,
                open_time: i * 1000,
                close_time: (i + 1) * 1000,
            });
        }

        assert!(app.scroll_left());
        assert_eq!(app.view_offset, 1);

        assert!(app.scroll_left());
        assert_eq!(app.view_offset, 2);
    }

    #[test]
    fn test_scroll_right_decreases_offset() {
        let mut app = App::new(test_config());
        app.view_offset = 5;

        assert!(app.scroll_right());
        assert_eq!(app.view_offset, 4);

        assert!(app.scroll_right());
        assert_eq!(app.view_offset, 3);
    }

    #[test]
    fn test_scroll_left_boundary() {
        use crate::data::types::Candle;

        let mut app = App::new(test_config());

        // Add 5 candles
        for i in 0..5 {
            app.candle_store.push(Candle {
                open: dec!(100),
                high: dec!(110),
                low: dec!(90),
                close: dec!(105),
                volume: dec!(1000),
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                trade_count: 100,
                open_time: i * 1000,
                close_time: (i + 1) * 1000,
            });
        }

        // Max offset is len - 1 = 4
        app.view_offset = 4;

        // At boundary, should return false
        assert!(!app.scroll_left());
        assert_eq!(app.view_offset, 4);
    }

    #[test]
    fn test_scroll_right_boundary() {
        let mut app = App::new(test_config());
        app.view_offset = 0;

        // Already at 0, should return false
        assert!(!app.scroll_right());
        assert_eq!(app.view_offset, 0);
    }

    #[test]
    fn test_return_to_live() {
        let mut app = App::new(test_config());
        app.view_offset = 10;

        assert!(app.return_to_live());
        assert_eq!(app.view_offset, 0);

        // Already live, should return false
        assert!(!app.return_to_live());
        assert_eq!(app.view_offset, 0);
    }

    #[test]
    fn test_handle_key_left_arrow() {
        use crate::data::types::Candle;

        let mut app = App::new(test_config());

        // Add candles so we can scroll
        for i in 0..10 {
            app.candle_store.push(Candle {
                open: dec!(100),
                high: dec!(110),
                low: dec!(90),
                close: dec!(105),
                volume: dec!(1000),
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                trade_count: 100,
                open_time: i * 1000,
                close_time: (i + 1) * 1000,
            });
        }

        let key_left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        assert!(app.handle_key(key_left));
        assert_eq!(app.view_offset, 1);
    }

    #[test]
    fn test_handle_key_right_arrow() {
        let mut app = App::new(test_config());
        app.view_offset = 5;

        let key_right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        assert!(app.handle_key(key_right));
        assert_eq!(app.view_offset, 4);
    }

    #[test]
    fn test_handle_key_l_returns_live() {
        let mut app = App::new(test_config());
        app.view_offset = 10;

        let key_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        assert!(app.handle_key(key_l));
        assert_eq!(app.view_offset, 0);

        // Should show "Live" info message
        let status = app.current_status();
        assert!(status.is_some());
        assert_eq!(status.unwrap().0, "Live");
    }

    // --- Resolution cycling tests ---

    fn time_based_config() -> Config {
        Config {
            symbol: "BTCUSDT".to_string(),
            mode: CandleMode::TimeBased,
            tick_size: 100,
            large_trade_threshold: dec!(10000),
            theme: ThemeName::Dark,
            resolution: Some(Resolution::OneMinute),
        }
    }

    #[test]
    fn test_cycle_resolution_forward_cycles() {
        let mut app = App::new(time_based_config());

        // Cycle through all 8 resolutions
        let expected = [
            Resolution::ThreeMinutes,
            Resolution::FiveMinutes,
            Resolution::FifteenMinutes,
            Resolution::ThirtyMinutes,
            Resolution::OneHour,
            Resolution::FourHours,
            Resolution::OneDay,
            Resolution::OneMinute, // wraps
        ];

        for (i, expected_res) in expected.iter().enumerate() {
            let result = app.cycle_resolution_forward();
            assert_eq!(
                result,
                Some(*expected_res),
                "Cycle {} should be {:?}",
                i + 1,
                expected_res
            );
            assert_eq!(app.config.resolution, Some(*expected_res));
        }

        // Verify wrapping: after 8 cycles from 1m we should be back at 1m
        assert_eq!(app.config.resolution, Some(Resolution::OneMinute));
    }

    #[test]
    fn test_cycle_resolution_forward_clears_candles() {
        use crate::data::types::Candle;

        let mut app = App::new(time_based_config());

        // Add some candles
        for i in 0..5 {
            app.candle_store.push(Candle {
                open: dec!(100),
                high: dec!(110),
                low: dec!(90),
                close: dec!(105),
                volume: dec!(1000),
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                trade_count: 100,
                open_time: i * 60000,
                close_time: (i + 1) * 60000,
            });
        }
        app.view_offset = 3;

        assert_eq!(app.candle_store.len(), 5);

        app.cycle_resolution_forward();

        assert_eq!(app.candle_store.len(), 0);
        assert_eq!(app.view_offset, 0);
    }

    #[test]
    fn test_cycle_resolution_forward_tick_mode_noop() {
        let mut app = App::new(test_config()); // tick-based
        let original_resolution = app.config.resolution;

        let result = app.cycle_resolution_forward();

        assert_eq!(result, None);
        assert_eq!(app.config.resolution, original_resolution);
        assert_eq!(app.resolution_generation, 0);
        assert!(!app.resolution_switched);
    }

    #[test]
    fn test_cycle_resolution_forward_increments_generation() {
        let mut app = App::new(time_based_config());
        assert_eq!(app.resolution_generation, 0);
        assert!(!app.resolution_switched);

        app.cycle_resolution_forward();
        assert_eq!(app.resolution_generation, 1);
        assert!(app.resolution_switched);

        app.cycle_resolution_forward();
        assert_eq!(app.resolution_generation, 2);

        app.cycle_resolution_forward();
        assert_eq!(app.resolution_generation, 3);
    }

    #[test]
    fn test_handle_key_r_cycles_resolution() {
        let mut app = App::new(time_based_config());
        assert_eq!(app.config.resolution, Some(Resolution::OneMinute));

        let key_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(app.handle_key(key_r));

        assert_eq!(app.config.resolution, Some(Resolution::ThreeMinutes));

        // Should show info message with new resolution
        let status = app.current_status();
        assert!(status.is_some());
        assert!(status.unwrap().0.contains("Resolution: 3m"));
    }

    #[test]
    fn test_resolution_display() {
        assert_eq!(format!("{}", Resolution::OneMinute), "1m");
        assert_eq!(format!("{}", Resolution::FiveMinutes), "5m");
        assert_eq!(format!("{}", Resolution::FifteenMinutes), "15m");
        assert_eq!(format!("{}", Resolution::OneHour), "1h");
        assert_eq!(format!("{}", Resolution::FourHours), "4h");
        assert_eq!(format!("{}", Resolution::OneDay), "1d");
    }

    #[test]
    fn test_cycle_resolution_backward_cycles() {
        let mut app = App::new(time_based_config());
        // Start at 1m, going backward wraps to 1d
        let result = app.cycle_resolution_backward();
        assert_eq!(result, Some(Resolution::OneDay));

        let result = app.cycle_resolution_backward();
        assert_eq!(result, Some(Resolution::FourHours));

        let result = app.cycle_resolution_backward();
        assert_eq!(result, Some(Resolution::OneHour));
    }

    #[test]
    fn test_cycle_resolution_backward_tick_mode_noop() {
        let mut app = App::new(test_config()); // tick-based
        let result = app.cycle_resolution_backward();
        assert_eq!(result, None);
    }

    #[test]
    fn test_handle_key_shift_r_cycles_backward() {
        let mut app = App::new(time_based_config());
        assert_eq!(app.config.resolution, Some(Resolution::OneMinute));

        let key_shift_r = KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert!(app.handle_key(key_shift_r));

        // 1m backward wraps to 1d
        assert_eq!(app.config.resolution, Some(Resolution::OneDay));

        let status = app.current_status();
        assert!(status.is_some());
        assert!(status.unwrap().0.contains("Resolution: 1d"));
    }

    // --- High precision toggle tests ---

    #[test]
    fn test_toggle_high_precision() {
        let mut app = App::new(test_config());
        assert!(!app.high_precision_chart);

        app.toggle_high_precision();
        assert!(app.high_precision_chart);

        app.toggle_high_precision();
        assert!(!app.high_precision_chart);
    }

    #[test]
    fn test_handle_key_p_toggles_precision() {
        let mut app = App::new(test_config());
        assert!(!app.high_precision_chart);
        let original_theme = app.theme_name;

        // 'p' should toggle precision, NOT change theme
        let key_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert!(app.handle_key(key_p));
        assert!(app.high_precision_chart);
        assert_eq!(app.theme_name, original_theme); // Theme unchanged

        // Status message should say "HI-RES: on"
        let status = app.current_status();
        assert!(status.is_some());
        assert!(status.unwrap().0.contains("HI-RES: on"));
    }

    #[test]
    fn test_handle_key_shift_p_cycles_theme() {
        let mut app = App::new(test_config());
        assert_eq!(app.theme_name, ThemeName::Dark);
        assert!(!app.high_precision_chart);

        // Shift+P should cycle theme, NOT toggle precision
        let key_shift_p = KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT);
        assert!(app.handle_key(key_shift_p));
        assert_eq!(app.theme_name, ThemeName::HighContrast);
        assert!(!app.high_precision_chart); // Precision unchanged

        // Status message should mention theme
        let status = app.current_status();
        assert!(status.is_some());
        assert!(status.unwrap().0.contains("Theme:"));
    }
}
