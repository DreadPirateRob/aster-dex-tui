// src/network/asterdex_stream_types.rs
// Serde deserialization types for AsterDEX User Data Stream events
// (ORDER_TRADE_UPDATE, listenKeyExpired, etc.)

use crate::data::order::{parse_order_side, parse_order_status, parse_order_type, Order};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;

/// Lightweight struct to peek at the event type (`e` field) before full deserialization.
///
/// Used to determine which concrete type to deserialize into.
#[derive(Debug, Deserialize)]
pub struct UserDataEvent {
    /// Event type: "ORDER_TRADE_UPDATE", "listenKeyExpired", etc.
    pub e: String,
}

/// Full ORDER_TRADE_UPDATE event from the AsterDEX User Data Stream.
///
/// Source: <https://asterdex.github.io/aster-api-website/futures/user-data-streams/>
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct OrderTradeUpdate {
    /// Event type: "ORDER_TRADE_UPDATE"
    pub e: String,
    /// Event time (ms since epoch)
    #[serde(rename = "E")]
    pub event_time: u64,
    /// Transaction time (ms since epoch)
    #[serde(rename = "T")]
    pub transaction_time: u64,
    /// Nested order update payload
    pub o: OrderUpdatePayload,
}

/// The nested `o` object within ORDER_TRADE_UPDATE containing order details.
///
/// Uses single-letter field names matching the AsterDEX/Binance WebSocket protocol.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct OrderUpdatePayload {
    /// Symbol (e.g., "BTCUSDT")
    pub s: String,
    /// Side: "BUY" or "SELL"
    #[serde(rename = "S")]
    pub side: String,
    /// Order type (LIMIT, MARKET, TRAILING_STOP_MARKET, etc.)
    #[serde(rename = "o")]
    pub order_type: String,
    /// Order ID
    pub i: u64,
    /// Order status (NEW, FILLED, CANCELED, etc.)
    #[serde(rename = "X")]
    pub status: String,
    /// Original price
    pub p: String,
    /// Original quantity
    pub q: String,
    /// Filled accumulated quantity
    pub z: String,
    /// Average price
    pub ap: String,
    /// Trade time (ms since epoch)
    #[serde(rename = "T")]
    pub trade_time: u64,
    /// Commission amount (may be missing on some events)
    #[serde(default)]
    pub n: String,
    /// Realized profit of the trade (string-encoded Decimal, may be absent on non-fill events)
    #[serde(default)]
    pub rp: String,
    /// Commission asset (e.g., "USDT")
    #[serde(default, rename = "N")]
    pub commission_asset: String,
}

/// Full ACCOUNT_UPDATE event from the AsterDEX User Data Stream.
///
/// Fires on: ORDER fill, FUNDING_FEE, MARGIN_TRANSFER, DEPOSIT, WITHDRAW, etc.
/// Contains delta-only balance and position changes.
/// Source: <https://asterdex.github.io/aster-api-website/futures/user-data-streams/>
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AccountUpdateEvent {
    /// Event type: "ACCOUNT_UPDATE"
    pub e: String,
    /// Event time (ms since epoch)
    #[serde(rename = "E")]
    pub event_time: u64,
    /// Transaction time (ms since epoch)
    #[serde(rename = "T")]
    pub transaction_time: u64,
    /// Update data
    pub a: AccountUpdatePayload,
}

/// Nested payload within ACCOUNT_UPDATE.
#[derive(Debug, Deserialize)]
pub struct AccountUpdatePayload {
    /// Event reason type: "ORDER", "FUNDING_FEE", "MARGIN_TRANSFER", etc.
    pub m: String,
    /// Balance changes (delta-only: only changed assets)
    #[serde(rename = "B")]
    pub balances: Vec<BalanceUpdate>,
    /// Position changes (delta-only: only changed positions)
    #[serde(rename = "P")]
    pub positions: Vec<PositionUpdate>,
}

/// Balance update within ACCOUNT_UPDATE.
#[derive(Debug, Deserialize)]
pub struct BalanceUpdate {
    /// Asset name (e.g., "USDT")
    pub a: String,
    /// Wallet balance
    pub wb: String,
    /// Cross wallet balance
    pub cw: String,
    /// Balance change (excluding PnL and commission)
    pub bc: String,
}

