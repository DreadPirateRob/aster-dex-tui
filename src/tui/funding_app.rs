// src/tui/funding_app.rs
// Application state for the funding rate dashboard TUI view.
//
// Manages sort column/direction, scroll position, symbol input mode,
// and keyboard handling. Table data is owned here; rendering is done
// by FundingDashboardWidget.

use crate::data::funding_rates::{FundingRateEntry, FundingRateTable, SortColumn};
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Application state for the funding rate dashboard TUI view.
pub struct FundingApp {
    /// Color theme
    pub theme: Theme,
    /// Funding rate table with all entries
    pub table: FundingRateTable,
    /// Optional symbol filter (uppercase, e.g., "BTCUSDT")
    pub symbol_filter: Option<String>,
    /// Current sort column
    pub sort_column: SortColumn,
    /// Sort direction: true = ascending, false = descending
    pub sort_ascending: bool,
    /// Scroll offset: 0 = top of table
    pub scroll_offset: usize,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Some = symbol input mode active, holds character buffer
    pub symbol_input: Option<String>,
}

impl FundingApp {
    /// Create a new FundingApp with initial state.
    pub fn new(
        theme: Theme,
        table: FundingRateTable,
        symbol_filter: Option<String>,
    ) -> Self {
        Self {
            theme,
            table,
            symbol_filter,
            sort_column: SortColumn::FundingRate,
            sort_ascending: false, // descending = most extreme rates first
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
                    if c.is_alphanumeric() {
                        buffer.push(c);
                    }
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
                let visible = self.visible_entries();
                self.scroll_offset = visible.len().saturating_sub(1);
                return;
            }
        }

        // Ctrl+C quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // Normal mode: only handle unmodified keys (plus SHIFT for G above)
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.visible_entries().len().saturating_sub(1);
                if self.scroll_offset < max {
                    self.scroll_offset += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.scroll_offset = 0;
            }
            KeyCode::End => {
                let visible = self.visible_entries();
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
            KeyCode::Tab => {
                // Cycle sort column
                self.sort_column = self.sort_column.next();
                self.scroll_offset = 0;
            }
            KeyCode::Char('r') => {
                // Reverse sort direction
                self.sort_ascending = !self.sort_ascending;
                self.scroll_offset = 0;
            }
            KeyCode::Char('s') => {
                self.sort_column = SortColumn::Symbol;
                self.scroll_offset = 0;
            }
            KeyCode::Char('f') => {
                self.sort_column = SortColumn::FundingRate;
                self.scroll_offset = 0;
            }
            KeyCode::Char('a') => {
                self.sort_column = SortColumn::AnnualRate;
                self.scroll_offset = 0;
            }
            KeyCode::Char('p') => {
                self.sort_column = SortColumn::MarkPrice;
                self.scroll_offset = 0;
            }
            _ => {}
        }
    }

    /// Return entries filtered by symbol_filter (if set) and sorted.
    ///
    /// Applies display-level filtering: data accumulation stays unfiltered,
    /// filtering happens at render time only.
    pub fn visible_entries(&self) -> Vec<&FundingRateEntry> {
        let sorted = self.table.sorted_entries(self.sort_column, self.sort_ascending);

        match &self.symbol_filter {
            Some(filter) => sorted
                .into_iter()
                .filter(|e| e.symbol.to_uppercase().contains(&filter.to_uppercase()))
                .collect(),
            None => sorted,
        }
    }
}
