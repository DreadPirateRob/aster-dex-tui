// src/data/heatmap_tracker.rs
// Heatmap overlay tracker for DOM price ladder.
// Maintains a rolling window of observed order book quantities to compute
// significance tiers for marker-based display on significant resting order levels.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Rolling window duration for heatmap intensity normalization (30 seconds).
const WINDOW_DURATION: Duration = Duration::from_secs(30);

/// Significance tier for heatmap marker display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignificanceTier {
    /// Notable level (>= 30% of rolling max)
    Tier1,
    /// Significant resting order (>= 60% of rolling max)
    Tier2,
    /// Exceptional size (>= 90% of rolling max)
    Tier3,
}

/// Tracks rolling-window order book quantity statistics and provides
/// significance tier classification for the DOM heatmap overlay.
///
/// Quantities are classified into three tiers based on their ratio
/// to the rolling window maximum: Tier1 (>= 30%), Tier2 (>= 60%), Tier3 (>= 90%).
pub struct HeatmapTracker {
    /// Rolling window of (timestamp, max_qty_seen_in_snapshot).
    window: VecDeque<(Instant, Decimal)>,
    /// Duration of the rolling window.
    window_duration: Duration,
    /// Cached minimum quantity in current window.
    current_min: Decimal,
    /// Cached maximum quantity in current window.
    current_max: Decimal,
}

impl HeatmapTracker {
    /// Create a new tracker with a 30-second rolling window.
    pub fn new() -> Self {
        Self {
            window: VecDeque::new(),
            window_duration: WINDOW_DURATION,
            current_min: Decimal::ZERO,
            current_max: Decimal::ZERO,
        }
    }

    /// Ingest all visible bid/ask quantities from a depth snapshot.
    ///
    /// Records the max quantity with timestamp, expires old entries,
    /// and recomputes the min/max range for intensity normalization.
    /// Includes a minimum range floor to prevent flickering in quiet markets.
    pub fn push_snapshot(&mut self, quantities: impl Iterator<Item = Decimal>) {
        let max_qty = quantities.max().unwrap_or(Decimal::ZERO);
        if max_qty.is_zero() {
            return;
        }

        let now = Instant::now();
        self.window.push_back((now, max_qty));

        // Expire entries older than window_duration
        while let Some(&(ts, _)) = self.window.front() {
            if now.duration_since(ts) > self.window_duration {
                self.window.pop_front();
            } else {
                break;
            }
        }

        // Recompute min/max from window
        if self.window.is_empty() {
            self.current_min = Decimal::ZERO;
            self.current_max = Decimal::ZERO;
        } else {
            let mut min = Decimal::MAX;
            let mut max = Decimal::ZERO;
            for &(_, qty) in &self.window {
                if qty < min {
                    min = qty;
                }
                if qty > max {
                    max = qty;
                }
            }
            self.current_min = min;
            self.current_max = max;

            // Minimum range floor: if range < max * 0.10, expand to max * 0.10
            // Prevents flickering during quiet markets
            let range = self.current_max - self.current_min;
            let floor = self.current_max * Decimal::new(10, 2); // 0.10
            if range < floor {
                self.current_min = self.current_max - floor;
                if self.current_min < Decimal::ZERO {
                    self.current_min = Decimal::ZERO;
                }
            }
        }
    }

    /// Classify a quantity into a significance tier based on the rolling window max.
    /// Returns None if the quantity is below Tier 1 threshold or the window is empty.
    pub fn significance_tier(&self, qty: Decimal) -> Option<SignificanceTier> {
        if self.current_max.is_zero() {
            return None;
        }
        let ratio = qty.to_f64().unwrap_or(0.0) / self.current_max.to_f64().unwrap_or(1.0);
        if ratio >= 0.90 {
            Some(SignificanceTier::Tier3)
        } else if ratio >= 0.60 {
            Some(SignificanceTier::Tier2)
        } else if ratio >= 0.30 {
            Some(SignificanceTier::Tier1)
        } else {
            None
        }
    }
}

impl Default for HeatmapTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_new_tracker_defaults() {
        let tracker = HeatmapTracker::new();
        assert!(tracker.window.is_empty());
        assert_eq!(tracker.current_min, Decimal::ZERO);
        assert_eq!(tracker.current_max, Decimal::ZERO);
    }

    #[test]
    fn test_push_snapshot_updates_range() {
        let mut tracker = HeatmapTracker::new();
        tracker.push_snapshot(vec![dec!(10), dec!(20), dec!(30)].into_iter());
        // max_qty from snapshot = 30
        // With single entry, min = max = 30, but floor rule expands
        assert!(tracker.current_max > Decimal::ZERO);
    }

    #[test]
    fn test_push_snapshot_multiple_builds_range() {
        let mut tracker = HeatmapTracker::new();
        tracker.push_snapshot(vec![dec!(10), dec!(20)].into_iter());
        tracker.push_snapshot(vec![dec!(5), dec!(50)].into_iter());
        // Range should span from min(20,50) to max(20,50) with floor
        assert_eq!(tracker.current_max, dec!(50));
    }

    #[test]
    fn test_empty_snapshot_ignored() {
        let mut tracker = HeatmapTracker::new();
        tracker.push_snapshot(std::iter::empty());
        assert!(tracker.window.is_empty());
    }

    #[test]
    fn test_significance_tier_classification() {
        let mut tracker = HeatmapTracker::new();
        tracker.push_snapshot(vec![dec!(10)].into_iter());
        tracker.push_snapshot(vec![dec!(100)].into_iter());
        // current_max = 100
        // Below 30% threshold -> None
        assert_eq!(tracker.significance_tier(dec!(20)), None);
        // 30% of 100 = 30 -> Tier1
        assert_eq!(tracker.significance_tier(dec!(30)), Some(SignificanceTier::Tier1));
        assert_eq!(tracker.significance_tier(dec!(50)), Some(SignificanceTier::Tier1));
        // 60% of 100 = 60 -> Tier2
        assert_eq!(tracker.significance_tier(dec!(60)), Some(SignificanceTier::Tier2));
        assert_eq!(tracker.significance_tier(dec!(80)), Some(SignificanceTier::Tier2));
        // 90% of 100 = 90 -> Tier3
        assert_eq!(tracker.significance_tier(dec!(90)), Some(SignificanceTier::Tier3));
        assert_eq!(tracker.significance_tier(dec!(100)), Some(SignificanceTier::Tier3));
    }

    #[test]
    fn test_significance_tier_empty_window() {
        let tracker = HeatmapTracker::new();
        // Empty window -> current_max is zero -> always None
        assert_eq!(tracker.significance_tier(dec!(100)), None);
    }

    #[test]
    fn test_significance_tier_ord() {
        // Verify Ord derivation works for max() comparison
        assert!(SignificanceTier::Tier3 > SignificanceTier::Tier2);
        assert!(SignificanceTier::Tier2 > SignificanceTier::Tier1);
        assert_eq!(
            SignificanceTier::Tier1.max(SignificanceTier::Tier3),
            SignificanceTier::Tier3
        );
    }
}
