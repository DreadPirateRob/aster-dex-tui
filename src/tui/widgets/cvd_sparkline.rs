// src/tui/widgets/cvd_sparkline.rs
// Braille-based CVD sparkline widget for full-width rendering between prod info and chart

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::Widget,
};

use crate::tui::theme::Theme;

/// Braille dot bits for left column positions (top to bottom).
/// Braille char = 0x2800 | bits
const BRAILLE_LEFT_BITS: [u8; 4] = [0x01, 0x02, 0x04, 0x40];

/// Full-width braille CVD sparkline widget.
///
/// Renders 60 CVD data points interpolated across the available width
/// using braille characters. Each cell row has 4 vertical dot positions,
/// giving 8 total positions across 2 rows. Area-fill effect from bottom up.
pub struct CvdSparkline<'a> {
    data: &'a [u64],
    theme: &'a Theme,
    net_delta_positive: bool,
}

impl<'a> CvdSparkline<'a> {
    pub fn new(data: &'a [u64], theme: &'a Theme, net_delta_positive: bool) -> Self {
        Self {
            data,
            theme,
            net_delta_positive,
        }
    }
}

impl Widget for CvdSparkline<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.data.is_empty() {
            return;
        }

        let color = if self.net_delta_positive {
            self.theme.bullish
        } else {
            self.theme.bearish
        };
        let style = Style::default().fg(color);

        // Render right-aligned label first to know how much width to reserve
        let label = self.render_label(area, buf);
        let chart_width = area.width.saturating_sub(label) as usize;
        if chart_width == 0 {
            return;
        }

        let max_val = *self.data.iter().max().unwrap_or(&0);
        if max_val == 0 {
            return;
        }

        // Total vertical positions: area.height rows * 4 dots per row
        let total_dots = area.height as usize * 4;

        // Interpolate data points across chart_width columns
        for col in 0..chart_width {
            let x = area.left() + col as u16;
            if x >= area.right() {
                break;
            }

            // Map column to data index via linear interpolation
            let data_pos = if chart_width <= 1 {
                (self.data.len() - 1) as f64
            } else {
                col as f64 * (self.data.len() - 1) as f64 / (chart_width - 1) as f64
            };

            let idx_lo = (data_pos as usize).min(self.data.len() - 1);
            let idx_hi = (idx_lo + 1).min(self.data.len() - 1);
            let frac = data_pos - idx_lo as f64;

            let val = self.data[idx_lo] as f64 * (1.0 - frac) + self.data[idx_hi] as f64 * frac;

            // How many dot positions to fill from bottom
            let fill_dots = ((val / max_val as f64) * total_dots as f64).round() as usize;
            if fill_dots == 0 {
                continue;
            }

            // Fill dots from bottom up across rows
            // Row 0 is top, row (height-1) is bottom
            // Dot 0 is bottom-most dot of bottom row
            for row_from_bottom in 0..area.height as usize {
                let y = area.bottom().saturating_sub(1 + row_from_bottom as u16);
                if y < area.top() {
                    break;
                }

                let mut bits: u8 = 0;
                for dot in 0..4u8 {
                    // dot 0 = bottom of this cell row, dot 3 = top
                    let global_dot = row_from_bottom * 4 + dot as usize;
                    if global_dot < fill_dots {
                        // Braille bits: index 3 is top of cell, 0 is bottom
                        // But BRAILLE_LEFT_BITS[0] is top dot, [3] is bottom dot
                        // So dot 0 (bottom) maps to BRAILLE_LEFT_BITS[3]
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
    }
}

impl CvdSparkline<'_> {
    /// Render right-aligned CVD label and return its width (chars consumed).
    fn render_label(&self, area: Rect, buf: &mut Buffer) -> u16 {
        // Compute net CVD from data: last value minus first (since data is shifted to min=0)
        let last_val = self.data.last().copied().unwrap_or(0);
        let first_val = self.data.first().copied().unwrap_or(0);
        let delta = last_val as i64 - first_val as i64;

        let prefix = if delta >= 0 { "+" } else { "" };
        let abs_val = (delta as f64).abs();

        let label = if abs_val >= 1_000_000.0 {
            format!("CVD {}{:.1}M", prefix, delta as f64 / 1_000_000.0)
        } else if abs_val >= 1_000.0 {
            format!("CVD {}{:.1}K", prefix, delta as f64 / 1_000.0)
        } else {
            format!("CVD {}{:.0}", prefix, delta as f64)
        };

        let label_len = label.len() as u16;
        if label_len >= area.width {
            return 0; // Not enough room for label
        }

        let color = if self.net_delta_positive {
            self.theme.bullish
        } else {
            self.theme.bearish
        };
        let style = Style::default().fg(color);

        let start_x = area.right().saturating_sub(label_len);
        let y = area.top(); // Put label on first row

        for (i, ch) in label.chars().enumerate() {
            let x = start_x + i as u16;
            if x < area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }

        label_len + 1 // +1 for spacing between chart and label
    }
}