/// Position update within ACCOUNT_UPDATE.
#[derive(Debug, Deserialize)]
pub struct PositionUpdate {
    /// Symbol (e.g., "BTCUSDT")
    pub s: String,
    /// Position amount (signed: positive=long, negative=short)
    pub pa: String,
    /// Entry price
    pub ep: String,
    /// Accumulated realized PnL
    pub cr: String,
    /// Unrealized PnL
    pub up: String,
    /// Margin type: "isolated" or "crossed"
    pub mt: String,
    /// Isolated wallet (only meaningful for isolated positions)
    pub iw: String,
    /// Position side: "BOTH", "LONG", "SHORT"
    pub ps: String,
}

/// Processed ACCOUNT_UPDATE data for channel transport.
///
/// Converted from raw serde types with Decimal parsing done upfront
/// (not in the event loop hot path).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AccountUpdateData {
    pub event_time: u64,
    pub reason: String,
    pub balances: Vec<BalanceUpdateData>,
    pub positions: Vec<PositionUpdateData>,
}

/// Processed balance update with Decimal fields for channel transport.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BalanceUpdateData {
    pub asset: String,
    pub wallet_balance: Decimal,
    pub cross_wallet_balance: Decimal,
    pub balance_change: Decimal,
}

/// Processed position update with Decimal fields for channel transport.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PositionUpdateData {
    pub symbol: String,
    pub position_amt: Decimal,
    pub entry_price: Decimal,
    pub accumulated_realized: Decimal,
    pub unrealized_pnl: Decimal,
    pub margin_type: String,
    pub isolated_wallet: Decimal,
    pub position_side: String,
}

impl From<AccountUpdateEvent> for AccountUpdateData {
    fn from(event: AccountUpdateEvent) -> Self {
        AccountUpdateData {
            event_time: event.event_time,
            reason: event.a.m,
            balances: event
                .a
                .balances
                .into_iter()
                .map(|b| BalanceUpdateData {
                    asset: b.a,
                    wallet_balance: Decimal::from_str(&b.wb).unwrap_or_default(),
                    cross_wallet_balance: Decimal::from_str(&b.cw).unwrap_or_default(),
                    balance_change: Decimal::from_str(&b.bc).unwrap_or_default(),
                })
                .collect(),
            positions: event
                .a
                .positions
                .into_iter()
                .map(|p| PositionUpdateData {
                    symbol: p.s,
                    position_amt: Decimal::from_str(&p.pa).unwrap_or_default(),
                    entry_price: Decimal::from_str(&p.ep).unwrap_or_default(),
                    accumulated_realized: Decimal::from_str(&p.cr).unwrap_or_default(),
                    unrealized_pnl: Decimal::from_str(&p.up).unwrap_or_default(),
                    margin_type: p.mt,
                    isolated_wallet: Decimal::from_str(&p.iw).unwrap_or_default(),
                    position_side: p.ps,
                })
                .collect(),
        }
    }
}

