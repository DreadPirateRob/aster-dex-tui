//! Time-based candle aggregation using timestamp bucket boundaries
//!
//! TimeBarBuilder aggregates trades into time-based candles (1m, 5m, 15m).
//! Unlike TradeCountBarBuilder which closes candles after N trades,
//! TimeBarBuilder closes candles at interval boundaries.

use rust_decimal::Decimal;

use crate::config::Resolution;
use super::types::{Candle, Trade};

// Interval durations now provided by Resolution.interval_ms()

/// In-progress time candle being built
struct TimeCandle {
    bucket_start: u64,
    open: Decimal,
    high: Decimal,
    low: Decimal,
    close: Decimal,
    volume: Decimal,
    buy_volume: Decimal,
    sell_volume: Decimal,
    trade_count: u32,
}

impl TimeCandle {
    /// Create a new time candle from the first trade in the bucket
    fn new(bucket_start: u64, trade: &Trade) -> Self {
        let (buy_volume, sell_volume) = if trade.is_buyer_maker {
            // Buyer was maker -> taker was seller
            (Decimal::ZERO, trade.quantity)
        } else {
            // Buyer was taker -> aggressive buy
            (trade.quantity, Decimal::ZERO)
        };

        Self {
            bucket_start,
            open: trade.price,
            high: trade.price,
            low: trade.price,
            close: trade.price,
            volume: trade.quantity,
            buy_volume,
            sell_volume,
            trade_count: 1,
        }
    }

    /// Update the candle with a new trade in the same bucket
    fn update(&mut self, trade: &Trade) {
        self.high = self.high.max(trade.price);
        self.low = self.low.min(trade.price);
        self.close = trade.price;
        self.volume += trade.quantity;
        self.trade_count += 1;

        // Track buy/sell volume based on trade aggressor
        if trade.is_buyer_maker {
            // Buyer was maker -> taker was seller
            self.sell_volume += trade.quantity;
        } else {
            // Buyer was taker -> aggressive buy
            self.buy_volume += trade.quantity;
        }
    }

    /// Finalize the candle for output
    fn finalize(self, interval_ms: u64) -> Candle {
        Candle {
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
            buy_volume: self.buy_volume,
            sell_volume: self.sell_volume,
            trade_count: self.trade_count,
            open_time: self.bucket_start,
            // close_time is the last millisecond of the interval
            close_time: self.bucket_start + interval_ms - 1,
        }
    }
}

/// Builds candles from a stream of trades based on time interval boundaries
///
/// Each candle covers a fixed time interval (1m, 5m, 15m).
/// When a trade arrives in a new bucket, the previous bucket is finalized
/// and returned as a completed candle.
pub struct TimeBarBuilder {
    _resolution: Resolution,
    interval_ms: u64,
    current: Option<TimeCandle>,
}

impl TimeBarBuilder {
    /// Create a new builder with the specified resolution
    pub fn new(resolution: Resolution) -> Self {
        Self {
            interval_ms: resolution.interval_ms(),
            _resolution: resolution,
            current: None,
        }
    }

    /// Add a trade to the builder
    ///
    /// Returns Some(Candle) when a bucket is finalized by a new trade arriving
    /// in a different (newer) bucket. Late trades (in older buckets) are ignored.
    pub fn add_trade(&mut self, trade: &Trade) -> Option<Candle> {
        // Calculate which bucket this trade belongs to
        let trade_bucket = (trade.timestamp / self.interval_ms) * self.interval_ms;

        match &mut self.current {
            None => {
                // First trade - start new bucket
                self.current = Some(TimeCandle::new(trade_bucket, trade));
                None
            }
            Some(current) => {
                use std::cmp::Ordering;
                match trade_bucket.cmp(&current.bucket_start) {
                    Ordering::Equal => {
                        // Same bucket - update in progress candle
                        current.update(trade);
                        None
                    }
                    Ordering::Greater => {
                        // New bucket - finalize previous and start new
                        let completed = self.current.take().unwrap().finalize(self.interval_ms);
                        self.current = Some(TimeCandle::new(trade_bucket, trade));
                        Some(completed)
                    }
                    Ordering::Less => {
                        // Late trade (older bucket) - ignore
                        // Per plan: log at debug level (using tracing when integrated)
                        None
                    }
                }
            }
        }
    }

    /// Get the start timestamp of the current bucket being built
    pub fn current_bucket_start(&self) -> Option<u64> {
        self.current.as_ref().map(|c| c.bucket_start)
    }

