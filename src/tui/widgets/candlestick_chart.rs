// src/tui/widgets/candlestick_chart.rs
// Candlestick chart widget for OHLC visualization with Unicode block characters

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Widget},
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::config::CandleMode;
use crate::data::candle::{CandleStore, PartialCandle};
use crate::data::types::Candle;
use crate::tui::theme::Theme;

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

/// Heavy vertical -- full body cell
const BODY_CHAR: char = '\u{2503}'; // BOX DRAWINGS HEAVY VERTICAL

/// Half-height body characters for sub-cell precision
const HALF_BODY_BOTTOM: char = '\u{257B}'; // BOX DRAWINGS HEAVY DOWN
const HALF_BODY_TOP: char = '\u{2579}'; // BOX DRAWINGS HEAVY UP

/// Half-height wick characters for sub-cell precision
const UPPER_WICK: char = '\u{2577}'; // BOX DRAWINGS LIGHT DOWN
const LOWER_WICK: char = '\u{2575}'; // BOX DRAWINGS LIGHT UP

/// Combo characters: body + wick extending beyond body
const BODY_WICK_UP: char = '\u{257D}'; // BOX DRAWINGS LIGHT UP AND HEAVY DOWN
const BODY_WICK_DOWN: char = '\u{257F}'; // BOX DRAWINGS HEAVY UP AND LIGHT DOWN

/// Vertical line for wick rendering
const WICK_CHAR: char = '\u{2502}'; // BOX DRAWINGS LIGHT VERTICAL

/// Vertical separator between candle area and price axis
const SEPARATOR_CHAR: char = '\u{2502}'; // BOX DRAWINGS LIGHT VERTICAL

/// Horizontal divider between OHLC and volume sections
const HORIZONTAL_DIVIDER_CHAR: char = '\u{2500}'; // BOX DRAWINGS LIGHT HORIZONTAL

/// Dashed horizontal line for price line overlay
const PRICE_LINE_CHAR: char = '\u{254C}'; // BOX DRAWINGS LIGHT DOUBLE DASH HORIZONTAL

/// Width of each candle column (body + gap)
const CANDLE_WIDTH: u16 = 2;

/// Width reserved for price axis: 1 (padding) + 1 (separator) + 1 (padding) + 8 (labels)
const PRICE_AXIS_WIDTH: u16 = 11;

/// Height reserved for volume histogram (in rows)
const VOLUME_HEIGHT: u16 = 4;

/// Height reserved for CVD indicator line (in rows)
const CVD_HEIGHT: u16 = 1;

/// Minimum height for OHLC chart area (below this, skip volume/CVD)
const MIN_OHLC_HEIGHT: u16 = 10;

/// Get color for volume bar based on candle direction
fn volume_bar_color(open: Decimal, close: Decimal, theme: &Theme) -> Color {
    if close >= open {
        theme.bullish
    } else {
        theme.bearish
    }
}

/// Calculate cumulative volume delta across candles
fn calculate_cvd(candles: &[&Candle], partial: Option<&PartialCandle>) -> Decimal {
    let mut cvd = Decimal::ZERO;

    for candle in candles {
        cvd += candle.buy_volume - candle.sell_volume;
    }

    if let Some(partial) = partial {
        cvd += partial.buy_volume - partial.sell_volume;
    }

    cvd
}

