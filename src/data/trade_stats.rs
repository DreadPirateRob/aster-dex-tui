// src/data/trade_stats.rs
// Rolling-window trade statistics accumulator for CVD sparkline, buy/sell volume stats,
// and large trade counting. Follows the same rolling-window pattern as TradeVolumeProfile.

use crate::data::types::TradeSide;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Number of one-second buckets in the CVD time series.
const CVD_BUCKET_COUNT: usize = 60;

/// Rolling window duration (60 seconds).
const TRADE_STATS_WINDOW: Duration = Duration::from_secs(60);

/// Scale factor for converting Decimal quantities to i64 for CVD series.
/// Preserves 6 decimal places of precision.
const CVD_SCALE_FACTOR: i64 = 1_000_000;

/// Entry in the trade log for aging/expiry.
#[derive(Debug, Clone)]
struct TradeLogEntry {
    /// When this trade was recorded.
    timestamp: Instant,
    /// Trade quantity.
    quantity: Decimal,
    /// Trade side (buy or sell).
    side: TradeSide,
    /// Whether this trade exceeds the large trade threshold.
    is_large: bool,
    /// Which CVD bucket this trade contributes to.
    bucket_index: usize,
}

/// Rolling-window trade statistics accumulator.
///
/// Maintains:
/// - CVD (Cumulative Volume Delta) time series bucketed by 1-second intervals
/// - Total buy/sell volume over the rolling window
/// - Count of large trades (value >= threshold)
///
/// Used by the chart trades panel to render CVD sparkline, buy/sell stats,
/// and large trade count.
pub struct TradeStats {
    /// Fixed-length CVD time series (one bucket per second, 60 buckets).
    /// Each bucket stores the net delta (buy_qty - sell_qty) scaled to i64.
    cvd_series: Vec<i64>,
    /// Total buy volume in rolling window.
    buy_volume: Decimal,
    /// Total sell volume in rolling window.
    sell_volume: Decimal,
    /// Count of large trades in rolling window.
    large_trade_count: u32,
    /// Trade log for aging: entries are popped when older than window.
    trade_log: VecDeque<TradeLogEntry>,
    /// Rolling window duration.
    window: Duration,
    /// Large trade threshold (price * quantity).
    large_threshold: Decimal,
    /// When the first trade arrived (for bucket calculation).
    window_start: Option<Instant>,
}

impl TradeStats {
    /// Create a new empty TradeStats with 60-entry zeroed CVD series.
    pub fn new(large_threshold: Decimal) -> Self {
        Self {
            cvd_series: vec![0i64; CVD_BUCKET_COUNT],
            buy_volume: Decimal::ZERO,
            sell_volume: Decimal::ZERO,
            large_trade_count: 0,
            trade_log: VecDeque::new(),
            window: TRADE_STATS_WINDOW,
            large_threshold,
            window_start: None,
        }
    }

    /// Record a trade, updating buy/sell volume, CVD series, and large trade count.
    ///
    /// The trade is added to the trade log for future expiry. Old entries
    /// beyond the rolling window are expired.
    pub fn record_trade(&mut self, price: Decimal, quantity: Decimal, side: TradeSide) {
        let now = Instant::now();

        // Initialize window start on first trade
        if self.window_start.is_none() {
            self.window_start = Some(now);
        }

        // Expire old entries first
        self.expire_old(now);

        // Compute bucket index (circular, 0-59)
        let elapsed = now.duration_since(self.window_start.unwrap());
        let bucket_index = (elapsed.as_secs() as usize) % CVD_BUCKET_COUNT;

        // Compute scaled quantity for CVD bucket
        let scaled_qty = (quantity * Decimal::from(CVD_SCALE_FACTOR))
            .to_i64()
            .unwrap_or(0);

        // Update CVD series bucket
        match side {
            TradeSide::Buy => {
                self.buy_volume += quantity;
                self.cvd_series[bucket_index] += scaled_qty;
            }
            TradeSide::Sell => {
                self.sell_volume += quantity;
                self.cvd_series[bucket_index] -= scaled_qty;
            }
        }

        // Check if this is a large trade
        let trade_value = price * quantity;
        let is_large = trade_value >= self.large_threshold;
        if is_large {
            self.large_trade_count += 1;
        }

        // Push to trade log
        self.trade_log.push_back(TradeLogEntry {
            timestamp: now,
            quantity,
            side,
            is_large,
            bucket_index,
        });
    }

    /// Expire trade entries older than the rolling window.
    ///
    /// Subtracts each expired entry's contribution from buy/sell volume,
    /// large trade count, and CVD series bucket.
    fn expire_old(&mut self, now: Instant) {
        while let Some(entry) = self.trade_log.front() {
            if now.duration_since(entry.timestamp) > self.window {
                let entry = self.trade_log.pop_front().unwrap();

                let scaled_qty = (entry.quantity * Decimal::from(CVD_SCALE_FACTOR))
                    .to_i64()
                    .unwrap_or(0);

                match entry.side {
                    TradeSide::Buy => {
                        self.buy_volume -= entry.quantity;
                        self.cvd_series[entry.bucket_index] -= scaled_qty;
                    }
                    TradeSide::Sell => {
                        self.sell_volume -= entry.quantity;
                        self.cvd_series[entry.bucket_index] += scaled_qty;
                    }
                }

                if entry.is_large {
                    self.large_trade_count = self.large_trade_count.saturating_sub(1);
                }
            } else {
                break;
            }
        }
    }

