// src/tui/widgets/dom_ladder.rs
// DOM (Depth of Market) price ladder widget with dynamic multi-column rendering.
// Displays bid/ask quantities, tick-aligned prices, optional volume profile bars,
// optional cumulative depth columns, optional heatmap background, and PnL column.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Widget},
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::data::heatmap_tracker::{HeatmapTracker, SignificanceTier};
use crate::data::order::OrderSide;
use crate::data::order_book::OrderBook;
use crate::data::trade_volume_profile::TradeVolumeProfile;
use crate::tui::dom_app::{VolumeProfileMode, WorkingOrder};
use crate::tui::theme::Theme;
use std::collections::HashMap;

/// Left-side block elements for sub-cell horizontal bar precision.
/// Index 0 = empty, 1 = 1/8th, ..., 8 = full block.
const H_BLOCKS: [char; 9] = [
    ' ',
    '\u{258F}', // LEFT ONE EIGHTH BLOCK
    '\u{258E}', // LEFT ONE QUARTER BLOCK
    '\u{258D}', // LEFT THREE EIGHTHS BLOCK
    '\u{258C}', // LEFT HALF BLOCK
    '\u{258B}', // LEFT FIVE EIGHTHS BLOCK
    '\u{258A}', // LEFT THREE QUARTERS BLOCK
    '\u{2589}', // LEFT SEVEN EIGHTHS BLOCK
    '\u{2588}', // FULL BLOCK
];

/// DOM price ladder widget with dynamic column layout.
///
/// Column layout (left to right):
/// [cum_bid] [vol_bid] [bid_qty] [price] [ask_qty] [vol_ask] [cum_ask] [pnl]
///
/// cum_bid/cum_ask: only when show_cumulative_depth is true
/// vol_bid/vol_ask: only when show_volume_profile is true
/// pnl: only when a position is open
///
/// Follows the CandlestickChart pattern: struct with builder-style methods,
/// implements Widget trait, direct `buf.cell_mut()` buffer writes.
pub struct DomLadder<'a> {
    order_book: Option<&'a OrderBook>,
    tick_size: Decimal,
    center_price: Option<Decimal>,
    best_bid: Option<Decimal>,
    best_ask: Option<Decimal>,
    mid_price: Option<Decimal>,
    theme: Option<&'a Theme>,
    block: Option<Block<'a>>,
    cursor_offset: Option<i16>,
    sl_preview_price: Option<Decimal>,
    tp_preview_price: Option<Decimal>,
    working_orders: Option<&'a HashMap<Decimal, Vec<WorkingOrder>>>,
    position_entry_price: Option<Decimal>,
    position_qty: Option<Decimal>, // signed: positive=long, negative=short
    show_heatmap: bool,
    heatmap_tracker: Option<&'a HeatmapTracker>,
    show_volume_profile: bool,
    show_cumulative_depth: bool,
    volume_profile_mode: Option<VolumeProfileMode>,
    trade_volume_profile: Option<&'a TradeVolumeProfile>,
}

impl<'a> Default for DomLadder<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> DomLadder<'a> {
    pub fn new() -> Self {
        Self {
            order_book: None,
            tick_size: Decimal::new(1, 2), // default 0.01
            center_price: None,
            best_bid: None,
            best_ask: None,
            mid_price: None,
            theme: None,
            block: None,
            cursor_offset: None,
            sl_preview_price: None,
            tp_preview_price: None,
            working_orders: None,
            position_entry_price: None,
            position_qty: None,
            show_heatmap: false,
            heatmap_tracker: None,
            show_volume_profile: false,
            show_cumulative_depth: false,
            volume_profile_mode: None,
            trade_volume_profile: None,
        }
    }

    pub fn order_book(mut self, order_book: &'a OrderBook) -> Self {
        self.order_book = Some(order_book);
        self
    }

    pub fn tick_size(mut self, tick_size: Decimal) -> Self {
        self.tick_size = tick_size;
        self
    }

    pub fn center_price(mut self, center_price: Option<Decimal>) -> Self {
        self.center_price = center_price;
        self
    }

    pub fn best_bid(mut self, best_bid: Option<Decimal>) -> Self {
        self.best_bid = best_bid;
        self
    }

    pub fn best_ask(mut self, best_ask: Option<Decimal>) -> Self {
        self.best_ask = best_ask;
        self
    }

    pub fn mid_price(mut self, mid_price: Option<Decimal>) -> Self {
        self.mid_price = mid_price;
        self
    }

    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn cursor_offset(mut self, offset: i16) -> Self {
        self.cursor_offset = Some(offset);
        self
    }