impl From<OrderTradeUpdate> for Order {
    fn from(update: OrderTradeUpdate) -> Self {
        let executed_qty = Decimal::from_str(&update.o.z).unwrap_or_default();
        let avg_price = Decimal::from_str(&update.o.ap).unwrap_or_default();

        Order {
            order_id: update.o.i,
            symbol: update.o.s.clone(),
            time: update.o.trade_time,
            update_time: update.event_time,
            side: parse_order_side(&update.o.side),
            order_type: parse_order_type(&update.o.order_type),
            status: parse_order_status(&update.o.status),
            price: Decimal::from_str(&update.o.p).unwrap_or_default(),
            orig_qty: Decimal::from_str(&update.o.q).unwrap_or_default(),
            executed_qty,
            avg_price,
            cum_quote: executed_qty * avg_price,
            realized_pnl: Decimal::from_str(&update.o.rp).unwrap_or_default(),
            commission: Decimal::from_str(&update.o.n).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::order::{OrderSide, OrderStatus, OrderType};

    /// Full ORDER_TRADE_UPDATE JSON from AsterDEX docs (BTCUSDT TRAILING_STOP_MARKET example)
    const ORDER_TRADE_UPDATE_JSON: &str = r#"{
        "e": "ORDER_TRADE_UPDATE",
        "E": 1568879465651,
        "T": 1568879465650,
        "o": {
            "s": "BTCUSDT",
            "c": "TEST",
            "S": "SELL",
            "o": "TRAILING_STOP_MARKET",
            "f": "GTC",
            "q": "0.001",
            "p": "0",
            "ap": "0",
            "sp": "7103.04",
            "x": "NEW",
            "X": "NEW",
            "i": 8886774,
            "l": "0",
            "z": "0",
            "L": "0",
            "N": "USDT",
            "n": "0",
            "T": 1568879465650,
            "t": 0,
            "b": "0",
            "a": "9.91",
            "m": false,
            "R": false,
            "wt": "CONTRACT_PRICE",
            "ot": "TRAILING_STOP_MARKET",
            "ps": "LONG",
            "cp": false,
            "AP": "7476.89",
            "cr": "5.0",
            "pP": false,
            "si": 0,
            "ss": 0,
            "rp": "0"
        }
    }"#;

    #[test]
    fn test_deserialize_order_trade_update() {
        let update: OrderTradeUpdate =
            serde_json::from_str(ORDER_TRADE_UPDATE_JSON).expect("should deserialize");

        assert_eq!(update.e, "ORDER_TRADE_UPDATE");
        assert_eq!(update.event_time, 1568879465651);
        assert_eq!(update.transaction_time, 1568879465650);
        assert_eq!(update.o.s, "BTCUSDT");
        assert_eq!(update.o.side, "SELL");
        assert_eq!(update.o.order_type, "TRAILING_STOP_MARKET");
        assert_eq!(update.o.i, 8886774);
        assert_eq!(update.o.status, "NEW");
        assert_eq!(update.o.p, "0");
        assert_eq!(update.o.q, "0.001");
        assert_eq!(update.o.z, "0");
        assert_eq!(update.o.ap, "0");
        assert_eq!(update.o.trade_time, 1568879465650);
        assert_eq!(update.o.n, "0");
    }

    #[test]
    fn test_deserialize_user_data_event_type() {
        let json = r#"{"e": "ORDER_TRADE_UPDATE", "E": 123}"#;
        let event: UserDataEvent = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(event.e, "ORDER_TRADE_UPDATE");
    }

    #[test]
    fn test_deserialize_listen_key_expired() {
        let json = r#"{"e": "listenKeyExpired", "E": 1576653824250}"#;
        let event: UserDataEvent = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(event.e, "listenKeyExpired");
    }

    #[test]
    fn test_order_trade_update_to_order() {
        let update: OrderTradeUpdate =
            serde_json::from_str(ORDER_TRADE_UPDATE_JSON).expect("should deserialize");

        let order: Order = update.into();

        assert_eq!(order.order_id, 8886774);
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.time, 1568879465650); // trade_time from o.T
        assert_eq!(order.update_time, 1568879465651); // event_time from top-level E
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.order_type, OrderType::TrailingStopMarket);
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.price, Decimal::from(0));
        assert_eq!(order.orig_qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(order.executed_qty, Decimal::from(0));
        assert_eq!(order.avg_price, Decimal::from(0));
        // cum_quote = executed_qty * avg_price = 0 * 0 = 0
        assert_eq!(order.cum_quote, Decimal::from(0));
    }

    #[test]
    fn test_order_trade_update_to_order_with_filled_values() {
        // Test with non-zero filled values to verify cum_quote computation
        let json = r#"{
            "e": "ORDER_TRADE_UPDATE",
            "E": 1700000000000,
            "T": 1700000000000,
            "o": {
                "s": "ETHUSDT",
                "S": "BUY",
                "o": "LIMIT",
                "i": 99999,
                "X": "FILLED",
                "p": "2000.50",
                "q": "1.5",
                "z": "1.5",
                "ap": "2000.25",
                "T": 1700000000000,
                "n": "0.15"
            }
        }"#;

        let update: OrderTradeUpdate = serde_json::from_str(json).expect("should deserialize");
        let order: Order = update.into();

        assert_eq!(order.order_id, 99999);
        assert_eq!(order.symbol, "ETHUSDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.executed_qty, Decimal::from_str("1.5").unwrap());
        assert_eq!(order.avg_price, Decimal::from_str("2000.25").unwrap());
        // cum_quote = 1.5 * 2000.25 = 3000.375
        assert_eq!(order.cum_quote, Decimal::from_str("3000.375").unwrap());
    }

    #[test]
    fn test_order_update_payload_defaults() {
        // Test that commission `n` defaults gracefully when missing
        let json = r#"{
            "e": "ORDER_TRADE_UPDATE",
            "E": 1700000000000,
            "T": 1700000000000,
            "o": {
                "s": "BTCUSDT",
                "S": "BUY",
                "o": "MARKET",
                "i": 12345,
                "X": "NEW",
                "p": "50000",
                "q": "0.01",
                "z": "0",
                "ap": "0",
                "T": 1700000000000
            }
        }"#;

        let update: OrderTradeUpdate = serde_json::from_str(json).expect("should deserialize");
        // n defaults to empty string when missing
        assert_eq!(update.o.n, "");
    }

    /// Full ACCOUNT_UPDATE JSON from AsterDEX docs (ORDER fill closing BTCUSDT position)
    const ACCOUNT_UPDATE_JSON: &str = r#"{
        "e": "ACCOUNT_UPDATE",
        "E": 1564745798939,
        "T": 1564745798938,
        "a": {
            "m": "ORDER",
            "B": [
                {
                    "a": "USDT",
                    "wb": "122624.12345678",
                    "cw": "100.12345678",
                    "bc": "50.12345678"
                }
            ],
            "P": [
                {
                    "s": "BTCUSDT",
                    "pa": "0",
                    "ep": "0.00000",
                    "cr": "200",
                    "up": "0",
                    "mt": "isolated",
                    "iw": "0.00000000",
                    "ps": "BOTH"
                }
            ]
        }
    }"#;

    #[test]
    fn test_deserialize_account_update_event() {
        let event: AccountUpdateEvent =
            serde_json::from_str(ACCOUNT_UPDATE_JSON).expect("should deserialize");

        assert_eq!(event.e, "ACCOUNT_UPDATE");
        assert_eq!(event.event_time, 1564745798939);
        assert_eq!(event.transaction_time, 1564745798938);
        assert_eq!(event.a.m, "ORDER");

        // Balance assertions
        assert_eq!(event.a.balances.len(), 1);
        let bal = &event.a.balances[0];
        assert_eq!(bal.a, "USDT");
        assert_eq!(bal.wb, "122624.12345678");
        assert_eq!(bal.cw, "100.12345678");
        assert_eq!(bal.bc, "50.12345678");

        // Position assertions
        assert_eq!(event.a.positions.len(), 1);
        let pos = &event.a.positions[0];
        assert_eq!(pos.s, "BTCUSDT");
        assert_eq!(pos.pa, "0");
        assert_eq!(pos.ep, "0.00000");
        assert_eq!(pos.cr, "200");
        assert_eq!(pos.up, "0");
        assert_eq!(pos.mt, "isolated");
        assert_eq!(pos.iw, "0.00000000");
        assert_eq!(pos.ps, "BOTH");
    }

    #[test]
    fn test_account_update_event_to_data() {
        let event: AccountUpdateEvent =
            serde_json::from_str(ACCOUNT_UPDATE_JSON).expect("should deserialize");

        let data: AccountUpdateData = event.into();

        assert_eq!(data.event_time, 1564745798939);
        assert_eq!(data.reason, "ORDER");

        // Balance Decimal conversion
        assert_eq!(data.balances.len(), 1);
        let bal = &data.balances[0];
        assert_eq!(bal.asset, "USDT");
        assert_eq!(bal.wallet_balance, Decimal::from_str("122624.12345678").unwrap());
        assert_eq!(bal.cross_wallet_balance, Decimal::from_str("100.12345678").unwrap());
        assert_eq!(bal.balance_change, Decimal::from_str("50.12345678").unwrap());

        // Position Decimal conversion
        assert_eq!(data.positions.len(), 1);
        let pos = &data.positions[0];
        assert_eq!(pos.symbol, "BTCUSDT");
        assert_eq!(pos.position_amt, Decimal::from(0));
        assert_eq!(pos.entry_price, Decimal::from_str("0.00000").unwrap());
        assert_eq!(pos.accumulated_realized, Decimal::from(200));
        assert_eq!(pos.unrealized_pnl, Decimal::from(0));
        assert_eq!(pos.margin_type, "isolated");
        assert_eq!(pos.isolated_wallet, Decimal::from_str("0.00000000").unwrap());
        assert_eq!(pos.position_side, "BOTH");
    }

    #[test]
    fn test_deserialize_account_update_event_type() {
        let json = r#"{"e": "ACCOUNT_UPDATE", "E": 1564745798939}"#;
        let event: UserDataEvent = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(event.e, "ACCOUNT_UPDATE");
    }
}
