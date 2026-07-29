//! Candle aggregation and storage for trade-count bars

use std::collections::VecDeque;

use rust_decimal::Decimal;

use super::types::{Candle, Trade};

/// In-progress candle being built from trades
struct CandleInProgress {
    open: Decimal,
    high: Decimal,
    low: Decimal,
    close: Decimal,
    volume: Decimal,
    /// Taker buy volume (buyer was aggressor, is_buyer_maker=false)
    buy_volume: Decimal,
    /// Taker sell volume (seller was aggressor, is_buyer_maker=true)
    sell_volume: Decimal,
    trade_count: u32,
    open_time: u64,
    close_time: u64,
}

/// Builds candles from a stream of trades based on trade count threshold
///
/// Each completed candle contains exactly `threshold` trades.
/// OHLCV values are calculated correctly:
/// - Open = price of first trade
/// - High = max price across all trades
/// - Low = min price across all trades
/// - Close = price of last trade
/// - Volume = sum of all trade quantities
pub struct TradeCountBarBuilder {
    threshold: u32,
    current: Option<CandleInProgress>,
}

impl TradeCountBarBuilder {
    /// Create a new builder with the specified trade count threshold
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold,
            current: None,
        }
    }

    /// Add a trade to the builder
    ///
    /// Returns Some(Candle) when threshold is reached, None otherwise.
    /// After returning a candle, the builder resets for the next candle.
    pub fn add_trade(&mut self, trade: &Trade) -> Option<Candle> {
        let candle = self.current.get_or_insert(CandleInProgress {
            open: trade.price,
            high: trade.price,
            low: trade.price,
            close: trade.price,
            volume: Decimal::ZERO,
            buy_volume: Decimal::ZERO,
            sell_volume: Decimal::ZERO,
            trade_count: 0,
            open_time: trade.timestamp,
            close_time: trade.timestamp,
        });

        // Update OHLCV values
        candle.high = candle.high.max(trade.price);
        candle.low = candle.low.min(trade.price);
        candle.close = trade.price;
        candle.volume += trade.quantity;

        // Track buy/sell volume based on trade aggressor
        if trade.is_buyer_maker {
            // Buyer was maker (passive) -> Seller was taker (aggressive sell)
            candle.sell_volume += trade.quantity;
        } else {
            // Buyer was taker (aggressive buy)
            candle.buy_volume += trade.quantity;
        }

        candle.trade_count += 1;
        candle.close_time = trade.timestamp;

        // Use == not >= to prevent boundary skipping (per research pitfalls)
        if candle.trade_count == self.threshold {
            let completed = self.current.take().unwrap();
            Some(Candle {
                open: completed.open,
                high: completed.high,
                low: completed.low,
                close: completed.close,
                volume: completed.volume,
                buy_volume: completed.buy_volume,
                sell_volume: completed.sell_volume,
                trade_count: completed.trade_count,
                open_time: completed.open_time,
                close_time: completed.close_time,
            })
        } else {
            None
        }
    }

    /// Get a view of the current in-progress candle (if any)
    pub fn current_partial(&self) -> Option<PartialCandle> {
        self.current.as_ref().map(|c| PartialCandle {
            open: c.open,
            high: c.high,
            low: c.low,
            close: c.close,
            volume: c.volume,
            buy_volume: c.buy_volume,
            sell_volume: c.sell_volume,
            trade_count: c.trade_count,
            threshold: self.threshold,
        })
    }
}

/// View into the in-progress candle for display purposes
#[derive(Debug, Clone, Copy)]
pub struct PartialCandle {
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
    /// Taker buy volume (buyer was aggressor, is_buyer_maker=false)
    pub buy_volume: Decimal,
    /// Taker sell volume (seller was aggressor, is_buyer_maker=true)
    pub sell_volume: Decimal,
    pub trade_count: u32,
    pub threshold: u32,
}

/// Bounded storage for completed candles
///
/// Evicts oldest candle when capacity is exceeded.
pub struct CandleStore {
    candles: VecDeque<Candle>,
    max_capacity: usize,
}

impl CandleStore {
    /// Create a new store with the specified maximum capacity
    pub fn new(max_capacity: usize) -> Self {
        Self {
            candles: VecDeque::with_capacity(max_capacity),
            max_capacity,
        }
    }

    /// Add a candle, evicting the oldest if at capacity
    pub fn push(&mut self, candle: Candle) {
        if self.candles.len() >= self.max_capacity {
            self.candles.pop_front();
        }
        self.candles.push_back(candle);
    }

    /// Iterate over all candles (oldest first)
    pub fn iter(&self) -> impl Iterator<Item = &Candle> {
        self.candles.iter()
    }

    /// Get the number of candles stored
    pub fn len(&self) -> usize {
        self.candles.len()
    }

