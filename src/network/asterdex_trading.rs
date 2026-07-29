// src/network/asterdex_trading.rs
// AsterDEX trading types, error handling, order parameter builder,
// and order placement / leverage change async functions
// for Binance-compatible Futures REST API.

use crate::config::AsterDexCredentials;
use crate::data::order::{OrderSide, TimeInForce};
use crate::network::asterdex_auth::{sign_request, ASTERDEX_REST_URL, DEFAULT_RECV_WINDOW};
use crate::network::reconnect::ExponentialBackoff;
use rust_decimal::Decimal;
use serde::Deserialize;

/// Maximum retry attempts for transient failures
pub const MAX_RETRIES: u32 = 3;

/// Errors that can occur during trading operations
#[derive(Debug, thiserror::Error)]
pub enum TradingError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Request timeout")]
    Timeout,
    #[error("API error {0}: {1}")]
    Api(u16, String),
    #[error("JSON parse error: {0}")]
    Parse(String),
    #[error("Invalid or inactive symbol: {0}")]
    InvalidSymbol(String),
}

impl TradingError {
    /// Returns true if the error is retriable (transient failure).
    ///
    /// Retriable: Timeout, Http errors, API errors with status >= 500 or 429 (rate limit).
    /// NOT retriable: 4xx client errors (bad request, invalid params) and InvalidSymbol.
    pub fn is_retriable(&self) -> bool {
        match self {
            TradingError::Timeout => true,
            TradingError::Http(_) => true,
            TradingError::Api(status, _) if *status >= 500 || *status == 429 => true,
            _ => false,
        }
    }

    /// Create a TradingError from an API error response.
    ///
    /// Tries to parse the body as a Binance API error JSON `{"code": N, "msg": "..."}`.
    /// On success, formats as "msg (code N)". On failure, falls back to raw body text.
    pub fn from_api_response(status_code: u16, body: &str) -> Self {
        if let Ok(api_error) = serde_json::from_str::<BinanceApiError>(body) {
            TradingError::Api(
                status_code,
                format!("{} (code {})", api_error.msg, api_error.code),
            )
        } else {
            TradingError::Api(status_code, body.to_string())
        }
    }
}

/// Binance-compatible API error response structure
#[derive(Debug, Deserialize)]
pub struct BinanceApiError {
    pub code: i32,
    pub msg: String,
}

/// Position mode detected from GET /fapi/v1/positionSide/dual.
///
/// Determines how order parameters are constructed:
/// - Hedge: positionSide=LONG/SHORT derived from side+intent, NO reduceOnly param
/// - OneWay: positionSide=BOTH always, reduceOnly=true for closing orders
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionMode {
    Hedge,
    OneWay,
}

/// Response from GET /fapi/v1/positionSide/dual
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DualSidePositionResponse {
    dual_side_position: bool,
}

/// Order parameters for the four supported Futures order types.
///
/// Each variant enforces the correct mandatory parameters at compile time,
/// preventing runtime API errors from missing fields.
#[derive(Debug, Clone)]
pub enum OrderParams {
    /// Market order: immediate execution at best available price
    Market {
        symbol: String,
        side: OrderSide,
        quantity: Decimal,
    },
    /// Limit order: execution at specified price or better
    Limit {
        symbol: String,
        side: OrderSide,
        quantity: Decimal,
        price: Decimal,
        time_in_force: TimeInForce,
    },
    /// Stop-market order: market order triggered at stop price
    StopMarket {
        symbol: String,
        side: OrderSide,
        quantity: Decimal,
        stop_price: Decimal,
    },
    /// Take-profit-market order: market order triggered at take-profit price
    TakeProfitMarket {
        symbol: String,
        side: OrderSide,
        quantity: Decimal,
        stop_price: Decimal,
    },
    /// Stop-limit order: limit order triggered at stop price
    /// NOTE: The Futures API type string for stop-limit is "STOP" (not "STOP_LIMIT")
    StopLimit {
        symbol: String,
        side: OrderSide,
        quantity: Decimal,
        price: Decimal,
        stop_price: Decimal,
        time_in_force: TimeInForce,
    },
    /// Trailing stop market order: market order triggered when price retraces by callback rate
    TrailingStopMarket {
        symbol: String,
        side: OrderSide,
        quantity: Decimal,
        callback_rate: Decimal,
        activation_price: Option<Decimal>,
    },
}

impl OrderParams {
    /// Extract the order side from any variant.
    fn side(&self) -> &OrderSide {
        match self {
            OrderParams::Market { side, .. }
            | OrderParams::Limit { side, .. }
            | OrderParams::StopMarket { side, .. }
            | OrderParams::TakeProfitMarket { side, .. }
            | OrderParams::StopLimit { side, .. }
            | OrderParams::TrailingStopMarket { side, .. } => side,
        }
    }

