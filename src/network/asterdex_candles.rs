// src/network/asterdex_candles.rs
// AsterDEX REST kline and aggTrade data fetching with Binance-compatible parsing

use crate::data::types::{Candle, Trade, TradeSide};
use crate::network::cache::KlineCache;
use crate::network::kline::{validate_ohlc, Kline, KlineError};
use crate::network::reconnect::ExponentialBackoff;
use rust_decimal::Decimal;
use serde::Deserialize;

const ASTERDEX_KLINES_URL: &str = "https://fapi.asterdex.com/fapi/v1/klines";
const ASTERDEX_AGG_TRADES_URL: &str = "https://fapi.asterdex.com/fapi/v1/aggTrades";
const MAX_RETRIES: u32 = 3;

/// Fetch klines from AsterDEX REST API with retry
///
/// AsterDEX uses the standard 12-element JSON array kline format,
/// parsed via the shared Kline struct from the kline module.
///
/// # Arguments
/// * `symbol` - Trading pair (e.g., "BTCUSDT")
/// * `interval` - Interval string (e.g., "1m", "5m", "15m")
/// * `limit` - Number of candles to fetch (max 500)
/// * `client` - HTTP client
pub async fn fetch_asterdex_klines(
    symbol: &str,
    interval: &str,
    limit: u32,
    client: &reqwest::Client,
) -> Result<Vec<Kline>, KlineError> {
    let url = format!(
        "{}?symbol={}&interval={}&limit={}",
        ASTERDEX_KLINES_URL, symbol, interval, limit
    );

    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let klines: Vec<Kline> = response
                        .json()
                        .await
                        .map_err(|e| KlineError::Parse(e.to_string()))?;
                    return Ok(klines);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = KlineError::Api(status_code, body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "AsterDEX kline request failed (attempt {}/{}): {}. Retrying in {:?}",
                                attempts,
                                MAX_RETRIES,
                                error,
                                delay
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    return Err(error);
                }
            }
            Err(e) => {
                let error = if e.is_timeout() {
                    KlineError::Timeout
                } else {
                    KlineError::Http(e)
                };

                if error.is_retriable() && attempts < MAX_RETRIES {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!(
                            "AsterDEX kline request failed (attempt {}/{}): {}. Retrying in {:?}",
                            attempts,
                            MAX_RETRIES,
                            error,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
                return Err(error);
            }
        }
    }
}