    /// Update an existing candle or insert if not found
    ///
    /// Matches by open_time (bucket identifier). Used for REST reconciliation
    /// where REST is source of truth and should replace client-aggregated candles.
    ///
    /// Per CONTEXT.md decision: "Trust REST on reconciliation"
    pub fn update_or_insert(&mut self, candle: Candle) {
        // Find candle with matching open_time
        if let Some(existing) = self.candles.iter_mut().find(|c| c.open_time == candle.open_time) {
            // Replace with REST data (source of truth)
            *existing = candle;
        } else {
            // Not found - this might be a candle we missed during disconnect
            self.push(candle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::TradeSide;
    use rust_decimal_macros::dec;

    fn make_trade(id: u64, price: Decimal, quantity: Decimal, is_buyer_maker: bool) -> Trade {
        Trade {
            id,
            price,
            quantity,
            timestamp: 1000 + id,
            side: if is_buyer_maker {
                TradeSide::Sell
            } else {
                TradeSide::Buy
            },
            is_buyer_maker,
        }
    }

    #[test]
    fn test_buy_sell_volume_tracking() {
        let mut builder = TradeCountBarBuilder::new(3);

        // Trade 1: is_buyer_maker=false (taker buy) - quantity 1.0
        let trade1 = make_trade(1, dec!(50000), dec!(1.0), false);
        assert!(builder.add_trade(&trade1).is_none());

        // Trade 2: is_buyer_maker=true (taker sell) - quantity 2.0
        let trade2 = make_trade(2, dec!(50010), dec!(2.0), true);
        assert!(builder.add_trade(&trade2).is_none());

        // Verify partial candle has correct volumes
        let partial = builder.current_partial().unwrap();
        assert_eq!(partial.buy_volume, dec!(1.0));
        assert_eq!(partial.sell_volume, dec!(2.0));
        assert_eq!(partial.volume, dec!(3.0));

        // Trade 3: is_buyer_maker=false (taker buy) - quantity 0.5
        let trade3 = make_trade(3, dec!(50020), dec!(0.5), false);
        let candle = builder.add_trade(&trade3).expect("Should complete candle");

        // Verify completed candle has correct buy/sell volumes
        assert_eq!(candle.buy_volume, dec!(1.5)); // 1.0 + 0.5
        assert_eq!(candle.sell_volume, dec!(2.0));
        assert_eq!(candle.volume, dec!(3.5)); // 1.0 + 2.0 + 0.5
        assert_eq!(candle.trade_count, 3);
    }

    #[test]
    fn test_all_buy_trades() {
        let mut builder = TradeCountBarBuilder::new(2);

        let trade1 = make_trade(1, dec!(100), dec!(1.0), false); // buy
        let trade2 = make_trade(2, dec!(101), dec!(1.0), false); // buy

        builder.add_trade(&trade1);
        let candle = builder.add_trade(&trade2).unwrap();

        assert_eq!(candle.buy_volume, dec!(2.0));
        assert_eq!(candle.sell_volume, dec!(0));
    }

    #[test]
    fn test_all_sell_trades() {
        let mut builder = TradeCountBarBuilder::new(2);

        let trade1 = make_trade(1, dec!(100), dec!(1.0), true); // sell
        let trade2 = make_trade(2, dec!(99), dec!(1.0), true); // sell

        builder.add_trade(&trade1);
        let candle = builder.add_trade(&trade2).unwrap();

        assert_eq!(candle.buy_volume, dec!(0));
        assert_eq!(candle.sell_volume, dec!(2.0));
    }

    #[test]
    fn test_update_or_insert_updates_existing() {
        let mut store = CandleStore::new(10);

        let candle1 = Candle {
            open_time: 1000,
            close_time: 1059,
            open: dec!(100),
            high: dec!(110),
            low: dec!(95),
            close: dec!(105),
            volume: dec!(10),
            buy_volume: dec!(6),
            sell_volume: dec!(4),
            trade_count: 5,
        };
        store.push(candle1);

        // Update with REST candle (different values, same open_time)
        let rest_candle = Candle {
            open_time: 1000, // Same bucket
            close_time: 1059,
            open: dec!(100),
            high: dec!(115), // Higher high from REST
            low: dec!(90),   // Lower low from REST
            close: dec!(108),
            volume: dec!(15),
            buy_volume: dec!(8),
            sell_volume: dec!(7),
            trade_count: 8,
        };
        store.update_or_insert(rest_candle);

        // Should still have 1 candle
        assert_eq!(store.len(), 1);

        // Should have REST values
        let updated = store.iter().next().unwrap();
        assert_eq!(updated.high, dec!(115));
        assert_eq!(updated.low, dec!(90));
        assert_eq!(updated.trade_count, 8);
    }

    #[test]
    fn test_update_or_insert_inserts_new() {
        let mut store = CandleStore::new(10);

        let candle1 = Candle {
            open_time: 1000,
            close_time: 1059,
            open: dec!(100),
            high: dec!(110),
            low: dec!(95),
            close: dec!(105),
            volume: dec!(10),
            buy_volume: dec!(6),
            sell_volume: dec!(4),
            trade_count: 5,
        };
        store.push(candle1);

        // Insert candle with different open_time
        let new_candle = Candle {
            open_time: 2000, // Different bucket
            close_time: 2059,
            open: dec!(105),
            high: dec!(120),
            low: dec!(100),
            close: dec!(115),
            volume: dec!(20),
            buy_volume: dec!(12),
            sell_volume: dec!(8),
            trade_count: 10,
        };
        store.update_or_insert(new_candle);

        // Should have 2 candles
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_update_or_insert_respects_capacity() {
        let mut store = CandleStore::new(2); // Small capacity

        // Fill the store
        for i in 0..2 {
            store.push(Candle {
                open_time: (i * 1000) as u64,
                close_time: (i * 1000 + 59) as u64,
                open: dec!(100),
                high: dec!(110),
                low: dec!(95),
                close: dec!(105),
                volume: dec!(10),
                buy_volume: dec!(6),
                sell_volume: dec!(4),
                trade_count: 5,
            });
        }
        assert_eq!(store.len(), 2);

        // Insert new candle (should evict oldest)
        store.update_or_insert(Candle {
            open_time: 3000, // New bucket
            close_time: 3059,
            open: dec!(100),
            high: dec!(110),
            low: dec!(95),
            close: dec!(105),
            volume: dec!(10),
            buy_volume: dec!(6),
            sell_volume: dec!(4),
            trade_count: 5,
        });

        // Should still be at capacity
        assert_eq!(store.len(), 2);

        // Oldest (open_time=0) should be evicted
        let first = store.iter().next().unwrap();
        assert_eq!(first.open_time, 1000); // Not 0
    }
}