    /// Convert the CVD time series to sparkline-compatible u64 data.
    ///
    /// Shifts all i64 values up so the minimum becomes 0, then converts to u64.
    /// The series is returned in temporal order starting from the oldest visible bucket.
    pub fn cvd_as_sparkline_data(&self) -> Vec<u64> {
        if self.cvd_series.is_empty() {
            return vec![];
        }

        // Determine the current bucket index for temporal ordering
        let current_bucket = if let Some(start) = self.window_start {
            let elapsed = Instant::now().duration_since(start);
            (elapsed.as_secs() as usize) % CVD_BUCKET_COUNT
        } else {
            0
        };

        // Rotate series so oldest bucket is first, newest is last
        // The bucket after current_bucket is the oldest
        let oldest_bucket = (current_bucket + 1) % CVD_BUCKET_COUNT;
        let mut ordered = Vec::with_capacity(CVD_BUCKET_COUNT);
        for i in 0..CVD_BUCKET_COUNT {
            let idx = (oldest_bucket + i) % CVD_BUCKET_COUNT;
            ordered.push(self.cvd_series[idx]);
        }

        // Shift all values so minimum becomes 0
        let min_val = *ordered.iter().min().unwrap_or(&0);
        ordered.iter().map(|v| (v - min_val) as u64).collect()
    }

    /// Get total buy volume in the rolling window.
    pub fn buy_volume(&self) -> Decimal {
        self.buy_volume
    }

    /// Get total sell volume in the rolling window.
    pub fn sell_volume(&self) -> Decimal {
        self.sell_volume
    }

    /// Get net delta (buy_volume - sell_volume).
    #[allow(dead_code)]
    pub fn net_delta(&self) -> Decimal {
        self.buy_volume - self.sell_volume
    }

    /// Get count of large trades in the rolling window.
    pub fn large_trade_count(&self) -> u32 {
        self.large_trade_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_new_trade_stats_empty() {
        let stats = TradeStats::new(dec!(10000));
        assert_eq!(stats.buy_volume(), Decimal::ZERO);
        assert_eq!(stats.sell_volume(), Decimal::ZERO);
        assert_eq!(stats.net_delta(), Decimal::ZERO);
        assert_eq!(stats.large_trade_count(), 0);
        assert_eq!(stats.cvd_series.len(), 60);
    }

    #[test]
    fn test_record_buy_trade() {
        let mut stats = TradeStats::new(dec!(10000));
        stats.record_trade(dec!(50000), dec!(1.5), TradeSide::Buy);

        assert_eq!(stats.buy_volume(), dec!(1.5));
        assert_eq!(stats.sell_volume(), Decimal::ZERO);
        assert_eq!(stats.net_delta(), dec!(1.5));
    }

    #[test]
    fn test_record_sell_trade() {
        let mut stats = TradeStats::new(dec!(10000));
        stats.record_trade(dec!(50000), dec!(2.0), TradeSide::Sell);

        assert_eq!(stats.buy_volume(), Decimal::ZERO);
        assert_eq!(stats.sell_volume(), dec!(2.0));
        assert_eq!(stats.net_delta(), dec!(-2.0));
    }

    #[test]
    fn test_record_both_sides() {
        let mut stats = TradeStats::new(dec!(10000));
        stats.record_trade(dec!(50000), dec!(1.0), TradeSide::Buy);
        stats.record_trade(dec!(50000), dec!(0.5), TradeSide::Sell);
        stats.record_trade(dec!(50000), dec!(0.3), TradeSide::Buy);

        assert_eq!(stats.buy_volume(), dec!(1.3));
        assert_eq!(stats.sell_volume(), dec!(0.5));
        assert_eq!(stats.net_delta(), dec!(0.8));
    }

    #[test]
    fn test_large_trade_count() {
        let mut stats = TradeStats::new(dec!(10000));

        // Small trade: 100 * 1 = 100 (below threshold)
        stats.record_trade(dec!(100), dec!(1.0), TradeSide::Buy);
        assert_eq!(stats.large_trade_count(), 0);

        // Large trade: 50000 * 0.5 = 25000 (above threshold)
        stats.record_trade(dec!(50000), dec!(0.5), TradeSide::Buy);
        assert_eq!(stats.large_trade_count(), 1);

        // Another large trade
        stats.record_trade(dec!(50000), dec!(1.0), TradeSide::Sell);
        assert_eq!(stats.large_trade_count(), 2);
    }

    #[test]
    fn test_cvd_sparkline_data_all_zeros() {
        let stats = TradeStats::new(dec!(10000));
        let data = stats.cvd_as_sparkline_data();
        assert_eq!(data.len(), 60);
        assert!(data.iter().all(|&v| v == 0));
    }

    #[test]
    fn test_cvd_sparkline_data_shifts_minimum() {
        let mut stats = TradeStats::new(dec!(10000));

        // Record a buy (positive delta) and a sell (negative delta)
        stats.record_trade(dec!(50000), dec!(1.0), TradeSide::Buy);
        stats.record_trade(dec!(50000), dec!(2.0), TradeSide::Sell);

        let data = stats.cvd_as_sparkline_data();
        assert_eq!(data.len(), 60);

        // All values are u64 (non-negative by type), verify max > 0 when there's data
        let max_val = *data.iter().max().unwrap();
        assert!(max_val > 0); // at least one non-zero bucket from the buy trade

        // The minimum value should be 0
        assert_eq!(*data.iter().min().unwrap(), 0);
    }

    #[test]
    fn test_cvd_sparkline_data_length() {
        let stats = TradeStats::new(dec!(10000));
        let data = stats.cvd_as_sparkline_data();
        assert_eq!(data.len(), CVD_BUCKET_COUNT);
    }

    #[test]
    fn test_net_delta() {
        let mut stats = TradeStats::new(dec!(10000));
        stats.record_trade(dec!(50000), dec!(3.0), TradeSide::Buy);
        stats.record_trade(dec!(50000), dec!(1.0), TradeSide::Sell);
        assert_eq!(stats.net_delta(), dec!(2.0));
    }
}