/// Format CVD value for display with K/M suffixes
fn format_cvd(cvd: Decimal) -> String {
    let value = cvd.to_f64().unwrap_or(0.0);
    let prefix = if value >= 0.0 { "+" } else { "" };

    if value.abs() >= 1_000_000.0 {
        format!("CVD:{}{:.1}M", prefix, value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("CVD:{}{:.1}K", prefix, value / 1_000.0)
    } else {
        format!("CVD:{}{:.1}", prefix, value)
    }
}

/// Map a price to a cell row (inverted: high price = low Y)
/// Returns (cell_row, sub_cell_fraction)
fn price_to_y(price: f64, min_price: f64, max_price: f64, chart_height: u16) -> (u16, f64) {
    let range = max_price - min_price;
    if range <= 0.0 {
        return (chart_height / 2, 0.0);
    }

    // Normalize to 0.0-1.0, then invert for terminal coords
    let normalized = (price - min_price) / range;
    let y_float = (1.0 - normalized) * (chart_height.saturating_sub(1) as f64);
    let cell_row = (y_float as u16).min(chart_height.saturating_sub(1));
    let sub_cell = y_float.fract();

    (cell_row, sub_cell)
}

/// Map a price to a float Y position (no integer rounding).
/// Higher price = lower Y (inverted for terminal coords).
fn price_to_y_float(price: f64, min_price: f64, max_price: f64, chart_height: u16) -> f64 {
    let range = max_price - min_price;
    if range <= 0.0 {
        return chart_height as f64 / 2.0;
    }
    let normalized = (price - min_price) / range;
    ((1.0 - normalized) * (chart_height.saturating_sub(1) as f64))
        .clamp(0.0, (chart_height.saturating_sub(1)) as f64)
}

/// Determine character for a single cell row based on float OHLC Y positions.
///
/// Each cell row spans from `row` (top edge) to `row + 1` (bottom edge).
/// Uses fractional thresholds for half-cell decisions:
/// - fraction < 0.25: treat as at top of cell
/// - fraction > 0.75: treat as at bottom of cell
/// - 0.25..=0.75: use half-cell character
///
/// Returns None if this row has no candle content.
fn pick_cell_char(
    row: u16,
    high_y: f64,
    low_y: f64,
    body_top_y: f64,
    body_bot_y: f64,
) -> Option<char> {
    let row_top = row as f64;
    let row_bot = row_top + 1.0;

    // Row completely outside candle range
    if row_bot <= high_y || row_top >= low_y {
        return None;
    }

    // Determine overlap with body and wick regions
    let overlaps_body = row_bot > body_top_y && row_top < body_bot_y;
    let has_upper_wick = high_y < body_top_y;
    let has_lower_wick = low_y > body_bot_y;

    // Handle doji case: body is very thin (< 0.5 cells)
    let body_height = body_bot_y - body_top_y;
    if body_height < 0.5 {
        // For doji, render at least one character at the body position
        let body_row = body_top_y.floor() as u16;
        if row == body_row {
            let frac = body_top_y - row_top;
            if frac > 0.5 {
                // Body in bottom half of cell
                if has_upper_wick && high_y < row_top {
                    return Some(BODY_WICK_UP);
                }
                return Some(HALF_BODY_BOTTOM);
            } else {
                // Body in top half of cell
                if has_lower_wick && low_y > row_bot {
                    return Some(BODY_WICK_DOWN);
                }
                return Some(HALF_BODY_TOP);
            }
        }
        // For rows outside the body row in a doji, render as wick
        if row_bot > high_y && row_top < body_top_y {
            // Upper wick region
            return pick_wick_char(row, high_y, body_top_y, true);
        }
        if row_bot > body_bot_y && row_top < low_y {
            // Lower wick region
            return pick_wick_char(row, body_bot_y, low_y, false);
        }
        return None;
    }

    if overlaps_body {
        // This row overlaps with the body region
        let body_fills_top = body_top_y <= row_top;
        let body_fills_bot = body_bot_y >= row_bot;

        if body_fills_top && body_fills_bot {
            // Row fully inside body
            return Some(BODY_CHAR);
        }

        if !body_fills_top && body_fills_bot {
            // Body top edge falls in this row
            let frac = body_top_y - row_top;
            if has_upper_wick && frac > 0.5 {
                // Wick above, body starts in bottom portion -> combo
                return Some(BODY_WICK_UP);
            }
            if frac > 0.75 {
                return Some(HALF_BODY_BOTTOM);
            }
            // Body fills most of the cell
            return Some(BODY_CHAR);
        }

        if body_fills_top && !body_fills_bot {
            // Body bottom edge falls in this row
            let frac = body_bot_y - row_top;
            if has_lower_wick && frac < 0.5 {
                // Body ends in top portion, wick below -> combo
                return Some(BODY_WICK_DOWN);
            }
            if frac < 0.25 {
                return Some(HALF_BODY_TOP);
            }
            // Body fills most of the cell
            return Some(BODY_CHAR);
        }

        // Both edges in this row (body shorter than 1 cell but >= 0.5)
        // Already handled doji above, so body has some visible extent
        let frac_top = body_top_y - row_top;
        let frac_bot = body_bot_y - row_top;
        if frac_top > 0.5 {
            // Body in bottom portion
            if has_upper_wick {
                return Some(BODY_WICK_UP);
            }
            return Some(HALF_BODY_BOTTOM);
        }
        if frac_bot < 0.5 {
            // Body in top portion
            if has_lower_wick {
                return Some(BODY_WICK_DOWN);
            }
            return Some(HALF_BODY_TOP);
        }
        return Some(BODY_CHAR);
    }

    // Row does not overlap body -- it's in a wick region
    if row_top < body_top_y && row_bot > high_y {
        // Upper wick
        return pick_wick_char(row, high_y, body_top_y, true);
    }
    if row_top >= body_bot_y && row_top < low_y {
        // Lower wick
        return pick_wick_char(row, body_bot_y, low_y, false);
    }

    None
}

/// Helper for wick character selection in a single cell row.
/// `wick_start` / `wick_end` are the Y boundaries of the wick segment.
fn pick_wick_char(row: u16, wick_start: f64, wick_end: f64, _is_upper: bool) -> Option<char> {
    let row_top = row as f64;
    let row_bot = row_top + 1.0;

    // Row outside wick
    if row_bot <= wick_start || row_top >= wick_end {
        return None;
    }

    let at_start = wick_start > row_top; // wick starts partway into this row
    let at_end = wick_end < row_bot; // wick ends partway through this row

    if at_start && at_end {
        // Wick starts and ends within this single row
        let frac_start = wick_start - row_top;
        let frac_end = wick_end - row_top;
        let mid = (frac_start + frac_end) / 2.0;
        if mid > 0.5 {
            return Some(LOWER_WICK); // wick in bottom half
        }
        return Some(UPPER_WICK); // wick in top half
    }

    if at_start {
        // Wick starts in this row (tip)
        let frac = wick_start - row_top;
        if frac > 0.5 {
            return Some(LOWER_WICK); // wick only in bottom half
        }
        return Some(WICK_CHAR); // wick fills most of cell
    }

    if at_end {
        // Wick ends in this row (approaching body)
        let frac = wick_end - row_top;
        if frac < 0.5 {
            return Some(UPPER_WICK); // wick only in top half
        }
        return Some(WICK_CHAR); // wick fills most of cell
    }

    // Fully inside wick
    Some(WICK_CHAR)
}

/// Calculate a "nice" number for axis scaling.
/// Nice numbers are 1, 2, 5, 10, 20, 50, 100, etc.
fn nice_num(range: f64, round: bool) -> f64 {
    if range <= 0.0 {
        return 1.0;
    }

    let exponent = range.log10().floor();
    let fraction = range / 10_f64.powf(exponent);

    let nice_fraction = if round {
        // Round to nearest nice number
        if fraction < 1.5 {
            1.0
        } else if fraction < 3.0 {
            2.0
        } else if fraction < 7.0 {
            5.0
        } else {
            10.0
        }
    } else {
        // Ceiling to next nice number
        if fraction <= 1.0 {
            1.0
        } else if fraction <= 2.0 {
            2.0
        } else if fraction <= 5.0 {
            5.0
        } else {
            10.0
        }
    };

    nice_fraction * 10_f64.powf(exponent)
}

/// Calculate nice axis bounds and tick spacing.
/// Returns (nice_min, nice_max, tick_spacing)
fn nice_axis_range(min_val: f64, max_val: f64, max_ticks: usize) -> (f64, f64, f64) {
    if max_ticks < 2 {
        return (min_val, max_val, max_val - min_val);
    }

    let range = nice_num(max_val - min_val, false);
    let tick_spacing = nice_num(range / (max_ticks - 1) as f64, true);

    let nice_min = (min_val / tick_spacing).floor() * tick_spacing;
    let nice_max = (max_val / tick_spacing).ceil() * tick_spacing;

    (nice_min, nice_max, tick_spacing)
}

/// Format price for axis label display.
/// Uses fixed 2 decimal places to match header price display.
fn format_price_label(price: f64) -> String {
    format!("{:.2}", price)
}

/// Format ticks remaining for compact display.
/// Uses "t" suffix for small numbers, "kt" for thousands.
/// Examples: 42 -> "42t", 1000 -> "1.0kt", 9847 -> "9.8kt"
fn format_ticks_remaining(ticks: u32) -> String {
    if ticks >= 1000 {
        format!("{:.1}kt", ticks as f64 / 1000.0)
    } else {
        format!("{}t", ticks)
    }
}

/// Calculate min/max price range from visible candles and optional current price
/// Extends range with 5% padding if current price approaches boundaries
fn calculate_price_range_with_current(candles: &[&Candle], current_price: Option<f64>) -> (f64, f64) {
    if candles.is_empty() && current_price.is_none() {
        return (0.0, 100.0); // Sensible default
    }

    let mut min = f64::MAX;
    let mut max = f64::MIN;

    for candle in candles {
        let low = candle.low.to_f64().unwrap_or(0.0);
        let high = candle.high.to_f64().unwrap_or(0.0);
        min = min.min(low);
        max = max.max(high);
    }

    // Include current price in range calculation
    if let Some(price) = current_price {
        min = min.min(price);
        max = max.max(price);
    }

    // Handle edge case where we only have current price
    if min == f64::MAX {
        min = current_price.unwrap_or(0.0);
        max = current_price.unwrap_or(100.0);
    }

    // Add 2% padding for candles
    let range = max - min;
    let padding = if range > 0.0 { range * 0.02 } else { 1.0 };

    let (mut final_min, mut final_max) = (min - padding, max + padding);

    // If current price is near boundaries (within 5% of edge), extend range
    if let Some(price) = current_price {
        let total_range = final_max - final_min;
        let boundary_threshold = total_range * 0.05;

        if price - final_min < boundary_threshold {
            final_min = price - total_range * 0.10; // Add 10% below
        }
        if final_max - price < boundary_threshold {
            final_max = price + total_range * 0.10; // Add 10% above
        }
    }

    (final_min, final_max)
}

/// Braille dot bits for left column positions (top to bottom).
/// Braille char = 0x2800 | bits
const BRAILLE_LEFT_BITS: [u8; 4] = [0x01, 0x02, 0x04, 0x40];

/// Braille dot bits for right column positions (top to bottom).
const BRAILLE_RIGHT_BITS: [u8; 4] = [0x08, 0x10, 0x20, 0x80];

/// Pick a Braille character for one cell row of a candle.
/// Divides the cell into 4 vertical sub-rows and lights dots for body/wick regions.
/// For body regions, lights both left AND right column dots (thicker appearance).
/// For wick regions, lights only left column dot (thinner appearance).
/// Returns None if this row has no candle content.
fn pick_braille_char(
    row: u16,
    high_y: f64,
    low_y: f64,
    body_top_y: f64,
    body_bot_y: f64,
) -> Option<char> {
    let row_f = row as f64;

    // Check if row is within candle range at all
    if row_f + 1.0 <= high_y || row_f >= low_y {
        return None;
    }

    let mut bits: u8 = 0;

    for i in 0..4u8 {
        // Each sub-row spans 0.25 of the cell
        let sub_top = row_f + (i as f64) * 0.25;
        let sub_bot = sub_top + 0.25;
        let sub_mid = sub_top + 0.125;

        // Check if this sub-row is within the candle range
        if sub_bot <= high_y || sub_top >= low_y {
            continue; // Outside candle entirely
        }

        // Check if this sub-row is in the body region
        let in_body = sub_mid >= body_top_y && sub_mid < body_bot_y;

        if in_body {
            // Body: light both columns for thicker appearance
            bits |= BRAILLE_LEFT_BITS[i as usize];
            bits |= BRAILLE_RIGHT_BITS[i as usize];
        } else {
            // Wick: light only left column for thinner appearance
            bits |= BRAILLE_LEFT_BITS[i as usize];
        }
    }

    if bits == 0 {
        None
    } else {
        Some(char::from_u32(0x2800 + bits as u32).unwrap_or(' '))
    }
}

/// Candlestick chart widget for OHLC visualization.
pub struct CandlestickChart<'a> {
    /// Reference to candle store (borrows, doesn't own)
    candle_store: &'a CandleStore,
    /// Current partial candle being built (optional display)
    partial_candle: Option<PartialCandle>,
    /// Tick size to display on chart (only shown in tick-based mode)
    tick_size: u32,
    /// Candle mode (TickBased or TimeBased)
    mode: CandleMode,
    /// Block widget for border/title
    block: Option<Block<'a>>,
    /// Current last traded price for price line overlay
    current_price: Option<Decimal>,
    /// Color theme for styling
    theme: Option<&'a Theme>,
    /// Show connection lost overlay
    connection_lost: bool,
    /// Whether volume histogram is visible
    volume_visible: bool,
    /// Scroll offset for historical viewing (0 = live)
    view_offset: usize,
    /// Whether high-precision Braille rendering is active
    high_precision: bool,
}

