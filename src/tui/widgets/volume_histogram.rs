// src/tui/widgets/volume_histogram.rs
// Standalone volume histogram widget showing buy/sell percentage bars

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Widget},
};
use rust_decimal::prelude::ToPrimitive;

use crate::data::candle::{CandleStore, PartialCandle};
use crate::data::types::Candle;
use crate::tui::theme::Theme;

/// Candle column width in cells (matches CandlestickChart)
const CANDLE_WIDTH: u16 = 2;

/// Unicode block characters for 0/8 through 8/8 fill levels
const BLOCKS: [char; 9] = [
    ' ',        // 0/8
    '\u{2581}', // 1/8 LOWER ONE EIGHTH BLOCK
    '\u{2582}', // 2/8 LOWER ONE QUARTER BLOCK
    '\u{2583}', // 3/8 LOWER THREE EIGHTHS BLOCK
    '\u{2584}', // 4/8 LOWER HALF BLOCK
    '\u{2585}', // 5/8 LOWER FIVE EIGHTHS BLOCK
    '\u{2586}', // 6/8 LOWER THREE QUARTERS BLOCK
    '\u{2587}', // 7/8 LOWER SEVEN EIGHTHS BLOCK
    '\u{2588}', // 8/8 FULL BLOCK
];

/// Standalone volume histogram widget that renders buy/sell percentage bars.
///
/// Positioned between the prod info bar and the candlestick chart in the layout.
/// Each bar column shows the buy/sell split as colored percentage segments rather
/// than raw notional volume values.
pub struct VolumeHistogram<'a> {
    candle_store: &'a CandleStore,
    partial_candle: Option<PartialCandle>,
    view_offset: usize,
    theme: Option<&'a Theme>,
    block: Option<Block<'a>>,
}

impl<'a> VolumeHistogram<'a> {
    pub fn new(candle_store: &'a CandleStore) -> Self {
        Self {
            candle_store,
            partial_candle: None,
            view_offset: 0,
            theme: None,
            block: None,
        }
    }

    pub fn partial_candle(mut self, partial: Option<PartialCandle>) -> Self {
        self.partial_candle = partial;
        self
    }

    pub fn view_offset(mut self, offset: usize) -> Self {
        self.view_offset = offset;
        self
    }

    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Calculate buy percentage for a candle's volume
    fn buy_pct(buy_volume: f64, total_volume: f64) -> f64 {
        if total_volume <= 0.0 {
            0.5 // Default to 50/50 when no volume data
        } else {
            (buy_volume / total_volume).clamp(0.0, 1.0)
        }
    }
}

impl<'a> Widget for VolumeHistogram<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Handle optional block border
        let inner_area = if let Some(block) = &self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        if inner_area.height == 0 || inner_area.width == 0 {
            return;
        }

        let default_theme = Theme::dark();
        let theme = self.theme.unwrap_or(&default_theme);

        // Calculate visible candles from available width
        let visible_candles = (inner_area.width / CANDLE_WIDTH) as usize;
        if visible_candles == 0 {
            return;
        }

        // Reserve space for partial candle (same logic as CandlestickChart)
        let candles_to_fetch = visible_candles.saturating_sub(1);

        // Calculate candle window with offset (same as CandlestickChart lines 676-687)
        let total_candles = self.candle_store.len();
        let end_index = total_candles.saturating_sub(self.view_offset);
        let start_index = end_index.saturating_sub(candles_to_fetch);

        let candles: Vec<&Candle> = self.candle_store
            .iter()
            .skip(start_index)
            .take(candles_to_fetch.min(end_index.saturating_sub(start_index)))
            .collect();

        if candles.is_empty() {
            return;
        }

        // Determine partial candle data if live (view_offset == 0)
        let partial = if self.view_offset == 0 {
            self.partial_candle.as_ref()
        } else {
            None
        };

        if inner_area.height == 1 {
            // Single-row mode: one character per candle showing buy percentage
            self.render_single_row(&candles, partial, theme, inner_area, buf);
        } else {
            // Multi-row mode: label row + bar rows
            self.render_multi_row(&candles, partial, theme, inner_area, buf);
        }
    }
}