    /// Get a view of the current in-progress candle (if any)
    pub fn current_partial(&self) -> Option<super::candle::PartialCandle> {
        self.current.as_ref().map(|c| super::candle::PartialCandle {
            open: c.open,
            high: c.high,
            low: c.low,
            close: c.close,
            volume: c.volume,
            buy_volume: c.buy_volume,
            sell_volume: c.sell_volume,
            trade_count: c.trade_count,
            threshold: 0, // time-based candles are bounded by time, not count
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::TradeSide;
    use rust_decimal_macros::dec;

    /// Helper to create a trade with the specified parameters
    fn make_trade(id: u64, price: Decimal, quantity: Decimal, timestamp: u64, is_buyer_maker: bool) -> Trade {
        Trade {
            id,
            price,
            quantity,
            timestamp,
            side: if is_buyer_maker { TradeSide::Sell } else { TradeSide::Buy },
            is_buyer_maker,
        }
    }

    /// Interval constants for reference
    const ONE_MINUTE_MS: u64 = 60_000;
    const FIVE_MINUTES_MS: u64 = 300_000;
    const FIFTEEN_MINUTES_MS: u64 = 900_000;

    #[test]
    fn test_first_trade_creates_bucket() {
        // First trade should start a new bucket, return None (no completed candle yet)
        let mut builder = TimeBarBuilder::new(Resolution::OneMinute);

        // Trade at 12:00:30 (30 seconds into the minute)
        // Bucket should start at 12:00:00 (aligned to minute boundary)
        let timestamp = 1700000000000 + 30_000; // 30 seconds past some minute
        let trade = make_trade(1, dec!(50000), dec!(1.0), timestamp, false);

        let result = builder.add_trade(&trade);

        assert!(result.is_none(), "First trade should not complete a candle");

        // Verify bucket started at aligned time
        let expected_bucket_start = (timestamp / ONE_MINUTE_MS) * ONE_MINUTE_MS;
        assert_eq!(
            builder.current_bucket_start(),
            Some(expected_bucket_start),
            "Bucket should be aligned to minute boundary"
        );
    }

    #[test]
    fn test_same_bucket_updates_ohlcv() {
        // Multiple trades in same bucket should update OHLCV correctly
        let mut builder = TimeBarBuilder::new(Resolution::OneMinute);

        // Use a timestamp that is exactly on a minute boundary
        // 28333333 * 60000 = 1699999980000 (minute-aligned)
        let bucket_start = 1699999980000u64;

        // Trade 1: Open price 100
        let trade1 = make_trade(1, dec!(100), dec!(1.0), bucket_start + 1000, false);
        builder.add_trade(&trade1);

        // Trade 2: Higher price (new high)
        let trade2 = make_trade(2, dec!(105), dec!(2.0), bucket_start + 2000, false);
        builder.add_trade(&trade2);

        // Trade 3: Lower price (new low)
        let trade3 = make_trade(3, dec!(95), dec!(1.5), bucket_start + 3000, false);
        builder.add_trade(&trade3);

        // Trade 4: Close price
        let trade4 = make_trade(4, dec!(102), dec!(0.5), bucket_start + 4000, false);
        let result = builder.add_trade(&trade4);

        // Still same bucket, no candle completed
        assert!(result.is_none(), "Should not complete candle within same bucket");

        // Now complete the bucket by adding a trade in the NEXT bucket
        let next_bucket_trade = make_trade(5, dec!(103), dec!(1.0), bucket_start + ONE_MINUTE_MS + 1000, false);
        let candle = builder.add_trade(&next_bucket_trade)
            .expect("Should complete previous bucket");

        // Verify OHLCV
        assert_eq!(candle.open, dec!(100), "Open should be first trade price");
        assert_eq!(candle.high, dec!(105), "High should be max price");
        assert_eq!(candle.low, dec!(95), "Low should be min price");
        assert_eq!(candle.close, dec!(102), "Close should be last trade price");
        assert_eq!(candle.volume, dec!(5.0), "Volume should be sum: 1.0 + 2.0 + 1.5 + 0.5");
        assert_eq!(candle.trade_count, 4, "Should have 4 trades");
    }

    #[test]
    fn test_new_bucket_finalizes_previous() {
        // When a trade arrives in a new bucket, previous bucket should be finalized
        let mut builder = TimeBarBuilder::new(Resolution::OneMinute);

        // Use minute-aligned timestamps
        let bucket1_start = 1699999980000u64; // 28333333 * 60000
        let bucket2_start = bucket1_start + ONE_MINUTE_MS;

        // Add trade to bucket 1
        let trade1 = make_trade(1, dec!(100), dec!(1.0), bucket1_start + 10_000, false);
        assert!(builder.add_trade(&trade1).is_none());

        // Add trade to bucket 2 - should finalize bucket 1
        let trade2 = make_trade(2, dec!(101), dec!(2.0), bucket2_start + 5_000, false);
        let candle = builder.add_trade(&trade2)
            .expect("Trade in new bucket should finalize previous");

        // Verify the finalized candle is from bucket 1
        assert_eq!(candle.open_time, bucket1_start);
        assert_eq!(candle.close_time, bucket1_start + ONE_MINUTE_MS - 1);
        assert_eq!(candle.open, dec!(100));
        assert_eq!(candle.close, dec!(100)); // Only one trade
        assert_eq!(candle.trade_count, 1);
    }

    #[test]
    fn test_late_trade_ignored() {
        // Trades with timestamps in previous buckets should be ignored
        let mut builder = TimeBarBuilder::new(Resolution::OneMinute);

        // Use minute-aligned timestamps
        let bucket1_start = 1699999980000u64; // 28333333 * 60000
        let bucket2_start = bucket1_start + ONE_MINUTE_MS;

        // Add trade to bucket 1
        let trade1 = make_trade(1, dec!(100), dec!(1.0), bucket1_start + 30_000, false);
        builder.add_trade(&trade1);

        // Add trade to bucket 2 (finalizes bucket 1)
        let trade2 = make_trade(2, dec!(101), dec!(2.0), bucket2_start + 5_000, false);
        let _candle1 = builder.add_trade(&trade2).expect("Should finalize bucket 1");

        // Now try to add a late trade that belongs to bucket 1 (already closed)
        let late_trade = make_trade(3, dec!(99), dec!(0.5), bucket1_start + 40_000, false);
        let result = builder.add_trade(&late_trade);

        // Late trade should be ignored (return None, not corrupt current bucket)
        assert!(result.is_none(), "Late trade should return None");

        // Current bucket should still be bucket 2
        assert_eq!(builder.current_bucket_start(), Some(bucket2_start));
    }

    #[test]
    fn test_bucket_alignment() {
        // Test that bucket_start is correctly aligned for different resolutions

        // 1-minute alignment
        let mut builder_1m = TimeBarBuilder::new(Resolution::OneMinute);
        let ts_1m = 1700000045000u64; // 45 seconds into a minute
        let trade_1m = make_trade(1, dec!(100), dec!(1.0), ts_1m, false);
        builder_1m.add_trade(&trade_1m);

        let expected_1m = (ts_1m / ONE_MINUTE_MS) * ONE_MINUTE_MS;
        assert_eq!(builder_1m.current_bucket_start(), Some(expected_1m));

        // 5-minute alignment
        let mut builder_5m = TimeBarBuilder::new(Resolution::FiveMinutes);
        let ts_5m = 1700000180000u64; // 3 minutes into a 5-minute period
        let trade_5m = make_trade(2, dec!(100), dec!(1.0), ts_5m, false);
        builder_5m.add_trade(&trade_5m);

        let expected_5m = (ts_5m / FIVE_MINUTES_MS) * FIVE_MINUTES_MS;
        assert_eq!(builder_5m.current_bucket_start(), Some(expected_5m));

        // 15-minute alignment
        let mut builder_15m = TimeBarBuilder::new(Resolution::FifteenMinutes);
        let ts_15m = 1700000600000u64; // 10 minutes into a 15-minute period
        let trade_15m = make_trade(3, dec!(100), dec!(1.0), ts_15m, false);
        builder_15m.add_trade(&trade_15m);

        let expected_15m = (ts_15m / FIFTEEN_MINUTES_MS) * FIFTEEN_MINUTES_MS;
        assert_eq!(builder_15m.current_bucket_start(), Some(expected_15m));
    }

    #[test]
    fn test_buy_sell_volume_tracking() {
        // Verify buy/sell volume based on is_buyer_maker
        let mut builder = TimeBarBuilder::new(Resolution::OneMinute);

        // Use minute-aligned timestamp
        let bucket_start = 1699999980000u64;

        // Buy trade (is_buyer_maker=false, taker is buyer)
        let buy_trade = make_trade(1, dec!(100), dec!(1.0), bucket_start + 1000, false);
        builder.add_trade(&buy_trade);

        // Sell trade (is_buyer_maker=true, taker is seller)
        let sell_trade = make_trade(2, dec!(101), dec!(2.5), bucket_start + 2000, true);
        builder.add_trade(&sell_trade);

        // Another buy trade
        let buy_trade2 = make_trade(3, dec!(100.5), dec!(0.5), bucket_start + 3000, false);
        builder.add_trade(&buy_trade2);

        // Complete bucket with next bucket trade
        let next_trade = make_trade(4, dec!(102), dec!(1.0), bucket_start + ONE_MINUTE_MS + 1000, false);
        let candle = builder.add_trade(&next_trade)
            .expect("Should finalize bucket");

        assert_eq!(candle.buy_volume, dec!(1.5), "Buy volume: 1.0 + 0.5");
        assert_eq!(candle.sell_volume, dec!(2.5), "Sell volume: 2.5");
        assert_eq!(candle.volume, dec!(4.0), "Total volume: 1.0 + 2.5 + 0.5");
    }

    #[test]
    fn test_builder_creates_with_resolution() {
        let builder = TimeBarBuilder::new(Resolution::FiveMinutes);
        // Verify builder works with the given resolution by checking interval behavior
        assert!(builder.current_bucket_start().is_none());
    }
}
