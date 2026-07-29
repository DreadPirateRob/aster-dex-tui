// src/tui/news_app.rs
// Application state for the news feed TUI view.
//
// Manages scroll position, symbol input mode, and keyboard handling.
// Feed data is owned here; rendering is done by NewsFeedWidget.

use crate::data::news_feed::{NewsFeed, NewsArticle};
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Application state for the news feed TUI view.
pub struct NewsApp {
    /// Color theme
    pub theme: Theme,
    /// News article feed with dedup and bounded storage
    pub feed: NewsFeed,
    /// Optional symbol filter (uppercase, e.g., "BTC")
    pub symbol_filter: Option<String>,
    /// Optional category filter (from CLI, read-only at runtime)
    pub category_filter: Option<String>,
    /// Scroll offset: 0 = auto-scroll at top, >0 = user scrolled down
    pub scroll_offset: usize,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Some = symbol input mode active, holds character buffer
    pub symbol_input: Option<String>,
}

impl NewsApp {
    /// Create a new NewsApp with initial state.
    pub fn new(
        theme: Theme,
        feed: NewsFeed,
        symbol_filter: Option<String>,
        category_filter: Option<String>,
    ) -> Self {
        Self {
            theme,
            feed,
            symbol_filter,
            category_filter,
            scroll_offset: 0,
            should_quit: false,
            symbol_input: None,
        }
    }

    /// Handle keyboard input.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Symbol input mode: intercept all keys
        if let Some(ref mut buffer) = self.symbol_input {
            match key.code {
                KeyCode::Enter => {
                    let input = buffer.trim().to_uppercase();
                    self.symbol_input = None;
                    if input.is_empty() {
                        self.symbol_filter = None;
                    } else {
                        self.symbol_filter = Some(input);
                    }
                    self.scroll_offset = 0;
                    return;
                }
                KeyCode::Esc => {
                    self.symbol_input = None;
                    return;
                }
                KeyCode::Backspace => {
                    buffer.pop();
                    return;
                }
                KeyCode::Char(c) => {
                    buffer.push(c);
                    return;
                }
                _ => {
                    return;
                }
            }
        }

        // Handle Shift+G BEFORE modifiers==NONE guard
        // (crossterm reports SHIFT modifier for uppercase chars)
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            if let KeyCode::Char('G') = key.code {
                let visible = self.visible_articles();
                self.scroll_offset = visible.len().saturating_sub(1);
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
                let max = self.visible_articles().len().saturating_sub(1);
                if self.scroll_offset < max {
                    self.scroll_offset += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Char('g') | KeyCode::Home => {
                // Jump to top (re-enables auto-scroll)
                self.scroll_offset = 0;
            }
            KeyCode::End => {
                let visible = self.visible_articles();
                self.scroll_offset = visible.len().saturating_sub(1);
            }
            KeyCode::Char('/') => {
                // Enter symbol input mode
                self.symbol_input = Some(String::new());
            }
            KeyCode::Char('c') => {
                // Clear symbol filter
                self.symbol_filter = None;
                self.scroll_offset = 0;
            }
            _ => {}
        }
    }

    /// Return articles filtered by symbol_filter (if set).
    ///
    /// Collects into a Vec for indexed access during rendering.
    pub fn visible_articles(&self) -> Vec<&NewsArticle> {
        match &self.symbol_filter {
            Some(filter) => self
                .feed
                .articles()
                .iter()
                .filter(|a| NewsFeed::matches_symbol_filter(a, filter))
                .collect(),
            None => self.feed.articles().iter().collect(),
        }
    }

}