impl<'a> CandlestickChart<'a> {
    pub fn new(candle_store: &'a CandleStore) -> Self {
        Self {
            candle_store,
            partial_candle: None,
            tick_size: 0,
            mode: CandleMode::TickBased,
            block: None,
            current_price: None,
            theme: None,
            connection_lost: false,
            volume_visible: true,
            view_offset: 0,
            high_precision: false,
        }
    }

    pub fn partial_candle(mut self, partial: Option<PartialCandle>) -> Self {
        self.partial_candle = partial;
        self
    }

    pub fn tick_size(mut self, size: u32) -> Self {
        self.tick_size = size;
        self
    }

    pub fn mode(mut self, mode: CandleMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn current_price(mut self, price: Option<Decimal>) -> Self {
        self.current_price = price;
        self
    }

    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Show connection lost overlay when WebSocket disconnected
    pub fn connection_lost(mut self, lost: bool) -> Self {
        self.connection_lost = lost;
        self
    }

    /// Set whether volume histogram is visible
    pub fn volume_visible(mut self, visible: bool) -> Self {
        self.volume_visible = visible;
        self
    }

    /// Set scroll offset for historical viewing
    pub fn view_offset(mut self, offset: usize) -> Self {
        self.view_offset = offset;
        self
    }

    /// Set high-precision Braille rendering mode
    pub fn high_precision(mut self, enabled: bool) -> Self {
        self.high_precision = enabled;
        self
    }

    /// Get the theme, defaulting to dark if not set.
    fn get_theme(&self) -> Theme {
        self.theme.copied().unwrap_or_else(Theme::dark)
    }
}

impl Widget for CandlestickChart<'_> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        // Take ownership of block to render it, get inner area first
        let chart_area = if let Some(block) = self.block.take() {
            let inner = block.inner(area);
            block.render(area, buf);
            inner
        } else {
            area
        };

        self.render_chart(chart_area, buf);
    }
}

