// src/data/order.rs
// Internal Order types for AsterDEX order history

use rust_decimal::Decimal;
use std::fmt;

/// Side of an order (buy or sell)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "BUY"),
            OrderSide::Sell => write!(f, "SELL"),
        }
    }
}

/// Status of an order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Expired,
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderStatus::New => write!(f, "NEW"),
            OrderStatus::PartiallyFilled => write!(f, "PARTIALLY_FILLED"),
            OrderStatus::Filled => write!(f, "FILLED"),
            OrderStatus::Canceled => write!(f, "CANCELED"),
            OrderStatus::Rejected => write!(f, "REJECTED"),
            OrderStatus::Expired => write!(f, "EXPIRED"),
        }
    }
}

/// Type of an order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Limit,
    Market,
    Stop,
    StopMarket,
    TakeProfit,
    TakeProfitMarket,
    TrailingStopMarket,
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Limit => write!(f, "LIMIT"),
            OrderType::Market => write!(f, "MARKET"),
            OrderType::Stop => write!(f, "STOP"),
            OrderType::StopMarket => write!(f, "STOP_MARKET"),
            OrderType::TakeProfit => write!(f, "TAKE_PROFIT"),
            OrderType::TakeProfitMarket => write!(f, "TAKE_PROFIT_MARKET"),
            OrderType::TrailingStopMarket => write!(f, "TRAILING_STOP_MARKET"),
        }
    }
}

/// Time-in-force policy for limit orders
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeInForce {
    /// Good 'Til Canceled
    Gtc,
    /// Immediate or Cancel
    Ioc,
    /// Fill or Kill
    Fok,
    /// Good 'Til Crossing (post-only)
    Gtx,
}

impl TimeInForce {
    /// Cycle to the next time-in-force value: GTC -> IOC -> FOK -> GTX -> GTC
    pub fn cycle(self) -> Self {
        match self {
            TimeInForce::Gtc => TimeInForce::Ioc,
            TimeInForce::Ioc => TimeInForce::Fok,
            TimeInForce::Fok => TimeInForce::Gtx,
            TimeInForce::Gtx => TimeInForce::Gtc,
        }
    }

    /// Cycle to the previous time-in-force value (reverse of cycle)
    pub fn cycle_back(self) -> Self {
        match self {
            TimeInForce::Gtc => TimeInForce::Gtx,
            TimeInForce::Ioc => TimeInForce::Gtc,
            TimeInForce::Fok => TimeInForce::Ioc,
            TimeInForce::Gtx => TimeInForce::Fok,
        }
    }

    /// Human-readable label for display
    pub fn label(&self) -> &'static str {
        match self {
            TimeInForce::Gtc => "GTC",
            TimeInForce::Ioc => "IOC",
            TimeInForce::Fok => "FOK",
            TimeInForce::Gtx => "GTX",
        }
    }
}

impl fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeInForce::Gtc => write!(f, "GTC"),
            TimeInForce::Ioc => write!(f, "IOC"),
            TimeInForce::Fok => write!(f, "FOK"),
            TimeInForce::Gtx => write!(f, "GTX"),
        }
    }
}

/// Internal Order struct representing an AsterDEX futures order.
///
/// Converted from `AsterDexOrder` API response type via the `From` trait.
/// Uses parsed enums for side/status/type instead of raw strings.
#[derive(Debug, Clone)]
pub struct Order {
    pub order_id: u64,
    pub symbol: String,
    pub time: u64,
    pub update_time: u64,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub price: Decimal,
    pub orig_qty: Decimal,
    pub executed_qty: Decimal,
    pub avg_price: Decimal,
    pub cum_quote: Decimal,
    pub status: OrderStatus,
    /// Realized PnL from this fill (from ORDER_TRADE_UPDATE rp field).
    /// Only populated for WebSocket orders; REST orders default to ZERO.
    pub realized_pnl: Decimal,
    /// Commission charged for this fill (from ORDER_TRADE_UPDATE n field).
    /// Only populated for WebSocket orders; REST orders default to ZERO.
    pub commission: Decimal,
}

/// Parse an order side from the API string representation.
/// Defaults to `Buy` for unknown values.
pub fn parse_order_side(s: &str) -> OrderSide {
    match s {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => OrderSide::Buy,
    }
}

/// Parse an order status from the API string representation.
/// Defaults to `New` for unknown values.
pub fn parse_order_status(s: &str) -> OrderStatus {
    match s {
        "NEW" => OrderStatus::New,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "FILLED" => OrderStatus::Filled,
        "CANCELED" => OrderStatus::Canceled,
        "REJECTED" => OrderStatus::Rejected,
        "EXPIRED" => OrderStatus::Expired,
        _ => OrderStatus::New,
    }
}