impl<'a> VolumeHistogram<'a> {
    fn render_single_row(
        &self,
        candles: &[&Candle],
        partial: Option<&PartialCandle>,
        theme: &Theme,
        area: Rect,
        buf: &mut Buffer,
    ) {
        for (i, candle) in candles.iter().enumerate() {
            let x = area.left() + (i as u16) * CANDLE_WIDTH;
            if x >= area.right() {
                break;
            }

            let buy_vol = candle.buy_volume.to_f64().unwrap_or(0.0);
            let total_vol = candle.volume.to_f64().unwrap_or(0.0);
            let pct = Self::buy_pct(buy_vol, total_vol);

            // Map percentage to block character (full block = 100% buy)
            let block_idx = (pct * 8.0).round() as usize;
            let block_idx = block_idx.clamp(0, 8);
            let ch = BLOCKS[block_idx];

            let color = if pct >= 0.5 { theme.bullish } else { theme.bearish };
            let style = Style::default().fg(color);

            if let Some(cell) = buf.cell_mut((x, area.top())) {
                cell.set_char(ch).set_style(style);
            }
        }

        // Partial candle
        if let Some(partial) = partial {
            let x = area.left() + (candles.len() as u16) * CANDLE_WIDTH;
            if x < area.right() {
                let buy_vol = partial.buy_volume.to_f64().unwrap_or(0.0);
                let total_vol = partial.volume.to_f64().unwrap_or(0.0);
                let pct = Self::buy_pct(buy_vol, total_vol);

                let block_idx = (pct * 8.0).round() as usize;
                let block_idx = block_idx.clamp(0, 8);
                let ch = BLOCKS[block_idx];

                let color = if pct >= 0.5 { theme.bullish } else { theme.bearish };
                let style = Style::default().fg(color);

                if let Some(cell) = buf.cell_mut((x, area.top())) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }

    fn render_multi_row(
        &self,
        candles: &[&Candle],
        partial: Option<&PartialCandle>,
        theme: &Theme,
        area: Rect,
        buf: &mut Buffer,
    ) {
        // Row 0 = label row, remaining rows = bar area
        let label_y = area.top();
        let bar_area = Rect::new(
            area.left(),
            area.top() + 1,
            area.width,
            area.height.saturating_sub(1),
        );

        // Render label row: "Vol" left-aligned, "B:XX% S:YY%" right-aligned
        self.render_label_row(candles, partial, theme, label_y, area, buf);

        if bar_area.height == 0 {
            return;
        }

        // Render percentage bars for each candle
        for (i, candle) in candles.iter().enumerate() {
            let x = area.left() + (i as u16) * CANDLE_WIDTH;
            if x >= area.right() {
                break;
            }

            let buy_vol = candle.buy_volume.to_f64().unwrap_or(0.0);
            let total_vol = candle.volume.to_f64().unwrap_or(0.0);
            let buy_pct = Self::buy_pct(buy_vol, total_vol);

            self.render_pct_bar(buf, x, bar_area, buy_pct, theme);
        }

        // Partial candle bar
        if let Some(partial) = partial {
            let x = area.left() + (candles.len() as u16) * CANDLE_WIDTH;
            if x < area.right() {
                let buy_vol = partial.buy_volume.to_f64().unwrap_or(0.0);
                let total_vol = partial.volume.to_f64().unwrap_or(0.0);
                let buy_pct = Self::buy_pct(buy_vol, total_vol);

                self.render_pct_bar(buf, x, bar_area, buy_pct, theme);
            }
        }
    }

    fn render_label_row(
        &self,
        candles: &[&Candle],
        partial: Option<&PartialCandle>,
        theme: &Theme,
        y: u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        // Get the most recent candle's buy/sell split for the summary
        let (buy_pct_display, sell_pct_display) = if let Some(partial) = partial {
            let buy_vol = partial.buy_volume.to_f64().unwrap_or(0.0);
            let total_vol = partial.volume.to_f64().unwrap_or(0.0);
            let pct = Self::buy_pct(buy_vol, total_vol);
            ((pct * 100.0) as u32, ((1.0 - pct) * 100.0) as u32)
        } else if let Some(last) = candles.last() {
            let buy_vol = last.buy_volume.to_f64().unwrap_or(0.0);
            let total_vol = last.volume.to_f64().unwrap_or(0.0);
            let pct = Self::buy_pct(buy_vol, total_vol);
            ((pct * 100.0) as u32, ((1.0 - pct) * 100.0) as u32)
        } else {
            (50, 50)
        };

        // "Vol" label on the left
        let label = "Vol";
        let dim_style = Style::default().fg(Color::DarkGray);
        for (i, ch) in label.chars().enumerate() {
            let x = area.left() + i as u16;
            if x < area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(dim_style);
                }
            }
        }

        // "B:XX% S:YY%" summary on the right
        let buy_label = format!("B:{}%", buy_pct_display);
        let sell_label = format!(" S:{}%", sell_pct_display);
        let summary_len = buy_label.len() + sell_label.len();

        if area.width as usize > summary_len + 4 {
            let start_x = area.right().saturating_sub(summary_len as u16);

            // Buy percentage in bullish color
            let buy_style = Style::default().fg(theme.bullish);
            for (i, ch) in buy_label.chars().enumerate() {
                let x = start_x + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch).set_style(buy_style);
                    }
                }
            }

            // Sell percentage in bearish color
            let sell_style = Style::default().fg(theme.bearish);
            for (i, ch) in sell_label.chars().enumerate() {
                let x = start_x + buy_label.len() as u16 + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch).set_style(sell_style);
                    }
                }
            }
        }
    }

    /// Render a percentage bar for a single candle column.
    ///
    /// The bar fills the full height with buy (bottom, bullish) and sell (top, bearish).
    /// Uses sub-cell block characters for the transition cell.
    fn render_pct_bar(
        &self,
        buf: &mut Buffer,
        x: u16,
        area: Rect,
        buy_pct: f64,
        theme: &Theme,
    ) {
        if area.height == 0 {
            return;
        }

        let total_height = area.height as f64;

        // Buy portion fills from bottom, sell from top
        let buy_height_f = buy_pct * total_height;
        let buy_full_cells = buy_height_f as u16;
        let buy_fraction = buy_height_f.fract();

        let bullish_style = Style::default().fg(theme.bullish);
        let bearish_style = Style::default().fg(theme.bearish);

        // Draw sell (bearish) cells from top down (full block)
        let sell_height_f = (1.0 - buy_pct) * total_height;
        let sell_full_cells = sell_height_f as u16;

        for row in 0..sell_full_cells.min(area.height) {
            let y = area.top() + row;
            if y < area.bottom() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(BLOCKS[8]).set_style(bearish_style);
                }
            }
        }

        // Draw buy (bullish) cells from bottom up (full block)
        for row in 0..buy_full_cells.min(area.height) {
            let y = area.bottom().saturating_sub(1 + row);
            if y >= area.top() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(BLOCKS[8]).set_style(bullish_style);
                }
            }
        }

        // Transition cell: the cell between full sell and full buy cells
        // Uses a partial block character. The lower portion is bullish (buy),
        // upper portion is the cell background or bearish.
        let transition_row = area.bottom().saturating_sub(1 + buy_full_cells);
        if transition_row >= area.top() && transition_row < area.bottom() {
            // Only render transition if we haven't already filled this cell
            let already_filled = (sell_full_cells + buy_full_cells) >= area.height;
            if !already_filled {
                let block_idx = if buy_fraction > 0.0 {
                    ((buy_fraction * 8.0).round() as usize).clamp(1, 8)
                } else {
                    0
                };

                if block_idx > 0 {
                    // Lower block chars naturally fill from bottom — perfect for buy portion
                    if let Some(cell) = buf.cell_mut((x, transition_row)) {
                        cell.set_char(BLOCKS[block_idx]).set_style(bullish_style);
                    }
                }
            }
        }
    }
}
