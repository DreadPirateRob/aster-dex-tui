// src/data/trade_imbalance.rs
// Multi-window rolling trade imbalance accumulator for order flow imbalance panel.
// Tracks buy vs sell volume across 1m, 5m, and 15m rolling windows using a single
// shared trade log with per-window running sums.

use crate::data::types::TradeSide;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Rolling window durations for imbalance tracking.
const WINDOW_1M: Duration = Duration::from_secs(60);
const WINDOW_5M: Duration = Duration::from_secs(300);
const WINDOW_15M: Duration = Duration::from_secs(900);

/// Internal trade log entry for expiry tracking.
struct TradeLogEntry {
    timestamp: Instant,
    quantity: Decimal,
    side: TradeSide,
}

/// Snapshot of buy/sell imbalance for a single time window.
pub struct ImbalanceSnapshot {
    #[allow(dead_code)]
    pub buy_volume: Decimal,
    #[allow(dead_code)]
    pub sell_volume: Decimal,
    /// Ratio of buy volume to total volume (0.0 to 1.0). 0.5 when no data.
    pub bid_ratio: f64,
    /// Whether any trade data exists in this window.
    pub has_data: bool,
}

/// Multi-window rolling trade imbalance accumulator.
///
/// Tracks buy vs sell volume across three rolling time windows (1m, 5m, 15m)
/// using a single shared trade log. The 15m window uses incremental subtract
/// on expiry; 1m and 5m are recomputed from scratch after expiry for correctness
/// (entries age out of shorter windows while remaining in the 15m log).
pub struct TradeImbalance {
    /// Shared trade log ordered by timestamp (oldest at front).
    trade_log: VecDeque<TradeLogEntry>,
    /// Running buy volume per window: [1m, 5m, 15m].
    buy_volume: [Decimal; 3],
    /// Running sell volume per window: [1m, 5m, 15m].
    sell_volume: [Decimal; 3],
}

impl TradeImbalance {
    /// Create a new empty accumulator with zeroed state.
    pub fn new() -> Self {
        Self {
            trade_log: VecDeque::new(),
            buy_volume: [Decimal::ZERO; 3],
            sell_volume: [Decimal::ZERO; 3],
        }
    }

    /// Record a trade, updating all three window accumulators.
    pub fn record_trade(&mut self, quantity: Decimal, side: TradeSide) {
        let now = Instant::now();

        // Expire old entries first
        self.expire_old(now);

        // Add quantity to all 3 windows
        match side {
            TradeSide::Buy => {
                self.buy_volume[0] += quantity;
                self.buy_volume[1] += quantity;
                self.buy_volume[2] += quantity;
            }
            TradeSide::Sell => {
                self.sell_volume[0] += quantity;
                self.sell_volume[1] += quantity;
                self.sell_volume[2] += quantity;
            }
        }

        // Push to trade log
        self.trade_log.push_back(TradeLogEntry {
            timestamp: now,
            quantity,
            side,
        });
    }

    /// Expire entries older than 15m from the trade log, then recompute 1m and 5m
    /// sums from scratch for correctness.
    fn expire_old(&mut self, now: Instant) {
        // Pop entries older than 15m (longest window) from front,
        // subtracting from the 15m window running totals
        while let Some(front) = self.trade_log.front() {
            if now.duration_since(front.timestamp) > WINDOW_15M {
                let entry = self.trade_log.pop_front().unwrap();
                match entry.side {
                    TradeSide::Buy => {
                        self.buy_volume[2] -= entry.quantity;
                    }
                    TradeSide::Sell => {
                        self.sell_volume[2] -= entry.quantity;
                    }
                }
            } else {
                break;
            }
        }

        // Recompute 1m and 5m sums from scratch.
        // Entries that aged out of 1m/5m but are still in the 15m window would
        // have incorrect running totals if only using incremental subtract.
        // The trade log is bounded at ~15 min of data so full recompute is cheap.
        self.buy_volume[0] = Decimal::ZERO;
        self.sell_volume[0] = Decimal::ZERO;
        self.buy_volume[1] = Decimal::ZERO;
        self.sell_volume[1] = Decimal::ZERO;

        for entry in &self.trade_log {
            let age = now.duration_since(entry.timestamp);
            match entry.side {
                TradeSide::Buy => {
                    if age <= WINDOW_1M {
                        self.buy_volume[0] += entry.quantity;
                    }
                    if age <= WINDOW_5M {
                        self.buy_volume[1] += entry.quantity;
                    }
                }
                TradeSide::Sell => {
                    if age <= WINDOW_1M {
                        self.sell_volume[0] += entry.quantity;
                    }
                    if age <= WINDOW_5M {
                        self.sell_volume[1] += entry.quantity;
                    }
                }
            }
        }
    }

    /// Get imbalance snapshots for all three windows: [1m, 5m, 15m].
    pub fn snapshots(&self) -> [ImbalanceSnapshot; 3] {
        [
            Self::make_snapshot(self.buy_volume[0], self.sell_volume[0]),
            Self::make_snapshot(self.buy_volume[1], self.sell_volume[1]),
            Self::make_snapshot(self.buy_volume[2], self.sell_volume[2]),
        ]
    }

    /// Compute a single snapshot from buy and sell volumes.
    fn make_snapshot(buy: Decimal, sell: Decimal) -> ImbalanceSnapshot {
        let total = buy + sell;
        let has_data = total > Decimal::ZERO;
        let bid_ratio = if has_data {
            buy.to_f64().unwrap_or(0.5) / total.to_f64().unwrap_or(1.0)
        } else {
            0.5
        };
        ImbalanceSnapshot {
            buy_volume: buy,
            sell_volume: sell,
            bid_ratio,
            has_data,
        }
    }
}

impl Default for TradeImbalance {
    fn default() -> Self {
        Self::new()
    }
}