/// Load historical klines from AsterDEX with caching and validation
///
/// Loads historical klines from AsterDEX with caching, validation, and OHLC checks.
/// and "asterdex" exchange prefix for cache filenames.
pub async fn load_asterdex_historical_klines(
    symbol: &str,
    interval: &str,
    limit: u32,
    client: &reqwest::Client,
) -> Result<Vec<Candle>, KlineError> {
    let cache = KlineCache::new();

    // Try cache first
    let klines: Vec<Kline> = if let Some(cached) = cache.read("asterdex", symbol, interval) {
        tracing::info!("Using cached AsterDEX klines for {}/{}", symbol, interval);
        cached
    } else {
        // Cache miss - fetch from API
        tracing::info!("Fetching AsterDEX klines from API for {}/{}", symbol, interval);
        let fetched = fetch_asterdex_klines(symbol, interval, limit, client).await?;

        // Write to cache on success
        if let Err(e) = cache.write("asterdex", symbol, interval, &fetched) {
            tracing::warn!("Failed to write AsterDEX klines to cache: {}", e);
        }

        fetched
    };

    // Convert and validate each kline
    let mut candles = Vec::with_capacity(klines.len());
    for (i, kline) in klines.into_iter().enumerate() {
        match Candle::try_from(kline) {
            Ok(candle) => {
                if validate_ohlc(&candle) {
                    candles.push(candle);
                } else {
                    tracing::warn!(
                        "Skipping AsterDEX kline {}: invalid OHLC (high={}, low={}, open={}, close={})",
                        i,
                        candle.high,
                        candle.low,
                        candle.open,
                        candle.close
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Skipping AsterDEX kline {}: conversion failed: {}", i, e);
            }
        }
    }

    Ok(candles)
}

/// AsterDEX aggregate trade from REST API
///
/// Used for tick-based bootstrap (fetching recent trades to populate initial chart).
/// Same JSON format as Binance aggTrades endpoint.
#[derive(Debug, Deserialize)]
pub struct AsterDexAggTrade {
    #[serde(rename = "a")]
    pub agg_trade_id: u64,
    #[serde(rename = "p")]
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(rename = "q")]
    #[serde(with = "rust_decimal::serde::str")]
    pub quantity: Decimal,
    #[serde(rename = "f")]
    #[allow(dead_code)] // Present for serde deserialization
    pub first_trade_id: u64,
    #[serde(rename = "l")]
    #[allow(dead_code)] // Present for serde deserialization
    pub last_trade_id: u64,
    #[serde(rename = "T")]
    pub timestamp: u64,
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,
}

impl From<AsterDexAggTrade> for Trade {
    fn from(agg: AsterDexAggTrade) -> Self {
        Trade {
            id: agg.agg_trade_id,
            price: agg.price,
            quantity: agg.quantity,
            timestamp: agg.timestamp,
            // is_buyer_maker = true means the buyer is the maker,
            // so the trade was initiated by a sell order
            side: if agg.is_buyer_maker {
                TradeSide::Sell
            } else {
                TradeSide::Buy
            },
            is_buyer_maker: agg.is_buyer_maker,
        }
    }
}

/// Fetch aggregate trades from AsterDEX REST API with retry
///
/// # Arguments
/// * `symbol` - Trading pair (e.g., "BTCUSDT")
/// * `limit` - Number of trades to fetch (max 1000 per AsterDEX API)
/// * `client` - HTTP client
pub async fn fetch_asterdex_agg_trades(
    symbol: &str,
    limit: u32,
    client: &reqwest::Client,
) -> Result<Vec<AsterDexAggTrade>, KlineError> {
    let url = format!(
        "{}?symbol={}&limit={}",
        ASTERDEX_AGG_TRADES_URL, symbol, limit
    );

    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let trades: Vec<AsterDexAggTrade> = response
                        .json()
                        .await
                        .map_err(|e| KlineError::Parse(e.to_string()))?;
                    return Ok(trades);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = KlineError::Api(status_code, body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "AsterDEX aggTrades request failed (attempt {}/{}): {}. Retrying in {:?}",
                                attempts,
                                MAX_RETRIES,
                                error,
                                delay
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    return Err(error);
                }
            }
            Err(e) => {
                let error = if e.is_timeout() {
                    KlineError::Timeout
                } else {
                    KlineError::Http(e)
                };

                if error.is_retriable() && attempts < MAX_RETRIES {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!(
                            "AsterDEX aggTrades request failed (attempt {}/{}): {}. Retrying in {:?}",
                            attempts,
                            MAX_RETRIES,
                            error,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::kline::Kline;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_asterdex_agg_trade_deserialization() {
        let json = r#"{
            "a": 12345678,
            "p": "43250.50",
            "q": "0.123",
            "f": 100000,
            "l": 100001,
            "T": 1699488000000,
            "m": true
        }"#;

        let trade: AsterDexAggTrade = serde_json::from_str(json).unwrap();
        assert_eq!(trade.agg_trade_id, 12345678);
        assert_eq!(trade.price, Decimal::from_str("43250.50").unwrap());
        assert_eq!(trade.quantity, Decimal::from_str("0.123").unwrap());
        assert_eq!(trade.first_trade_id, 100000);
        assert_eq!(trade.last_trade_id, 100001);
        assert_eq!(trade.timestamp, 1699488000000);
        assert!(trade.is_buyer_maker);
    }

    #[test]
    fn test_asterdex_agg_trade_to_trade_sell() {
        let agg = AsterDexAggTrade {
            agg_trade_id: 999,
            price: Decimal::from_str("50000.00").unwrap(),
            quantity: Decimal::from_str("1.5").unwrap(),
            first_trade_id: 1,
            last_trade_id: 2,
            timestamp: 1699488000000,
            is_buyer_maker: true, // buyer is maker -> sell initiated
        };

        let trade: Trade = agg.into();
        assert_eq!(trade.id, 999);
        assert_eq!(trade.price, Decimal::from_str("50000.00").unwrap());
        assert_eq!(trade.quantity, Decimal::from_str("1.5").unwrap());
        assert_eq!(trade.timestamp, 1699488000000);
        assert_eq!(trade.side, TradeSide::Sell);
        assert!(trade.is_buyer_maker);
    }

    #[test]
    fn test_asterdex_agg_trade_to_trade_buy() {
        let agg = AsterDexAggTrade {
            agg_trade_id: 1000,
            price: Decimal::from_str("50100.00").unwrap(),
            quantity: Decimal::from_str("0.5").unwrap(),
            first_trade_id: 3,
            last_trade_id: 4,
            timestamp: 1699488001000,
            is_buyer_maker: false, // buyer is taker -> buy initiated
        };

        let trade: Trade = agg.into();
        assert_eq!(trade.id, 1000);
        assert_eq!(trade.side, TradeSide::Buy);
        assert!(!trade.is_buyer_maker);
    }

    #[test]
    fn test_kline_deserialization_for_asterdex() {
        // AsterDEX uses the standard 12-element JSON array kline format
        let json = r#"[
            1699488000000,
            "43250.50",
            "43500.00",
            "43100.25",
            "43350.75",
            "500.123",
            1699488059999,
            "21675000.00",
            250,
            "300.5",
            "13000000.00",
            "0"
        ]"#;

        let kline: Kline = serde_json::from_str(json).unwrap();
        assert_eq!(kline.open_time, 1699488000000);
        assert_eq!(kline.open, "43250.50");
        assert_eq!(kline.high, "43500.00");
        assert_eq!(kline.low, "43100.25");
        assert_eq!(kline.close, "43350.75");
        assert_eq!(kline.volume, "500.123");
        assert_eq!(kline.close_time, 1699488059999);
        assert_eq!(kline.trades, 250);

        // Verify conversion to Candle works
        let candle: Candle = kline.try_into().unwrap();
        assert_eq!(candle.open, Decimal::from_str("43250.50").unwrap());
        assert_eq!(candle.high, Decimal::from_str("43500.00").unwrap());
        assert!(validate_ohlc(&candle));
    }
}
