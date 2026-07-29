// src/tui/liquidations_app.rs
// Application state for the liquidation feed TUI view.
//
// Manages scroll position, symbol input mode, and keyboard handling.
// Feed data is owned here; rendering is done by LiquidationFeedWidget.

use crate::data::liquidation::LiquidationFeed;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rust_decimal::Decimal;

/// Application state for the liquidation feed TUI view.
pub struct LiquidationsApp {
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Color theme
    pub theme: Theme,
    /// Liquidation event feed with scroll state and volume stats
    pub feed: LiquidationFeed,
    /// Optional symbol filter (uppercase, e.g., "BTCUSDT")
    pub symbol_filter: Option<String>,
    /// Some = symbol input mode active, holds character buffer
    pub symbol_input_buffer: Option<String>,
}

impl LiquidationsApp {
    /// Create a new LiquidationsApp with initial state.
    pub fn new(theme: Theme, feed: LiquidationFeed, symbol_filter: Option<String>) -> Self {
        Self {
            should_quit: false,
            theme,
            feed,
            symbol_filter,
            symbol_input_buffer: None,
        }
    }

    /// Handle keyboard input.
    ///
    /// Returns true if the key was handled, false otherwise.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Symbol input mode: intercept all keys
        if let Some(ref mut buffer) = self.symbol_input_buffer {
            match key.code {
                KeyCode::Enter => {
                    let input = buffer.clone();
                    self.symbol_input_buffer = None;
                    if input.is_empty() {
                        // Empty input = clear filter
                        self.symbol_filter = None;
                    } else {
                        self.symbol_filter = Some(input.to_uppercase());
                    }
                    // Clear feed for fresh start with new filter
                    self.clear_feed();
                    return true;
                }
                KeyCode::Esc => {
                    // Cancel input mode without changing filter
                    self.symbol_input_buffer = None;
                    return true;
                }
                KeyCode::Backspace => {
                    buffer.pop();
                    return true;
                }
                KeyCode::Char(c) => {
                    if c.is_alphanumeric() {
                        buffer.push(c);
                    }
                    return true;
                }
                _ => {
                    // Consume all other keys while in input mode
                    return true;
                }
            }
        }

        // Handle Shift+key hotkeys BEFORE modifiers==NONE guard
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Char('G') => {
                    // Jump to oldest (bottom)
                    self.feed.scroll_offset = self.feed.events.len().saturating_sub(1);
                    return true;
                }
                _ => {}
            }
        }

        // Ctrl+C quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return true;
        }

        // Normal mode: only handle unmodified keys
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return false;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                // Scroll down (toward older events)
                let max = self.feed.events.len().saturating_sub(1);
                if self.feed.scroll_offset < max {
                    self.feed.scroll_offset += 1;
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                // Scroll up (toward newer events)
                self.feed.scroll_offset = self.feed.scroll_offset.saturating_sub(1);
                true
            }
            KeyCode::Char('g') | KeyCode::Home => {
                // Jump to latest (top)
                self.feed.scroll_offset = 0;
                true
            }
            KeyCode::End => {
                // Jump to oldest (bottom)
                self.feed.scroll_offset = self.feed.events.len().saturating_sub(1);
                true
            }
            KeyCode::Char('/') => {
                // Enter symbol input mode
                self.symbol_input_buffer = Some(String::new());
                true
            }
            KeyCode::Char('c') => {
                // Clear symbol filter
                self.symbol_filter = None;
                self.clear_feed();
                true
            }
            _ => false,
        }
    }

    /// Clear feed events and reset volumes (used when switching symbol filter).
    pub fn clear_feed(&mut self) {
        self.feed.events.clear();
        self.feed.scroll_offset = 0;
        self.feed.total_volume = Decimal::ZERO;
        self.feed.long_liq_volume = Decimal::ZERO;
        self.feed.short_liq_volume = Decimal::ZERO;
        self.feed.flash_events.clear();
    }
}
