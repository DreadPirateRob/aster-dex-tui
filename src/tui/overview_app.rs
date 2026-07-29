// src/tui/overview_app.rs
// Application state for the market overview TUI view.
//
// Manages sort column/direction, scroll position, cursor selection,
// symbol input mode, and keyboard handling. Table data is owned here;
// rendering is done by OverviewDashboardWidget.

use crate::data::ticker::{TickerEntry, TickerSortColumn, TickerTable};
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Application state for the market overview TUI view.
pub struct OverviewApp {
    /// Color theme
    pub theme: Theme,
    /// Ticker table with all entries
    pub table: TickerTable,
    /// Optional symbol filter (uppercase, e.g., "BTC")
    pub symbol_filter: Option<String>,
    /// Current sort column
    pub sort_column: TickerSortColumn,
    /// Sort direction: true = ascending, false = descending
    pub sort_ascending: bool,
    /// Scroll offset: 0 = top of table
    pub scroll_offset: usize,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Some = symbol input mode active, holds character buffer
    pub symbol_input: Option<String>,
    /// Cursor position for row selection
    pub selected_index: usize,
    /// Symbol selected via Enter (printed on quit)
    pub selected_symbol: Option<String>,
}

impl OverviewApp {
    /// Create a new OverviewApp with initial state.
    pub fn new(
        theme: Theme,
        table: TickerTable,
        symbol_filter: Option<String>,
    ) -> Self {
        Self {
            theme,
            table,
            symbol_filter,
            sort_column: TickerSortColumn::Volume,
            sort_ascending: false, // descending = highest volume first
            scroll_offset: 0,
            should_quit: false,
            symbol_input: None,
            selected_index: 0,
            selected_symbol: None,
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
                    self.selected_index = 0;
                    return;
                }
                KeyCode::Esc => {
                    self.symbol_input = None;
                    return;
                }
                KeyCode::Backspace => {
                    buffer.pop();
                    if buffer.is_empty() {
                        self.symbol_input = None;
                    }
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
                let visible_count = self.visible_entries().len();
                if visible_count > 0 {
                    self.selected_index = visible_count - 1;
                    // Ensure scroll follows cursor (scroll_offset adjusted below)
                    self.scroll_offset = visible_count.saturating_sub(1);
                }
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
                let visible_count = self.visible_entries().len();
                if visible_count > 0 && self.selected_index < visible_count - 1 {
                    self.selected_index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.selected_index = 0;
                self.scroll_offset = 0;
            }
            KeyCode::End => {
                let visible_count = self.visible_entries().len();
                if visible_count > 0 {
                    self.selected_index = visible_count - 1;
                    self.scroll_offset = visible_count.saturating_sub(1);
                }
            }
            KeyCode::Char('/') => {
                // Enter symbol input mode
                self.symbol_input = Some(String::new());
            }
            KeyCode::Char('c') => {
                // Clear symbol filter
                self.symbol_filter = None;
                self.scroll_offset = 0;
                self.selected_index = 0;
            }
            KeyCode::Tab => {
                // Cycle sort column
                self.sort_column = self.sort_column.cycle();
                self.scroll_offset = 0;
            }
            KeyCode::Char('r') => {
                // Reverse sort direction
                self.sort_ascending = !self.sort_ascending;
                self.scroll_offset = 0;
            }
            KeyCode::Char('s') => {
                self.sort_column = TickerSortColumn::Symbol;
                self.scroll_offset = 0;
            }
            KeyCode::Char('p') => {
                self.sort_column = TickerSortColumn::Price;
                self.scroll_offset = 0;
            }
            KeyCode::Char('d') | KeyCode::Char('%') => {
                self.sort_column = TickerSortColumn::Change;
                self.scroll_offset = 0;
            }
            KeyCode::Char('v') => {
                self.sort_column = TickerSortColumn::Volume;
                self.scroll_offset = 0;
            }
            KeyCode::Char('n') => {
                self.sort_column = TickerSortColumn::Trades;
                self.scroll_offset = 0;
            }
            KeyCode::Enter => {
                // Capture current selected symbol
                let visible = self.visible_entries();
                if let Some(entry) = visible.get(self.selected_index) {
                    self.selected_symbol = Some(entry.symbol.clone());
                    self.should_quit = true;
                }
            }
            _ => {}
        }
    }

    /// Return entries filtered by symbol_filter (if set) and sorted.
    ///
    /// Applies display-level filtering: data accumulation stays unfiltered,
    /// filtering happens at render time only.
    pub fn visible_entries(&self) -> Vec<&TickerEntry> {
        let sorted = self.table.sorted_entries(&self.sort_column, self.sort_ascending);

        match &self.symbol_filter {
            Some(filter) => sorted
                .into_iter()
                .filter(|e| e.symbol.to_uppercase().contains(&filter.to_uppercase()))
                .collect(),
            None => sorted,
        }
    }
}
