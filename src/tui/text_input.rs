// src/tui/text_input.rs
// Shared TextInput widget state for single-line text editing.
// Used by trade_app.rs (trade ticket form) and positions_app.rs (position management overlays).

use crossterm::event::{KeyCode, KeyEvent};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Hand-rolled text input widget state (per ratatui official user_input example).
///
/// Manages a string buffer with cursor position for single-line editing.
#[derive(Debug, Clone)]
pub struct TextInput {
    /// Current text content
    content: String,
    /// Byte-index cursor position within content
    cursor_position: usize,
}

impl TextInput {
    /// Create a new empty TextInput with cursor at position 0.
    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor_position: 0,
        }
    }

    /// Handle a key event for this text input.
    ///
    /// Supports: Char insert, Backspace, Delete, Left/Right cursor, Home/End jump.
    /// Returns true if the key was handled.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                self.content.insert(self.cursor_position, c);
                self.cursor_position += c.len_utf8();
                true
            }
            KeyCode::Backspace => {
                if self.cursor_position > 0 {
                    // Find the previous character boundary
                    let prev = self.content[..self.cursor_position]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.content.remove(prev);
                    self.cursor_position = prev;
                    true
                } else {
                    true // consumed but no-op
                }
            }
            KeyCode::Delete => {
                if self.cursor_position < self.content.len() {
                    self.content.remove(self.cursor_position);
                    true
                } else {
                    true // consumed but no-op
                }
            }
            KeyCode::Left => {
                if self.cursor_position > 0 {
                    let prev = self.content[..self.cursor_position]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.cursor_position = prev;
                }
                true
            }
            KeyCode::Right => {
                if self.cursor_position < self.content.len() {
                    let next = self.content[self.cursor_position..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.cursor_position + i)
                        .unwrap_or(self.content.len());
                    self.cursor_position = next;
                }
                true
            }
            KeyCode::Home => {
                self.cursor_position = 0;
                true
            }
            KeyCode::End => {
                self.cursor_position = self.content.len();
                true
            }
            _ => false,
        }
    }

    /// Parse content as a Decimal. Returns None if empty or invalid.
    pub fn as_decimal(&self) -> Option<Decimal> {
        if self.content.is_empty() {
            return None;
        }
        Decimal::from_str(&self.content).ok()
    }

    /// Clear content and reset cursor to 0.
    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor_position = 0;
    }

    /// Returns true if content is empty.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Borrow the content string.
    pub fn value(&self) -> &str {
        &self.content
    }

    /// Get current cursor position (byte index).
    pub fn cursor(&self) -> usize {
        self.cursor_position
    }

    /// Set content programmatically and move cursor to end.
    ///
    /// Used by quick-size to populate the quantity field.
    pub fn set_content(&mut self, s: String) {
        self.cursor_position = s.len();
        self.content = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    /// Helper: create a KeyEvent with no modifiers.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Helper: type a string into a TextInput via key events.
    fn type_str(input: &mut TextInput, s: &str) {
        for c in s.chars() {
            input.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn test_text_input_insert_and_backspace() {
        let mut input = TextInput::new();
        type_str(&mut input, "123.45");
        assert_eq!(input.value(), "123.45");
        assert_eq!(input.cursor(), 6);

        // Backspace removes last char
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.value(), "123.4");
        assert_eq!(input.cursor(), 5);
    }

    #[test]
    fn test_text_input_as_decimal_valid() {
        let mut input = TextInput::new();
        type_str(&mut input, "100.5");
        let d = input.as_decimal().unwrap();
        assert_eq!(d, Decimal::from_str("100.5").unwrap());
    }

    #[test]
    fn test_text_input_as_decimal_invalid() {
        let mut input = TextInput::new();
        type_str(&mut input, "abc");
        assert!(input.as_decimal().is_none());
    }

    #[test]
    fn test_text_input_as_decimal_empty() {
        let input = TextInput::new();
        assert!(input.as_decimal().is_none());
    }

    #[test]
    fn test_text_input_cursor_movement() {
        let mut input = TextInput::new();
        type_str(&mut input, "ABCD");
        assert_eq!(input.cursor(), 4);

        // Left
        input.handle_key(key(KeyCode::Left));
        assert_eq!(input.cursor(), 3);

        // Home
        input.handle_key(key(KeyCode::Home));
        assert_eq!(input.cursor(), 0);

        // Right
        input.handle_key(key(KeyCode::Right));
        assert_eq!(input.cursor(), 1);

        // End
        input.handle_key(key(KeyCode::End));
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn test_text_input_clear() {
        let mut input = TextInput::new();
        type_str(&mut input, "hello");
        assert!(!input.is_empty());
        input.clear();
        assert!(input.is_empty());
        assert_eq!(input.cursor(), 0);
        assert_eq!(input.value(), "");
    }

    #[test]
    fn test_text_input_delete() {
        let mut input = TextInput::new();
        type_str(&mut input, "ABC");
        input.handle_key(key(KeyCode::Home));
        input.handle_key(key(KeyCode::Delete));
        assert_eq!(input.value(), "BC");
    }

    #[test]
    fn test_text_input_set_content() {
        let mut input = TextInput::new();
        input.set_content("42.5".to_string());
        assert_eq!(input.value(), "42.5");
        assert_eq!(input.cursor(), 4); // cursor at end
    }
}
