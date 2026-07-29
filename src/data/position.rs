// src/data/position.rs
// Position data model for AsterDEX position tracking
//
// Pure computation methods for unrealized PnL ($/%),
// liquidation distance percentage, estimated funding payment,
// and position reconciliation merge logic.

use crate::network::asterdex_positions::AsterDexPosition;
use crate::network::asterdex_stream_types::PositionUpdateData;
use rust_decimal::Decimal;

/// Side of a position (Long or Short)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Long,
    Short,
}

/// A single open futures position from AsterDEX
#[derive(Debug, Clone)]
pub struct Position {
    // REST fields (from /fapi/v2/positionRisk)
    pub symbol: String,
    pub position_side: PositionSide,
    pub position_amt: Decimal,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub unrealized_profit: Decimal,
    pub liquidation_price: Decimal,
    pub leverage: u32,
    pub margin_type: String,
    pub isolated_margin: Decimal,
    pub notional: Decimal,
    #[allow(dead_code)] // Populated from REST API, retained for future use
    pub update_time: u64,

    // Derived/streaming fields
    pub funding_rate: Option<Decimal>,
    pub next_funding_time: Option<u64>,
}

impl Position {
    /// Unrealized PnL in USD: position_amt * (mark_price - entry_price)
    ///
    /// The sign of position_amt handles direction:
    /// - Long (positive amt): profit when mark > entry
    /// - Short (negative amt): profit when mark < entry
    pub fn unrealized_pnl(&self) -> Decimal {
        self.position_amt * (self.mark_price - self.entry_price)
    }

    /// Unrealized PnL as percentage of entry margin (ROI%).
    ///
    /// Formula: (unrealized_pnl / abs(position_amt * entry_price)) * leverage * 100
    /// Returns zero when entry notional is zero (no position or zero entry price).
    pub fn unrealized_pnl_pct(&self) -> Decimal {
        let entry_notional = self.position_amt.abs() * self.entry_price;
        if entry_notional.is_zero() {
            return Decimal::ZERO;
        }
        (self.unrealized_pnl() / entry_notional)
            * Decimal::from(self.leverage)
            * Decimal::from(100)
    }

    /// Liquidation distance as percentage: abs(mark - liq) / mark * 100
    ///
    /// Returns None when liquidation_price is zero (cross-margin or not yet calculated)
    /// or when mark_price is zero (no data yet).
    pub fn liquidation_distance_pct(&self) -> Option<Decimal> {
        if self.liquidation_price.is_zero() || self.mark_price.is_zero() {
            return None;
        }
        let distance = (self.mark_price - self.liquidation_price).abs();
        Some(distance / self.mark_price * Decimal::from(100))
    }

    /// Estimated next funding payment in USD.
    ///
    /// Formula: funding_rate * position_amt * mark_price
    /// Positive = you pay (long when rate > 0), negative = you receive.
    /// Returns None when no funding rate is available.
    pub fn estimated_funding_payment(&self) -> Option<Decimal> {
        let rate = self.funding_rate?;
        Some(rate * self.position_amt * self.mark_price)
    }

    /// Create a Position from an ACCOUNT_UPDATE WebSocket position delta.
    ///
    /// Used when a new position appears (first fill on a new symbol).
    /// mark_price, funding_rate, and next_funding_time are left at defaults
    /// since they come from the mark price stream (fresher source).
    /// liquidation_price is zero (ACCOUNT_UPDATE doesn't include it).
    pub fn from_ws_update(ws: &PositionUpdateData) -> Self {
        let position_side = parse_position_side(&ws.position_side, ws.position_amt);
        Position {
            symbol: ws.symbol.clone(),
            position_side,
            position_amt: ws.position_amt,
            entry_price: ws.entry_price,
            mark_price: Decimal::ZERO,
            unrealized_profit: ws.unrealized_pnl,
            liquidation_price: Decimal::ZERO,
            leverage: 1,
            margin_type: ws.margin_type.clone(),
            isolated_margin: ws.isolated_wallet,
            notional: Decimal::ZERO,
            update_time: 0,
            funding_rate: None,
            next_funding_time: None,
        }
    }
}

