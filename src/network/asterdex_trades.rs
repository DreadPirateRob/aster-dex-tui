// src/network/asterdex_trades.rs
// AsterDEX authenticated REST fetch for user trade history (GET /fapi/v1/userTrades)
// Used for daily realized PnL computation in the account overview.

use crate::config::AsterDexCredentials;
use crate::network::asterdex_auth::{sign_request, ASTERDEX_REST_URL, DEFAULT_RECV_WINDOW};
use crate::network::asterdex_trading::TradingError;
use crate::network::reconnect::ExponentialBackoff;
use rust_decimal::Decimal;
use serde::Deserialize;

/// Maximum retry attempts for transient failures
const MAX_RETRIES: u32 = 3;

/// AsterDEX user trade response from GET /fapi/v1/userTrades
///
/// Field names use camelCase per the Binance-compatible API.
/// Decimal fields are returned as JSON strings.
///
/// NOTE: `realized_pnl` is the realized profit/loss for this trade (can be negative).
/// NOTE: `commission` is the trading fee charged for this trade.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsterDexUserTrade {
    pub id: u64,
    pub symbol: String,
    pub order_id: u64,
    pub side: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub realized_pnl: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub commission: Decimal,
    pub commission_asset: String,
    pub time: u64,
    pub buyer: bool,
    pub maker: bool,
    pub position_side: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub quote_qty: Decimal,
}

/// Returns UTC midnight today as milliseconds since UNIX epoch.
///
/// Uses chrono to get the current UTC date, constructs midnight (00:00:00),
/// and converts to milliseconds. This avoids timezone pitfalls -- always UTC.
pub fn start_of_today_utc_ms() -> u64 {
    use chrono::Utc;
    let today = Utc::now().date_naive();
    let midnight = today.and_hms_opt(0, 0, 0).unwrap();
    midnight.and_utc().timestamp_millis() as u64
}

