// src/data/ticker.rs
// Data types for the market overview ticker widget.
//
// TickerEntry stores per-symbol 24hr ticker data.
// TickerTable accumulates entries in a HashMap for O(1) upsert
// with sorted output for rendering.

use rust_decimal::Decimal;
use std::collections::HashMap;

/// Per-symbol 24hr ticker data from REST or WebSocket stream.
#[derive(Debug, Clone)]
pub struct TickerEntry {
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Last traded price
    pub last_price: Decimal,
    /// Absolute 24h price change
    #[allow(dead_code)]
    pub price_change: Decimal,
    /// 24h price change percentage (e.g., 2.34 means +2.34%)
    pub price_change_percent: Decimal,
    /// 24h high price
    pub high_price: Decimal,
    /// 24h low price
    pub low_price: Decimal,
    /// 24h quote volume (USD)
    pub quote_volume: Decimal,
    /// 24h trade count
    pub trade_count: u64,
}

/// Sort column for the ticker table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickerSortColumn {
    Symbol,
    Price,
    Change,
    Volume,
    Trades,
}

impl TickerSortColumn {
    /// Cycle to the next sort column.
    pub fn cycle(self) -> Self {
        match self {
            TickerSortColumn::Symbol => TickerSortColumn::Price,
            TickerSortColumn::Price => TickerSortColumn::Change,
            TickerSortColumn::Change => TickerSortColumn::Volume,
            TickerSortColumn::Volume => TickerSortColumn::Trades,
            TickerSortColumn::Trades => TickerSortColumn::Symbol,
        }
    }

    /// Display label for the sort column.
    pub fn label(self) -> &'static str {
        match self {
            TickerSortColumn::Symbol => "Symbol",
            TickerSortColumn::Price => "Price",
            TickerSortColumn::Change => "24h %",
            TickerSortColumn::Volume => "Volume",
            TickerSortColumn::Trades => "Trades",
        }
    }
}

/// Accumulator for 24hr ticker data across all trading pairs.
///
/// Uses a HashMap keyed by symbol for O(1) upsert from the ticker stream.
/// Sorted output is computed on demand for rendering.
pub struct TickerTable {
    entries: HashMap<String, TickerEntry>,
}

impl TickerTable {
    /// Create a new empty table.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Insert or update an entry by symbol.
    pub fn upsert(&mut self, entry: TickerEntry) {
        self.entries.insert(entry.symbol.clone(), entry);
    }

    /// Return entries sorted by the given column and direction.
    ///
    /// For Change column, sorts by absolute value of price_change_percent
    /// (most volatile first when descending).
    pub fn sorted_entries(
        &self,
        sort_column: &TickerSortColumn,
        ascending: bool,
    ) -> Vec<&TickerEntry> {
        let mut entries: Vec<&TickerEntry> = self.entries.values().collect();

        entries.sort_by(|a, b| {
            let cmp = match sort_column {
                TickerSortColumn::Symbol => a.symbol.cmp(&b.symbol),
                TickerSortColumn::Price => a.last_price.cmp(&b.last_price),
                TickerSortColumn::Change => {
                    a.price_change_percent
                        .abs()
                        .cmp(&b.price_change_percent.abs())
                }
                TickerSortColumn::Volume => a.quote_volume.cmp(&b.quote_volume),
                TickerSortColumn::Trades => a.trade_count.cmp(&b.trade_count),
            };

            if ascending {
                cmp
            } else {
                cmp.reverse()
            }
        });

        entries
    }

    /// Number of entries in the table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_upsert_and_sorted_entries_by_volume() {
        let mut table = TickerTable::new();

        table.upsert(TickerEntry {
            symbol: "BTCUSDT".to_string(),
            last_price: Decimal::from_str("50000.00").unwrap(),
            price_change: Decimal::from_str("500.00").unwrap(),
            price_change_percent: Decimal::from_str("1.01").unwrap(),
            high_price: Decimal::from_str("51000.00").unwrap(),
            low_price: Decimal::from_str("49000.00").unwrap(),
            quote_volume: Decimal::from_str("5000000000").unwrap(), // 5B
            trade_count: 1_000_000,
        });