    pub fn sl_preview_price(mut self, price: Option<Decimal>) -> Self {
        self.sl_preview_price = price;
        self
    }

    pub fn tp_preview_price(mut self, price: Option<Decimal>) -> Self {
        self.tp_preview_price = price;
        self
    }

    pub fn working_orders(mut self, orders: &'a HashMap<Decimal, Vec<WorkingOrder>>) -> Self {
        self.working_orders = Some(orders);
        self
    }

    pub fn position_entry_price(mut self, price: Option<Decimal>) -> Self {
        self.position_entry_price = price;
        self
    }

    pub fn position_qty(mut self, qty: Decimal) -> Self {
        self.position_qty = Some(qty);
        self
    }

    pub fn show_heatmap(mut self, val: bool) -> Self {
        self.show_heatmap = val;
        self
    }

    pub fn heatmap_tracker(mut self, tracker: Option<&'a HeatmapTracker>) -> Self {
        self.heatmap_tracker = tracker;
        self
    }

    pub fn show_volume_profile(mut self, val: bool) -> Self {
        self.show_volume_profile = val;
        self
    }

    pub fn show_cumulative_depth(mut self, val: bool) -> Self {
        self.show_cumulative_depth = val;
        self
    }

    pub fn volume_profile_mode(mut self, mode: Option<VolumeProfileMode>) -> Self {
        self.volume_profile_mode = mode;
        self
    }

    pub fn trade_volume_profile(mut self, profile: Option<&'a TradeVolumeProfile>) -> Self {
        self.trade_volume_profile = profile;
        self
    }

    /// Get the theme, defaulting to dark if not set.
    fn get_theme(&self) -> Theme {
        self.theme.copied().unwrap_or_else(Theme::dark)
    }
}

impl Widget for DomLadder<'_> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        // Render block if present, use inner area for content
        let inner = if let Some(block) = self.block.take() {
            let inner = block.inner(area);
            block.render(area, buf);
            inner
        } else {
            area
        };

        // If center_price is None, render empty state
        if self.center_price.is_none() {
            self.render_empty_state(inner, buf);
            return;
        }

        self.render_ladder(inner, buf);
    }
}

/// Per-row pre-computed data for rendering.
struct RowData {
    price: Decimal,
    bid_qty: Option<Decimal>,
    ask_qty: Option<Decimal>,
    bid_cumulative: Decimal,
    ask_cumulative: Decimal,
    is_spread_zone: bool,
}

