// src/network/asterdex_orders.rs
// AsterDEX authenticated REST fetch for order history (GET /fapi/v1/allOrders)

use crate::config::AsterDexCredentials;
use crate::data::order::{parse_order_side, parse_order_status, parse_order_type, Order};
use crate::network::asterdex_auth::{sign_request, ASTERDEX_REST_URL, DEFAULT_RECV_WINDOW};
use crate::network::reconnect::ExponentialBackoff;
use rust_decimal::Decimal;
use serde::Deserialize;

/// Maximum retry attempts for transient failures
const MAX_RETRIES: u32 = 3;

/// Maximum orders per request (AsterDEX/Binance limit)
const ORDER_LIMIT: u32 = 500;

/// AsterDEX order response from GET /fapi/v1/allOrders
///
/// Field names use camelCase per the Binance-compatible API.
/// Decimal fields are returned as JSON strings.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsterDexOrder {
    pub order_id: u64,
    pub symbol: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub orig_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub executed_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub avg_price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub cum_quote: Decimal,
    pub time: u64,
    pub update_time: u64,
}

impl From<AsterDexOrder> for Order {
    fn from(api: AsterDexOrder) -> Self {
        Order {
            order_id: api.order_id,
            symbol: api.symbol,
            time: api.time,
            update_time: api.update_time,
            side: parse_order_side(&api.side),
            order_type: parse_order_type(&api.order_type),
            status: parse_order_status(&api.status),
            price: api.price,
            orig_qty: api.orig_qty,
            executed_qty: api.executed_qty,
            avg_price: api.avg_price,
            cum_quote: api.cum_quote,
            realized_pnl: Decimal::ZERO,
            commission: Decimal::ZERO,
        }
    }
}

/// Errors that can occur during order fetch operations
#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Request timeout")]
    Timeout,
    #[error("API error {0}: {1}")]
    Api(u16, String),
    #[error("JSON parse error: {0}")]
    Parse(String),
}

impl OrderError {
    /// Returns true if the error is retriable (transient failure)
    pub fn is_retriable(&self) -> bool {
        match self {
            OrderError::Timeout => true,
            OrderError::Http(_) => true,
            OrderError::Api(status, _) if *status >= 500 || *status == 429 => true,
            _ => false,
        }
    }
}