/// Merge fresh REST positions with existing positions.
///
/// - New positions in fresh but not existing: added as-is
/// - Positions in existing but not fresh: removed (closed)
/// - Positions in both: fields updated from fresh EXCEPT mark_price, funding_rate, next_funding_time preserved from existing
/// - Match key: (symbol, position_side) tuple
pub fn reconcile_positions(existing: &[Position], fresh: Vec<Position>) -> Vec<Position> {
    fresh
        .into_iter()
        .map(|mut fresh_pos| {
            // Try to find a matching existing position by (symbol, position_side)
            if let Some(existing_pos) = existing.iter().find(|e| {
                e.symbol == fresh_pos.symbol && e.position_side == fresh_pos.position_side
            }) {
                // Preserve WebSocket-updated fields from existing position
                fresh_pos.mark_price = existing_pos.mark_price;
                fresh_pos.funding_rate = existing_pos.funding_rate;
                fresh_pos.next_funding_time = existing_pos.next_funding_time;
            }
            // New positions (no match) are added as-is from fresh
            // Closed positions (in existing but not fresh) are implicitly dropped
            fresh_pos
        })
        .collect()
}

/// Parse position side from API string and position amount.
///
/// - "LONG" -> Long
/// - "SHORT" -> Short
/// - "BOTH" (one-way mode) -> determine from sign of position_amt
///   - positive or zero -> Long
///   - negative -> Short
fn parse_position_side(side_str: &str, position_amt: Decimal) -> PositionSide {
    match side_str {
        "LONG" => PositionSide::Long,
        "SHORT" => PositionSide::Short,
        _ => {
            // "BOTH" or unknown: derive from sign of position_amt
            if position_amt < Decimal::ZERO {
                PositionSide::Short
            } else {
                PositionSide::Long
            }
        }
    }
}

impl From<AsterDexPosition> for Position {
    fn from(api: AsterDexPosition) -> Self {
        let position_side = parse_position_side(&api.position_side, api.position_amt);
        // Convert leverage from Decimal to u32 (API returns as string like "10")
        let leverage = api.leverage.try_into().unwrap_or(1u32);

        Position {
            symbol: api.symbol,
            position_side,
            position_amt: api.position_amt,
            entry_price: api.entry_price,
            mark_price: api.mark_price,
            unrealized_profit: api.un_realized_profit,
            liquidation_price: api.liquidation_price,
            leverage,
            margin_type: api.margin_type,
            isolated_margin: api.isolated_margin,
            notional: api.notional,
            update_time: api.update_time,
            // Funding fields populated later by mark price WebSocket stream
            funding_rate: None,
            next_funding_time: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::asterdex_stream_types::PositionUpdateData;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::str::FromStr;

    /// Helper: create a long position with sensible defaults
    fn make_long_position(amt: &str, entry: &str, mark: &str, leverage: u32) -> Position {
        Position {
            symbol: "BTCUSDT".to_string(),
            position_side: PositionSide::Long,
            position_amt: Decimal::from_str(amt).unwrap(),
            entry_price: Decimal::from_str(entry).unwrap(),
            mark_price: Decimal::from_str(mark).unwrap(),
            unrealized_profit: Decimal::ZERO,
            liquidation_price: Decimal::from_str("45000").unwrap(),
            leverage,
            margin_type: "isolated".to_string(),
            isolated_margin: Decimal::ZERO,
            notional: Decimal::ZERO,
            update_time: 1700000000000,
            funding_rate: Some(Decimal::from_str("0.0001").unwrap()),
            next_funding_time: Some(1700003600000),
        }
    }

    /// Helper: create a short position with sensible defaults
    fn make_short_position(amt: &str, entry: &str, mark: &str, leverage: u32) -> Position {
        Position {
            symbol: "BTCUSDT".to_string(),
            position_side: PositionSide::Short,
            position_amt: Decimal::from_str(amt).unwrap(),
            entry_price: Decimal::from_str(entry).unwrap(),
            mark_price: Decimal::from_str(mark).unwrap(),
            unrealized_profit: Decimal::ZERO,
            liquidation_price: Decimal::from_str("55000").unwrap(),
            leverage,
            margin_type: "isolated".to_string(),
            isolated_margin: Decimal::ZERO,
            notional: Decimal::ZERO,
            update_time: 1700000000000,
            funding_rate: Some(Decimal::from_str("0.0001").unwrap()),
            next_funding_time: Some(1700003600000),
        }
    }

    // ---- unrealized_pnl() tests ----

    #[test]
    fn test_unrealized_pnl_long_profit() {
        // Long 0.5 BTC, entry 50000, mark 51000 => 0.5 * (51000 - 50000) = 500
        let pos = make_long_position("0.5", "50000", "51000", 10);
        assert_eq!(pos.unrealized_pnl(), dec!(500));
    }

    #[test]
    fn test_unrealized_pnl_short_profit() {
        // Short -0.5 BTC, entry 50000, mark 49000 => -0.5 * (49000 - 50000) = 500
        let pos = make_short_position("-0.5", "50000", "49000", 10);
        assert_eq!(pos.unrealized_pnl(), dec!(500));
    }

    #[test]
    fn test_unrealized_pnl_short_loss() {
        // Short -0.5 BTC, entry 50000, mark 51000 => -0.5 * (51000 - 50000) = -500
        let pos = make_short_position("-0.5", "50000", "51000", 10);
        assert_eq!(pos.unrealized_pnl(), dec!(-500));
    }

    #[test]
    fn test_unrealized_pnl_zero_size() {
        let pos = make_long_position("0", "50000", "51000", 10);
        assert_eq!(pos.unrealized_pnl(), dec!(0));
    }

    // ---- unrealized_pnl_pct() tests ----

    #[test]
    fn test_unrealized_pnl_pct_long() {
        // Long 0.5 BTC, entry 50000, mark 51000, leverage 10
        // PnL = 500, entry_notional = 0.5 * 50000 = 25000
        // PnL% = (500 / 25000) * 10 * 100 = 20.0
        let pos = make_long_position("0.5", "50000", "51000", 10);
        assert_eq!(pos.unrealized_pnl_pct(), dec!(20));
    }

    #[test]
    fn test_unrealized_pnl_pct_zero_entry_notional() {
        // Zero position_amt => zero entry_notional => should return 0
        let pos = make_long_position("0", "50000", "51000", 10);
        assert_eq!(pos.unrealized_pnl_pct(), dec!(0));
    }

    #[test]
    fn test_unrealized_pnl_pct_zero_entry_price() {
        // Zero entry_price => zero entry_notional => should return 0
        let pos = make_long_position("0.5", "0", "51000", 10);
        assert_eq!(pos.unrealized_pnl_pct(), dec!(0));
    }

    // ---- liquidation_distance_pct() tests ----

    #[test]
    fn test_liquidation_distance_pct_normal() {
        // mark=50000, liq=45000 => abs(50000-45000)/50000*100 = 10.0
        let pos = make_long_position("0.5", "50000", "50000", 10);
        let result = pos.liquidation_distance_pct();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), dec!(10));
    }

