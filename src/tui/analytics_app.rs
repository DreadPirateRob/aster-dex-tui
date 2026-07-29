// src/tui/analytics_app.rs
// Application state for the session analytics TUI view.
//
// Manages scroll position for recent fills, symbol input mode,
// and keyboard handling. SessionAnalytics data is owned here;
// rendering is done by AnalyticsDashboardWidget.

use crate::data::SessionAnalytics;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Approximate maximum visible fills in the scrollable list.
const VISIBLE_FILLS: usize = 20;

/// Input mode for the analytics view.
pub enum AnalyticsInputMode {
    /// Normal mode — keyboard shortcuts active.
    Normal,
    /// Symbol input mode — typing filter text.
    SymbolInput,
}

/// Application state for the session analytics TUI view.
pub struct AnalyticsApp {
    /// SessionAnalytics data accumulator
    pub analytics: SessionAnalytics,
    /// Color theme
    pub theme: Theme,
    /// Optional symbol filter (uppercase, e.g., "BTCUSDT")
    pub symbol_filter: Option<String>,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Scroll offset for recent fills list (0 = newest visible)
    pub scroll_offset: usize,
    /// Current input mode
    pub input_mode: AnalyticsInputMode,
    /// Character buffer when in SymbolInput mode
    pub input_buffer: Option<String>,
}

impl AnalyticsApp {
    /// Create a new AnalyticsApp with initial state.
    pub fn new(analytics: SessionAnalytics, theme: Theme, symbol_filter: Option<String>) -> Self {
        Self {
            analytics,
            theme,
            symbol_filter,
            should_quit: false,
            scroll_offset: 0,
            input_mode: AnalyticsInputMode::Normal,
            input_buffer: None,
        }
    }

    /// Handle keyboard input.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Symbol input mode: intercept all keys
        if let AnalyticsInputMode::SymbolInput = self.input_mode {
            if let Some(ref mut buffer) = self.input_buffer {
                match key.code {
                    KeyCode::Enter => {
                        let input = buffer.clone();
                        if input.is_empty() {
                            // Empty input = clear filter
                            self.symbol_filter = None;
                        } else {
                            self.symbol_filter = Some(input.to_uppercase());
                        }
                        self.input_mode = AnalyticsInputMode::Normal;
                        self.input_buffer = None;
                        return;
                    }
                    KeyCode::Esc => {
                        // Cancel input mode without changing filter
                        self.input_mode = AnalyticsInputMode::Normal;
                        self.input_buffer = None;
                        return;
                    }
                    KeyCode::Backspace => {
                        buffer.pop();
                        return;
                    }
                    KeyCode::Char(c) => {
                        if c.is_alphanumeric() {
                            buffer.push(c);
                        }
                        return;
                    }
                    _ => {
                        // Consume all other keys while in input mode
                        return;
                    }
                }
            }
        }

        // Handle Shift+G BEFORE modifiers==NONE guard
        // (crossterm reports SHIFT modifier — documented project pattern)
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            if let KeyCode::Char('G') = key.code {
                self.scroll_to_bottom();
                return;
            }
        }

        // Ctrl+C quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // Normal mode: only handle unmodified keys
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_down();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_up();
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.scroll_to_top();
            }
            KeyCode::End => {
                self.scroll_to_bottom();
            }
            KeyCode::Char('/') => {
                // Enter symbol input mode
                self.input_mode = AnalyticsInputMode::SymbolInput;
                self.input_buffer = Some(String::new());
            }
            KeyCode::Char('c') => {
                // Clear symbol filter
                self.symbol_filter = None;
            }
            _ => {}
        }
    }

    /// Scroll down (toward older fills).
    fn scroll_down(&mut self) {
        let max = self.analytics.recent_fills.len().saturating_sub(VISIBLE_FILLS);
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    /// Scroll up (toward newer fills).
    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Jump to top (newest fills).
    fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    /// Jump to bottom (oldest fills).
    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.analytics.recent_fills.len().saturating_sub(VISIBLE_FILLS);
    }

}