/// Parse an order type from the API string representation.
/// Defaults to `Limit` for unknown values.
pub fn parse_order_type(s: &str) -> OrderType {
    match s {
        "LIMIT" => OrderType::Limit,
        "MARKET" => OrderType::Market,
        "STOP" => OrderType::Stop,
        "STOP_MARKET" => OrderType::StopMarket,
        "TAKE_PROFIT" => OrderType::TakeProfit,
        "TAKE_PROFIT_MARKET" => OrderType::TakeProfitMarket,
        "TRAILING_STOP_MARKET" => OrderType::TrailingStopMarket,
        _ => OrderType::Limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_order_side_display() {
        assert_eq!(format!("{}", OrderSide::Buy), "BUY");
        assert_eq!(format!("{}", OrderSide::Sell), "SELL");
    }

    #[test]
    fn test_order_status_display() {
        assert_eq!(format!("{}", OrderStatus::New), "NEW");
        assert_eq!(format!("{}", OrderStatus::PartiallyFilled), "PARTIALLY_FILLED");
        assert_eq!(format!("{}", OrderStatus::Filled), "FILLED");
        assert_eq!(format!("{}", OrderStatus::Canceled), "CANCELED");
        assert_eq!(format!("{}", OrderStatus::Rejected), "REJECTED");
        assert_eq!(format!("{}", OrderStatus::Expired), "EXPIRED");
    }

    #[test]
    fn test_time_in_force_display() {
        assert_eq!(format!("{}", TimeInForce::Gtc), "GTC");
        assert_eq!(format!("{}", TimeInForce::Ioc), "IOC");
        assert_eq!(format!("{}", TimeInForce::Fok), "FOK");
        assert_eq!(format!("{}", TimeInForce::Gtx), "GTX");
    }

    #[test]
    fn test_order_type_display() {
        assert_eq!(format!("{}", OrderType::Limit), "LIMIT");
        assert_eq!(format!("{}", OrderType::Market), "MARKET");
        assert_eq!(format!("{}", OrderType::Stop), "STOP");
        assert_eq!(format!("{}", OrderType::StopMarket), "STOP_MARKET");
        assert_eq!(format!("{}", OrderType::TakeProfit), "TAKE_PROFIT");
        assert_eq!(format!("{}", OrderType::TakeProfitMarket), "TAKE_PROFIT_MARKET");
        assert_eq!(format!("{}", OrderType::TrailingStopMarket), "TRAILING_STOP_MARKET");
    }

    #[test]
    fn test_parse_order_side() {
        assert_eq!(parse_order_side("BUY"), OrderSide::Buy);
        assert_eq!(parse_order_side("SELL"), OrderSide::Sell);
        // Unknown defaults to Buy
        assert_eq!(parse_order_side("UNKNOWN"), OrderSide::Buy);
        assert_eq!(parse_order_side(""), OrderSide::Buy);
    }

    #[test]
    fn test_parse_order_status() {
        assert_eq!(parse_order_status("NEW"), OrderStatus::New);
        assert_eq!(parse_order_status("PARTIALLY_FILLED"), OrderStatus::PartiallyFilled);
        assert_eq!(parse_order_status("FILLED"), OrderStatus::Filled);
        assert_eq!(parse_order_status("CANCELED"), OrderStatus::Canceled);
        assert_eq!(parse_order_status("REJECTED"), OrderStatus::Rejected);
        assert_eq!(parse_order_status("EXPIRED"), OrderStatus::Expired);
        // Unknown defaults to New
        assert_eq!(parse_order_status("UNKNOWN"), OrderStatus::New);
        assert_eq!(parse_order_status(""), OrderStatus::New);
    }

    #[test]
    fn test_parse_order_type() {
        assert_eq!(parse_order_type("LIMIT"), OrderType::Limit);
        assert_eq!(parse_order_type("MARKET"), OrderType::Market);
        assert_eq!(parse_order_type("STOP"), OrderType::Stop);
        assert_eq!(parse_order_type("STOP_MARKET"), OrderType::StopMarket);
        assert_eq!(parse_order_type("TAKE_PROFIT"), OrderType::TakeProfit);
        assert_eq!(parse_order_type("TAKE_PROFIT_MARKET"), OrderType::TakeProfitMarket);
        assert_eq!(parse_order_type("TRAILING_STOP_MARKET"), OrderType::TrailingStopMarket);
        // Unknown defaults to Limit
        assert_eq!(parse_order_type("UNKNOWN"), OrderType::Limit);
        assert_eq!(parse_order_type(""), OrderType::Limit);
    }

    #[test]
    fn test_time_in_force_cycle() {
        let tif = TimeInForce::Gtc;
        let tif = tif.cycle();
        assert_eq!(tif, TimeInForce::Ioc);
        let tif = tif.cycle();
        assert_eq!(tif, TimeInForce::Fok);
        let tif = tif.cycle();
        assert_eq!(tif, TimeInForce::Gtx);
        let tif = tif.cycle();
        assert_eq!(tif, TimeInForce::Gtc); // wraps around
    }

    #[test]
    fn test_time_in_force_cycle_back() {
        let tif = TimeInForce::Gtc;
        let tif = tif.cycle_back();
        assert_eq!(tif, TimeInForce::Gtx);
        let tif = tif.cycle_back();
        assert_eq!(tif, TimeInForce::Fok);
        let tif = tif.cycle_back();
        assert_eq!(tif, TimeInForce::Ioc);
        let tif = tif.cycle_back();
        assert_eq!(tif, TimeInForce::Gtc); // wraps around
    }

    #[test]
    fn test_time_in_force_label() {
        assert_eq!(TimeInForce::Gtc.label(), "GTC");
        assert_eq!(TimeInForce::Ioc.label(), "IOC");
        assert_eq!(TimeInForce::Fok.label(), "FOK");
        assert_eq!(TimeInForce::Gtx.label(), "GTX");
    }

    #[test]
    fn test_order_struct_creation() {
        let order = Order {
            order_id: 12345,
            symbol: "BTCUSDT".to_string(),
            time: 1700000000000,
            update_time: 1700000001000,
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            price: Decimal::from_str("50000.00").unwrap(),
            orig_qty: Decimal::from_str("0.001").unwrap(),
            executed_qty: Decimal::from_str("0.001").unwrap(),
            avg_price: Decimal::from_str("49999.50").unwrap(),
            cum_quote: Decimal::from_str("49.9995").unwrap(),
            status: OrderStatus::Filled,
            realized_pnl: Decimal::ZERO,
            commission: Decimal::ZERO,
        };

        assert_eq!(order.order_id, 12345);
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Filled);
    }
}
