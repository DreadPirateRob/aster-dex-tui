// src/data/funding_rates.rs
// Data types for the funding rate dashboard widget.
//
// FundingRateEntry stores per-symbol funding rate data.
// FundingRateTable accumulates entries in a HashMap for O(1) upsert
// with sorted output for rendering.

use rust_decimal::Decimal;
use std::collections::HashMap;

/// Per-symbol funding rate data from mark price stream or REST premiumIndex.
#[derive(Debug, Clone)]
pub struct FundingRateEntry {
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Current mark price
    pub mark_price: Decimal,
    /// Current funding rate (raw, e.g., 0.00010000 = 0.01%)
    pub funding_rate: Decimal,
    /// Next funding time (ms epoch)
    pub next_funding_time: u64,
}

/// Sort column for the funding rate table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Symbol,
    FundingRate,
    AnnualRate,
    MarkPrice,
}

impl SortColumn {
    /// Cycle to the next sort column.
    pub fn next(self) -> Self {
        match self {
            SortColumn::Symbol => SortColumn::FundingRate,
            SortColumn::FundingRate => SortColumn::AnnualRate,
            SortColumn::AnnualRate => SortColumn::MarkPrice,
            SortColumn::MarkPrice => SortColumn::Symbol,
        }
    }

    /// Display label for the sort column.
    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Symbol => "Symbol",
            SortColumn::FundingRate => "Funding Rate",
            SortColumn::AnnualRate => "Annual Rate",
            SortColumn::MarkPrice => "Mark Price",
        }
    }
}

/// Accumulator for funding rate data across all trading pairs.
///
/// Uses a HashMap keyed by symbol for O(1) upsert from the mark price stream.
/// Sorted output is computed on demand for rendering.
pub struct FundingRateTable {
    entries: HashMap<String, FundingRateEntry>,
}

impl FundingRateTable {
    /// Create a new empty table.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Insert or update an entry by symbol.
    pub fn upsert(&mut self, entry: FundingRateEntry) {
        self.entries.insert(entry.symbol.clone(), entry);
    }

    /// Return entries sorted by the given column and direction.
    ///
    /// For FundingRate and AnnualRate, sorts by absolute value (most extreme first
    /// when descending) to surface the most interesting rates regardless of sign.
    pub fn sorted_entries(
        &self,
        sort_column: SortColumn,
        ascending: bool,
    ) -> Vec<&FundingRateEntry> {
        let mut entries: Vec<&FundingRateEntry> = self.entries.values().collect();

        entries.sort_by(|a, b| {
            let cmp = match sort_column {
                SortColumn::Symbol => a.symbol.cmp(&b.symbol),
                SortColumn::FundingRate => {
                    a.funding_rate.abs().cmp(&b.funding_rate.abs())
                }
                SortColumn::AnnualRate => {
                    // Annual = funding_rate * 3 * 365, abs comparison
                    a.funding_rate.abs().cmp(&b.funding_rate.abs())
                }
                SortColumn::MarkPrice => a.mark_price.cmp(&b.mark_price),
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
    fn test_upsert_and_sorted_entries() {
        let mut table = FundingRateTable::new();

        table.upsert(FundingRateEntry {
            symbol: "BTCUSDT".to_string(),
            mark_price: Decimal::from_str("50000.00").unwrap(),
            funding_rate: Decimal::from_str("0.00010000").unwrap(), // 0.01%
            next_funding_time: 1700003600000,
        });

        table.upsert(FundingRateEntry {
            symbol: "ETHUSDT".to_string(),
            mark_price: Decimal::from_str("3000.00").unwrap(),
            funding_rate: Decimal::from_str("-0.00050000").unwrap(), // -0.05% (negative)
            next_funding_time: 1700003600000,
        });

        table.upsert(FundingRateEntry {
            symbol: "SOLUSDT".to_string(),
            mark_price: Decimal::from_str("100.00").unwrap(),
            funding_rate: Decimal::from_str("0.00200000").unwrap(), // 0.2%
            next_funding_time: 1700003600000,
        });

        assert_eq!(table.len(), 3);

        // Sort by funding rate descending (absolute value) — most extreme first
        let sorted = table.sorted_entries(SortColumn::FundingRate, false);
        assert_eq!(sorted[0].symbol, "SOLUSDT"); // 0.002 abs
        assert_eq!(sorted[1].symbol, "ETHUSDT"); // 0.0005 abs
        assert_eq!(sorted[2].symbol, "BTCUSDT"); // 0.0001 abs

        // Sort by symbol ascending
        let sorted = table.sorted_entries(SortColumn::Symbol, true);
        assert_eq!(sorted[0].symbol, "BTCUSDT");
        assert_eq!(sorted[1].symbol, "ETHUSDT");
        assert_eq!(sorted[2].symbol, "SOLUSDT");

        // Sort by mark price descending
        let sorted = table.sorted_entries(SortColumn::MarkPrice, false);
        assert_eq!(sorted[0].symbol, "BTCUSDT"); // 50000
        assert_eq!(sorted[1].symbol, "ETHUSDT"); // 3000
        assert_eq!(sorted[2].symbol, "SOLUSDT"); // 100
    }

    #[test]
    fn test_upsert_overwrites_existing() {
        let mut table = FundingRateTable::new();

        table.upsert(FundingRateEntry {
            symbol: "BTCUSDT".to_string(),
            mark_price: Decimal::from_str("50000.00").unwrap(),
            funding_rate: Decimal::from_str("0.00010000").unwrap(),
            next_funding_time: 1700003600000,
        });

        // Upsert same symbol with new data
        table.upsert(FundingRateEntry {
            symbol: "BTCUSDT".to_string(),
            mark_price: Decimal::from_str("51000.00").unwrap(),
            funding_rate: Decimal::from_str("0.00020000").unwrap(),
            next_funding_time: 1700007200000,
        });

        assert_eq!(table.len(), 1);
        let sorted = table.sorted_entries(SortColumn::Symbol, true);
        assert_eq!(sorted[0].mark_price, Decimal::from_str("51000.00").unwrap());
        assert_eq!(
            sorted[0].funding_rate,
            Decimal::from_str("0.00020000").unwrap()
        );
    }

    #[test]
    fn test_sort_column_cycle() {
        assert_eq!(SortColumn::Symbol.next(), SortColumn::FundingRate);
        assert_eq!(SortColumn::FundingRate.next(), SortColumn::AnnualRate);
        assert_eq!(SortColumn::AnnualRate.next(), SortColumn::MarkPrice);
        assert_eq!(SortColumn::MarkPrice.next(), SortColumn::Symbol);
    }
}
