// src/data/trade_volume_profile.rs
// Trade volume profile accumulator for DOM volume profile overlay.
// Maintains a rolling window of trade volume by price level, split by side (buy/sell).

use crate::data::types::TradeSide;
use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Rolling window duration for trade volume accumulation (5 minutes).
const TRADE_VOLUME_WINDOW: Duration = Duration::from_secs(300);

/// Buy and sell volume at a single price level.
#[derive(Debug, Clone, Copy, Default)]
struct SidedVolume {
    buy: Decimal,
    sell: Decimal,
}

/// Accumulates trade volume at each price level over a rolling time window,
/// split by trade side (buy vs sell).
///
/// Used by the DOM volume profile overlay in TradeVolume mode to show
/// where trading activity concentrates. Buy volume renders on the bid side,
/// sell volume on the ask side.
pub struct TradeVolumeProfile {
    /// Accumulated buy/sell volume at each price.
    volume_at_price: HashMap<Decimal, SidedVolume>,
    /// Trade log for expiry: (timestamp, price, quantity, side).
    trade_log: VecDeque<(Instant, Decimal, Decimal, TradeSide)>,
    /// Duration of the rolling window.
    window_duration: Duration,
}

impl TradeVolumeProfile {
    /// Create a new empty accumulator with 5-minute rolling window.
    pub fn new() -> Self {
        Self {
            volume_at_price: HashMap::new(),
            trade_log: VecDeque::new(),
            window_duration: TRADE_VOLUME_WINDOW,
        }
    }

    /// Record a trade at a given price, quantity, and side.
    ///
    /// Adds to the volume_at_price HashMap, pushes to trade_log for aging,
    /// and expires entries older than the window duration.
    pub fn record_trade(&mut self, price: Decimal, quantity: Decimal, side: TradeSide) {
        let now = Instant::now();

        // Add to accumulator by side
        let entry = self.volume_at_price.entry(price).or_default();
        match side {
            TradeSide::Buy => entry.buy += quantity,
            TradeSide::Sell => entry.sell += quantity,
        }

        // Push to trade log for future expiry
        self.trade_log.push_back((now, price, quantity, side));

        // Expire old entries from front of log
        self.expire_old(now);
    }

    /// Get accumulated buy volume at a specific price level.
    pub fn buy_volume_at(&self, price: &Decimal) -> Decimal {
        self.volume_at_price.get(price).map_or(Decimal::ZERO, |v| v.buy)
    }

    /// Get accumulated sell volume at a specific price level.
    pub fn sell_volume_at(&self, price: &Decimal) -> Decimal {
        self.volume_at_price.get(price).map_or(Decimal::ZERO, |v| v.sell)
    }

    /// Get the maximum volume across all price levels for a given side (for bar scaling).
    pub fn max_buy_volume(&self) -> Decimal {
        self.volume_at_price.values().map(|v| v.buy).max().unwrap_or(Decimal::ZERO)
    }

    pub fn max_sell_volume(&self) -> Decimal {
        self.volume_at_price.values().map(|v| v.sell).max().unwrap_or(Decimal::ZERO)
    }

    /// Get the maximum volume across both sides (for unified bar scaling).
    pub fn max_volume(&self) -> Decimal {
        self.max_buy_volume().max(self.max_sell_volume())
    }

    /// Expire trade entries older than window_duration.
    fn expire_old(&mut self, now: Instant) {
        while let Some(&(ts, price, qty, side)) = self.trade_log.front() {
            if now.duration_since(ts) > self.window_duration {
                self.trade_log.pop_front();
                // Subtract expired trade from accumulator
                if let Some(vol) = self.volume_at_price.get_mut(&price) {
                    match side {
                        TradeSide::Buy => {
                            vol.buy -= qty;
                        }
                        TradeSide::Sell => {
                            vol.sell -= qty;
                        }
                    }
                    if vol.buy <= Decimal::ZERO && vol.sell <= Decimal::ZERO {
                        self.volume_at_price.remove(&price);
                    }
                }
            } else {
                break;
            }
        }
    }
}

impl Default for TradeVolumeProfile {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_new_profile_empty() {
        let profile = TradeVolumeProfile::new();
        assert_eq!(profile.buy_volume_at(&dec!(100)), Decimal::ZERO);
        assert_eq!(profile.sell_volume_at(&dec!(100)), Decimal::ZERO);
        assert_eq!(profile.max_volume(), Decimal::ZERO);
    }

    #[test]
    fn test_record_buy_trade() {
        let mut profile = TradeVolumeProfile::new();
        profile.record_trade(dec!(100.50), dec!(1.5), TradeSide::Buy);
        assert_eq!(profile.buy_volume_at(&dec!(100.50)), dec!(1.5));
        assert_eq!(profile.sell_volume_at(&dec!(100.50)), Decimal::ZERO);
        assert_eq!(profile.max_buy_volume(), dec!(1.5));
        assert_eq!(profile.max_sell_volume(), Decimal::ZERO);
    }

    #[test]
    fn test_record_sell_trade() {
        let mut profile = TradeVolumeProfile::new();
        profile.record_trade(dec!(100.50), dec!(2.0), TradeSide::Sell);
        assert_eq!(profile.buy_volume_at(&dec!(100.50)), Decimal::ZERO);
        assert_eq!(profile.sell_volume_at(&dec!(100.50)), dec!(2.0));
    }

    #[test]
    fn test_record_both_sides_same_price() {
        let mut profile = TradeVolumeProfile::new();
        profile.record_trade(dec!(100.50), dec!(1.0), TradeSide::Buy);
        profile.record_trade(dec!(100.50), dec!(2.0), TradeSide::Sell);
        profile.record_trade(dec!(100.50), dec!(0.5), TradeSide::Buy);
        assert_eq!(profile.buy_volume_at(&dec!(100.50)), dec!(1.5));
        assert_eq!(profile.sell_volume_at(&dec!(100.50)), dec!(2.0));
    }

    #[test]
    fn test_max_volume_across_sides() {
        let mut profile = TradeVolumeProfile::new();
        profile.record_trade(dec!(100.00), dec!(1.0), TradeSide::Buy);
        profile.record_trade(dec!(101.00), dec!(3.0), TradeSide::Sell);
        profile.record_trade(dec!(102.00), dec!(2.0), TradeSide::Buy);
        assert_eq!(profile.max_buy_volume(), dec!(2.0));
        assert_eq!(profile.max_sell_volume(), dec!(3.0));
        assert_eq!(profile.max_volume(), dec!(3.0));
    }

    #[test]
    fn test_missing_price_returns_zero() {
        let profile = TradeVolumeProfile::new();
        assert_eq!(profile.buy_volume_at(&dec!(999)), Decimal::ZERO);
        assert_eq!(profile.sell_volume_at(&dec!(999)), Decimal::ZERO);
    }
}