impl CandlestickChart<'_> {
    fn render_chart(&self, area: Rect, buf: &mut Buffer) {
        if area.width < PRICE_AXIS_WIDTH + CANDLE_WIDTH || area.height < 3 {
            return; // Too small to render
        }

        // Get theme at start for consistent styling
        let theme = self.get_theme();

        // Calculate chart dimensions (exclude price axis)
        let chart_width = area.width.saturating_sub(PRICE_AXIS_WIDTH);
        let chart_height = area.height;

        // Calculate sub-areas for OHLC, volume, and CVD
        // Dynamically allocate space based on volume_visible flag
        let (ohlc_height, volume_area, cvd_area) = if self.volume_visible && chart_height >= MIN_OHLC_HEIGHT + VOLUME_HEIGHT + CVD_HEIGHT {
            // Volume visible: reserve space for both volume and CVD
            let ohlc_h = chart_height - VOLUME_HEIGHT - CVD_HEIGHT;
            let vol_area = Rect::new(
                area.left(),
                area.top() + ohlc_h,
                chart_width,
                VOLUME_HEIGHT,
            );
            let cvd_a = Rect::new(
                area.left(),
                area.top() + ohlc_h + VOLUME_HEIGHT,
                area.width, // CVD can use full width including price axis area
                CVD_HEIGHT,
            );
            (ohlc_h, Some(vol_area), Some(cvd_a))
        } else if !self.volume_visible && chart_height >= MIN_OHLC_HEIGHT + CVD_HEIGHT {
            // Volume hidden: only reserve space for CVD, OHLC expands to fill volume area
            let ohlc_h = chart_height - CVD_HEIGHT;
            let cvd_a = Rect::new(
                area.left(),
                area.top() + ohlc_h,
                area.width, // CVD can use full width including price axis area
                CVD_HEIGHT,
            );
            (ohlc_h, None, Some(cvd_a))
        } else {
            // Not enough space - show OHLC only
            (chart_height, None, None)
        };

        // Calculate how many candles fit
        let visible_candles = (chart_width / CANDLE_WIDTH) as usize;
        if visible_candles == 0 {
            return;
        }

        // Always reserve space for partial candle (show one fewer complete candle)
        // This ensures room is available even during the brief window between
        // candle completion and next trade arrival when partial_candle is None
        let candles_to_fetch = visible_candles.saturating_sub(1);

        // Calculate candle window with offset
        // view_offset=0 means live (show newest), view_offset=N means skip N newest
        let total_candles = self.candle_store.len();
        let end_index = total_candles.saturating_sub(self.view_offset);
        let start_index = end_index.saturating_sub(candles_to_fetch);

        // Collect candles from start_index to end_index
        let candles: Vec<&Candle> = self.candle_store
            .iter()
            .skip(start_index)
            .take(candles_to_fetch.min(end_index.saturating_sub(start_index)))
            .collect();
        if candles.is_empty() {
            // Render placeholder text when no candles
            self.render_empty_state(area, buf, &theme);
            return;
        }

        // Calculate price range for scaling (include current price for boundary extension)
        let current_price_f64 = self.current_price.and_then(|p| p.to_f64());
        let (min_price, max_price) = calculate_price_range_with_current(&candles, current_price_f64);

        // Render each candle (using ohlc_height for OHLC area)
        for (i, candle) in candles.iter().enumerate() {
            let x = area.left() + (i as u16) * CANDLE_WIDTH;
            self.render_candle(buf, area, x, candle, min_price, max_price, ohlc_height, &theme);
        }

        // Render partial candle only in live mode (offset = 0)
        if self.view_offset == 0 {
            if let Some(ref partial) = self.partial_candle {
                let x = area.left() + (candles.len() as u16) * CANDLE_WIDTH;
                if x + CANDLE_WIDTH <= area.left() + chart_width {
                    self.render_partial_candle(buf, area, x, partial, min_price, max_price, ohlc_height, &theme);
                }
            }
        }

        // Render tick size indicator in top-left (only in tick-based mode)
        if self.mode == CandleMode::TickBased && self.tick_size > 0 {
            let tick_text = format!("{}T", self.tick_size);
            let tick_style = Style::default().fg(theme.text_muted);
            for (i, ch) in tick_text.chars().enumerate() {
                if area.left() + (i as u16) < area.right() {
                    if let Some(cell) = buf.cell_mut((area.left() + (i as u16), area.top())) {
                        cell.set_char(ch).set_style(tick_style);
                    }
                }
            }
        }

        // Render historical mode indicator when not live
        if self.view_offset > 0 {
            let indicator_text = format!("[-{}]", self.view_offset);
            let indicator_style = Style::default().fg(Color::Yellow);
            // Position after tick size text or at start if no tick size
            let indicator_x = if self.mode == CandleMode::TickBased && self.tick_size > 0 {
                let tick_text = format!("{}T", self.tick_size);
                area.left() + tick_text.len() as u16 + 1 // +1 for spacing
            } else {
                area.left()
            };
            for (i, ch) in indicator_text.chars().enumerate() {
                let x = indicator_x + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, area.top())) {
                        cell.set_char(ch).set_style(indicator_style);
                    }
                }
            }
        }

        // Render HI-RES indicator when high precision mode is active
        if self.high_precision {
            let hires_text = "HI-RES";
            let hires_style = Style::default().fg(Color::Cyan);
            // Position: after tick size text and historical indicator
            let mut hires_x = area.left();

            // Skip past tick size text if present
            if self.mode == CandleMode::TickBased && self.tick_size > 0 {
                let tick_text = format!("{}T", self.tick_size);
                hires_x += tick_text.len() as u16 + 1;
            }
            // Skip past historical indicator if present
            if self.view_offset > 0 {
                let indicator_text = format!("[-{}]", self.view_offset);
                hires_x += indicator_text.len() as u16 + 1;
            }

            for (i, ch) in hires_text.chars().enumerate() {
                let x = hires_x + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, area.top())) {
                        cell.set_char(ch).set_style(hires_style);
                    }
                }
            }
        }

        // Render vertical separator between candle area and price axis (full height)
        self.render_separator(buf, area, &theme);

        // Render price axis on right side (uses ohlc_height)
        self.render_price_axis(buf, area, min_price, max_price, ohlc_height, &theme);

        // Render price line overlay (after axis so label is visible on top of ticks)
        self.render_price_line(buf, area, min_price, max_price, ohlc_height, chart_width, &theme);

        // Render volume histogram if we have space AND volume is visible
        if let Some(vol_area) = volume_area {
            if self.volume_visible {
                // Render horizontal divider line between OHLC and volume sections
                let divider_y = vol_area.top().saturating_sub(1);
                if divider_y >= area.top() {
                    self.render_horizontal_divider(buf, divider_y, area, &theme);
                }

                self.render_volume_histogram(buf, vol_area, &candles, self.partial_candle.as_ref(), &theme);
            }
        }

        // Render CVD indicator if we have space
        if let Some(cvd_a) = cvd_area {
            self.render_cvd_indicator(buf, cvd_a, &candles, self.partial_candle.as_ref(), &theme);
        }

        // Connection lost overlay (rendered last, on top of everything)
        if self.connection_lost {
            self.render_connection_overlay(buf, area);
        }
    }

    fn render_connection_overlay(&self, buf: &mut Buffer, area: Rect) {
        // Render centered "Connection Lost - Reconnecting..." message
        // Uses yellow on black for visibility over chart content
        let overlay_style = Style::default()
            .bg(Color::Black)
            .fg(Color::Yellow);

        let msg = "Connection Lost - Reconnecting...";
        let msg_width = msg.len() as u16;
        let center_x = area.x + (area.width.saturating_sub(msg_width)) / 2;
        let center_y = area.y + area.height / 2;

        // Render centered message
        if center_y < area.y + area.height && center_y >= area.y {
            for (i, ch) in msg.chars().enumerate() {
                let x = center_x.max(area.x) + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, center_y)) {
                        cell.set_char(ch).set_style(overlay_style);
                    }
                }
            }
        }
    }

    fn render_empty_state(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let msg = "Waiting for candles...";
        let x = area.left() + (area.width.saturating_sub(msg.len() as u16)) / 2;
        let y = area.top() + area.height / 2;
        let style = Style::default().fg(theme.text_muted);
        for (i, ch) in msg.chars().enumerate() {
            if x + (i as u16) < area.right() {
                if let Some(cell) = buf.cell_mut((x + (i as u16), y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }

    fn render_candle(
        &self,
        buf: &mut Buffer,
        area: Rect,
        x: u16,
        candle: &Candle,
        min_price: f64,
        max_price: f64,
        chart_height: u16,
        theme: &Theme,
    ) {
        let open = candle.open.to_f64().unwrap_or(0.0);
        let high = candle.high.to_f64().unwrap_or(0.0);
        let low = candle.low.to_f64().unwrap_or(0.0);
        let close = candle.close.to_f64().unwrap_or(0.0);

        let is_bullish = close >= open;
        let color = if is_bullish { theme.bullish } else { theme.bearish };
        let style = Style::default().fg(color);

        // Get FLOAT Y positions for sub-cell precision
        let high_y = price_to_y_float(high, min_price, max_price, chart_height);
        let low_y = price_to_y_float(low, min_price, max_price, chart_height);
        let open_y = price_to_y_float(open, min_price, max_price, chart_height);
        let close_y = price_to_y_float(close, min_price, max_price, chart_height);

        let body_top_y = open_y.min(close_y);
        let body_bot_y = open_y.max(close_y);

        // Iterate over cell rows that this candle spans
        let first_row = high_y.floor() as u16;
        let last_row = (low_y.ceil() as u16).min(chart_height.saturating_sub(1));

        for row in first_row..=last_row {
            let ch = if self.high_precision {
                pick_braille_char(row, high_y, low_y, body_top_y, body_bot_y)
            } else {
                pick_cell_char(row, high_y, low_y, body_top_y, body_bot_y)
            };
            if let Some(ch) = ch {
                let abs_y = area.top() + row;
                if abs_y < area.bottom() {
                    if let Some(cell) = buf.cell_mut((x, abs_y)) {
                        cell.set_char(ch).set_style(style);
                    }
                }
            }
        }
    }

    fn render_partial_candle(
        &self,
        buf: &mut Buffer,
        area: Rect,
        x: u16,
        partial: &PartialCandle,
        min_price: f64,
        max_price: f64,
        chart_height: u16,
        theme: &Theme,
    ) {
        // Same sub-cell rendering as render_candle but with muted style
        let open = partial.open.to_f64().unwrap_or(0.0);
        let high = partial.high.to_f64().unwrap_or(0.0);
        let low = partial.low.to_f64().unwrap_or(0.0);
        let close = partial.close.to_f64().unwrap_or(0.0);

        // Partial candles use same bullish/bearish coloring as completed candles
        let is_bullish = close >= open;
        let color = if is_bullish { theme.bullish } else { theme.bearish };
        let style = Style::default().fg(color);

        // Get FLOAT Y positions for sub-cell precision
        let high_y = price_to_y_float(high, min_price, max_price, chart_height);
        let low_y = price_to_y_float(low, min_price, max_price, chart_height);
        let open_y = price_to_y_float(open, min_price, max_price, chart_height);
        let close_y = price_to_y_float(close, min_price, max_price, chart_height);

        let body_top_y = open_y.min(close_y);
        let body_bot_y = open_y.max(close_y);

        // Iterate over cell rows that this candle spans
        let first_row = high_y.floor() as u16;
        let last_row = (low_y.ceil() as u16).min(chart_height.saturating_sub(1));

        for row in first_row..=last_row {
            let ch = if self.high_precision {
                pick_braille_char(row, high_y, low_y, body_top_y, body_bot_y)
            } else {
                pick_cell_char(row, high_y, low_y, body_top_y, body_bot_y)
            };
            if let Some(ch) = ch {
                let abs_y = area.top() + row;
                if abs_y < area.bottom() {
                    if let Some(cell) = buf.cell_mut((x, abs_y)) {
                        cell.set_char(ch).set_style(style);
                    }
                }
            }
        }
    }

    fn render_price_line(
        &self,
        buf: &mut Buffer,
        area: Rect,
        min_price: f64,
        max_price: f64,
        chart_height: u16,
        chart_width: u16,
        theme: &Theme,
    ) {
        // Skip if no current price
        let price = match self.current_price {
            Some(p) => match p.to_f64() {
                Some(f) => f,
                None => return,
            },
            None => return,
        };

        // Get Y coordinate for price
        let (y, _) = price_to_y(price, min_price, max_price, chart_height);
        let abs_y = area.top() + y;

        // Skip if outside visible area
        if abs_y < area.top() || abs_y >= area.bottom() {
            return;
        }

        let style = Style::default().fg(theme.price_line);

        // Draw dashed line only in gap columns (odd offsets) to preserve candle bodies
        // With CANDLE_WIDTH=2, candle body is at offset 0, gap at offset 1, body at 2, gap at 3, etc.
        for x in area.left()..(area.left() + chart_width) {
            let offset = x - area.left();
            // Skip even offsets (candle body positions)
            if offset % 2 == 0 {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, abs_y)) {
                cell.set_char(PRICE_LINE_CHAR).set_style(style);
            }
        }

        // Render price label and ticks counter in Y-axis area if terminal wide enough
        const MIN_WIDTH_FOR_COUNTER: u16 = 60;
        if area.width >= MIN_WIDTH_FOR_COUNTER {
            let counter_x = area.right().saturating_sub(PRICE_AXIS_WIDTH - 3);

            // Price label on price line row (abs_y)
            let price_text = format!("{:.2}", price);
            let price_style = Style::default().fg(theme.price_label_fg).bg(theme.price_label_bg);

            for (i, ch) in price_text.chars().enumerate() {
                let x = counter_x + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, abs_y)) {
                        cell.set_char(ch).set_style(price_style);
                    }
                }
            }

            // Ticks counter one row below price label (only in tick-based mode)
            if self.mode == CandleMode::TickBased {
                if let Some(ref partial) = self.partial_candle {
                    let ticks_remaining = partial.threshold.saturating_sub(partial.trade_count);
                    let counter_text = format_ticks_remaining(ticks_remaining);
                    let counter_style = Style::default().fg(theme.price_label_fg).bg(theme.price_label_bg);

                    let counter_y = abs_y + 1;
                    if counter_y < area.bottom() {
                        for (i, ch) in counter_text.chars().enumerate() {
                            let x = counter_x + i as u16;
                            if x < area.right() {
                                if let Some(cell) = buf.cell_mut((x, counter_y)) {
                                    cell.set_char(ch).set_style(counter_style);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn render_separator(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        // Separator position: 1 padding space before axis labels
        // Layout: [candles][space][separator][space][labels]
        let separator_x = area.right().saturating_sub(PRICE_AXIS_WIDTH - 1);

        if separator_x <= area.left() {
            return;
        }

        let style = Style::default().fg(theme.border);
        for y in area.top()..area.bottom() {
            if let Some(cell) = buf.cell_mut((separator_x, y)) {
                cell.set_char(SEPARATOR_CHAR).set_style(style);
            }
        }
    }

    fn render_horizontal_divider(&self, buf: &mut Buffer, y: u16, area: Rect, theme: &Theme) {
        // Render horizontal divider line across the full chart width (excluding price axis)
        let chart_width = area.width.saturating_sub(PRICE_AXIS_WIDTH);
        let style = Style::default().fg(theme.border);

        for x in area.left()..(area.left() + chart_width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(HORIZONTAL_DIVIDER_CHAR).set_style(style);
            }
        }
    }

    fn render_price_axis(
        &self,
        buf: &mut Buffer,
        area: Rect,
        min_price: f64,
        max_price: f64,
        chart_height: u16,
        theme: &Theme,
    ) {
        // Position price axis at right edge
        let axis_x = area.right().saturating_sub(PRICE_AXIS_WIDTH);
        if axis_x <= area.left() {
            return;
        }

        let style = Style::default().fg(theme.text_muted);

        // Calculate nice tick values
        let max_ticks = (chart_height / 3).max(2) as usize; // ~1 label per 3 rows
        let (nice_min, nice_max, tick_spacing) = nice_axis_range(min_price, max_price, max_ticks);

        // Generate tick values
        let mut tick = nice_min;
        while tick <= nice_max {
            // Map tick price to Y coordinate
            let (y, _) = price_to_y(tick, min_price, max_price, chart_height);
            let abs_y = area.top() + y;

            if abs_y >= area.top() && abs_y < area.bottom() {
                // Format price label
                let label = format_price_label(tick);

                // Render label (right-aligned within axis area, after separator)
                // Layout: [separator][space][labels], separator is at PRICE_AXIS_WIDTH - 1 from right
                let label_area_start = axis_x + 2; // Skip separator and padding
                let label_area_width = PRICE_AXIS_WIDTH.saturating_sub(2); // Remaining width for labels
                let label_start =
                    label_area_start + (label_area_width as usize).saturating_sub(label.len()) as u16;
                for (i, ch) in label.chars().enumerate() {
                    let x = label_start + i as u16;
                    if x < area.right() {
                        if let Some(cell) = buf.cell_mut((x, abs_y)) {
                            cell.set_char(ch).set_style(style);
                        }
                    }
                }
                // Note: Tick mark dash removed - separator line replaces it
            }

            tick += tick_spacing;
            // Safety: prevent infinite loop if tick_spacing is 0 or negative
            if tick_spacing <= 0.0 {
                break;
            }
        }
    }

    fn render_volume_histogram(
        &self,
        buf: &mut Buffer,
        area: Rect,
        candles: &[&Candle],
        partial: Option<&PartialCandle>,
        theme: &Theme,
    ) {
        if area.height == 0 || candles.is_empty() {
            return;
        }

        // Find max volume for scaling (include partial candle)
        let max_volume = candles
            .iter()
            .map(|c| c.volume.to_f64().unwrap_or(0.0))
            .chain(partial.map(|p| p.volume.to_f64().unwrap_or(0.0)))
            .fold(0.0_f64, f64::max);

        if max_volume <= 0.0 {
            return;
        }

        // Render each volume bar aligned with candle X position
        for (i, candle) in candles.iter().enumerate() {
            let x = area.left() + (i as u16) * CANDLE_WIDTH;
            if x >= area.left() + area.width {
                break; // Out of bounds
            }

            let volume = candle.volume.to_f64().unwrap_or(0.0);
            let color = volume_bar_color(candle.open, candle.close, theme);
            if self.high_precision {
                self.render_volume_bar_braille(buf, x, area, volume, max_volume, color);
            } else {
                self.render_volume_bar(buf, x, area, volume, max_volume, color);
            }
        }

        // Render partial candle volume with same bullish/bearish coloring
        if let Some(partial) = partial {
            let x = area.left() + (candles.len() as u16) * CANDLE_WIDTH;
            if x < area.left() + area.width {
                let volume = partial.volume.to_f64().unwrap_or(0.0);
                let color = volume_bar_color(partial.open, partial.close, theme);
                if self.high_precision {
                    self.render_volume_bar_braille(buf, x, area, volume, max_volume, color);
                } else {
                    self.render_volume_bar(buf, x, area, volume, max_volume, color);
                }
            }
        }
    }

    fn render_volume_bar(
        &self,
        buf: &mut Buffer,
        x: u16,
        area: Rect,
        volume: f64,
        max_volume: f64,
        color: Color,
    ) {
        if volume <= 0.0 || max_volume <= 0.0 {
            return;
        }

        // Calculate exact float bar height in cells (8 sub-cell levels per cell)
        let normalized = volume / max_volume;
        let bar_height_f = (normalized * area.height as f64).max(0.125); // At least 1/8 of a cell

        // Split into full cells and fractional top cell
        let full_cells = bar_height_f as u16;
        let fraction = bar_height_f.fract();

        // Map fraction to BLOCKS index (1-8). Round to nearest eighth.
        // 0.0 fraction means no partial cell needed (bar is exact cells).
        // Any non-zero fraction gets at least BLOCKS[1] (1/8 block).
        let top_block_idx = if fraction > 0.0 {
            ((fraction * 8.0).round() as usize).clamp(1, 8)
        } else if full_cells == 0 {
            1 // Minimum visible bar for any non-zero volume
        } else {
            0 // No partial cell needed
        };

        let style = Style::default().fg(color);

        // Draw full cells from bottom up
        for row in 0..full_cells.min(area.height) {
            let y = area.bottom().saturating_sub(1 + row);
            if y >= area.top() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(BLOCKS[8]).set_style(style);
                }
            }
        }

        // Draw fractional top cell
        if top_block_idx > 0 {
            let y = area.bottom().saturating_sub(1 + full_cells);
            if y >= area.top() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(BLOCKS[top_block_idx]).set_style(style);
                }
            }
        }
    }

    fn render_volume_bar_braille(
        &self,
        buf: &mut Buffer,
        x: u16,
        area: Rect,
        volume: f64,
        max_volume: f64,
        color: Color,
    ) {
        if volume <= 0.0 || max_volume <= 0.0 || area.height == 0 {
            return;
        }

        let style = Style::default().fg(color);

        // Total vertical dot positions: 4 per cell row
        let total_dots = area.height as usize * 4;
        let normalized = volume / max_volume;
        let fill_dots = ((normalized * total_dots as f64).round() as usize).max(1);

        // Fill dots from bottom up across cell rows
        for row_from_bottom in 0..area.height as usize {
            let y = area.bottom().saturating_sub(1 + row_from_bottom as u16);
            if y < area.top() {
                break;
            }

            let mut bits: u8 = 0;
            for dot in 0..4u8 {
                // dot 0 = bottom of cell, dot 3 = top of cell
                let global_dot = row_from_bottom * 4 + dot as usize;
                if global_dot < fill_dots {
                    // BRAILLE_LEFT_BITS[0] is top dot, [3] is bottom
                    // dot 0 (bottom) maps to index 3, dot 3 (top) maps to index 0
                    bits |= BRAILLE_LEFT_BITS[3 - dot as usize];
                }
            }

            if bits != 0 {
                let ch = char::from_u32(0x2800 | bits as u32).unwrap_or(' ');
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }

    fn render_cvd_indicator(
        &self,
        buf: &mut Buffer,
        area: Rect,
        candles: &[&Candle],
        partial: Option<&PartialCandle>,
        theme: &Theme,
    ) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        let cvd = calculate_cvd(candles, partial);
        let cvd_text = format_cvd(cvd);

        // Color based on CVD sign: bullish positive, bearish negative, neutral near zero
        let value = cvd.to_f64().unwrap_or(0.0);
        let color = if value > 10.0 {
            theme.bullish
        } else if value < -10.0 {
            theme.bearish
        } else {
            theme.neutral
        };

        let style = Style::default().fg(color);

        // Render CVD text at left of area
        let y = area.top();
        for (i, ch) in cvd_text.chars().enumerate() {
            let x = area.left() + i as u16;
            if x < area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_price_to_y_basic() {
        // Price at max should map to Y=0 (top)
        let (y, _) = price_to_y(100.0, 0.0, 100.0, 10);
        assert_eq!(y, 0);

        // Price at min should map to Y=height-1 (bottom)
        let (y, _) = price_to_y(0.0, 0.0, 100.0, 10);
        assert_eq!(y, 9);

        // Price at midpoint should map to middle
        let (y, _) = price_to_y(50.0, 0.0, 100.0, 10);
        assert!(y >= 4 && y <= 5);
    }

    #[test]
    fn test_price_to_y_zero_range() {
        // When all prices same, should map to middle
        let (y, _) = price_to_y(50.0, 50.0, 50.0, 10);
        assert_eq!(y, 5); // chart_height / 2
    }

    #[test]
    fn test_candlestick_chart_builder() {
        let store = CandleStore::new(100);
        let chart = CandlestickChart::new(&store)
            .tick_size(100)
            .partial_candle(None);

        assert_eq!(chart.tick_size, 100);
        assert!(chart.partial_candle.is_none());
    }

    #[test]
    fn test_nice_num() {
        // Should round to nice values (within order of magnitude)
        // Note: nice_num returns nice numbers like 1, 2, 5, 10, 20, 50, etc.
        assert_eq!(nice_num(10.0, true), 10.0);
        assert_eq!(nice_num(15.0, true), 20.0);
        assert_eq!(nice_num(40.0, true), 50.0);
        assert_eq!(nice_num(80.0, true), 100.0);
        // Edge cases
        assert_eq!(nice_num(0.0, true), 1.0); // Zero defaults to 1.0
        assert!(nice_num(100.0, true) >= 100.0); // Should be at least 100
    }

    #[test]
    fn test_nice_axis_range() {
        let (nice_min, nice_max, spacing) = nice_axis_range(3.0, 97.0, 5);
        // Should produce nice round numbers
        assert!(nice_min <= 3.0);
        assert!(nice_max >= 97.0);
        assert!(spacing > 0.0);
    }

    #[test]
    fn test_format_price_label() {
        // All prices use fixed 2 decimal places to match header display
        assert_eq!(format_price_label(50000.0), "50000.00");
        assert_eq!(format_price_label(100.5), "100.50");
        assert_eq!(format_price_label(1.234), "1.23");
        assert_eq!(format_price_label(0.0001), "0.00");
    }

    #[test]
    fn test_format_ticks_remaining() {
        assert_eq!(format_ticks_remaining(42), "42t");
        assert_eq!(format_ticks_remaining(999), "999t");
        assert_eq!(format_ticks_remaining(1000), "1.0kt");
        assert_eq!(format_ticks_remaining(9847), "9.8kt");
        assert_eq!(format_ticks_remaining(0), "0t");
    }

    #[test]
    fn test_candlestick_chart_current_price() {
        let store = CandleStore::new(100);
        let chart = CandlestickChart::new(&store)
            .current_price(Some(dec!(50000)));

        assert_eq!(chart.current_price, Some(dec!(50000)));
    }

    #[test]
    fn test_calculate_price_range_with_current_extends() {
        let candle = Candle {
            open: dec!(100),
            high: dec!(110),
            low: dec!(90),
            close: dec!(105),
            volume: dec!(1000),
            buy_volume: Decimal::ZERO,
            sell_volume: Decimal::ZERO,
            trade_count: 100,
            open_time: 0,
            close_time: 100,
        };
        let candles = vec![&candle];

        // Price within range - should not extend much
        let (min1, max1) = calculate_price_range_with_current(&candles, Some(100.0));
        assert!(min1 < 90.0);
        assert!(max1 > 110.0);

        // Price below range - should extend downward
        let (min2, _) = calculate_price_range_with_current(&candles, Some(85.0));
        assert!(min2 < 85.0);

        // Price above range - should extend upward
        let (_, max3) = calculate_price_range_with_current(&candles, Some(115.0));
        assert!(max3 > 115.0);
    }

    #[test]
    fn test_volume_bar_color() {
        let theme = Theme::dark();
        // Bullish candle (close >= open) -> theme.bullish
        assert_eq!(volume_bar_color(dec!(100), dec!(110), &theme), theme.bullish);
        // Bearish candle (close < open) -> theme.bearish
        assert_eq!(volume_bar_color(dec!(110), dec!(100), &theme), theme.bearish);
        // Doji (close == open) -> theme.bullish
        assert_eq!(volume_bar_color(dec!(100), dec!(100), &theme), theme.bullish);
    }

    #[test]
    fn test_calculate_cvd() {
        let candle1 = Candle {
            open: dec!(100),
            high: dec!(110),
            low: dec!(90),
            close: dec!(105),
            volume: dec!(100),
            buy_volume: dec!(60),
            sell_volume: dec!(40),
            trade_count: 10,
            open_time: 0,
            close_time: 100,
        };
        let candle2 = Candle {
            open: dec!(105),
            high: dec!(115),
            low: dec!(100),
            close: dec!(102),
            volume: dec!(80),
            buy_volume: dec!(30),
            sell_volume: dec!(50),
            trade_count: 10,
            open_time: 100,
            close_time: 200,
        };

        let candles = vec![&candle1, &candle2];
        let cvd = calculate_cvd(&candles, None);

        // CVD = (60-40) + (30-50) = 20 + (-20) = 0
        assert_eq!(cvd, dec!(0));
    }

    #[test]
    fn test_format_cvd() {
        assert_eq!(format_cvd(dec!(123.4)), "CVD:+123.4");
        assert_eq!(format_cvd(dec!(-456.7)), "CVD:-456.7");
        assert_eq!(format_cvd(dec!(1500)), "CVD:+1.5K");
        assert_eq!(format_cvd(dec!(-2500000)), "CVD:-2.5M");
        assert_eq!(format_cvd(dec!(0)), "CVD:+0.0");
    }

    #[test]
    fn test_candlestick_chart_volume_visible() {
        let store = CandleStore::new(100);
        let chart = CandlestickChart::new(&store)
            .volume_visible(false);

        assert!(!chart.volume_visible);
    }

    #[test]
    fn test_candlestick_chart_view_offset() {
        let store = CandleStore::new(100);
        let chart = CandlestickChart::new(&store)
            .view_offset(5);

        assert_eq!(chart.view_offset, 5);
    }

    // --- pick_cell_char tests ---

    #[test]
    fn test_pick_cell_char_full_body() {
        // Row 5 fully inside body (body from 3.0 to 8.0)
        let ch = pick_cell_char(5, 1.0, 10.0, 3.0, 8.0);
        assert_eq!(ch, Some(BODY_CHAR));
    }

    #[test]
    fn test_pick_cell_char_body_top_edge_with_wick() {
        // Body starts at 5.6 (bottom half of row 5), wick extends above
        // high_y=2.0, body_top=5.6, body_bot=9.0, low_y=10.0
        let ch = pick_cell_char(5, 2.0, 10.0, 5.6, 9.0);
        // frac = 5.6 - 5.0 = 0.6, wick above, frac > 0.5 -> BODY_WICK_UP
        assert_eq!(ch, Some(BODY_WICK_UP));
    }

    #[test]
    fn test_pick_cell_char_body_top_edge_half_body() {
        // Body starts at 5.8 (far bottom of row 5), no wick above
        // high_y=5.8, body_top=5.8, body_bot=9.0, low_y=10.0
        let ch = pick_cell_char(5, 5.8, 10.0, 5.8, 9.0);
        // No upper wick (high_y == body_top_y), frac=0.8 > 0.75 -> HALF_BODY_BOTTOM
        assert_eq!(ch, Some(HALF_BODY_BOTTOM));
    }

    #[test]
    fn test_pick_cell_char_body_bottom_edge_with_wick() {
        // Body ends at 7.3 (top portion of row 7), wick extends below
        // high_y=2.0, body_top=3.0, body_bot=7.3, low_y=10.0
        let ch = pick_cell_char(7, 2.0, 10.0, 3.0, 7.3);
        // frac = 7.3 - 7.0 = 0.3, wick below, frac < 0.5 -> BODY_WICK_DOWN
        assert_eq!(ch, Some(BODY_WICK_DOWN));
    }

    #[test]
    fn test_pick_cell_char_body_bottom_edge_half_body() {
        // Body ends at 7.1, no wick below (low_y==body_bot)
        // high_y=2.0, body_top=3.0, body_bot=7.1, low_y=7.1
        let ch = pick_cell_char(7, 2.0, 7.1, 3.0, 7.1);
        // frac = 7.1 - 7.0 = 0.1, frac < 0.25 -> HALF_BODY_TOP
        assert_eq!(ch, Some(HALF_BODY_TOP));
    }

    #[test]
    fn test_pick_cell_char_pure_wick() {
        // Row 2, fully inside upper wick (high=1.0, body_top=4.0)
        let ch = pick_cell_char(2, 1.0, 10.0, 4.0, 8.0);
        assert_eq!(ch, Some(WICK_CHAR));
    }

    #[test]
    fn test_pick_cell_char_wick_tip_upper() {
        // Wick tip at high_y=3.7 (bottom portion of row 3)
        // body_top=5.0, so row 3 is the wick tip row
        let ch = pick_cell_char(3, 3.7, 10.0, 5.0, 8.0);
        // at_start (wick_start=3.7 > row_top=3.0), frac=0.7, 0.25 < 0.7 < 0.75 -> LOWER_WICK
        assert_eq!(ch, Some(LOWER_WICK));
    }

    #[test]
    fn test_pick_cell_char_wick_tip_lower() {
        // Lower wick tip at low_y=9.3 (top portion of row 9)
        // body_bot=7.0, so row 9 is the lower wick tip row
        let ch = pick_cell_char(9, 2.0, 9.3, 3.0, 7.0);
        // at_end (wick_end=9.3 < row_bot=10.0), frac=0.3, 0.25 < 0.3 < 0.75 -> UPPER_WICK
        assert_eq!(ch, Some(UPPER_WICK));
    }

    #[test]
    fn test_pick_cell_char_doji() {
        // Doji: body_top == body_bot at 5.3
        let ch = pick_cell_char(5, 2.0, 9.0, 5.3, 5.3);
        // Body height < 0.5, body_row=5, frac=0.3 <= 0.5
        // has_lower_wick (low_y=9.0 > body_bot=5.3) -> BODY_WICK_DOWN
        assert_eq!(ch, Some(BODY_WICK_DOWN));
    }

    #[test]
    fn test_pick_cell_char_outside_range() {
        // Row 0, candle is at rows 3-8
        let ch = pick_cell_char(0, 3.0, 8.0, 4.0, 7.0);
        assert_eq!(ch, None);
    }

    #[test]
    fn test_pick_cell_char_outside_below() {
        // Row 10, candle is at rows 3-8
        let ch = pick_cell_char(10, 3.0, 8.0, 4.0, 7.0);
        assert_eq!(ch, None);
    }

    #[test]
    fn test_price_to_y_float_basic() {
        // Max price -> top (Y=0)
        let y = price_to_y_float(100.0, 0.0, 100.0, 10);
        assert!((y - 0.0).abs() < 0.001);

        // Min price -> bottom (Y=9)
        let y = price_to_y_float(0.0, 0.0, 100.0, 10);
        assert!((y - 9.0).abs() < 0.001);

        // Mid price -> middle (Y=4.5)
        let y = price_to_y_float(50.0, 0.0, 100.0, 10);
        assert!((y - 4.5).abs() < 0.001);
    }

    #[test]
    fn test_price_to_y_float_zero_range() {
        let y = price_to_y_float(50.0, 50.0, 50.0, 10);
        assert_eq!(y, 5.0); // chart_height / 2
    }

    #[test]
    fn test_volume_bar_fractional_height() {
        // Verify fractional block index calculation
        // 30% of 4 rows = 1.2 cells -> 1 full cell, fraction=0.2 -> BLOCKS[2] (2/8)
        // 50% of 2 rows = 1.0 cells -> exact, no fraction
        // 10% of 4 rows = 0.4 cells -> 0 full cells, fraction=0.4 -> BLOCKS[3] (3/8)
        let fraction = 0.2_f64;
        let idx = ((fraction * 8.0).round() as usize).clamp(1, 8);
        assert_eq!(idx, 2); // 2/8 block

        let fraction = 0.5_f64;
        let idx = ((fraction * 8.0).round() as usize).clamp(1, 8);
        assert_eq!(idx, 4); // 4/8 block (half)

        let fraction = 0.875_f64;
        let idx = ((fraction * 8.0).round() as usize).clamp(1, 8);
        assert_eq!(idx, 7); // 7/8 block
    }

    // --- pick_braille_char tests ---

    #[test]
    fn test_pick_braille_char_full_body_row() {
        // Row 5 fully inside body (body from 3.0 to 8.0, candle from 1.0 to 10.0)
        // All 4 sub-rows (5.0-5.25, 5.25-5.5, 5.5-5.75, 5.75-6.0) are in body
        // Body -> both left+right columns lit
        let ch = pick_braille_char(5, 1.0, 10.0, 3.0, 8.0);
        assert!(ch.is_some());
        let c = ch.unwrap();
        // All 4 sub-rows in body: left bits 0x01|0x02|0x04|0x40 = 0x47
        // Plus right bits 0x08|0x10|0x20|0x80 = 0xB8
        // Total = 0x47 | 0xB8 = 0xFF
        let expected = char::from_u32(0x2800 + 0xFF).unwrap();
        assert_eq!(c, expected);
    }

    #[test]
    fn test_pick_braille_char_wick_only_row() {
        // Row 2 in upper wick (high=1.0, body_top=4.0, body_bot=8.0, low=10.0)
        // All 4 sub-rows are wick -> only left column lit
        let ch = pick_braille_char(2, 1.0, 10.0, 4.0, 8.0);
        assert!(ch.is_some());
        let c = ch.unwrap();
        // All 4 sub-rows are wick: left bits only = 0x01|0x02|0x04|0x40 = 0x47
        let expected = char::from_u32(0x2800 + 0x47).unwrap();
        assert_eq!(c, expected);
    }

    #[test]
    fn test_pick_braille_char_outside_range() {
        // Row 0, candle is at rows 3-8
        let ch = pick_braille_char(0, 3.0, 8.0, 4.0, 7.0);
        assert_eq!(ch, None);
    }

    #[test]
    fn test_pick_braille_char_doji_at_least_one_dot() {
        // Doji: body_top == body_bot at 5.3
        // Candle from 2.0 to 9.0
        let ch = pick_braille_char(5, 2.0, 9.0, 5.3, 5.3);
        assert!(ch.is_some());
        // Sub-row midpoints: 5.125, 5.375, 5.625, 5.875
        // body check: sub_mid >= 5.3 && sub_mid < 5.3 => always false for zero-width body
        // All 4 sub-rows are wick (in range but not body)
        // So we get left column dots for all 4: 0x47
        let c = ch.unwrap();
        let expected = char::from_u32(0x2800 + 0x47).unwrap();
        assert_eq!(c, expected);
    }

    #[test]
    fn test_pick_braille_char_mixed_body_wick() {
        // Row 4: candle from 2.0 to 10.0, body from 4.5 to 8.0
        // Sub-rows at row 4:
        //   sub 0: 4.0-4.25, mid=4.125 -> not in body (4.125 < 4.5) -> wick (left only)
        //   sub 1: 4.25-4.5, mid=4.375 -> not in body (4.375 < 4.5) -> wick (left only)
        //   sub 2: 4.5-4.75, mid=4.625 -> in body (4.625 >= 4.5 && < 8.0) -> both columns
        //   sub 3: 4.75-5.0, mid=4.875 -> in body -> both columns
        let ch = pick_braille_char(4, 2.0, 10.0, 4.5, 8.0);
        assert!(ch.is_some());
        let c = ch.unwrap();
        // Sub 0: left 0x01
        // Sub 1: left 0x02
        // Sub 2: left 0x04 + right 0x20
        // Sub 3: left 0x40 + right 0x80
        let expected_bits: u8 = 0x01 | 0x02 | 0x04 | 0x20 | 0x40 | 0x80;
        let expected = char::from_u32(0x2800 + expected_bits as u32).unwrap();
        assert_eq!(c, expected);
    }

    #[test]
    fn test_pick_braille_char_high_precision_builder() {
        let store = CandleStore::new(100);
        let chart = CandlestickChart::new(&store)
            .high_precision(true);
        assert!(chart.high_precision);

        let chart2 = CandlestickChart::new(&store)
            .high_precision(false);
        assert!(!chart2.high_precision);
    }
}
