// src/data/order_book.rs
// Order book data structure for partial depth snapshot storage

use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// Sorted order book maintaining bid and ask price levels from partial depth snapshots.
///
/// Bids stored in BTreeMap (ascending by price; iterate .rev() for best-first).
/// Asks stored in BTreeMap (ascending by price; iterate forward for best-first).
/// Both map price -> aggregate quantity (Decimal -> Decimal).
///
/// Designed for the @depth20@100ms partial book depth stream which sends
/// complete top-20 snapshots. Each apply_partial_snapshot() call replaces
/// all levels (clear + rebuild), so no incremental state tracking is needed.
pub struct OrderBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    last_update_id: u64,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_update_id: 0,
        }
    }

    /// Replace entire book from a full depth snapshot (e.g., REST API).
    /// Clears existing data and inserts all non-zero levels.
    pub fn apply_partial_snapshot(
        &mut self,
        bids: Vec<(Decimal, Decimal)>,
        asks: Vec<(Decimal, Decimal)>,
        last_update_id: u64,
    ) {
        self.bids.clear();
        for (price, qty) in bids {
            if !qty.is_zero() {
                self.bids.insert(price, qty);
            }
        }
        self.asks.clear();
        for (price, qty) in asks {
            if !qty.is_zero() {
                self.asks.insert(price, qty);
            }
        }
        self.last_update_id = last_update_id;
    }

    /// Update top-of-book levels from a partial depth stream (@depth20).
    /// Replaces only the price range covered by the incoming data,
    /// preserving deeper levels from a previous REST snapshot.
    pub fn apply_top_levels(
        &mut self,
        bids: Vec<(Decimal, Decimal)>,
        asks: Vec<(Decimal, Decimal)>,
        last_update_id: u64,
    ) {
        // Bids: remove all existing bids at or above the lowest incoming bid price,
        // then insert the incoming levels (they are the authoritative top-of-book).
        if let Some(min_bid_price) = bids.iter().map(|(p, _)| *p).min() {
            self.bids.retain(|&price, _| price < min_bid_price);
        }
        for (price, qty) in bids {
            if !qty.is_zero() {
                self.bids.insert(price, qty);
            }
        }

        // Asks: remove all existing asks at or below the highest incoming ask price,
        // then insert the incoming levels.
        if let Some(max_ask_price) = asks.iter().map(|(p, _)| *p).max() {
            self.asks.retain(|&price, _| price > max_ask_price);
        }
        for (price, qty) in asks {
            if !qty.is_zero() {
                self.asks.insert(price, qty);
            }
        }

        self.last_update_id = last_update_id;
    }

    /// Best bid price (highest bid). O(log n).
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.keys().next_back().copied()
    }

    /// Best ask price (lowest ask). O(log n).
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.keys().next().copied()
    }

    /// Spread = best_ask - best_bid. None if either side is empty.
    #[allow(dead_code)] // Used by tests; future DOM phases will consume
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    /// Mid price = (best_bid + best_ask) / 2. None if either side is empty.
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            _ => None,
        }
    }

    /// Top N bid levels, best (highest price) first.
    #[allow(dead_code)] // Used by tests; available for future DOM features
    pub fn top_bids(&self, n: usize) -> Vec<(Decimal, Decimal)> {
        self.bids.iter().rev().take(n).map(|(&p, &q)| (p, q)).collect()
    }

    /// Top N ask levels, best (lowest price) first.
    #[allow(dead_code)] // Used by tests; available for future DOM features
    pub fn top_asks(&self, n: usize) -> Vec<(Decimal, Decimal)> {
        self.asks.iter().take(n).map(|(&p, &q)| (p, q)).collect()
    }

    /// Total bid quantity across all levels.
    pub fn total_bid_qty(&self) -> Decimal {
        self.bids.values().sum()
    }

    /// Total ask quantity across all levels.
    pub fn total_ask_qty(&self) -> Decimal {
        self.asks.values().sum()
    }

    /// Number of bid levels.
    #[allow(dead_code)] // Used by tests; available for future DOM features
    pub fn bid_count(&self) -> usize {
        self.bids.len()
    }

    /// Number of ask levels.
    #[allow(dead_code)] // Used by tests; available for future DOM features
    pub fn ask_count(&self) -> usize {
        self.asks.len()
    }

    /// Quantity at a specific bid price level. O(log n) BTreeMap lookup.
    pub fn bid_qty_at(&self, price: &Decimal) -> Option<Decimal> {
        self.bids.get(price).copied()
    }

    /// Quantity at a specific ask price level. O(log n) BTreeMap lookup.
    pub fn ask_qty_at(&self, price: &Decimal) -> Option<Decimal> {
        self.asks.get(price).copied()
    }

    /// Iterator over all bid and ask quantities (for heatmap intensity tracking).
    pub fn all_quantities(&self) -> impl Iterator<Item = Decimal> + '_ {
        self.bids.values().copied().chain(self.asks.values().copied())
    }

    /// Whether the book has no data (both sides empty).
    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    /// Last update ID from the most recent snapshot.
    #[allow(dead_code)] // Used by tests; available for future DOM features
    pub fn last_update_id(&self) -> u64 {
        self.last_update_id
    }
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn test_empty_order_book() {
        let book = OrderBook::new();
        assert!(book.is_empty());
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.spread(), None);
        assert_eq!(book.mid_price(), None);
        assert_eq!(book.total_bid_qty(), Decimal::ZERO);
        assert_eq!(book.total_ask_qty(), Decimal::ZERO);
        assert_eq!(book.bid_count(), 0);
        assert_eq!(book.ask_count(), 0);
        assert_eq!(book.last_update_id(), 0);
    }

    #[test]
    fn test_apply_partial_snapshot() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec("100.00"), dec("1.5")), (dec("99.50"), dec("2.0"))],
            vec![(dec("100.50"), dec("0.8")), (dec("101.00"), dec("1.2"))],
            42,
        );
        assert_eq!(book.best_bid(), Some(dec("100.00")));
        assert_eq!(book.best_ask(), Some(dec("100.50")));
        assert_eq!(book.spread(), Some(dec("0.50")));
        assert_eq!(book.mid_price(), Some(dec("100.25")));
        assert_eq!(book.last_update_id(), 42);
        assert!(!book.is_empty());
    }

    #[test]
    fn test_snapshot_replaces_previous_data() {
        let mut book = OrderBook::new();
        // First snapshot
        book.apply_partial_snapshot(
            vec![(dec("100.00"), dec("1.0"))],
            vec![(dec("101.00"), dec("1.0"))],
            1,
        );
        // Second snapshot with completely different levels
        book.apply_partial_snapshot(
            vec![(dec("200.00"), dec("2.0"))],
            vec![(dec("201.00"), dec("2.0"))],
            2,
        );
        // Old levels must be gone
        assert_eq!(book.best_bid(), Some(dec("200.00")));
        assert_eq!(book.best_ask(), Some(dec("201.00")));
        assert_eq!(book.bid_count(), 1);
        assert_eq!(book.ask_count(), 1);
        assert_eq!(book.last_update_id(), 2);
    }

    #[test]
    fn test_zero_quantity_levels_filtered() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec("100.00"), dec("0.0")), (dec("99.50"), dec("1.0"))],
            vec![(dec("100.50"), dec("0.0")), (dec("101.00"), dec("1.0"))],
            1,
        );
        assert_eq!(book.bid_count(), 1);
        assert_eq!(book.ask_count(), 1);
        assert_eq!(book.best_bid(), Some(dec("99.50")));
        assert_eq!(book.best_ask(), Some(dec("101.00")));
    }

    #[test]
    fn test_top_bids_descending_order() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![
                (dec("97.00"), dec("1.0")),
                (dec("99.00"), dec("3.0")),
                (dec("98.00"), dec("2.0")),
            ],
            vec![],
            1,
        );
        let top = book.top_bids(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0], (dec("99.00"), dec("3.0"))); // best first
        assert_eq!(top[1], (dec("98.00"), dec("2.0")));
    }

    #[test]
    fn test_top_asks_ascending_order() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![],
            vec![
                (dec("103.00"), dec("1.0")),
                (dec("101.00"), dec("3.0")),
                (dec("102.00"), dec("2.0")),
            ],
            1,
        );
        let top = book.top_asks(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0], (dec("101.00"), dec("3.0"))); // best first
        assert_eq!(top[1], (dec("102.00"), dec("2.0")));
    }

    #[test]
    fn test_top_bids_n_greater_than_available() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec("100.00"), dec("1.0")), (dec("99.00"), dec("2.0"))],
            vec![],
            1,
        );
        let top = book.top_bids(10);
        assert_eq!(top.len(), 2); // only 2 available
    }

    #[test]
    fn test_top_asks_n_greater_than_available() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![],
            vec![(dec("101.00"), dec("1.0"))],
            1,
        );
        let top = book.top_asks(5);
        assert_eq!(top.len(), 1); // only 1 available
    }

    #[test]
    fn test_total_bid_qty() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![
                (dec("100.00"), dec("1.5")),
                (dec("99.00"), dec("2.5")),
                (dec("98.00"), dec("3.0")),
            ],
            vec![],
            1,
        );
        assert_eq!(book.total_bid_qty(), dec("7.0"));
    }

    #[test]
    fn test_total_ask_qty() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![],
            vec![
                (dec("101.00"), dec("0.5")),
                (dec("102.00"), dec("1.5")),
            ],
            1,
        );
        assert_eq!(book.total_ask_qty(), dec("2.0"));
    }

    #[test]
    fn test_mid_price_uses_decimal_two() {
        let mut book = OrderBook::new();
        // best_bid=99, best_ask=101 => mid_price = (99+101)/2 = 100
        book.apply_partial_snapshot(
            vec![(dec("99"), dec("1.0"))],
            vec![(dec("101"), dec("1.0"))],
            1,
        );
        assert_eq!(book.mid_price(), Some(dec("100")));

        // Odd values: best_bid=100.10, best_ask=100.30 => mid = 100.20
        book.apply_partial_snapshot(
            vec![(dec("100.10"), dec("1.0"))],
            vec![(dec("100.30"), dec("1.0"))],
            2,
        );
        assert_eq!(book.mid_price(), Some(dec("100.20")));
    }

    #[test]
    fn test_default_trait() {
        let book = OrderBook::default();
        assert!(book.is_empty());
        assert_eq!(book.last_update_id(), 0);
    }

    #[test]
    fn test_bid_qty_at() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![(dec("100.00"), dec("1.5")), (dec("99.50"), dec("2.0"))],
            vec![],
            1,
        );
        // Existing level returns Some(qty)
        assert_eq!(book.bid_qty_at(&dec("100.00")), Some(dec("1.5")));
        assert_eq!(book.bid_qty_at(&dec("99.50")), Some(dec("2.0")));
        // Missing level returns None
        assert_eq!(book.bid_qty_at(&dec("98.00")), None);
    }

    #[test]
    fn test_ask_qty_at() {
        let mut book = OrderBook::new();
        book.apply_partial_snapshot(
            vec![],
            vec![(dec("100.50"), dec("0.8")), (dec("101.00"), dec("1.2"))],
            1,
        );
        // Existing level returns Some(qty)
        assert_eq!(book.ask_qty_at(&dec("100.50")), Some(dec("0.8")));
        assert_eq!(book.ask_qty_at(&dec("101.00")), Some(dec("1.2")));
        // Missing level returns None
        assert_eq!(book.ask_qty_at(&dec("102.00")), None);
    }
}