/// Fetch orders from AsterDEX REST API with retry and authentication.
///
/// Sends an authenticated GET to /fapi/v1/allOrders with HMAC-SHA256 signed query.
/// Retries transient failures (timeouts, 429, 5xx) with exponential backoff.
///
/// CRITICAL: The signed query is regenerated on each retry attempt because the
/// timestamp expires after the recvWindow (5 seconds), and backoff delays can
/// exceed this window.
///
/// # Arguments
/// * `symbol` - Trading pair in AsterDEX format (e.g., "BTCUSDT")
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(Vec<AsterDexOrder>)` - Raw API orders (convert to `Order` via `From` trait)
/// * `Err(OrderError)` - If request fails after retries
pub async fn fetch_orders(
    symbol: &str,
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<Vec<AsterDexOrder>, OrderError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        // Regenerate signed query on each attempt (fresh timestamp)
        let params = format!("symbol={}&limit={}", symbol, ORDER_LIMIT);
        let (signed_query, _timestamp) =
            sign_request(&params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v1/allOrders?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .get(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let orders: Vec<AsterDexOrder> = response
                        .json()
                        .await
                        .map_err(|e| OrderError::Parse(e.to_string()))?;
                    return Ok(orders);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = OrderError::Api(status_code, body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Order fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
                    OrderError::Timeout
                } else {
                    OrderError::Http(e)
                };

                if error.is_retriable() && attempts < MAX_RETRIES {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!(
                            "Order fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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

/// Fetch currently open orders from AsterDEX for ALL trading pairs.
///
/// Sends an authenticated GET to /fapi/v1/openOrders WITHOUT a symbol parameter,
/// returning open orders across all pairs. API weight: 40 (vs 1 for single-symbol).
///
/// Uses the same retry/backoff pattern as `fetch_orders`. The signed query is
/// regenerated on each retry attempt (fresh timestamp).
///
/// # Arguments
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(Vec<AsterDexOrder>)` - Raw API orders for all pairs (convert via `From` trait)
/// * `Err(OrderError)` - If request fails after retries
pub async fn fetch_open_orders(
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<Vec<AsterDexOrder>, OrderError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        // No symbol param -- returns open orders for ALL pairs
        let params = "";
        let (signed_query, _timestamp) =
            sign_request(params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v1/openOrders?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .get(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let orders: Vec<AsterDexOrder> = response
                        .json()
                        .await
                        .map_err(|e| OrderError::Parse(e.to_string()))?;
                    return Ok(orders);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = OrderError::Api(status_code, body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Open orders fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
                    OrderError::Timeout
                } else {
                    OrderError::Http(e)
                };

                if error.is_retriable() && attempts < MAX_RETRIES {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!(
                            "Open orders fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
    fn test_deserialize_asterdex_order() {
        let json = r#"{
            "orderId": 12345678,
            "symbol": "BTCUSDT",
            "side": "BUY",
            "type": "LIMIT",
            "status": "FILLED",
            "price": "50000.00",
            "origQty": "0.001",
            "executedQty": "0.001",
            "avgPrice": "49999.50",
            "cumQuote": "49.99950000",
            "time": 1700000000000,
            "updateTime": 1700000001000
        }"#;

        let order: AsterDexOrder = serde_json::from_str(json).unwrap();
        assert_eq!(order.order_id, 12345678);
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.side, "BUY");
        assert_eq!(order.order_type, "LIMIT");
        assert_eq!(order.status, "FILLED");
        assert_eq!(order.price, Decimal::from_str("50000.00").unwrap());
        assert_eq!(order.orig_qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(order.executed_qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(order.avg_price, Decimal::from_str("49999.50").unwrap());
        assert_eq!(order.cum_quote, Decimal::from_str("49.99950000").unwrap());
        assert_eq!(order.time, 1700000000000);
        assert_eq!(order.update_time, 1700000001000);
    }

    #[test]
    fn test_deserialize_asterdex_order_array() {
        let json = r#"[
            {
                "orderId": 111,
                "symbol": "BTCUSDT",
                "side": "BUY",
                "type": "MARKET",
                "status": "FILLED",
                "price": "0",
                "origQty": "0.01",
                "executedQty": "0.01",
                "avgPrice": "50000.00",
                "cumQuote": "500.00",
                "time": 1700000000000,
                "updateTime": 1700000000100
            },
            {
                "orderId": 222,
                "symbol": "BTCUSDT",
                "side": "SELL",
                "type": "LIMIT",
                "status": "NEW",
                "price": "55000.00",
                "origQty": "0.01",
                "executedQty": "0",
                "avgPrice": "0",
                "cumQuote": "0",
                "time": 1700000001000,
                "updateTime": 1700000001000
            }
        ]"#;

        let orders: Vec<AsterDexOrder> = serde_json::from_str(json).unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].order_id, 111);
        assert_eq!(orders[0].side, "BUY");
        assert_eq!(orders[1].order_id, 222);
        assert_eq!(orders[1].side, "SELL");
    }

    #[test]
    fn test_from_asterdex_order_to_order() {
        let api_order = AsterDexOrder {
            order_id: 99999,
            symbol: "ETHUSDT".to_string(),
            side: "SELL".to_string(),
            order_type: "STOP_MARKET".to_string(),
            status: "CANCELED".to_string(),
            price: Decimal::from_str("3000.00").unwrap(),
            orig_qty: Decimal::from_str("1.5").unwrap(),
            executed_qty: Decimal::from_str("0.5").unwrap(),
            avg_price: Decimal::from_str("3001.25").unwrap(),
            cum_quote: Decimal::from_str("1500.625").unwrap(),
            time: 1700100000000,
            update_time: 1700100005000,
        };

        let order = Order::from(api_order);
        assert_eq!(order.order_id, 99999);
        assert_eq!(order.symbol, "ETHUSDT");
        assert_eq!(order.side, crate::data::order::OrderSide::Sell);
        assert_eq!(order.order_type, crate::data::order::OrderType::StopMarket);
        assert_eq!(order.status, crate::data::order::OrderStatus::Canceled);
        assert_eq!(order.price, Decimal::from_str("3000.00").unwrap());
        assert_eq!(order.orig_qty, Decimal::from_str("1.5").unwrap());
        assert_eq!(order.executed_qty, Decimal::from_str("0.5").unwrap());
        assert_eq!(order.avg_price, Decimal::from_str("3001.25").unwrap());
        assert_eq!(order.cum_quote, Decimal::from_str("1500.625").unwrap());
        assert_eq!(order.time, 1700100000000);
        assert_eq!(order.update_time, 1700100005000);
    }

    #[test]
    fn test_order_error_is_retriable() {
        // Retriable errors
        assert!(OrderError::Timeout.is_retriable());
        assert!(OrderError::Api(429, "rate limited".to_string()).is_retriable());
        assert!(OrderError::Api(500, "server error".to_string()).is_retriable());
        assert!(OrderError::Api(502, "bad gateway".to_string()).is_retriable());
        assert!(OrderError::Api(503, "service unavailable".to_string()).is_retriable());

        // Non-retriable errors
        assert!(!OrderError::Api(400, "bad request".to_string()).is_retriable());
        assert!(!OrderError::Api(401, "unauthorized".to_string()).is_retriable());
        assert!(!OrderError::Api(404, "not found".to_string()).is_retriable());
        assert!(!OrderError::Parse("invalid json".to_string()).is_retriable());
    }

    #[test]
    fn test_order_error_display() {
        assert_eq!(
            format!("{}", OrderError::Timeout),
            "Request timeout"
        );
        assert_eq!(
            format!("{}", OrderError::Api(401, "Unauthorized".to_string())),
            "API error 401: Unauthorized"
        );
        assert_eq!(
            format!("{}", OrderError::Parse("bad json".to_string())),
            "JSON parse error: bad json"
        );
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires valid API credentials)
    async fn test_fetch_orders_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let orders = fetch_orders("BTCUSDT", &credentials, &client)
            .await
            .expect("Should fetch orders");

        println!("Fetched {} orders", orders.len());
        for order in &orders {
            println!(
                "  {} {} {} {} @ {} (status: {})",
                order.order_id, order.symbol, order.side, order.order_type, order.price, order.status
            );
        }
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires valid API credentials)
    async fn test_fetch_open_orders_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let orders = fetch_open_orders(&credentials, &client)
            .await
            .expect("Should fetch open orders");

        println!("Fetched {} open orders (all pairs)", orders.len());
        for order in &orders {
            println!(
                "  {} {} {} {} @ {} (status: {})",
                order.order_id, order.symbol, order.side, order.order_type, order.price, order.status
            );
        }
    }
}
