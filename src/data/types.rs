use rust_decimal::Decimal;

/// Trade side (buy or sell)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeSide {
    Buy,
    Sell,
}

/// Internal normalized trade representation
#[derive(Debug, Clone)]
pub struct Trade {
    pub id: u64,
    pub price: Decimal,
    pub quantity: Decimal,
    pub timestamp: u64, // milliseconds since epoch
    pub side: TradeSide,
    /// Whether the buyer is the market maker (true = sell initiated, false = buy initiated)
    pub is_buyer_maker: bool,
}

/// OHLCV candlestick
#[derive(Debug, Clone)]
pub struct Candle {
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
    /// Taker buy volume (buyer was aggressor, is_buyer_maker=false)
    pub buy_volume: Decimal,
    /// Taker sell volume (seller was aggressor, is_buyer_maker=true)
    pub sell_volume: Decimal,
    #[allow(dead_code)] // Used in candle aggregation output
    pub trade_count: u32,
    pub open_time: u64,
    #[allow(dead_code)] // Used in candle aggregation output
    pub close_time: u64,
}
