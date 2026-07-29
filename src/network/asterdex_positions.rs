// src/network/asterdex_positions.rs
// AsterDEX authenticated REST fetch for position risk data (GET /fapi/v2/positionRisk)

use crate::config::AsterDexCredentials;
use crate::network::asterdex_auth::{sign_request, ASTERDEX_REST_URL, DEFAULT_RECV_WINDOW};
use crate::network::asterdex_trading::TradingError;
use crate::network::reconnect::ExponentialBackoff;
use rust_decimal::Decimal;
use serde::Deserialize;

/// Maximum retry attempts for transient failures
const MAX_RETRIES: u32 = 3;

/// AsterDEX position risk response from GET /fapi/v2/positionRisk
///
/// Field names use camelCase per the Binance-compatible API.
/// Decimal fields are returned as JSON strings.
///
/// NOTE: `position_amt` is SIGNED -- positive for long, negative for short.
/// NOTE: `un_realized_profit` maps to JSON field `unRealizedProfit` via camelCase rename.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsterDexPosition {
    pub symbol: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub position_amt: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub entry_price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub mark_price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub un_realized_profit: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub liquidation_price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub leverage: Decimal,
    pub margin_type: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub isolated_margin: Decimal,
    pub position_side: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub notional: Decimal,
    pub update_time: u64,
}