    /// Derive positionSide based on the account's position mode.
    ///
    /// - Hedge mode: positionSide=LONG/SHORT derived from side + reduce_only intent
    ///   - Opening: BUY → LONG, SELL → SHORT
    ///   - Closing (reduceOnly): BUY → SHORT (closing short), SELL → LONG (closing long)
    /// - OneWay mode: positionSide=BOTH always
    fn position_side(&self, reduce_only: bool, mode: PositionMode) -> &'static str {
        match mode {
            PositionMode::OneWay => "BOTH",
            PositionMode::Hedge => {
                let is_buy = matches!(self.side(), OrderSide::Buy);
                if reduce_only {
                    if is_buy { "SHORT" } else { "LONG" }
                } else {
                    if is_buy { "LONG" } else { "SHORT" }
                }
            }
        }
    }

    /// Build the query string for this order type.
    ///
    /// Produces the correct parameter set per Binance Futures API docs.
    /// Always includes `newOrderRespType=RESULT` so MARKET orders return fill details.
    /// Includes `positionSide` derived from the account's position mode.
    ///
    /// - Hedge mode: positionSide=LONG/SHORT, NO reduceOnly param (API rejects it)
    /// - OneWay mode: positionSide=BOTH, reduceOnly=true for closing orders
    pub fn to_query_string(&self, reduce_only: bool, mode: PositionMode) -> String {
        let base = match self {
            OrderParams::Market {
                symbol,
                side,
                quantity,
            } => {
                format!(
                    "symbol={}&side={}&type=MARKET&quantity={}&newOrderRespType=RESULT",
                    symbol, side, quantity,
                )
            }
            OrderParams::Limit {
                symbol,
                side,
                quantity,
                price,
                time_in_force,
            } => {
                format!(
                    "symbol={}&side={}&type=LIMIT&quantity={}&price={}&timeInForce={}&newOrderRespType=RESULT",
                    symbol, side, quantity, price, time_in_force,
                )
            }
            OrderParams::StopMarket {
                symbol,
                side,
                quantity,
                stop_price,
            } => {
                format!(
                    "symbol={}&side={}&type=STOP_MARKET&quantity={}&stopPrice={}&newOrderRespType=RESULT",
                    symbol, side, quantity, stop_price,
                )
            }
            OrderParams::TakeProfitMarket {
                symbol,
                side,
                quantity,
                stop_price,
            } => {
                format!(
                    "symbol={}&side={}&type=TAKE_PROFIT_MARKET&quantity={}&stopPrice={}&newOrderRespType=RESULT",
                    symbol, side, quantity, stop_price,
                )
            }
            OrderParams::StopLimit {
                symbol,
                side,
                quantity,
                price,
                stop_price,
                time_in_force,
            } => {
                format!(
                    "symbol={}&side={}&type=STOP&quantity={}&price={}&stopPrice={}&timeInForce={}&newOrderRespType=RESULT",
                    symbol, side, quantity, price, stop_price, time_in_force,
                )
            }
            OrderParams::TrailingStopMarket {
                symbol,
                side,
                quantity,
                callback_rate,
                activation_price,
            } => {
                let mut qs = format!(
                    "symbol={}&side={}&type=TRAILING_STOP_MARKET&quantity={}&callbackRate={}&newOrderRespType=RESULT",
                    symbol, side, quantity, callback_rate,
                );
                if let Some(ap) = activation_price {
                    qs.push_str(&format!("&activationPrice={}", ap));
                }
                qs
            }
        };
        let base = format!("{}&positionSide={}", base, self.position_side(reduce_only, mode));
        match mode {
            PositionMode::Hedge => {
                // Hedge mode: never emit reduceOnly (API rejects it with -1106)
                base
            }
            PositionMode::OneWay => {
                // OneWay mode: emit reduceOnly=true for closing orders
                if reduce_only {
                    format!("{}&reduceOnly=true", base)
                } else {
                    base
                }
            }
        }
    }
}

/// Account balance entry from GET /fapi/v2/balance
///
/// Each entry represents one asset in the account (e.g., USDT, BNB).
/// Only a subset of fields are captured -- add more as needed.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceEntry {
    pub asset: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub balance: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub available_balance: Decimal,
    pub margin_available: bool,
    pub update_time: u64,
}

/// Response from POST /fapi/v1/order (new order placement)
///
/// Uses `newOrderRespType=RESULT` which returns fill details for MARKET orders.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub order_id: u64,
    pub symbol: String,
    pub status: String,
    pub client_order_id: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub avg_price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub orig_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub executed_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub cum_quote: Decimal,
    pub time_in_force: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub side: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub stop_price: Decimal,
    pub update_time: u64,
}

/// Response from POST /fapi/v1/leverage (change leverage)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeverageResponse {
    pub leverage: u32,
    pub max_notional_value: String,
    pub symbol: String,
}

/// Response from GET /fapi/v1/leverageBracket (one per symbol)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeverageBracketResponse {
    pub symbol: String,
    pub brackets: Vec<LeverageBracket>,
}

/// A single leverage tier bracket defining max leverage for a notional range.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeverageBracket {
    pub bracket: u32,
    pub initial_leverage: u32,
    pub notional_cap: u64,
    pub notional_floor: u64,
    pub maint_margin_ratio: f64,
    pub cum: f64,
}

