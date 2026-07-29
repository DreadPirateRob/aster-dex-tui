// src/network/kline.rs
// Kline (candlestick) data structures and parsing

use crate::data::types::Candle;
use rust_decimal::Decimal;
use serde_tuple::{Deserialize_tuple, Serialize_tuple};
use std::str::FromStr;

/// Kline (candlestick) data from REST API
///
/// Deserializes from 12-element JSON array format used by exchanges:
/// [open_time, open, high, low, close, volume, close_time, quote_volume, trades, taker_buy_base, taker_buy_quote, ignore]
#[derive(Debug, Clone, Deserialize_tuple, Serialize_tuple)]
pub struct Kline {
    pub open_time: u64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub close_time: u64,
    pub quote_volume: String,
    pub trades: u32,
    pub taker_buy_base: String,
    pub taker_buy_quote: String,
    pub ignore: String,
}

/// Errors that can occur during kline operations
#[derive(Debug, thiserror::Error)]
pub enum KlineError {
    #[error("Failed to parse decimal: {0}")]
    DecimalParse(#[from] rust_decimal::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Request timeout")]
    Timeout,
    #[error("JSON parse error: {0}")]
    Parse(String),
    #[error("API error {0}: {1}")]
    Api(u16, String),
}

impl KlineError {
    /// Returns true if the error is retriable (transient failure)
    pub fn is_retriable(&self) -> bool {
        match self {
            KlineError::Timeout => true,
            KlineError::Http(_) => true,
            KlineError::Api(status, _) if *status >= 500 => true,
            _ => false,
        }
    }
}

impl TryFrom<Kline> for Candle {
    type Error = KlineError;

    fn try_from(kline: Kline) -> Result<Self, Self::Error> {
        let volume = Decimal::from_str(&kline.volume)?;
        let buy_volume = Decimal::from_str(&kline.taker_buy_base)?;
        let sell_volume = volume - buy_volume;

        Ok(Candle {
            open_time: kline.open_time,
            close_time: kline.close_time,
            open: Decimal::from_str(&kline.open)?,
            high: Decimal::from_str(&kline.high)?,
            low: Decimal::from_str(&kline.low)?,
            close: Decimal::from_str(&kline.close)?,
            volume,
            buy_volume,
            sell_volume,
            trade_count: kline.trades,
        })
    }
}

/// Validate OHLC rules for a candle
///
/// Returns true if:
/// - high >= open
/// - high >= close
/// - low <= open
/// - low <= close
pub fn validate_ohlc(candle: &Candle) -> bool {
    candle.high >= candle.open
        && candle.high >= candle.close
        && candle.low <= candle.open
        && candle.low <= candle.close
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_deserialize_kline() {
        let json = r#"[
            1699488000000,
            "36000.00",
            "36500.50",
            "35800.25",
            "36200.75",
            "100.5",
            1699488059999,
            "3620000.00",
            150,
            "60.3",
            "2172000.00",
            "0"
        ]"#;
        let kline: Kline = serde_json::from_str(json).unwrap();
        assert_eq!(kline.open_time, 1699488000000);
        assert_eq!(kline.open, "36000.00");
        assert_eq!(kline.high, "36500.50");
        assert_eq!(kline.low, "35800.25");
        assert_eq!(kline.close, "36200.75");
        assert_eq!(kline.volume, "100.5");
        assert_eq!(kline.close_time, 1699488059999);
        assert_eq!(kline.trades, 150);
        assert_eq!(kline.taker_buy_base, "60.3");
    }

    #[test]
    fn test_convert_kline_to_candle() {
        use crate::data::types::Candle;

        let kline = Kline {
            open_time: 1699488000000,
            open: "36000.00".to_string(),
            high: "36500.50".to_string(),
            low: "35800.25".to_string(),
            close: "36200.75".to_string(),
            volume: "100.5".to_string(),
            close_time: 1699488059999,
            quote_volume: "3620000.00".to_string(),
            trades: 150,
            taker_buy_base: "60.3".to_string(),
            taker_buy_quote: "2172000.00".to_string(),
            ignore: "0".to_string(),
        };
        let candle: Candle = kline.try_into().unwrap();
        assert_eq!(candle.open_time, 1699488000000);
        assert_eq!(candle.open, Decimal::from_str("36000.00").unwrap());
        assert_eq!(candle.buy_volume, Decimal::from_str("60.3").unwrap());
        // sell_volume = volume - taker_buy_base = 100.5 - 60.3 = 40.2
        assert_eq!(candle.sell_volume, Decimal::from_str("40.2").unwrap());
    }

    #[test]
    fn test_validate_ohlc_valid() {
        use crate::data::types::Candle;

        let candle = Candle {
            open_time: 0,
            close_time: 1,
            open: Decimal::from(100),
            high: Decimal::from(110),
            low: Decimal::from(90),
            close: Decimal::from(105),
            volume: Decimal::from(1000),
            buy_volume: Decimal::from(600),
            sell_volume: Decimal::from(400),
            trade_count: 10,
        };
        assert!(validate_ohlc(&candle));
    }

    #[test]
    fn test_validate_ohlc_invalid_high_less_than_low() {
        use crate::data::types::Candle;

        let candle = Candle {
            open_time: 0,
            close_time: 1,
            open: Decimal::from(100),
            high: Decimal::from(80), // Invalid: high < low
            low: Decimal::from(90),
            close: Decimal::from(85),
            volume: Decimal::from(1000),
            buy_volume: Decimal::from(600),
            sell_volume: Decimal::from(400),
            trade_count: 10,
        };
        assert!(!validate_ohlc(&candle));
    }

    #[test]
    fn test_decimal_precision_preserved() {
        use crate::data::types::Candle;

        let kline = Kline {
            open_time: 0,
            open: "0.00000100".to_string(),
            high: "0.00000100".to_string(),
            low: "0.00000100".to_string(),
            close: "0.00000100".to_string(),
            volume: "1000000.00000000".to_string(),
            close_time: 1,
            quote_volume: "0".to_string(),
            trades: 1,
            taker_buy_base: "500000.00000000".to_string(),
            taker_buy_quote: "0".to_string(),
            ignore: "0".to_string(),
        };
        let candle: Candle = kline.try_into().unwrap();
        // Precision preserved - not 0.000001 (6 places) but full 0.00000100 (8 places)
        assert_eq!(candle.open.to_string(), "0.00000100");
    }
}