impl DomLadder<'_> {
    /// Render "Waiting for depth data..." centered in the area.
    fn render_empty_state(&self, area: Rect, buf: &mut Buffer) {
        let theme = self.get_theme();
        let msg = "Waiting for depth data...";
        let x = area.left() + (area.width.saturating_sub(msg.len() as u16)) / 2;
        let y = area.top() + area.height / 2;
        let style = Style::default().fg(theme.text_muted);
        for (i, ch) in msg.chars().enumerate() {
            let cx = x + i as u16;
            if cx < area.right() && y < area.bottom() {
                if let Some(cell) = buf.cell_mut((cx, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }

    /// Core rendering: dynamic multi-column layout with tick-aligned prices.
    fn render_ladder(&self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height == 0 {
            return; // Too small to render
        }

        let theme = self.get_theme();
        let order_book = match self.order_book {
            Some(ob) => ob,
            None => return,
        };
        let center = match self.center_price {
            Some(c) => c,
            None => return,
        };

        // --- Dynamic column width calculation ---
        let total_width = area.width;
        let has_position = self.position_qty.map_or(false, |q| q != Decimal::ZERO);
        let show_vol = self.show_volume_profile;
        let show_cum = self.show_cumulative_depth;

        // PnL column (rightmost, only with position)
        let pnl_w: u16 = if has_position { (total_width * 12 / 100).max(8) } else { 0 };

        // Overlay columns: only allocated when active
        let cum_w: u16 = if show_cum { (total_width * 12 / 100).max(7) } else { 0 };
        let vol_w: u16 = if show_vol { (total_width * 8 / 100).max(4) } else { 0 };

        // Remaining space for core columns (bid | price | ask)
        let remaining = total_width.saturating_sub(pnl_w + cum_w * 2 + vol_w * 2);
        let bid_w = (remaining * 30 / 100).max(6);
        let price_w = (remaining * 40 / 100).max(8);
        let ask_w = remaining.saturating_sub(bid_w + price_w);

        if ask_w == 0 {
            return;
        }

        // Column positions (left to right):
        // [cum_bid] [vol_bid] [bid_qty] [price] [ask_qty] [vol_ask] [cum_ask] [pnl]
        let cum_bid_x = area.left();
        let vol_bid_x = cum_bid_x + cum_w;
        let bid_x = vol_bid_x + vol_w;
        let price_x = bid_x + bid_w;
        let ask_x = price_x + price_w;
        let vol_ask_x = ask_x + ask_w;
        let cum_ask_x = vol_ask_x + vol_w;
        let pnl_x = cum_ask_x + cum_w;

        // --- Generate tick-aligned price levels and pre-compute row data ---
        let num_rows = area.height;
        let aligned_center = (center / self.tick_size).floor() * self.tick_size;
        let half = num_rows / 2;

        // Compute cursor row index from offset
        let cursor_row_index = self.cursor_offset.map(|offset| half as i16 - offset);

        // Compute current price line row from live mid-price position in the grid.
        // The grid is anchored at aligned_center (row = half). Mid-price may be
        // anywhere in the visible range as the book moves.
        let price_line_row: Option<usize> = self.mid_price.and_then(|mid| {
            // How many ticks above aligned_center is the mid-price?
            let ticks_from_center = ((mid - aligned_center) / self.tick_size).floor();
            // Row index: higher prices have lower row indices
            let row = Decimal::from(half as i64) - ticks_from_center;
            let row_i = row.to_i64()?;
            if row_i >= 0 && row_i < num_rows as i64 {
                Some(row_i as usize)
            } else {
                None // mid-price scrolled off screen
            }
        });

        // Compute spread for the spread indicator
        let spread = match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        };

        // First pass: build row data array with prices and quantities
        let mut rows: Vec<RowData> = Vec::with_capacity(num_rows as usize);
        for row_index in 0..num_rows {
            let price = aligned_center
                + (Decimal::from(half as i64) - Decimal::from(row_index as i64)) * self.tick_size;
            let bid_qty = order_book.bid_qty_at(&price);
            let ask_qty = order_book.ask_qty_at(&price);
            let is_spread_zone = match (self.best_bid, self.best_ask) {
                (Some(bid), Some(ask)) => price > bid && price < ask,
                _ => false,
            };
            rows.push(RowData {
                price,
                bid_qty,
                ask_qty,
                bid_cumulative: Decimal::ZERO,
                ask_cumulative: Decimal::ZERO,
                is_spread_zone,
            });
        }

        // --- Pre-compute cumulative depth (if overlay active) ---
        let (max_bid_cum, max_ask_cum) = if show_cum {
            // Bid cumulative: rows are in price-descending order (top = high price, bottom = low price)
            // Bids exist at prices <= best_bid. Accumulate starting from best_bid going downward.
            let mut bid_cum = Decimal::ZERO;
            for row in rows.iter_mut() {
                if let Some(qty) = row.bid_qty {
                    if !row.is_spread_zone {
                        bid_cum += qty;
                    }
                }
                row.bid_cumulative = if row.bid_qty.is_some() && !row.is_spread_zone {
                    bid_cum
                } else {
                    Decimal::ZERO
                };
            }

            // Ask cumulative: accumulate from best_ask going upward (iterate in reverse)
            let mut ask_cum = Decimal::ZERO;
            for row in rows.iter_mut().rev() {
                if let Some(qty) = row.ask_qty {
                    if !row.is_spread_zone {
                        ask_cum += qty;
                    }
                }
                row.ask_cumulative = if row.ask_qty.is_some() && !row.is_spread_zone {
                    ask_cum
                } else {
                    Decimal::ZERO
                };
            }

            let max_bid = rows.iter().map(|r| r.bid_cumulative).max().unwrap_or(Decimal::ZERO);
            let max_ask = rows.iter().map(|r| r.ask_cumulative).max().unwrap_or(Decimal::ZERO);
            (max_bid, max_ask)
        } else {
            (Decimal::ZERO, Decimal::ZERO)
        };

        // --- Pre-compute volume profile max for bar scaling ---
        let vol_max = if show_vol {
            match self.volume_profile_mode {
                Some(VolumeProfileMode::TradeVolume) => {
                    self.trade_volume_profile.map_or(Decimal::ZERO, |p| p.max_volume())
                }
                Some(VolumeProfileMode::RestingDepth) => {
                    // Max resting depth across all visible rows
                    rows.iter()
                        .filter_map(|r| {
                            if r.is_spread_zone {
                                None
                            } else {
                                r.bid_qty.or(r.ask_qty)
                            }
                        })
                        .max()
                        .unwrap_or(Decimal::ZERO)
                }
                None => Decimal::ZERO,
            }
        } else {
            Decimal::ZERO
        };

        // --- Identify spread indicator row ---
        let spread_zone_rows: Vec<u16> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| if r.is_spread_zone { Some(i as u16) } else { None })
            .collect();
        let spread_indicator_row = if !spread_zone_rows.is_empty() {
            Some(spread_zone_rows[spread_zone_rows.len() / 2])
        } else {
            None
        };

        // --- Render each row ---
        for (row_index, row) in rows.iter().enumerate() {
            let abs_y = area.top() + row_index as u16;
            if abs_y >= area.bottom() {
                break;
            }

            let is_spread_indicator_row = spread_indicator_row == Some(row_index as u16);

            // --- Cumulative depth: bid side (leftmost column) ---
            if show_cum && cum_w > 0 && row.bid_cumulative > Decimal::ZERO {
                let fill_ratio = if max_bid_cum > Decimal::ZERO {
                    (row.bid_cumulative.to_f64().unwrap_or(0.0) / max_bid_cum.to_f64().unwrap_or(1.0)).min(1.0)
                } else {
                    0.0
                };
                let fill_cells = ((fill_ratio * cum_w as f64).round() as u16).min(cum_w);
                // Background bar (green for bids)
                for x in cum_bid_x..cum_bid_x + fill_cells {
                    if let Some(cell) = buf.cell_mut((x, abs_y)) {
                        cell.set_bg(Color::Rgb(0, 80, 0));
                    }
                }
                // Numeric overlay (right-aligned)
                let text = format_qty(row.bid_cumulative, cum_w as usize);
                let padding = (cum_w as usize).saturating_sub(text.len());
                for (i, ch) in text.chars().enumerate() {
                    let cx = cum_bid_x + padding as u16 + i as u16;
                    if cx < cum_bid_x + cum_w {
                        if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                            cell.set_char(ch).set_fg(Color::White);
                        }
                    }
                }
            }

            // --- Volume profile: bid side ---
            if show_vol && vol_w > 0 && !row.is_spread_zone {
                let vol = self.get_volume_for_row(row, true);
                if vol > Decimal::ZERO && vol_max > Decimal::ZERO {
                    let fill_ratio = (vol.to_f64().unwrap_or(0.0) / vol_max.to_f64().unwrap_or(1.0)).min(1.0);
                    render_h_bar(buf, vol_bid_x, abs_y, vol_w, fill_ratio, Color::Rgb(60, 100, 60));
                }
            }

            // --- Bid column (right-aligned, green) ---
            if let Some(qty) = row.bid_qty {
                let text = format_qty(qty, bid_w as usize);
                let style = Style::default().fg(theme.bullish);
                let padding = (bid_w as usize).saturating_sub(text.len());
                let start_x = bid_x + padding as u16;
                for (i, ch) in text.chars().enumerate() {
                    let cx = start_x + i as u16;
                    if cx < bid_x + bid_w {
                        if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                            cell.set_char(ch).set_style(style);
                        }
                    }
                }
            }

            // --- Price column (center) ---
            if is_spread_indicator_row {
                if let Some(sp) = spread {
                    let ticks = if self.tick_size > Decimal::ZERO {
                        (sp / self.tick_size).to_u32().unwrap_or(0)
                    } else {
                        0
                    };
                    let spread_text = format!("spread: {} ({}t)", sp, ticks);
                    let style = Style::default().fg(theme.text_muted);
                    let text_len = spread_text.len() as u16;
                    let center_x = price_x + (price_w.saturating_sub(text_len)) / 2;
                    for (i, ch) in spread_text.chars().enumerate() {
                        let cx = center_x + i as u16;
                        if cx < price_x + price_w && cx >= price_x {
                            if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                cell.set_char(ch).set_style(style);
                            }
                        }
                    }
                }
            } else {
                let price_text = row.price.to_string();
                let price_style = if row.is_spread_zone {
                    Style::default().fg(theme.text_primary).bg(theme.background_highlight)
                } else {
                    Style::default().fg(theme.text_primary)
                };
                let text_len = price_text.len() as u16;
                let center_x = price_x + (price_w.saturating_sub(text_len)) / 2;
                for (i, ch) in price_text.chars().enumerate() {
                    let cx = center_x + i as u16;
                    if cx < price_x + price_w && cx >= price_x {
                        if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                            cell.set_char(ch).set_style(price_style);
                        }
                    }
                }

                // --- Heatmap significance markers (in price column, left of price text) ---
                if self.show_heatmap && !row.is_spread_zone {
                    if let Some(tracker) = self.heatmap_tracker {
                        // Check both sides, use the higher tier
                        let bid_tier = row.bid_qty.and_then(|q| tracker.significance_tier(q));
                        let ask_tier = row.ask_qty.and_then(|q| tracker.significance_tier(q));
                        let tier = match (bid_tier, ask_tier) {
                            (Some(a), Some(b)) => Some(a.max(b)),
                            (Some(a), None) => Some(a),
                            (None, Some(b)) => Some(b),
                            (None, None) => None,
                        };
                        if let Some(tier) = tier {
                            let (marker_str, marker_color) = match tier {
                                SignificanceTier::Tier1 => (">", Color::Rgb(100, 100, 200)),
                                SignificanceTier::Tier2 => (">>", Color::Rgb(220, 200, 50)),
                                SignificanceTier::Tier3 => (">>>", Color::Rgb(255, 80, 180)),
                            };
                            let marker_len = marker_str.len() as u16;
                            let marker_x = center_x.saturating_sub(marker_len);
                            for (i, ch) in marker_str.chars().enumerate() {
                                let cx = marker_x + i as u16;
                                if cx >= price_x && cx < price_x + price_w {
                                    if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                        cell.set_char(ch).set_fg(marker_color);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // --- Ask column (left-aligned, red) ---
            if let Some(qty) = row.ask_qty {
                let text = format_qty(qty, ask_w as usize);
                let style = Style::default().fg(theme.bearish);
                for (i, ch) in text.chars().enumerate() {
                    let cx = ask_x + i as u16;
                    if cx < ask_x + ask_w {
                        if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                            cell.set_char(ch).set_style(style);
                        }
                    }
                }
            }

            // --- Volume profile: ask side ---
            if show_vol && vol_w > 0 && !row.is_spread_zone {
                let vol = self.get_volume_for_row(row, false);
                if vol > Decimal::ZERO && vol_max > Decimal::ZERO {
                    let fill_ratio = (vol.to_f64().unwrap_or(0.0) / vol_max.to_f64().unwrap_or(1.0)).min(1.0);
                    render_h_bar(buf, vol_ask_x, abs_y, vol_w, fill_ratio, Color::Rgb(100, 60, 60));
                }
            }

            // --- Cumulative depth: ask side (right outer column) ---
            if show_cum && cum_w > 0 && row.ask_cumulative > Decimal::ZERO {
                let fill_ratio = if max_ask_cum > Decimal::ZERO {
                    (row.ask_cumulative.to_f64().unwrap_or(0.0) / max_ask_cum.to_f64().unwrap_or(1.0)).min(1.0)
                } else {
                    0.0
                };
                let fill_cells = ((fill_ratio * cum_w as f64).round() as u16).min(cum_w);
                // Background bar (red for asks)
                for x in cum_ask_x..cum_ask_x + fill_cells {
                    if let Some(cell) = buf.cell_mut((x, abs_y)) {
                        cell.set_bg(Color::Rgb(80, 0, 0));
                    }
                }
                // Numeric overlay (left-aligned)
                let text = format_qty(row.ask_cumulative, cum_w as usize);
                for (i, ch) in text.chars().enumerate() {
                    let cx = cum_ask_x + i as u16;
                    if cx < cum_ask_x + cum_w {
                        if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                            cell.set_char(ch).set_fg(Color::White);
                        }
                    }
                }
            }

            // --- Working order markers ---
            if let Some(orders_map) = self.working_orders {
                if let Some(orders) = orders_map.get(&row.price) {
                    if let Some(first) = orders.first() {
                        let marker_text = if orders.len() > 1 {
                            format!("{}x{}", if first.side == OrderSide::Buy { "B" } else { "S" }, orders.len())
                        } else {
                            format!("{} {}", if first.side == OrderSide::Buy { "B" } else { "S" }, format_qty(first.orig_qty, 6))
                        };
                        let style = match first.side {
                            OrderSide::Buy => Style::default().fg(theme.bullish).add_modifier(ratatui::style::Modifier::BOLD),
                            OrderSide::Sell => Style::default().fg(theme.bearish).add_modifier(ratatui::style::Modifier::BOLD),
                        };
                        let (start_x, max_x) = match first.side {
                            OrderSide::Buy => (bid_x, bid_x + bid_w),
                            OrderSide::Sell => (ask_x, ask_x + ask_w),
                        };
                        let text_start = if first.side == OrderSide::Buy {
                            start_x + (bid_w as usize).saturating_sub(marker_text.len()) as u16
                        } else {
                            start_x
                        };
                        for (i, ch) in marker_text.chars().enumerate() {
                            let cx = text_start + i as u16;
                            if cx < max_x && cx >= start_x {
                                if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                    cell.set_char(ch).set_style(style);
                                }
                            }
                        }
                    }
                }
            }

            // --- Position entry price marker ---
            if let Some(entry_price) = self.position_entry_price {
                if row.price == entry_price {
                    let is_long = self.position_qty.map_or(false, |q| q > Decimal::ZERO);
                    let entry_color = if is_long { theme.bullish } else { theme.bearish };
                    let marker = if is_long { ">>> ENTRY" } else { "<<< ENTRY" };
                    let style = Style::default().fg(entry_color).add_modifier(ratatui::style::Modifier::BOLD);
                    for (i, ch) in marker.chars().enumerate() {
                        let cx = bid_x + i as u16;
                        if cx < bid_x + bid_w {
                            if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                cell.set_char(ch).set_style(style);
                            }
                        }
                    }
                }
            }

            // --- Hypothetical PnL column (rightmost, only with position) ---
            if has_position && pnl_w > 0 {
                if let (Some(entry_price), Some(pos_qty)) = (self.position_entry_price, self.position_qty) {
                    if pos_qty != Decimal::ZERO {
                        let pnl = pos_qty * (row.price - entry_price);
                        let pnl_text = format_pnl(pnl, pnl_w as usize);
                        let pnl_style = if pnl > Decimal::ZERO {
                            Style::default().fg(theme.bullish)
                        } else if pnl < Decimal::ZERO {
                            Style::default().fg(theme.bearish)
                        } else {
                            Style::default().fg(theme.text_muted)
                        };
                        for (i, ch) in pnl_text.chars().enumerate() {
                            let cx = pnl_x + i as u16;
                            if cx < area.right() {
                                if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                    cell.set_char(ch).set_style(pnl_style);
                                }
                            }
                        }
                    }
                }
            }

            // --- Ghost bracket preview markers (SL / TP) ---
            if let Some(sl_price) = self.sl_preview_price {
                if row.price == sl_price && !is_spread_indicator_row {
                    let marker = " SL";
                    let style = Style::default().fg(theme.bearish);
                    for (i, ch) in marker.chars().enumerate() {
                        let cx = bid_x + i as u16;
                        if cx < bid_x + bid_w {
                            if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                cell.set_char(ch).set_style(style);
                            }
                        }
                    }
                }
            }
            if let Some(tp_price) = self.tp_preview_price {
                if row.price == tp_price && !is_spread_indicator_row {
                    let marker = " TP";
                    let style = Style::default().fg(theme.bullish);
                    for (i, ch) in marker.chars().enumerate() {
                        let cx = bid_x + i as u16;
                        if cx < bid_x + bid_w {
                            if let Some(cell) = buf.cell_mut((cx, abs_y)) {
                                cell.set_char(ch).set_style(style);
                            }
                        }
                    }
                }
            }

            // --- Cursor row: dimmed gray background highlight (preserves fg colors) ---
            let is_cursor_row = cursor_row_index == Some(row_index as i16);
            if is_cursor_row {
                let cursor_bg = Color::Rgb(40, 40, 40);
                for x in area.left()..area.right() {
                    if let Some(cell) = buf.cell_mut((x, abs_y)) {
                        cell.set_bg(cursor_bg);
                    }
                }
            }

            // --- Current price line (dotted line at top of mid-price row) ---
            // Draw underline on the row above, which visually appears as the top edge.
            if price_line_row == Some(row_index + 1) && row_index > 0 {
                let underline_style = Style::default()
                    .add_modifier(Modifier::UNDERLINED)
                    .underline_color(Color::Rgb(160, 160, 160));
                for x in area.left()..area.right() {
                    if (x - area.left()) % 2 == 0 {
                        if let Some(cell) = buf.cell_mut((x, abs_y)) {
                            cell.set_style(underline_style);
                        }
                    }
                }
            }
        }
    }

    /// Get volume for a row based on current volume profile mode.
    /// `is_bid_side`: true for bid-side volume, false for ask-side.
    fn get_volume_for_row(&self, row: &RowData, is_bid_side: bool) -> Decimal {
        match self.volume_profile_mode {
            Some(VolumeProfileMode::TradeVolume) => {
                // Buy volume renders on bid side, sell volume on ask side
                if is_bid_side {
                    self.trade_volume_profile
                        .map_or(Decimal::ZERO, |p| p.buy_volume_at(&row.price))
                } else {
                    self.trade_volume_profile
                        .map_or(Decimal::ZERO, |p| p.sell_volume_at(&row.price))
                }
            }
            Some(VolumeProfileMode::RestingDepth) => {
                // Resting depth: show bid qty on bid side, ask qty on ask side
                if is_bid_side {
                    row.bid_qty.unwrap_or(Decimal::ZERO)
                } else {
                    row.ask_qty.unwrap_or(Decimal::ZERO)
                }
            }
            None => Decimal::ZERO,
        }
    }
}

