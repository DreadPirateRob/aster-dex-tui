// src/tui/terminal.rs
// Terminal lifecycle management with automatic panic recovery

use ratatui::DefaultTerminal;
use std::io;

/// Wrapper around ratatui's DefaultTerminal providing RAII cleanup.
///
/// On creation: enters raw mode, alternate screen, installs panic hook.
/// On drop: leaves alternate screen, disables raw mode.
pub struct Tui {
    terminal: DefaultTerminal,
}

impl Tui {
    /// Initialize terminal in TUI mode.
    ///
    /// This enables:
    /// - Raw mode (keyboard input without line buffering)
    /// - Alternate screen (preserves shell scrollback)
    /// - Panic hook (restores terminal on panic)
    pub fn new() -> io::Result<Self> {
        let terminal = ratatui::init();
        Ok(Self { terminal })
    }

    /// Get mutable reference to underlying terminal for drawing.
    pub fn terminal(&mut self) -> &mut DefaultTerminal {
        &mut self.terminal
    }

    /// Explicitly restore terminal state.
    ///
    /// Called automatically on drop, but can be called early
    /// if you need to print to stdout before exit.
    pub fn restore(&mut self) {
        ratatui::restore();
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Restore terminal (idempotent, safe to call multiple times)
        ratatui::restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Run with: cargo test --package crypto-tui test_terminal_lifecycle -- --ignored
    fn test_terminal_lifecycle() {
        // This test is ignored by default because it actually
        // modifies terminal state. Run manually to verify.

        // Create Tui (enters raw mode, alternate screen)
        let mut tui = Tui::new().expect("Failed to initialize terminal");

        // Verify we can get terminal reference
        let _terminal = tui.terminal();

        // Explicit restore (before drop)
        tui.restore();

        // Drop will call restore again (should be idempotent)
    }
}
