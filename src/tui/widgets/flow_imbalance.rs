// src/tui/widgets/flow_imbalance.rs
// Flow imbalance panel widget for the DOM view.
// Renders 4 rows of bidirectional dominance bars (Book, 1m, 5m, 15m)
// using block characters for sub-cell precision.

use crate::data::trade_imbalance::ImbalanceSnapshot;
use crate::tui::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// Left-block Unicode characters for sub-cell bar precision (same values as dom_ladder.rs).
/// Defined locally to avoid coupling between widgets.
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

/// Light shade character for the ask (sell) portion of the bar.
const ASK_SHADE: char = '\u{2591}';

/// Flow imbalance panel displaying 4 rows of bidirectional dominance bars.
/// Row 0: Book (order book bid/ask ratio)
/// Rows 1-3: Trade imbalance over 1m, 5m, 15m windows
pub struct FlowImbalance<'a> {
    /// Book-level bid ratio (0.0 to 1.0): bid_qty / (bid_qty + ask_qty)
    pub book_bid_ratio: f64,
    /// Whether the order book has data
    pub book_has_data: bool,
    /// Trade imbalance snapshots for [1m, 5m, 15m]
    pub trade_snapshots: &'a [ImbalanceSnapshot; 3],
    /// Theme for color choices
    pub theme: &'a Theme,
}

impl Widget for FlowImbalance<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Guard: need at least 4 rows and 20 columns for meaningful rendering
        if area.height < 4 || area.width < 20 {
            return;
        }

        let labels = ["Book:", " 1m: ", " 5m: ", "15m: "];
        let label_w: u16 = 6;
        let pct_w: u16 = 8;
        let bar_w = area.width.saturating_sub(label_w + pct_w);

        for row_idx in 0u16..4 {
            let y = area.top() + row_idx;
            if y >= area.bottom() {
                break;
            }

            // Determine ratio and has_data for this row
            let (ratio, has_data) = if row_idx == 0 {
                (self.book_bid_ratio, self.book_has_data)
            } else {
                let snap = &self.trade_snapshots[(row_idx - 1) as usize];
                (snap.bid_ratio, snap.has_data)
            };

            // Render label
            let label = labels[row_idx as usize];
            let label_style = Style::default().fg(self.theme.text_muted);
            for (i, ch) in label.chars().enumerate() {
                let x = area.left() + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch).set_style(label_style);
                    }
                }
            }

            let bar_start = area.left() + label_w;

            // No data: render centered "N/A"
            if !has_data {
                let na_text = "  N/A  ";
                let na_start = bar_start + bar_w.saturating_sub(na_text.len() as u16) / 2;
                let na_style = Style::default().fg(self.theme.text_muted);
                for (i, ch) in na_text.chars().enumerate() {
                    let x = na_start + i as u16;
                    if x < area.right() {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(ch).set_style(na_style);
                        }
                    }
                }
                continue;
            }

            // Compute sub-cell bid fill
            let bid_eighths = (ratio * bar_w as f64 * 8.0).round() as u32;
            let bid_full = bid_eighths / 8;
            let bid_remainder = (bid_eighths % 8) as usize;

            let bid_style = Style::default().fg(self.theme.bullish);
            let ask_style = Style::default().fg(self.theme.bearish);

            for i in 0..bar_w {
                let x = bar_start + i;
                if x >= area.right() {
                    break;
                }
                let (ch, style) = if (i as u32) < bid_full {
                    (H_BLOCKS[8], bid_style)
                } else if (i as u32) == bid_full && bid_remainder > 0 {
                    (H_BLOCKS[bid_remainder], bid_style)
                } else {
                    (ASK_SHADE, ask_style)
                };
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }

            // Render percentage label
            let pct_start = bar_start + bar_w;
            let (pct_text, pct_style) = if ratio >= 0.5 {
                let pct = (ratio * 100.0).round() as u32;
                (format!(" {}% bid", pct), bid_style)
            } else {
                let pct = ((1.0 - ratio) * 100.0).round() as u32;
                (format!(" {}% ask", pct), ask_style)
            };
            for (i, ch) in pct_text.chars().enumerate() {
                let x = pct_start + i as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch).set_style(pct_style);
                    }
                }
            }
        }
    }
}