/// Render a horizontal bar using left-side block elements for sub-cell precision.
/// Bar grows left-to-right within the column starting at `start_x`.
fn render_h_bar(buf: &mut Buffer, start_x: u16, y: u16, col_width: u16, fill_ratio: f64, color: Color) {
    let total_eighths = (fill_ratio * col_width as f64 * 8.0).round() as u32;
    let full_cells = total_eighths / 8;
    let remainder = (total_eighths % 8) as usize;

    for i in 0..col_width {
        let cx = start_x + i;
        if let Some(cell) = buf.cell_mut((cx, y)) {
            if (i as u32) < full_cells {
                cell.set_char(H_BLOCKS[8]).set_fg(color);
            } else if (i as u32) == full_cells && remainder > 0 {
                cell.set_char(H_BLOCKS[remainder]).set_fg(color);
            }
            // Remaining cells stay empty (space)
        }
    }
}

/// Format a PnL value for display: +12.34 or -5.67, right-aligned within max_width.
fn format_pnl(pnl: Decimal, max_width: usize) -> String {
    let sign = if pnl > Decimal::ZERO { "+" } else { "" };
    let value = pnl.to_f64().unwrap_or(0.0);
    let text = if value.abs() >= 10_000.0 {
        format!("{}{:.0}", sign, value)
    } else if value.abs() >= 1_000.0 {
        format!("{}{:.1}", sign, value)
    } else {
        format!("{}{:.2}", sign, value)
    };
    if text.len() <= max_width {
        // Right-align within column
        format!("{:>width$}", text, width = max_width)
    } else {
        text[..max_width].to_string()
    }
}

