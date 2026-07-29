// src/tui/calendar_app.rs
// Application state for the economic calendar TUI view.
//
// Manages sort column/direction, scroll position, date range selection,
// high-impact filter, country filter with modal input mode, and all
// keyboard handling. Calendar data is owned here; rendering is done
// by CalendarWidget.

use crate::data::economic_calendar::{
    DateRange, EconomicCalendar, EconomicEvent, Impact, SortColumn,
};
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Application state for the economic calendar TUI view.
pub struct CalendarApp {
    /// Color theme
    pub theme: Theme,
    /// Economic calendar data container
    pub calendar: EconomicCalendar,
    /// Current sort column
    pub sort_column: SortColumn,
    /// Sort direction: true = ascending, false = descending
    pub sort_ascending: bool,
    /// Current date range for queries
    pub date_range: DateRange,
    /// Scroll offset: 0 = top of table
    pub scroll_offset: usize,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Filter to show only high-impact events
    pub high_only: bool,
    /// Optional country code filter (e.g., "US")
    pub country_filter: Option<String>,
    /// Some when typing country filter (modal input)
    pub input_mode: Option<String>,
    /// Flag for event loop to trigger re-fetch when date range changes
    pub date_range_changed: bool,
}

impl CalendarApp {
    /// Create a new CalendarApp with initial state.
    pub fn new(
        theme: Theme,
        calendar: EconomicCalendar,
        high_only: bool,
        country_filter: Option<String>,
    ) -> Self {
        Self {
            theme,
            calendar,
            sort_column: SortColumn::Time,
            sort_ascending: true,
            date_range: DateRange::ThisWeek,
            scroll_offset: 0,
            should_quit: false,
            high_only,
            country_filter,
            input_mode: None,
            date_range_changed: false,
        }
    }

    /// Handle keyboard input.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Input mode: intercept all keys for country filter typing
        if let Some(ref mut buffer) = self.input_mode {
            match key.code {
                KeyCode::Enter => {
                    let input = buffer.trim().to_uppercase();
                    self.input_mode = None;
                    if input.is_empty() {
                        self.country_filter = None;
                    } else {
                        self.country_filter = Some(input);
                    }
                    self.scroll_offset = 0;
                    return;
                }
                KeyCode::Esc => {
                    self.input_mode = None;
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
            match key.code {
                KeyCode::Char('G') => {
                    let visible = self.visible_events();
                    self.scroll_offset = visible.len().saturating_sub(1);
                    return;
                }
                KeyCode::Char('D') => {
                    self.date_range = self.date_range.prev();
                    self.date_range_changed = true;
                    self.scroll_offset = 0;
                    return;
                }
                _ => {}
            }
        }

        // Ctrl+C quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // Normal mode: only handle unmodified keys (plus SHIFT for G/D above)
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.visible_events().len().saturating_sub(1);
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
                let visible = self.visible_events();
                self.scroll_offset = visible.len().saturating_sub(1);
            }
            KeyCode::Tab | KeyCode::Char('s') => {
                // Cycle sort column
                self.sort_column = self.sort_column.cycle();
                self.scroll_offset = 0;
            }
            KeyCode::Char('r') => {
                // Toggle sort direction
                self.sort_ascending = !self.sort_ascending;
                self.scroll_offset = 0;
            }
            KeyCode::Char('d') => {
                // Cycle date range forward
                self.date_range = self.date_range.next();
                self.date_range_changed = true;
                self.scroll_offset = 0;
            }
            KeyCode::Char('h') => {
                // Toggle high-impact-only filter
                self.high_only = !self.high_only;
                self.scroll_offset = 0;
            }
            KeyCode::Char('/') => {
                // Enter input mode for country filter
                self.input_mode = Some(String::new());
            }
            KeyCode::Char('c') => {
                // Clear country filter
                self.country_filter = None;
                self.scroll_offset = 0;
            }
            _ => {}
        }
    }

    /// Return events filtered and sorted for display.
    ///
    /// Applies: sort by current column/direction, then high_only filter,
    /// then country_filter. Display-level filtering only.
    pub fn visible_events(&self) -> Vec<&EconomicEvent> {
        let sorted = self.calendar.sorted_events(self.sort_column, self.sort_ascending);

        sorted
            .into_iter()
            .filter(|e| {
                if self.high_only && e.impact != Impact::High {
                    return false;
                }
                if let Some(ref filter) = self.country_filter {
                    if !e.country.to_uppercase().contains(&filter.to_uppercase()) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }
}