/// Fetch user trades from AsterDEX REST API with retry and authentication.
///
/// Sends an authenticated GET to /fapi/v1/userTrades with HMAC-SHA256 signed query.
/// Returns trades since `start_time_ms` with a limit of 1000 results.
/// Retries transient failures (timeouts, 429, 5xx) with exponential backoff.
///
/// If exactly 1000 trades are returned, a warning is logged because the result
/// may be truncated (Pitfall 7: pagination not implemented for daily PnL).
///
/// CRITICAL: The signed query is regenerated on each retry attempt because the
/// timestamp expires after the recvWindow (5 seconds), and backoff delays can
/// exceed this window.
///
/// # Arguments
/// * `start_time_ms` - Start time in milliseconds (e.g., from `start_of_today_utc_ms()`)
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(Vec<AsterDexUserTrade>)` - User trades since start_time_ms
/// * `Err(TradingError)` - If request fails after retries
pub async fn fetch_user_trades(
    start_time_ms: u64,
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<Vec<AsterDexUserTrade>, TradingError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        // Parameters: startTime for filtering, limit=1000 (max allowed)
        // NOTE: symbol is NOT passed -- fetches trades across all pairs for account-wide PnL
        let params = format!("startTime={}&limit=1000", start_time_ms);
        let (signed_query, _timestamp) =
            sign_request(&params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!(
            "{}/fapi/v1/userTrades?{}",
            ASTERDEX_REST_URL, signed_query
        );

        match client
            .get(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let trades: Vec<AsterDexUserTrade> = response
                        .json()
                        .await
                        .map_err(|e| TradingError::Parse(e.to_string()))?;

                    if trades.len() == 1000 {
                        tracing::warn!(
                            "userTrades returned 1000 results, daily PnL may be truncated"
                        );
                    }

                    return Ok(trades);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = TradingError::from_api_response(status_code, &body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "User trades fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
                            "User trades fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
    fn test_user_trade_deserialization() {
        let json = r#"{
            "id": 123456789,
            "symbol": "BTCUSDT",
            "orderId": 987654321,
            "side": "BUY",
            "price": "50250.50",
            "qty": "0.001",
            "realizedPnl": "12.345",
            "commission": "0.025",
            "commissionAsset": "USDT",
            "time": 1700000000000,
            "buyer": true,
            "maker": false,
            "positionSide": "BOTH",
            "quoteQty": "50.25050"
        }"#;

        let trade: AsterDexUserTrade = serde_json::from_str(json).unwrap();
        assert_eq!(trade.id, 123456789);
        assert_eq!(trade.symbol, "BTCUSDT");
        assert_eq!(trade.order_id, 987654321);
        assert_eq!(trade.side, "BUY");
        assert_eq!(trade.price, Decimal::from_str("50250.50").unwrap());
        assert_eq!(trade.qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(trade.realized_pnl, Decimal::from_str("12.345").unwrap());
        assert_eq!(trade.commission, Decimal::from_str("0.025").unwrap());
        assert_eq!(trade.commission_asset, "USDT");
        assert_eq!(trade.time, 1700000000000);
        assert!(trade.buyer);
        assert!(!trade.maker);
        assert_eq!(trade.position_side, "BOTH");
        assert_eq!(trade.quote_qty, Decimal::from_str("50.25050").unwrap());
    }

    #[test]
    fn test_user_trade_deserialization_negative_pnl() {
        let json = r#"{
            "id": 100,
            "symbol": "ETHUSDT",
            "orderId": 200,
            "side": "SELL",
            "price": "2900.00",
            "qty": "1.500",
            "realizedPnl": "-45.678",
            "commission": "0.100",
            "commissionAsset": "USDT",
            "time": 1700000001000,
            "buyer": false,
            "maker": true,
            "positionSide": "BOTH",
            "quoteQty": "4350.00"
        }"#;

        let trade: AsterDexUserTrade = serde_json::from_str(json).unwrap();
        assert_eq!(trade.realized_pnl, Decimal::from_str("-45.678").unwrap());
        assert!(trade.realized_pnl < Decimal::ZERO, "Realized PnL can be negative");
        assert!(trade.maker);
        assert!(!trade.buyer);
    }

    #[test]
    fn test_user_trade_deserialization_array() {
        let json = r#"[
            {
                "id": 1,
                "symbol": "BTCUSDT",
                "orderId": 10,
                "side": "BUY",
                "price": "50000.00",
                "qty": "0.01",
                "realizedPnl": "0",
                "commission": "0.025",
                "commissionAsset": "USDT",
                "time": 1700000000000,
                "buyer": true,
                "maker": false,
                "positionSide": "BOTH",
                "quoteQty": "500.00"
            },
            {
                "id": 2,
                "symbol": "ETHUSDT",
                "orderId": 20,
                "side": "SELL",
                "price": "3000.00",
                "qty": "1.0",
                "realizedPnl": "25.50",
                "commission": "0.150",
                "commissionAsset": "USDT",
                "time": 1700000001000,
                "buyer": false,
                "maker": true,
                "positionSide": "BOTH",
                "quoteQty": "3000.00"
            }
        ]"#;

        let trades: Vec<AsterDexUserTrade> = serde_json::from_str(json).unwrap();
        assert_eq!(trades.len(), 2);
        assert_eq!(trades[0].symbol, "BTCUSDT");
        assert_eq!(trades[0].realized_pnl, Decimal::ZERO);
        assert_eq!(trades[1].symbol, "ETHUSDT");
        assert_eq!(trades[1].realized_pnl, Decimal::from_str("25.50").unwrap());
    }

    #[test]
    fn test_start_of_today_utc_ms() {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        let today_ms = start_of_today_utc_ms();

        // Must be less than or equal to current time
        assert!(today_ms <= now_ms, "Start of today must be <= now");

        // Must be a round day boundary (divisible by 86_400_000 ms = 24h)
        assert_eq!(
            today_ms % 86_400_000,
            0,
            "Start of today must be on a day boundary (divisible by 86400000)"
        );

        // Must be within the last 24 hours
        let diff = now_ms - today_ms;
        assert!(
            diff < 86_400_000,
            "Start of today must be within the last 24 hours"
        );
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires valid API credentials)
    async fn test_fetch_user_trades_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let start_ms = start_of_today_utc_ms();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let trades = fetch_user_trades(start_ms, &credentials, &client)
            .await
            .expect("Should fetch user trades");

        println!("Fetched {} trades since today UTC midnight", trades.len());
        for trade in &trades {
            println!(
                "  {} {} {} qty={} price={} pnl={} commission={}",
                trade.symbol,
                trade.side,
                trade.position_side,
                trade.qty,
                trade.price,
                trade.realized_pnl,
                trade.commission,
            );
        }
    }
}