/// Place an order on AsterDEX Futures via POST /fapi/v1/order.
///
/// Sends a signed POST request with the order parameters. Retries transient
/// failures (timeouts, 5xx, 429) with exponential backoff. Does NOT retry
/// 4xx client errors (bad request, unauthorized, etc.).
///
/// CRITICAL: The signed query is regenerated on each retry attempt because
/// the timestamp expires after the recvWindow (5 seconds), and backoff
/// delays can exceed this window.
///
/// # Arguments
/// * `params` - Order parameters (Market, Limit, StopMarket, or StopLimit)
/// * `credentials` - AsterDEX API key and secret key
/// * `reduce_only` - If true, marks order as closing (behavior depends on mode)
/// * `mode` - Account position mode (Hedge or OneWay), determines parameter emission
///
/// # Returns
/// * `Ok(OrderResponse)` - Order accepted by the exchange
/// * `Err(TradingError)` - If request fails after retries
pub async fn place_order(
    params: &OrderParams,
    credentials: &AsterDexCredentials,
    reduce_only: bool,
    mode: PositionMode,
    client: &reqwest::Client,
) -> Result<OrderResponse, TradingError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    // Log order submission intent
    let param_string = params.to_query_string(reduce_only, mode);
    tracing::info!(
        params = %param_string,
        "Submitting order to AsterDEX"
    );

    loop {
        attempts += 1;

        // Regenerate signed query on each attempt (fresh timestamp)
        let param_string = params.to_query_string(reduce_only, mode);
        let (signed_query, _timestamp) =
            sign_request(&param_string, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v1/order?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .post(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let order_response: OrderResponse = response
                        .json()
                        .await
                        .map_err(|e| TradingError::Parse(e.to_string()))?;
                    tracing::info!(
                        order_id = order_response.order_id,
                        status = %order_response.status,
                        avg_price = %order_response.avg_price,
                        "Order placed successfully"
                    );
                    return Ok(order_response);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = TradingError::from_api_response(status_code, &body);

                    tracing::error!(
                        status = status_code,
                        body = %body,
                        "Order placement API error response"
                    );

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Order placement failed (attempt {}/{}): {}. Retrying in {:?}",
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
                            "Order placement failed (attempt {}/{}): {}. Retrying in {:?}",
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

/// Change leverage for a symbol on AsterDEX Futures via POST /fapi/v1/leverage.
///
/// This is a simpler operation than order placement -- no retry needed because
/// leverage changes are idempotent and rarely fail transiently.
///
/// # Arguments
/// * `symbol` - Trading pair (e.g., "BTCUSDT")
/// * `leverage` - New leverage value (1-125 depending on symbol)
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(LeverageResponse)` - Leverage changed successfully (or already at target)
/// * `Err(TradingError)` - If request fails
pub async fn change_leverage(
    symbol: &str,
    leverage: u32,
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<LeverageResponse, TradingError> {
    let params = format!("symbol={}&leverage={}", symbol, leverage);
    let (signed_query, _timestamp) =
        sign_request(&params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
    let url = format!("{}/fapi/v1/leverage?{}", ASTERDEX_REST_URL, signed_query);

    let response = client
        .post(&url)
        .header("X-MBX-APIKEY", &credentials.api_key)
        .send()
        .await?;

    let status = response.status();
    if status.is_success() {
        let leverage_response: LeverageResponse = response
            .json()
            .await
            .map_err(|e| TradingError::Parse(e.to_string()))?;
        tracing::info!(
            symbol = %leverage_response.symbol,
            leverage = leverage_response.leverage,
            max_notional = %leverage_response.max_notional_value,
            "Leverage changed successfully"
        );
        Ok(leverage_response)
    } else {
        let status_code = status.as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(TradingError::from_api_response(status_code, &body))
    }
}

/// Fetch leverage brackets for a symbol via GET /fapi/v1/leverageBracket.
///
/// Returns the tiered bracket list. The first bracket's `initial_leverage`
/// is the maximum leverage for the symbol (at smallest notional tier).
///
/// # Arguments
/// * `symbol` - Trading pair (e.g., "BTCUSDT")
/// * `credentials` - AsterDEX API key and secret key
/// * `client` - Shared reqwest client
///
/// # Returns
/// * `Ok(Vec<LeverageBracket>)` - Bracket tiers for the symbol
/// * `Err(TradingError)` - If request fails
pub async fn fetch_leverage_brackets(
    symbol: &str,
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<Vec<LeverageBracket>, TradingError> {
    let params = format!("symbol={}", symbol);
    let (signed_query, _timestamp) =
        sign_request(&params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
    let url = format!("{}/fapi/v1/leverageBracket?{}", ASTERDEX_REST_URL, signed_query);

    let response = client
        .get(&url)
        .header("X-MBX-APIKEY", &credentials.api_key)
        .send()
        .await?;

    let status = response.status();
    if status.is_success() {
        // With symbol param, API returns a single object; without, an array.
        // Try single object first (our use case), fall back to array.
        let body = response.text().await.map_err(|e| TradingError::Parse(e.to_string()))?;

        let entry: Option<LeverageBracketResponse> =
            serde_json::from_str::<LeverageBracketResponse>(&body)
                .ok()
                .or_else(|| {
                    serde_json::from_str::<Vec<LeverageBracketResponse>>(&body)
                        .ok()
                        .and_then(|v| v.into_iter().find(|e| e.symbol.eq_ignore_ascii_case(symbol)))
                });

        if let Some(entry) = entry {
            tracing::info!(
                symbol = %entry.symbol,
                bracket_count = entry.brackets.len(),
                max_leverage = entry.brackets.first().map(|b| b.initial_leverage).unwrap_or(0),
                "Leverage brackets fetched"
            );
            Ok(entry.brackets)
        } else {
            tracing::warn!(symbol = %symbol, body = %body, "No leverage brackets found for symbol");
            Ok(Vec::new())
        }
    } else {
        let status_code = status.as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(TradingError::from_api_response(status_code, &body))
    }
}

/// Fetch account balance from AsterDEX Futures via GET /fapi/v2/balance.
///
/// Returns balance entries for all assets in the account. Each entry contains
/// the total balance and available (withdrawable) balance.
///
/// Uses the same retry pattern as `place_order`: exponential backoff with
/// fresh timestamp per attempt, retry on 5xx/429/timeout only.
///
/// # Arguments
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(Vec<BalanceEntry>)` - All asset balances
/// * `Err(TradingError)` - If request fails after retries
pub async fn fetch_account_balance(
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<Vec<BalanceEntry>, TradingError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        // Regenerate signed query on each attempt (fresh timestamp)
        let params = "";
        let (signed_query, _timestamp) =
            sign_request(params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v2/balance?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .get(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let balances: Vec<BalanceEntry> = response
                        .json()
                        .await
                        .map_err(|e| TradingError::Parse(e.to_string()))?;
                    return Ok(balances);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = TradingError::from_api_response(status_code, &body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Balance fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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
                            "Balance fetch failed (attempt {}/{}): {}. Retrying in {:?}",
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

/// Response from DELETE /fapi/v1/order (cancel order)
///
/// Contains the order details after cancellation. Only the fields needed
/// for user-facing confirmation are captured.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResponse {
    pub order_id: u64,
    pub symbol: String,
    pub status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub orig_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub executed_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
}

/// Cancel an order on AsterDEX Futures via DELETE /fapi/v1/order.
///
/// Sends a signed DELETE request with symbol and orderId. Retries transient
/// failures (timeouts, 5xx, 429) with exponential backoff, same as place_order.
///
/// CRITICAL: The signed query is regenerated on each retry attempt because
/// the timestamp expires after the recvWindow (5 seconds).
///
/// # Arguments
/// * `symbol` - Trading pair (e.g., "BTCUSDT")
/// * `order_id` - The orderId to cancel
/// * `credentials` - AsterDEX API key and secret key
/// * `client` - HTTP client
///
/// # Returns
/// * `Ok(CancelResponse)` - Order cancelled successfully
/// * `Err(TradingError)` - If request fails after retries
pub async fn cancel_order(
    symbol: &str,
    order_id: u64,
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<CancelResponse, TradingError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    let params = format!("symbol={}&orderId={}", symbol, order_id);
    tracing::info!(
        params = %params,
        "Submitting cancel order to AsterDEX"
    );

    loop {
        attempts += 1;

        // Regenerate signed query on each attempt (fresh timestamp)
        let params = format!("symbol={}&orderId={}", symbol, order_id);
        let (signed_query, _timestamp) =
            sign_request(&params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v1/order?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .delete(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let cancel_response: CancelResponse = response
                        .json()
                        .await
                        .map_err(|e| TradingError::Parse(e.to_string()))?;
                    tracing::info!(
                        order_id = cancel_response.order_id,
                        status = %cancel_response.status,
                        "Order cancelled successfully"
                    );
                    return Ok(cancel_response);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = TradingError::from_api_response(status_code, &body);

                    tracing::error!(
                        status = status_code,
                        body = %body,
                        "Cancel order API error response"
                    );

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Cancel order failed (attempt {}/{}): {}. Retrying in {:?}",
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
                            "Cancel order failed (attempt {}/{}): {}. Retrying in {:?}",
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

/// Map cancel-specific API error codes to human-readable messages.
///
/// Known codes:
/// - -2011: Order already filled or cancelled (most common cancel error)
/// - -2013: Order does not exist
pub fn user_friendly_cancel_error(error: &TradingError) -> String {
    match error {
        TradingError::Api(_, msg) if msg.contains("code -2011") => {
            "Order already filled or cancelled".to_string()
        }
        TradingError::Api(_, msg) if msg.contains("code -2013") => {
            "Order does not exist".to_string()
        }
        other => format!("Cancel failed: {}", other),
    }
}

/// Detect account position mode (hedge vs one-way) via GET /fapi/v1/positionSide/dual.
///
/// Called once at startup. Returns Hedge if dualSidePosition=true,
/// OneWay if false. On failure, defaults to Hedge with a warning log
/// (hedge mode is more common and the user is confirmed in hedge mode).
pub async fn fetch_position_mode(
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> PositionMode {
    let params = "";
    let (signed_query, _) = sign_request(params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
    let url = format!("{}/fapi/v1/positionSide/dual?{}", ASTERDEX_REST_URL, signed_query);

    match client
        .get(&url)
        .header("X-MBX-APIKEY", &credentials.api_key)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            match response.json::<DualSidePositionResponse>().await {
                Ok(body) => {
                    let mode = if body.dual_side_position {
                        PositionMode::Hedge
                    } else {
                        PositionMode::OneWay
                    };
                    tracing::info!(?mode, "Detected account position mode");
                    mode
                }
                Err(e) => {
                    tracing::warn!("Failed to parse position mode response: {}. Defaulting to Hedge", e);
                    PositionMode::Hedge
                }
            }
        }
        Ok(response) => {
            let status = response.status();
            tracing::warn!(%status, "Position mode detection failed. Defaulting to Hedge");
            PositionMode::Hedge
        }
        Err(e) => {
            tracing::warn!("Position mode detection request failed: {}. Defaulting to Hedge", e);
            PositionMode::Hedge
        }
    }
}

/// Cancel all open orders for a symbol via DELETE /fapi/v1/allOpenOrders.
///
/// Atomic batch cancel -- single API call cancels all working orders for the symbol.
/// No retry logic needed (idempotent -- calling twice is safe).
pub async fn cancel_all_open_orders(
    symbol: &str,
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<(), TradingError> {
    let params = format!("symbol={}", symbol);
    let (signed_query, _) = sign_request(&params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
    let url = format!("{}/fapi/v1/allOpenOrders?{}", ASTERDEX_REST_URL, signed_query);

    tracing::info!(symbol = %symbol, "Cancelling all open orders");

    let response = client
        .delete(&url)
        .header("X-MBX-APIKEY", &credentials.api_key)
        .send()
        .await?;

    if response.status().is_success() {
        tracing::info!(symbol = %symbol, "All open orders cancelled");
        Ok(())
    } else {
        let status_code = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        tracing::error!(status = status_code, body = %body, "Cancel all orders failed");
        Err(TradingError::from_api_response(status_code, &body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_market_order_query_string() {
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=BTCUSDT"));
        assert!(qs.contains("side=BUY"));
        assert!(qs.contains("type=MARKET"));
        assert!(qs.contains("quantity=0.001"));
        assert!(qs.contains("newOrderRespType=RESULT"));
        // Market orders must NOT contain timeInForce or price
        assert!(!qs.contains("timeInForce"));
        assert!(!qs.contains("price="));
        // reduce_only=false should NOT include reduceOnly
        assert!(!qs.contains("reduceOnly"));
    }

    #[test]
    fn test_limit_order_query_string() {
        let params = OrderParams::Limit {
            symbol: "ETHUSDT".to_string(),
            side: OrderSide::Sell,
            quantity: Decimal::from_str("1.5").unwrap(),
            price: Decimal::from_str("3000.00").unwrap(),
            time_in_force: TimeInForce::Gtc,
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=ETHUSDT"));
        assert!(qs.contains("side=SELL"));
        assert!(qs.contains("type=LIMIT"));
        assert!(qs.contains("quantity=1.5"));
        assert!(qs.contains("price=3000.00"));
        assert!(qs.contains("timeInForce=GTC"));
        assert!(qs.contains("newOrderRespType=RESULT"));
    }

    #[test]
    fn test_stop_market_order_query_string() {
        let params = OrderParams::StopMarket {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Sell,
            quantity: Decimal::from_str("0.01").unwrap(),
            stop_price: Decimal::from_str("45000.00").unwrap(),
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=BTCUSDT"));
        assert!(qs.contains("side=SELL"));
        assert!(qs.contains("type=STOP_MARKET"));
        assert!(qs.contains("quantity=0.01"));
        assert!(qs.contains("stopPrice=45000.00"));
        assert!(qs.contains("newOrderRespType=RESULT"));
    }

    #[test]
    fn test_stop_limit_order_query_string() {
        let params = OrderParams::StopLimit {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.005").unwrap(),
            price: Decimal::from_str("46000.00").unwrap(),
            stop_price: Decimal::from_str("45500.00").unwrap(),
            time_in_force: TimeInForce::Gtc,
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=BTCUSDT"));
        assert!(qs.contains("side=BUY"));
        // CRITICAL: Futures API string for stop-limit is "STOP" (not "STOP_LIMIT")
        assert!(qs.contains("type=STOP"));
        assert!(!qs.contains("type=STOP_LIMIT"));
        assert!(qs.contains("quantity=0.005"));
        assert!(qs.contains("price=46000.00"));
        assert!(qs.contains("stopPrice=45500.00"));
        assert!(qs.contains("timeInForce=GTC"));
        assert!(qs.contains("newOrderRespType=RESULT"));
    }

    #[test]
    fn test_take_profit_market_query_string() {
        let params = OrderParams::TakeProfitMarket {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Sell,
            quantity: Decimal::from_str("0.01").unwrap(),
            stop_price: Decimal::from_str("55000.00").unwrap(),
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=BTCUSDT"));
        assert!(qs.contains("side=SELL"));
        assert!(qs.contains("type=TAKE_PROFIT_MARKET"));
        assert!(qs.contains("quantity=0.01"));
        assert!(qs.contains("stopPrice=55000.00"));
        assert!(qs.contains("newOrderRespType=RESULT"));
        // Take-profit-market orders must NOT contain price= (no limit price for market execution)
        assert!(!qs.contains("price="));
        // reduce_only=false should NOT include reduceOnly
        assert!(!qs.contains("reduceOnly"));
    }

    #[test]
    fn test_trailing_stop_market_query_string() {
        let params = OrderParams::TrailingStopMarket {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
            callback_rate: Decimal::from_str("1.5").unwrap(),
            activation_price: None,
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=BTCUSDT"));
        assert!(qs.contains("side=BUY"));
        assert!(qs.contains("type=TRAILING_STOP_MARKET"));
        assert!(qs.contains("quantity=0.001"));
        assert!(qs.contains("callbackRate=1.5"));
        assert!(qs.contains("newOrderRespType=RESULT"));
        // Without activation price, should NOT contain activationPrice
        assert!(!qs.contains("activationPrice"));
        // Should NOT contain stopPrice or price
        assert!(!qs.contains("stopPrice"));
        assert!(!qs.contains("price="));
    }

    #[test]
    fn test_trailing_stop_market_query_string_with_activation() {
        let params = OrderParams::TrailingStopMarket {
            symbol: "ETHUSDT".to_string(),
            side: OrderSide::Sell,
            quantity: Decimal::from_str("1.0").unwrap(),
            callback_rate: Decimal::from_str("0.5").unwrap(),
            activation_price: Some(Decimal::from_str("3500.00").unwrap()),
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("symbol=ETHUSDT"));
        assert!(qs.contains("side=SELL"));
        assert!(qs.contains("type=TRAILING_STOP_MARKET"));
        assert!(qs.contains("quantity=1.0"));
        assert!(qs.contains("callbackRate=0.5"));
        assert!(qs.contains("activationPrice=3500.00"));
        assert!(qs.contains("newOrderRespType=RESULT"));
    }

    #[test]
    fn test_to_query_string_reduce_only_true() {
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        // In hedge mode, reduceOnly should NOT appear (mode handles via positionSide)
        let qs = params.to_query_string(true, PositionMode::Hedge);
        assert!(!qs.contains("reduceOnly"));
    }

    #[test]
    fn test_to_query_string_reduce_only_false() {
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(!qs.contains("reduceOnly"));
    }

    #[test]
    fn test_to_query_string_reduce_only_limit() {
        let params = OrderParams::Limit {
            symbol: "ETHUSDT".to_string(),
            side: OrderSide::Sell,
            quantity: Decimal::from_str("1.0").unwrap(),
            price: Decimal::from_str("3000").unwrap(),
            time_in_force: TimeInForce::Gtc,
        };
        // In hedge mode, reduceOnly should NOT appear
        let qs = params.to_query_string(true, PositionMode::Hedge);
        assert!(!qs.contains("reduceOnly"));
        assert!(qs.contains("type=LIMIT"));
    }

    #[test]
    fn test_from_api_response_parses_binance_error() {
        let body = r#"{"code": -1121, "msg": "Invalid symbol."}"#;
        let err = TradingError::from_api_response(400, body);
        match err {
            TradingError::Api(status, msg) => {
                assert_eq!(status, 400);
                assert_eq!(msg, "Invalid symbol. (code -1121)");
            }
            _ => panic!("Expected Api variant"),
        }
    }

    #[test]
    fn test_from_api_response_falls_back_to_raw_body() {
        let body = "Not JSON at all";
        let err = TradingError::from_api_response(500, body);
        match err {
            TradingError::Api(status, msg) => {
                assert_eq!(status, 500);
                assert_eq!(msg, "Not JSON at all");
            }
            _ => panic!("Expected Api variant"),
        }
    }

    #[test]
    fn test_is_retriable() {
        // Retriable
        assert!(TradingError::Timeout.is_retriable());
        assert!(TradingError::Api(429, "rate limited".to_string()).is_retriable());
        assert!(TradingError::Api(500, "server error".to_string()).is_retriable());
        assert!(TradingError::Api(502, "bad gateway".to_string()).is_retriable());
        assert!(TradingError::Api(503, "unavailable".to_string()).is_retriable());

        // Not retriable
        assert!(!TradingError::Api(400, "bad request".to_string()).is_retriable());
        assert!(!TradingError::Api(401, "unauthorized".to_string()).is_retriable());
        assert!(!TradingError::Api(403, "forbidden".to_string()).is_retriable());
        assert!(!TradingError::InvalidSymbol("NOSUCH".to_string()).is_retriable());
        assert!(!TradingError::Parse("bad json".to_string()).is_retriable());
    }

    #[test]
    fn test_order_response_deserialization() {
        let json = r#"{
            "orderId": 8888888,
            "symbol": "BTCUSDT",
            "status": "FILLED",
            "clientOrderId": "testOrder123",
            "price": "0",
            "avgPrice": "50250.50",
            "origQty": "0.001",
            "executedQty": "0.001",
            "cumQuote": "50.25050000",
            "timeInForce": "GTC",
            "type": "MARKET",
            "side": "BUY",
            "stopPrice": "0",
            "updateTime": 1700000000000
        }"#;

        let resp: OrderResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.order_id, 8888888);
        assert_eq!(resp.symbol, "BTCUSDT");
        assert_eq!(resp.status, "FILLED");
        assert_eq!(resp.client_order_id, "testOrder123");
        assert_eq!(resp.price, Decimal::from_str("0").unwrap());
        assert_eq!(resp.avg_price, Decimal::from_str("50250.50").unwrap());
        assert_eq!(resp.orig_qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(resp.executed_qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(resp.cum_quote, Decimal::from_str("50.25050000").unwrap());
        assert_eq!(resp.time_in_force, "GTC");
        assert_eq!(resp.order_type, "MARKET");
        assert_eq!(resp.side, "BUY");
        assert_eq!(resp.stop_price, Decimal::from_str("0").unwrap());
        assert_eq!(resp.update_time, 1700000000000);
    }

    #[test]
    fn test_leverage_response_deserialization() {
        let json = r#"{
            "leverage": 20,
            "maxNotionalValue": "1000000",
            "symbol": "BTCUSDT"
        }"#;

        let resp: LeverageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.leverage, 20);
        assert_eq!(resp.max_notional_value, "1000000");
        assert_eq!(resp.symbol, "BTCUSDT");
    }

    #[test]
    fn test_trading_error_display() {
        assert_eq!(format!("{}", TradingError::Timeout), "Request timeout");
        assert_eq!(
            format!("{}", TradingError::Api(401, "Unauthorized".to_string())),
            "API error 401: Unauthorized"
        );
        assert_eq!(
            format!("{}", TradingError::Parse("bad json".to_string())),
            "JSON parse error: bad json"
        );
        assert_eq!(
            format!("{}", TradingError::InvalidSymbol("NOSUCH".to_string())),
            "Invalid or inactive symbol: NOSUCH"
        );
    }

    // --- Integration tests (require valid API credentials) ---
    // Run with: cargo test -- --ignored
    // WARNING: test_place_market_order_real_api places a REAL order!

    #[tokio::test]
    #[ignore] // Requires ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY
    async fn test_change_leverage_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let result = change_leverage("BTCUSDT", 5, &credentials, &client).await;
        match result {
            Ok(resp) => {
                println!(
                    "Leverage changed: symbol={}, leverage={}, max_notional={}",
                    resp.symbol, resp.leverage, resp.max_notional_value
                );
                assert_eq!(resp.symbol, "BTCUSDT");
            }
            Err(e) => {
                panic!("change_leverage failed: {}", e);
            }
        }
    }

    // WARNING: This test places a REAL order on AsterDEX.
    // Only run on testnet or with a very small quantity.
    // The order will execute immediately at market price.
    #[tokio::test]
    #[ignore] // Requires ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY -- PLACES REAL ORDER
    async fn test_place_market_order_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let result = place_order(&params, &credentials, false, PositionMode::Hedge, &client).await;
        println!("Market order result: {:?}", result);
    }

    #[tokio::test]
    #[ignore] // Requires ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY
    async fn test_place_limit_order_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        // Very low limit price - unlikely to fill, tests order acceptance
        let params = OrderParams::Limit {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
            price: Decimal::from_str("1000.00").unwrap(),
            time_in_force: TimeInForce::Gtc,
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let result = place_order(&params, &credentials, false, PositionMode::Hedge, &client).await;
        println!("Limit order result: {:?}", result);
    }

    #[tokio::test]
    #[ignore] // Requires ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY
    async fn test_place_order_invalid_symbol() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let params = OrderParams::Market {
            symbol: "NOSYMBOLXYZ".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let result = place_order(&params, &credentials, false, PositionMode::Hedge, &client).await;
        assert!(result.is_err(), "Invalid symbol should return error");
        match result.unwrap_err() {
            TradingError::Api(status, _msg) => {
                assert_eq!(status, 400, "Invalid symbol should return 400");
            }
            other => panic!("Expected Api(400, _) error, got: {:?}", other),
        }
    }

    #[test]
    fn test_cancel_response_deserialization() {
        let json = r#"{
            "orderId": 9999999,
            "symbol": "ETHUSDT",
            "status": "CANCELED",
            "origQty": "1.5",
            "executedQty": "0.0",
            "price": "3000.00",
            "side": "BUY",
            "type": "LIMIT"
        }"#;

        let resp: CancelResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.order_id, 9999999);
        assert_eq!(resp.symbol, "ETHUSDT");
        assert_eq!(resp.status, "CANCELED");
        assert_eq!(resp.orig_qty, Decimal::from_str("1.5").unwrap());
        assert_eq!(resp.executed_qty, Decimal::from_str("0.0").unwrap());
        assert_eq!(resp.price, Decimal::from_str("3000.00").unwrap());
        assert_eq!(resp.side, "BUY");
        assert_eq!(resp.order_type, "LIMIT");
    }

    #[test]
    fn test_user_friendly_cancel_error_filled() {
        let error = TradingError::Api(400, "Unknown order sent. (code -2011)".to_string());
        assert_eq!(
            user_friendly_cancel_error(&error),
            "Order already filled or cancelled"
        );
    }

    #[test]
    fn test_user_friendly_cancel_error_not_found() {
        let error = TradingError::Api(400, "No such order. (code -2013)".to_string());
        assert_eq!(
            user_friendly_cancel_error(&error),
            "Order does not exist"
        );
    }

    #[test]
    fn test_user_friendly_cancel_error_generic() {
        let error = TradingError::Api(500, "Internal server error".to_string());
        assert_eq!(
            user_friendly_cancel_error(&error),
            "Cancel failed: API error 500: Internal server error"
        );
    }

    #[test]
    fn test_balance_entry_deserialization() {
        let json = r#"{
            "asset": "USDT",
            "balance": "1000.50",
            "availableBalance": "800.25",
            "marginAvailable": true,
            "updateTime": 1700000000000
        }"#;

        let entry: BalanceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.asset, "USDT");
        assert_eq!(entry.balance, Decimal::from_str("1000.50").unwrap());
        assert_eq!(entry.available_balance, Decimal::from_str("800.25").unwrap());
        assert!(entry.margin_available);
        assert_eq!(entry.update_time, 1700000000000);
    }

    // ---- Position mode tests ----

    #[test]
    fn test_to_query_string_hedge_mode_no_reduce_only_param() {
        // Hedge mode + reduce_only=true: positionSide should be SHORT (closing short),
        // and reduceOnly param must NOT appear
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        let qs = params.to_query_string(true, PositionMode::Hedge);
        assert!(qs.contains("positionSide=SHORT"), "BUY+reduce_only=true in hedge -> SHORT");
        assert!(!qs.contains("reduceOnly"), "Hedge mode must never emit reduceOnly");
    }

    #[test]
    fn test_to_query_string_hedge_mode_opening_order() {
        // Hedge mode + reduce_only=false: positionSide should be LONG (opening long),
        // and reduceOnly param must NOT appear
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        let qs = params.to_query_string(false, PositionMode::Hedge);
        assert!(qs.contains("positionSide=LONG"), "BUY+reduce_only=false in hedge -> LONG");
        assert!(!qs.contains("reduceOnly"), "Hedge mode must never emit reduceOnly");
    }

    #[test]
    fn test_to_query_string_one_way_mode_closing() {
        // OneWay mode + reduce_only=true: positionSide=BOTH, reduceOnly=true
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        let qs = params.to_query_string(true, PositionMode::OneWay);
        assert!(qs.contains("positionSide=BOTH"), "OneWay mode always uses BOTH");
        assert!(qs.contains("reduceOnly=true"), "OneWay closing must emit reduceOnly=true");
    }

    #[test]
    fn test_to_query_string_one_way_mode_opening() {
        // OneWay mode + reduce_only=false: positionSide=BOTH, NO reduceOnly
        let params = OrderParams::Market {
            symbol: "BTCUSDT".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::from_str("0.001").unwrap(),
        };
        let qs = params.to_query_string(false, PositionMode::OneWay);
        assert!(qs.contains("positionSide=BOTH"), "OneWay mode always uses BOTH");
        assert!(!qs.contains("reduceOnly"), "OneWay opening must NOT emit reduceOnly");
    }

    #[test]
    fn test_position_mode_response_hedge() {
        let json = r#"{"dualSidePosition": true}"#;
        let resp: DualSidePositionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.dual_side_position);
    }

    #[test]
    fn test_position_mode_response_one_way() {
        let json = r#"{"dualSidePosition": false}"#;
        let resp: DualSidePositionResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.dual_side_position);
    }

    #[tokio::test]
    #[ignore] // Requires ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY
    async fn test_fetch_account_balance_real_api() {
        let credentials = AsterDexCredentials::from_env()
            .expect("ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY must be set");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let balances = fetch_account_balance(&credentials, &client)
            .await
            .expect("Should fetch balances");

        // Should have at least one USDT entry
        let usdt = balances.iter().find(|b| b.asset == "USDT");
        assert!(usdt.is_some(), "Should have a USDT balance entry");
        println!("USDT balance: {:?}", usdt.unwrap());
    }
}