/// Fetch all positions from AsterDEX REST API with retry and authentication.
///
/// Sends an authenticated GET to /fapi/v2/positionRisk with HMAC-SHA256 signed query.
/// Returns ALL positions (including zero-size) -- caller filters when converting.
/// Retries transient failures (timeouts, 429, 5xx) with exponential backoff.
///
/// CRITICAL: The signed query is regenerated on each retry attempt because the
/// timestamp expires after the recvWindow (5 seconds), and backoff delays can
/// exceed this window.
///
/// # Arguments
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(Vec<AsterDexPosition>)` - Raw API positions (includes zero-size entries)
/// * `Err(TradingError)` - If request fails after retries
pub async fn fetch_positions(
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<Vec<AsterDexPosition>, TradingError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        // No params -- returns positions for ALL symbols
        let params = "";
        let (signed_query, _timestamp) =
            sign_request(params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v2/positionRisk?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .get(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let positions: Vec<AsterDexPosition> = response
                        .json()
                        .await
                        .map_err(|e| TradingError::Parse(e.to_string()))?;
                    return Ok(positions);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = TradingError::from_api_response(status_code, &body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Position fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
                    TradingError::Timeout
                } else {
                    TradingError::Http(e)
                };

                if error.is_retriable() && attempts < MAX_RETRIES {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!(
                            "Position fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_deserialize_asterdex_position() {
        let json = r#"{
            "symbol": "BTCUSDT",
            "positionAmt": "0.500",
            "entryPrice": "50000.00",
            "markPrice": "51000.50",
            "unRealizedProfit": "500.25",
            "liquidationPrice": "40000.00",
            "leverage": "10",
            "marginType": "isolated",
            "isolatedMargin": "2500.00",
            "positionSide": "BOTH",
            "notional": "25500.25",
            "updateTime": 1700000000000
        }"#;

        let pos: AsterDexPosition = serde_json::from_str(json).unwrap();
        assert_eq!(pos.symbol, "BTCUSDT");
        assert_eq!(pos.position_amt, Decimal::from_str("0.500").unwrap());
        assert_eq!(pos.entry_price, Decimal::from_str("50000.00").unwrap());
        assert_eq!(pos.mark_price, Decimal::from_str("51000.50").unwrap());
        assert_eq!(pos.un_realized_profit, Decimal::from_str("500.25").unwrap());
        assert_eq!(pos.liquidation_price, Decimal::from_str("40000.00").unwrap());
        assert_eq!(pos.leverage, Decimal::from_str("10").unwrap());
        assert_eq!(pos.margin_type, "isolated");
        assert_eq!(pos.isolated_margin, Decimal::from_str("2500.00").unwrap());
        assert_eq!(pos.position_side, "BOTH");
        assert_eq!(pos.notional, Decimal::from_str("25500.25").unwrap());
        assert_eq!(pos.update_time, 1700000000000);
    }

    #[test]
    fn test_deserialize_position_array() {
        let json = r#"[
            {
                "symbol": "BTCUSDT",
                "positionAmt": "0.100",
                "entryPrice": "50000.00",
                "markPrice": "51000.00",
                "unRealizedProfit": "100.00",
                "liquidationPrice": "40000.00",
                "leverage": "20",
                "marginType": "cross",
                "isolatedMargin": "0",
                "positionSide": "BOTH",
                "notional": "5100.00",
                "updateTime": 1700000000000
            },
            {
                "symbol": "ETHUSDT",
                "positionAmt": "-2.000",
                "entryPrice": "3000.00",
                "markPrice": "2900.00",
                "unRealizedProfit": "200.00",
                "liquidationPrice": "5000.00",
                "leverage": "5",
                "marginType": "isolated",
                "isolatedMargin": "1200.00",
                "positionSide": "BOTH",
                "notional": "-5800.00",
                "updateTime": 1700000001000
            }
        ]"#;

        let positions: Vec<AsterDexPosition> = serde_json::from_str(json).unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].symbol, "BTCUSDT");
        assert_eq!(positions[0].position_amt, Decimal::from_str("0.100").unwrap());
        assert_eq!(positions[1].symbol, "ETHUSDT");
        assert_eq!(positions[1].position_amt, Decimal::from_str("-2.000").unwrap());
        assert_eq!(positions[1].un_realized_profit, Decimal::from_str("200.00").unwrap());
    }

    #[test]
    fn test_deserialize_zero_position_amt() {
        let json = r#"{
            "symbol": "BTCUSDT",
            "positionAmt": "0",
            "entryPrice": "0",
            "markPrice": "50000.00",
            "unRealizedProfit": "0",
            "liquidationPrice": "0",
            "leverage": "20",
            "marginType": "cross",
            "isolatedMargin": "0",
            "positionSide": "BOTH",
            "notional": "0",
            "updateTime": 0
        }"#;

        let pos: AsterDexPosition = serde_json::from_str(json).unwrap();
        assert_eq!(pos.position_amt, Decimal::ZERO);
        assert_eq!(pos.entry_price, Decimal::ZERO);
        assert_eq!(pos.un_realized_profit, Decimal::ZERO);
    }

    #[test]
    fn test_deserialize_negative_position_amt() {
        let json = r#"{
            "symbol": "BTCUSDT",
            "positionAmt": "-0.5",
            "entryPrice": "50000.00",
            "markPrice": "51000.00",
            "unRealizedProfit": "-500.00",
            "liquidationPrice": "60000.00",
            "leverage": "10",
            "marginType": "isolated",
            "isolatedMargin": "2500.00",
            "positionSide": "BOTH",
            "notional": "-25500.00",
            "updateTime": 1700000000000
        }"#;

        let pos: AsterDexPosition = serde_json::from_str(json).unwrap();
        assert_eq!(pos.position_amt, Decimal::from_str("-0.5").unwrap());
        assert!(pos.position_amt < Decimal::ZERO, "Short position should be negative");
        assert_eq!(pos.un_realized_profit, Decimal::from_str("-500.00").unwrap());
        assert!(pos.un_realized_profit < Decimal::ZERO, "Unrealized profit can be negative");
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires valid API credentials)
    async fn test_fetch_positions_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let positions = fetch_positions(&credentials, &client)
            .await
            .expect("Should fetch positions");

        println!("Fetched {} positions", positions.len());
        for pos in &positions {
            if pos.position_amt != Decimal::ZERO {
                println!(
                    "  {} amt={} entry={} mark={} pnl={} lev={}x {}",
                    pos.symbol,
                    pos.position_amt,
                    pos.entry_price,
                    pos.mark_price,
                    pos.un_realized_profit,
                    pos.leverage,
                    pos.margin_type,
                );
            }
        }
    }
}
