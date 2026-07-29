// src/tui/theme.rs
// Color theme definitions for the TUI

use ratatui::style::Color;

/// Available color themes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ThemeName {
    /// Dark theme (default) - standard terminal colors
    #[default]
    Dark,
    /// High contrast theme - brighter colors for visibility
    HighContrast,
    /// Light theme - for light terminal backgrounds
    Light,
}

impl ThemeName {
    /// Convert theme name to actual theme colors.
    pub fn to_theme(&self) -> Theme {
        match self {
            ThemeName::Dark => Theme::dark(),
            ThemeName::HighContrast => Theme::high_contrast(),
            ThemeName::Light => Theme::light(),
        }
    }
}

/// Semantic color theme for the TUI.
///
/// Uses ANSI basic colors for maximum terminal compatibility.
/// All colors are named semantically (by purpose, not by color name)
/// to make theme creation and switching straightforward.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    // Trading colors
    /// Color for bullish/buy indicators (typically green)
    pub bullish: Color,
    /// Color for bearish/sell indicators (typically red)
    pub bearish: Color,
    /// Color for neutral/unchanged indicators
    pub neutral: Color,

    // UI chrome
    /// Primary text color (main content)
    pub text_primary: Color,
    /// Secondary text color (labels, headers)
    pub text_secondary: Color,
    /// Muted text color (timestamps, less important info)
    pub text_muted: Color,
    /// Border color for widgets
    pub border: Color,
    /// Background highlight color (selected rows, etc.)
    pub background_highlight: Color,

    // Status indicators
    /// Color for connected/healthy status
    pub status_connected: Color,
    /// Color for warning status
    pub status_warning: Color,
    /// Color for error/disconnected status
    pub status_error: Color,

    // Price line
    /// Color for the current price line on chart
    pub price_line: Color,
    /// Foreground color for price label
    pub price_label_fg: Color,
    /// Background color for price label
    pub price_label_bg: Color,
}

impl Theme {
    /// Dark theme - standard terminal colors.
    ///
    /// This is the default theme, using standard ANSI colors
    /// that work well on dark terminal backgrounds.
    pub fn dark() -> Self {
        Self {
            // Trading colors
            bullish: Color::Green,
            bearish: Color::Red,
            neutral: Color::Cyan,

            // UI chrome
            text_primary: Color::White,
            text_secondary: Color::Gray,
            text_muted: Color::DarkGray,
            border: Color::DarkGray,
            background_highlight: Color::DarkGray,

            // Status indicators
            status_connected: Color::Green,
            status_warning: Color::Yellow,
            status_error: Color::Red,

            // Price line
            price_line: Color::Cyan,
            price_label_fg: Color::Black,
            price_label_bg: Color::Cyan,
        }
    }

    /// High contrast theme - brighter colors for visibility.
    ///
    /// Uses lighter/brighter variants of colors for better
    /// visibility on dark backgrounds or for accessibility.
    pub fn high_contrast() -> Self {
        Self {
            // Trading colors - brighter variants
            bullish: Color::LightGreen,
            bearish: Color::LightRed,
            neutral: Color::LightCyan,

            // UI chrome - white borders, brighter text
            text_primary: Color::White,
            text_secondary: Color::White,
            text_muted: Color::Gray,
            border: Color::White,
            background_highlight: Color::DarkGray,

            // Status indicators - brighter
            status_connected: Color::LightGreen,
            status_warning: Color::LightYellow,
            status_error: Color::LightRed,

            // Price line - brighter
            price_line: Color::LightCyan,
            price_label_fg: Color::Black,
            price_label_bg: Color::LightCyan,
        }
    }

    /// Light theme - for light terminal backgrounds.
    ///
    /// Uses darker colors that are visible on light backgrounds.
    /// Trading colors remain green/red for universal recognition.
    pub fn light() -> Self {
        Self {
            // Trading colors - keep green/red, use blue for neutral
            bullish: Color::Green,
            bearish: Color::Red,
            neutral: Color::Blue,

            // UI chrome - dark text on light background
            text_primary: Color::Black,
            text_secondary: Color::DarkGray,
            text_muted: Color::Gray,
            border: Color::DarkGray,
            background_highlight: Color::Gray,

            // Status indicators
            status_connected: Color::Green,
            status_warning: Color::Yellow,
            status_error: Color::Red,

            // Price line - blue for light backgrounds
            price_line: Color::Blue,
            price_label_fg: Color::White,
            price_label_bg: Color::Blue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_name_default() {
        let name = ThemeName::default();
        assert_eq!(name, ThemeName::Dark);
    }

    #[test]
    fn test_theme_name_to_theme() {
        // Dark theme
        let dark = ThemeName::Dark.to_theme();
        assert_eq!(dark.bullish, Color::Green);
        assert_eq!(dark.text_primary, Color::White);

        // High contrast theme
        let hc = ThemeName::HighContrast.to_theme();
        assert_eq!(hc.bullish, Color::LightGreen);
        assert_eq!(hc.border, Color::White);

        // Light theme
        let light = ThemeName::Light.to_theme();
        assert_eq!(light.text_primary, Color::Black);
        assert_eq!(light.neutral, Color::Blue);
    }

    #[test]
    fn test_dark_theme_trading_colors() {
        let theme = Theme::dark();
        assert_eq!(theme.bullish, Color::Green);
        assert_eq!(theme.bearish, Color::Red);
        assert_eq!(theme.neutral, Color::Cyan);
    }

    #[test]
    fn test_high_contrast_brighter_colors() {
        let theme = Theme::high_contrast();
        // Should use Light* variants
        assert_eq!(theme.bullish, Color::LightGreen);
        assert_eq!(theme.bearish, Color::LightRed);
        assert_eq!(theme.neutral, Color::LightCyan);
        assert_eq!(theme.border, Color::White);
    }

    #[test]
    fn test_light_theme_dark_text() {
        let theme = Theme::light();
        // Text should be dark for light backgrounds
        assert_eq!(theme.text_primary, Color::Black);
        assert_eq!(theme.text_secondary, Color::DarkGray);
        // Neutral should be blue (cyan is hard to read on light)
        assert_eq!(theme.neutral, Color::Blue);
    }
}