/// Format a quantity for display within a column of max_width characters.
///
/// Uses compact notation for large values:
/// - >= 1,000,000: "{:.1}M" (e.g., "1.2M")
/// - >= 1,000: "{:.1}K" (e.g., "45.3K")
/// - Otherwise: direct Decimal::to_string()
///
/// Truncates to max_width if still too long.
fn format_qty(qty: Decimal, max_width: usize) -> String {
    let direct = qty.to_string();
    if direct.len() <= max_width {
        return direct;
    }

    let value = qty.to_f64().unwrap_or(0.0);
    let compact = if value.abs() >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        direct
    };

    if compact.len() <= max_width {
        compact
    } else {
        compact[..max_width].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_format_qty_small_value() {
        assert_eq!(format_qty(dec!(1.5), 10), "1.5");
        assert_eq!(format_qty(dec!(0.001), 10), "0.001");
    }

    #[test]
    fn test_format_qty_thousands() {
        // "45300" is 5 chars, fits in 10
        assert_eq!(format_qty(dec!(45300), 10), "45300");
        // Force compact by using small max_width
        assert_eq!(format_qty(dec!(45300), 4), "45.3");
    }

    #[test]
    fn test_format_qty_millions() {
        // "1234567" is 7 chars, fits in 10
        assert_eq!(format_qty(dec!(1234567), 10), "1234567");
        // Force compact by small max_width
        assert_eq!(format_qty(dec!(1234567), 5), "1.2M");
    }

    #[test]
    fn test_format_qty_truncation() {
        // Very long number that doesn't fit
        let result = format_qty(dec!(123.456789012345), 6);
        assert!(result.len() <= 6);
    }

    #[test]
    fn test_dom_ladder_builder() {
        let ladder = DomLadder::new()
            .tick_size(dec!(0.01))
            .center_price(Some(dec!(100.00)));

        assert_eq!(ladder.tick_size, dec!(0.01));
        assert_eq!(ladder.center_price, Some(dec!(100.00)));
        assert!(ladder.block.is_none());
    }

    #[test]
    fn test_dom_ladder_empty_state_render() {
        let ladder = DomLadder::new()
            .center_price(None); // No center price -> empty state

        // Just verify it doesn't panic when rendering
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        ladder.render(area, &mut buf);

        // Check that "Waiting for depth data..." appears somewhere in the buffer
        let mut content = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                let ch = buf.cell((x, y)).unwrap().symbol().chars().next().unwrap_or(' ');
                content.push(ch);
            }
        }
        assert!(content.contains("Waiting for depth data..."));
    }

    #[test]
    fn test_dom_ladder_renders_prices() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec!(100.00), dec!(1.5)), (dec!(99.99), dec!(2.0))],
            vec![(dec!(100.01), dec!(0.8)), (dec!(100.02), dec!(1.2))],
            1,
        );

        let ladder = DomLadder::new()
            .order_book(&book)
            .tick_size(dec!(0.01))
            .center_price(Some(dec!(100.005)))
            .best_bid(Some(dec!(100.00)))
            .best_ask(Some(dec!(100.01)));

        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        ladder.render(area, &mut buf);

        // Verify buffer is not empty (some cells have been written)
        let has_content = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                let ch = buf.cell((x, y)).unwrap().symbol().chars().next().unwrap_or(' ');
                ch != ' '
            })
        });
        assert!(has_content, "Ladder should render non-empty content");
    }

    #[test]
    fn test_dom_ladder_with_volume_profile_renders() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec!(100.00), dec!(1.5))],
            vec![(dec!(100.01), dec!(0.8))],
            1,
        );

        let profile = TradeVolumeProfile::new();

        let ladder = DomLadder::new()
            .order_book(&book)
            .tick_size(dec!(0.01))
            .center_price(Some(dec!(100.005)))
            .best_bid(Some(dec!(100.00)))
            .best_ask(Some(dec!(100.01)))
            .show_volume_profile(true)
            .volume_profile_mode(Some(VolumeProfileMode::RestingDepth))
            .trade_volume_profile(Some(&profile));

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        // Should not panic
        ladder.render(area, &mut buf);
    }

    #[test]
    fn test_dom_ladder_with_cumulative_depth_renders() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec!(100.00), dec!(1.5)), (dec!(99.99), dec!(2.0))],
            vec![(dec!(100.01), dec!(0.8)), (dec!(100.02), dec!(1.2))],
            1,
        );

        let ladder = DomLadder::new()
            .order_book(&book)
            .tick_size(dec!(0.01))
            .center_price(Some(dec!(100.005)))
            .best_bid(Some(dec!(100.00)))
            .best_ask(Some(dec!(100.01)))
            .show_cumulative_depth(true);

        let area = Rect::new(0, 0, 100, 10);
        let mut buf = Buffer::empty(area);
        // Should not panic
        ladder.render(area, &mut buf);
    }

    #[test]
    fn test_dom_ladder_all_overlays_combined() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec!(100.00), dec!(1.5)), (dec!(99.99), dec!(2.0))],
            vec![(dec!(100.01), dec!(0.8)), (dec!(100.02), dec!(1.2))],
            1,
        );

        let tracker = HeatmapTracker::new();
        let profile = TradeVolumeProfile::new();

        let ladder = DomLadder::new()
            .order_book(&book)
            .tick_size(dec!(0.01))
            .center_price(Some(dec!(100.005)))
            .best_bid(Some(dec!(100.00)))
            .best_ask(Some(dec!(100.01)))
            .show_heatmap(true)
            .heatmap_tracker(Some(&tracker))
            .show_volume_profile(true)
            .volume_profile_mode(Some(VolumeProfileMode::TradeVolume))
            .trade_volume_profile(Some(&profile))
            .show_cumulative_depth(true);

        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        // All three overlays combined should not panic
        ladder.render(area, &mut buf);
    }

    #[test]
    fn test_h_blocks_bar_rendering() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);

        // Full fill (100%)
        render_h_bar(&mut buf, 0, 0, 10, 1.0, Color::Green);
        for x in 0..10u16 {
            assert_eq!(buf.cell((x, 0)).unwrap().symbol(), "\u{2588}");
        }

        // Half fill (50%)
        let mut buf2 = Buffer::empty(area);
        render_h_bar(&mut buf2, 0, 0, 10, 0.5, Color::Green);
        // First 5 cells should be full blocks
        for x in 0..5u16 {
            assert_eq!(buf2.cell((x, 0)).unwrap().symbol(), "\u{2588}");
        }
        // Cell 5 might be empty or partial, rest empty
    }
}