        table.upsert(TickerEntry {
            symbol: "ETHUSDT".to_string(),
            last_price: Decimal::from_str("3000.00").unwrap(),
            price_change: Decimal::from_str("-60.00").unwrap(),
            price_change_percent: Decimal::from_str("-2.00").unwrap(),
            high_price: Decimal::from_str("3100.00").unwrap(),
            low_price: Decimal::from_str("2900.00").unwrap(),
            quote_volume: Decimal::from_str("2000000000").unwrap(), // 2B
            trade_count: 500_000,
        });

        table.upsert(TickerEntry {
            symbol: "SOLUSDT".to_string(),
            last_price: Decimal::from_str("100.00").unwrap(),
            price_change: Decimal::from_str("5.00").unwrap(),
            price_change_percent: Decimal::from_str("5.26").unwrap(),
            high_price: Decimal::from_str("105.00").unwrap(),
            low_price: Decimal::from_str("95.00").unwrap(),
            quote_volume: Decimal::from_str("800000000").unwrap(), // 800M
            trade_count: 200_000,
        });

        assert_eq!(table.len(), 3);

        // Sort by volume descending (default) — most liquid first
        let sorted = table.sorted_entries(&TickerSortColumn::Volume, false);
        assert_eq!(sorted[0].symbol, "BTCUSDT"); // 5B
        assert_eq!(sorted[1].symbol, "ETHUSDT"); // 2B
        assert_eq!(sorted[2].symbol, "SOLUSDT"); // 800M

        // Sort by symbol ascending
        let sorted = table.sorted_entries(&TickerSortColumn::Symbol, true);
        assert_eq!(sorted[0].symbol, "BTCUSDT");
        assert_eq!(sorted[1].symbol, "ETHUSDT");
        assert_eq!(sorted[2].symbol, "SOLUSDT");

        // Sort by change descending (absolute value — most volatile first)
        let sorted = table.sorted_entries(&TickerSortColumn::Change, false);
        assert_eq!(sorted[0].symbol, "SOLUSDT"); // 5.26% abs
        assert_eq!(sorted[1].symbol, "ETHUSDT"); // 2.00% abs
        assert_eq!(sorted[2].symbol, "BTCUSDT"); // 1.01% abs
    }

    #[test]
    fn test_upsert_overwrites_existing() {
        let mut table = TickerTable::new();

        table.upsert(TickerEntry {
            symbol: "BTCUSDT".to_string(),
            last_price: Decimal::from_str("50000.00").unwrap(),
            price_change: Decimal::from_str("500.00").unwrap(),
            price_change_percent: Decimal::from_str("1.01").unwrap(),
            high_price: Decimal::from_str("51000.00").unwrap(),
            low_price: Decimal::from_str("49000.00").unwrap(),
            quote_volume: Decimal::from_str("5000000000").unwrap(),
            trade_count: 1_000_000,
        });

        // Upsert same symbol with new data
        table.upsert(TickerEntry {
            symbol: "BTCUSDT".to_string(),
            last_price: Decimal::from_str("51000.00").unwrap(),
            price_change: Decimal::from_str("1500.00").unwrap(),
            price_change_percent: Decimal::from_str("3.03").unwrap(),
            high_price: Decimal::from_str("52000.00").unwrap(),
            low_price: Decimal::from_str("49500.00").unwrap(),
            quote_volume: Decimal::from_str("6000000000").unwrap(),
            trade_count: 1_200_000,
        });

        assert_eq!(table.len(), 1);
        let sorted = table.sorted_entries(&TickerSortColumn::Symbol, true);
        assert_eq!(
            sorted[0].last_price,
            Decimal::from_str("51000.00").unwrap()
        );
        assert_eq!(
            sorted[0].price_change_percent,
            Decimal::from_str("3.03").unwrap()
        );
    }

    #[test]
    fn test_sort_column_cycle() {
        assert_eq!(TickerSortColumn::Symbol.cycle(), TickerSortColumn::Price);
        assert_eq!(TickerSortColumn::Price.cycle(), TickerSortColumn::Change);
        assert_eq!(TickerSortColumn::Change.cycle(), TickerSortColumn::Volume);
        assert_eq!(TickerSortColumn::Volume.cycle(), TickerSortColumn::Trades);
        assert_eq!(TickerSortColumn::Trades.cycle(), TickerSortColumn::Symbol);
    }
}