    #[test]
    fn test_liquidation_distance_pct_zero_liq() {
        let mut pos = make_long_position("0.5", "50000", "50000", 10);
        pos.liquidation_price = Decimal::ZERO;
        assert_eq!(pos.liquidation_distance_pct(), None);
    }

    #[test]
    fn test_liquidation_distance_pct_zero_mark() {
        let mut pos = make_long_position("0.5", "50000", "0", 10);
        pos.liquidation_price = Decimal::from_str("45000").unwrap();
        assert_eq!(pos.liquidation_distance_pct(), None);
    }

    // ---- estimated_funding_payment() tests ----

    #[test]
    fn test_estimated_funding_payment_long() {
        // rate=0.0001, amt=0.5, mark=50000 => 0.0001 * 0.5 * 50000 = 2.5
        let pos = make_long_position("0.5", "50000", "50000", 10);
        let result = pos.estimated_funding_payment();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Decimal::from_str("2.5").unwrap());
    }

    #[test]
    fn test_estimated_funding_payment_no_rate() {
        let mut pos = make_long_position("0.5", "50000", "50000", 10);
        pos.funding_rate = None;
        assert_eq!(pos.estimated_funding_payment(), None);
    }

    #[test]
    fn test_estimated_funding_payment_short() {
        // Short: rate=0.0001, amt=-0.5, mark=50000 => 0.0001 * -0.5 * 50000 = -2.5
        // Negative means short receives payment when rate positive
        let pos = make_short_position("-0.5", "50000", "50000", 10);
        let result = pos.estimated_funding_payment();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Decimal::from_str("-2.5").unwrap());
    }

    // ---- reconcile_positions() tests ----

    #[test]
    fn test_reconcile_new_position_added() {
        let existing: Vec<Position> = vec![];
        let fresh = vec![make_long_position("0.5", "50000", "51000", 10)];
        let result = reconcile_positions(&existing, fresh);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "BTCUSDT");
    }

    #[test]
    fn test_reconcile_closed_position_removed() {
        let existing = vec![make_long_position("0.5", "50000", "51000", 10)];
        let fresh: Vec<Position> = vec![];
        let result = reconcile_positions(&existing, fresh);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_reconcile_preserves_mark_price_and_funding() {
        // Existing position has mark_price=51500 from WebSocket, funding_rate=0.0002
        let mut existing_pos = make_long_position("0.5", "50000", "51500", 10);
        existing_pos.funding_rate = Some(Decimal::from_str("0.0002").unwrap());
        existing_pos.next_funding_time = Some(1700003600000);
        let existing = vec![existing_pos];

        // Fresh position from REST has mark_price=51000 (staler), entry updated to 50500
        let mut fresh_pos = make_long_position("0.5", "50500", "51000", 10);
        fresh_pos.funding_rate = None; // REST doesn't include funding
        fresh_pos.next_funding_time = None;
        let fresh = vec![fresh_pos];

        let result = reconcile_positions(&existing, fresh);
        assert_eq!(result.len(), 1);
        // Entry price should be updated from fresh
        assert_eq!(result[0].entry_price, Decimal::from_str("50500").unwrap());
        // Mark price should be preserved from existing (fresher WebSocket data)
        assert_eq!(result[0].mark_price, Decimal::from_str("51500").unwrap());
        // Funding should be preserved from existing
        assert_eq!(result[0].funding_rate, Some(Decimal::from_str("0.0002").unwrap()));
        assert_eq!(result[0].next_funding_time, Some(1700003600000));
    }

    #[test]
    fn test_reconcile_match_key_symbol_and_side() {
        // Two positions: BTCUSDT Long and ETHUSDT Short
        let btc = make_long_position("0.5", "50000", "51000", 10);
        let mut eth = make_short_position("-2.0", "3000", "2950", 5);
        eth.symbol = "ETHUSDT".to_string();
        let existing = vec![btc, eth];

        // Fresh only has BTCUSDT Long with updated entry
        let fresh_btc = make_long_position("0.6", "50100", "51000", 10);
        let fresh = vec![fresh_btc];

        let result = reconcile_positions(&existing, fresh);
        // ETHUSDT Short should be removed (closed)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "BTCUSDT");
        assert_eq!(result[0].position_amt, Decimal::from_str("0.6").unwrap());
    }

    // ---- From<AsterDexPosition> tests ----

    #[test]
    fn test_from_asterdex_position_long() {
        let api = AsterDexPosition {
            symbol: "BTCUSDT".to_string(),
            position_amt: Decimal::from_str("0.5").unwrap(),
            entry_price: Decimal::from_str("50000").unwrap(),
            mark_price: Decimal::from_str("51000").unwrap(),
            un_realized_profit: Decimal::from_str("500").unwrap(),
            liquidation_price: Decimal::from_str("45000").unwrap(),
            leverage: Decimal::from_str("10").unwrap(),
            margin_type: "isolated".to_string(),
            isolated_margin: Decimal::from_str("2500").unwrap(),
            position_side: "LONG".to_string(),
            notional: Decimal::from_str("25500").unwrap(),
            update_time: 1700000000000,
        };

        let pos = Position::from(api);
        assert_eq!(pos.symbol, "BTCUSDT");
        assert_eq!(pos.position_side, PositionSide::Long);
        assert_eq!(pos.position_amt, Decimal::from_str("0.5").unwrap());
        assert_eq!(pos.entry_price, Decimal::from_str("50000").unwrap());
        assert_eq!(pos.mark_price, Decimal::from_str("51000").unwrap());
        assert_eq!(pos.unrealized_profit, Decimal::from_str("500").unwrap());
        assert_eq!(pos.liquidation_price, Decimal::from_str("45000").unwrap());
        assert_eq!(pos.leverage, 10);
        assert_eq!(pos.margin_type, "isolated");
        assert_eq!(pos.isolated_margin, Decimal::from_str("2500").unwrap());
        assert_eq!(pos.notional, Decimal::from_str("25500").unwrap());
        assert_eq!(pos.update_time, 1700000000000);
        assert_eq!(pos.funding_rate, None);
        assert_eq!(pos.next_funding_time, None);
    }

    #[test]
    fn test_from_asterdex_position_short() {
        let api = AsterDexPosition {
            symbol: "ETHUSDT".to_string(),
            position_amt: Decimal::from_str("-2.0").unwrap(),
            entry_price: Decimal::from_str("3000").unwrap(),
            mark_price: Decimal::from_str("2900").unwrap(),
            un_realized_profit: Decimal::from_str("200").unwrap(),
            liquidation_price: Decimal::from_str("5000").unwrap(),
            leverage: Decimal::from_str("5").unwrap(),
            margin_type: "cross".to_string(),
            isolated_margin: Decimal::ZERO,
            position_side: "SHORT".to_string(),
            notional: Decimal::from_str("-5800").unwrap(),
            update_time: 1700000001000,
        };

        let pos = Position::from(api);
        assert_eq!(pos.position_side, PositionSide::Short);
        assert_eq!(pos.position_amt, Decimal::from_str("-2.0").unwrap());
        assert_eq!(pos.leverage, 5);
    }

    #[test]
    fn test_from_asterdex_position_both_side() {
        // "BOTH" position side (one-way mode) -- determine from sign of position_amt
        let api = AsterDexPosition {
            symbol: "BTCUSDT".to_string(),
            position_amt: Decimal::from_str("0.5").unwrap(),
            entry_price: Decimal::from_str("50000").unwrap(),
            mark_price: Decimal::from_str("51000").unwrap(),
            un_realized_profit: Decimal::from_str("500").unwrap(),
            liquidation_price: Decimal::from_str("45000").unwrap(),
            leverage: Decimal::from_str("10").unwrap(),
            margin_type: "isolated".to_string(),
            isolated_margin: Decimal::from_str("2500").unwrap(),
            position_side: "BOTH".to_string(),
            notional: Decimal::from_str("25500").unwrap(),
            update_time: 1700000000000,
        };

        let pos = Position::from(api);
        // Positive amt => Long
        assert_eq!(pos.position_side, PositionSide::Long);
    }

    #[test]
    fn test_from_asterdex_position_both_side_short() {
        let api = AsterDexPosition {
            symbol: "BTCUSDT".to_string(),
            position_amt: Decimal::from_str("-0.5").unwrap(),
            entry_price: Decimal::from_str("50000").unwrap(),
            mark_price: Decimal::from_str("51000").unwrap(),
            un_realized_profit: Decimal::from_str("-500").unwrap(),
            liquidation_price: Decimal::from_str("55000").unwrap(),
            leverage: Decimal::from_str("10").unwrap(),
            margin_type: "isolated".to_string(),
            isolated_margin: Decimal::from_str("2500").unwrap(),
            position_side: "BOTH".to_string(),
            notional: Decimal::from_str("-25500").unwrap(),
            update_time: 1700000000000,
        };

        let pos = Position::from(api);
        // Negative amt => Short
        assert_eq!(pos.position_side, PositionSide::Short);
    }

    // ---- End-to-end pipeline test (POS-09 building blocks) ----

    #[test]
    fn test_end_to_end_convert_and_reconcile() {
        // Simulate REST response: 2 positions (BTC long, ETH short)
        let api_positions = vec![
            AsterDexPosition {
                symbol: "BTCUSDT".to_string(),
                position_amt: Decimal::from_str("0.5").unwrap(),
                entry_price: Decimal::from_str("50000").unwrap(),
                mark_price: Decimal::from_str("51000").unwrap(),
                un_realized_profit: Decimal::from_str("500").unwrap(),
                liquidation_price: Decimal::from_str("45000").unwrap(),
                leverage: Decimal::from_str("10").unwrap(),
                margin_type: "isolated".to_string(),
                isolated_margin: Decimal::from_str("2500").unwrap(),
                position_side: "BOTH".to_string(),
                notional: Decimal::from_str("25500").unwrap(),
                update_time: 1700000000000,
            },
            AsterDexPosition {
                symbol: "ETHUSDT".to_string(),
                position_amt: Decimal::from_str("-2.0").unwrap(),
                entry_price: Decimal::from_str("3000").unwrap(),
                mark_price: Decimal::from_str("2900").unwrap(),
                un_realized_profit: Decimal::from_str("200").unwrap(),
                liquidation_price: Decimal::from_str("5000").unwrap(),
                leverage: Decimal::from_str("5").unwrap(),
                margin_type: "cross".to_string(),
                isolated_margin: Decimal::ZERO,
                position_side: "BOTH".to_string(),
                notional: Decimal::from_str("-5800").unwrap(),
                update_time: 1700000001000,
            },
        ];

        // Step 1: Convert each via From trait
        let fresh: Vec<Position> = api_positions.into_iter().map(Position::from).collect();
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh[0].position_side, PositionSide::Long);
        assert_eq!(fresh[1].position_side, PositionSide::Short);

        // Step 2: Existing positions with WebSocket-updated mark price
        let mut existing_btc = make_long_position("0.5", "49000", "51500", 10);
        existing_btc.funding_rate = Some(Decimal::from_str("0.0003").unwrap());
        existing_btc.next_funding_time = Some(1700007200000);
        let existing = vec![existing_btc];

        // Step 3: Reconcile
        let result = reconcile_positions(&existing, fresh);

        // BTC should be updated from fresh but preserve mark_price/funding from existing
        assert_eq!(result.len(), 2);

        let btc = result.iter().find(|p| p.symbol == "BTCUSDT").unwrap();
        assert_eq!(btc.entry_price, Decimal::from_str("50000").unwrap()); // Updated from fresh
        assert_eq!(btc.mark_price, Decimal::from_str("51500").unwrap()); // Preserved from existing
        assert_eq!(btc.funding_rate, Some(Decimal::from_str("0.0003").unwrap())); // Preserved
        assert_eq!(btc.next_funding_time, Some(1700007200000)); // Preserved

        // ETH is new (not in existing) -- added as-is with REST mark_price
        let eth = result.iter().find(|p| p.symbol == "ETHUSDT").unwrap();
        assert_eq!(eth.entry_price, Decimal::from_str("3000").unwrap());
        assert_eq!(eth.mark_price, Decimal::from_str("2900").unwrap()); // From REST (no existing to preserve)
        assert_eq!(eth.funding_rate, None); // From conversion (no WebSocket data yet)
    }

    // ---- from_ws_update() tests ----

    #[test]
    fn test_from_ws_update_new_long_position() {
        let ws = PositionUpdateData {
            symbol: "ETHUSDT".to_string(),
            position_amt: Decimal::from_str("2.0").unwrap(),
            entry_price: Decimal::from_str("3000").unwrap(),
            accumulated_realized: Decimal::ZERO,
            unrealized_pnl: Decimal::from_str("100").unwrap(),
            margin_type: "isolated".to_string(),
            isolated_wallet: Decimal::from_str("600").unwrap(),
            position_side: "LONG".to_string(),
        };
        let pos = Position::from_ws_update(&ws);
        assert_eq!(pos.symbol, "ETHUSDT");
        assert_eq!(pos.position_side, PositionSide::Long);
        assert_eq!(pos.position_amt, Decimal::from_str("2.0").unwrap());
        assert_eq!(pos.entry_price, Decimal::from_str("3000").unwrap());
        assert_eq!(pos.mark_price, Decimal::ZERO);
        assert_eq!(pos.isolated_margin, Decimal::from_str("600").unwrap());
        assert_eq!(pos.unrealized_profit, Decimal::from_str("100").unwrap());
        assert_eq!(pos.liquidation_price, Decimal::ZERO);
        assert_eq!(pos.leverage, 1);
        assert_eq!(pos.funding_rate, None);
        assert_eq!(pos.next_funding_time, None);
    }

    #[test]
    fn test_from_ws_update_new_short_position() {
        let ws = PositionUpdateData {
            symbol: "BTCUSDT".to_string(),
            position_amt: Decimal::from_str("-0.5").unwrap(),
            entry_price: Decimal::from_str("50000").unwrap(),
            accumulated_realized: Decimal::ZERO,
            unrealized_pnl: Decimal::from_str("200").unwrap(),
            margin_type: "cross".to_string(),
            isolated_wallet: Decimal::ZERO,
            position_side: "SHORT".to_string(),
        };
        let pos = Position::from_ws_update(&ws);
        assert_eq!(pos.symbol, "BTCUSDT");
        assert_eq!(pos.position_side, PositionSide::Short);
        assert_eq!(pos.position_amt, Decimal::from_str("-0.5").unwrap());
        assert_eq!(pos.margin_type, "cross");
    }

    #[test]
    fn test_from_ws_update_both_side_derives_from_amount() {
        let ws = PositionUpdateData {
            symbol: "SOLUSDT".to_string(),
            position_amt: Decimal::from_str("-10.0").unwrap(),
            entry_price: Decimal::from_str("100").unwrap(),
            accumulated_realized: Decimal::ZERO,
            unrealized_pnl: Decimal::ZERO,
            margin_type: "isolated".to_string(),
            isolated_wallet: Decimal::from_str("50").unwrap(),
            position_side: "BOTH".to_string(),
        };
        let pos = Position::from_ws_update(&ws);
        assert_eq!(pos.position_side, PositionSide::Short);
    }
}
