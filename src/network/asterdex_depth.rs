// src/network/asterdex_depth.rs
// Partial depth WebSocket stream handler for AsterDEX Futures.
// Connects to the @depth20@100ms stream, parses partial depth snapshots,
// and sends processed DepthUpdate structs through an mpsc channel.
//
// Follows the proven asterdex_mark_price.rs pattern for reconnection,
// backoff, and channel delivery.

use crate::network::asterdex_trading::TradingError;
use crate::network::reconnect::ExponentialBackoff;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const ASTERDEX_DEPTH_URL: &str = "https://fapi.asterdex.com/fapi/v1/depth";

/// Deserialization type for the partial depth update event.
///
/// Stream: `<symbol>@depth20@100ms` (public, no auth required)
/// Each message contains a complete top-20 snapshot (not incremental diffs).
/// Price and quantity arrive as JSON string arrays: ["price", "quantity"].
///
/// Example payload:
/// ```json
/// {
///   "e": "depthUpdate",
///   "E": 1700000000000,
///   "T": 1700000000000,
///   "s": "BTCUSDT",
///   "U": 157,
///   "u": 160,
///   "pu": 149,
///   "b": [["97457.50", "1.200"], ["97457.00", "0.900"]],
///   "a": [["97458.00", "0.500"], ["97458.50", "3.100"]]
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct DepthEvent {
    /// Event type: "depthUpdate"
    #[allow(dead_code)] // Serde deserialization field
    pub e: String,
    /// Event time (ms since epoch)
    #[serde(rename = "E")]
    pub event_time: u64,
    /// Transaction time (ms since epoch)
    #[serde(rename = "T")]
    #[allow(dead_code)] // Serde deserialization field
    pub transaction_time: u64,
    /// Symbol (e.g., "BTCUSDT")
    #[allow(dead_code)] // Serde deserialization field
    pub s: String,
    /// First update ID in this event
    #[serde(rename = "U")]
    #[allow(dead_code)] // Serde deserialization field
    pub first_update_id: u64,
    /// Final update ID in this event
    pub u: u64,
    /// Previous stream's final update ID
    #[allow(dead_code)] // Serde deserialization field
    pub pu: u64,
    /// Bids: array of [price_string, quantity_string]
    pub b: Vec<[String; 2]>,
    /// Asks: array of [price_string, quantity_string]
    pub a: Vec<[String; 2]>,
}

/// Processed depth snapshot for channel transport.
///
/// Parsed from raw DepthEvent with all Decimal conversions done upfront
/// (not in the event loop hot path). Analogous to MarkPriceData.
#[derive(Debug, Clone)]
pub struct DepthUpdate {
    pub bids: Vec<(Decimal, Decimal)>,
    pub asks: Vec<(Decimal, Decimal)>,
    pub last_update_id: u64,
    #[allow(dead_code)] // Available for future latency monitoring
    pub event_time: u64,
}

/// Parse raw string pairs from depth stream into (price, qty) Decimal tuples.
/// Silently drops any level with unparseable price or quantity.
fn parse_levels(raw: &[[String; 2]]) -> Vec<(Decimal, Decimal)> {
    raw.iter()
        .filter_map(|level| {
            let price = Decimal::from_str(&level[0]).ok()?;
            let qty = Decimal::from_str(&level[1]).ok()?;
            Some((price, qty))
        })
        .collect()
}

/// Run the partial depth WebSocket stream with automatic reconnection.
///
/// Connects to `wss://fstream.asterdex.com/ws/<symbol>@depth20@100ms`
/// and sends parsed depth snapshots through the mpsc channel.
///
/// This function runs forever (until the receiver is dropped).
/// Designed to be spawned with `tokio::spawn`.
///
/// # Arguments
/// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
/// * `depth_tx` - mpsc sender for depth update snapshots
pub async fn run_depth_stream(symbol: String, depth_tx: mpsc::Sender<DepthUpdate>) {
    let url = format!(
        "wss://fstream.asterdex.com/ws/{}@depth20@100ms",
        symbol.to_lowercase()
    );

    loop {
        // Outer restart loop: creates a fresh backoff each iteration
        let mut backoff = ExponentialBackoff::new();

        loop {
            // Inner backoff loop: reconnects with exponential backoff
            tracing::info!(symbol = %symbol, "Connecting to depth stream");

            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(&url),
            )
            .await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!(symbol = %symbol, "Depth stream connected");
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, symbol = %symbol, "Depth stream connection failed");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break; // Backoff exhausted, break to outer loop
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!(symbol = %symbol, "Depth stream connection timed out");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break; // Backoff exhausted, break to outer loop
                    }
                    continue;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            // Inner message loop
            loop {
                let msg = tokio::time::timeout(Duration::from_secs(30), read.next()).await;

                match msg {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        match serde_json::from_str::<DepthEvent>(&text) {
                            Ok(event) => {
                                let update = DepthUpdate {
                                    bids: parse_levels(&event.b),
                                    asks: parse_levels(&event.a),
                                    last_update_id: event.u,
                                    event_time: event.event_time,
                                };

                                match depth_tx.try_send(update) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => {
                                        tracing::debug!(
                                            "Depth channel full, dropping update"
                                        );
                                    }
                                    Err(TrySendError::Closed(_)) => {
                                        tracing::info!(
                                            "Depth receiver dropped, exiting"
                                        );
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(
                                    error = %e,
                                    "Failed to parse depth event"
                                );
                            }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        if let Err(e) = write.send(Message::Pong(data)).await {
                            tracing::warn!(error = %e, "Failed to send pong");
                            break;
                        }
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        tracing::info!(symbol = %symbol, "Depth stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {
                        // Binary, Pong, Frame - ignore
                    }
                    Ok(Some(Err(e))) => {
                        tracing::warn!(error = %e, "Depth stream WebSocket error");
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("Depth stream ended");
                        break;
                    }
                    Err(_) => {
                        // Timeout: no message received in 30 seconds (stale connection)
                        tracing::warn!(symbol = %symbol, "Depth stream read timeout (30s)");
                        break;
                    }
                }
            }

            tracing::info!(symbol = %symbol, "Depth stream disconnected, reconnecting...");
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break; // Backoff exhausted, break to outer loop
            }
        }

        // Backoff exhausted -- cooldown then restart with fresh backoff
        tracing::warn!(symbol = %symbol, "Depth stream backoff exhausted, restarting after 60s cooldown");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

/// REST depth snapshot response.
///
/// `GET /fapi/v1/depth?symbol=X&limit=N`
/// Returns bids and asks as arrays of [price_string, quantity_string].
#[derive(Debug, Deserialize)]
struct DepthSnapshot {
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
}

/// Fetch order book depth via REST API.
///
/// Returns a DepthUpdate with the requested number of levels per side.
/// Valid limits: 5, 10, 20, 50, 100, 500, 1000.
pub async fn fetch_depth(
    symbol: &str,
    limit: u32,
    client: &reqwest::Client,
) -> Result<DepthUpdate, TradingError> {
    let url = format!("{}?symbol={}&limit={}", ASTERDEX_DEPTH_URL, symbol, limit);

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(TradingError::Http)?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(TradingError::Api(status, body));
    }

    let snapshot: DepthSnapshot = response
        .json()
        .await
        .map_err(|e| TradingError::Parse(e.to_string()))?;

    Ok(DepthUpdate {
        bids: parse_levels(&snapshot.bids),
        asks: parse_levels(&snapshot.asks),
        last_update_id: snapshot.last_update_id,
        event_time: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_depth_event_deserialization() {
        let json = r#"{
            "e": "depthUpdate",
            "E": 1700000000000,
            "T": 1700000000000,
            "s": "BTCUSDT",
            "U": 157,
            "u": 160,
            "pu": 149,
            "b": [["97457.50", "1.200"], ["97457.00", "0.900"]],
            "a": [["97458.00", "0.500"], ["97458.50", "3.100"]]
        }"#;

        let event: DepthEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.e, "depthUpdate");
        assert_eq!(event.event_time, 1700000000000);
        assert_eq!(event.transaction_time, 1700000000000);
        assert_eq!(event.s, "BTCUSDT");
        assert_eq!(event.first_update_id, 157);
        assert_eq!(event.u, 160);
        assert_eq!(event.pu, 149);
        assert_eq!(event.b.len(), 2);
        assert_eq!(event.b[0][0], "97457.50");
        assert_eq!(event.b[0][1], "1.200");
        assert_eq!(event.b[1][0], "97457.00");
        assert_eq!(event.b[1][1], "0.900");
        assert_eq!(event.a.len(), 2);
        assert_eq!(event.a[0][0], "97458.00");
        assert_eq!(event.a[0][1], "0.500");
        assert_eq!(event.a[1][0], "97458.50");
        assert_eq!(event.a[1][1], "3.100");
    }

    #[test]
    fn test_parse_levels_valid() {
        let raw = vec![
            ["97457.50".to_string(), "1.200".to_string()],
            ["97457.00".to_string(), "0.900".to_string()],
        ];
        let levels = parse_levels(&raw);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].0, Decimal::from_str("97457.50").unwrap());
        assert_eq!(levels[0].1, Decimal::from_str("1.200").unwrap());
        assert_eq!(levels[1].0, Decimal::from_str("97457.00").unwrap());
        assert_eq!(levels[1].1, Decimal::from_str("0.900").unwrap());
    }

    #[test]
    fn test_parse_levels_malformed() {
        let raw = vec![
            ["97457.50".to_string(), "1.200".to_string()],
            ["".to_string(), "0.900".to_string()],       // malformed price
            ["97456.00".to_string(), "bad".to_string()],  // malformed qty
            ["97455.00".to_string(), "0.500".to_string()],
        ];
        let levels = parse_levels(&raw);
        assert_eq!(levels.len(), 2); // only the 2 valid entries
        assert_eq!(levels[0].0, Decimal::from_str("97457.50").unwrap());
        assert_eq!(levels[1].0, Decimal::from_str("97455.00").unwrap());
    }

    #[test]
    fn test_parse_levels_empty() {
        let raw: Vec<[String; 2]> = vec![];
        let levels = parse_levels(&raw);
        assert!(levels.is_empty());
    }

    #[test]
    fn test_websocket_url_construction() {
        let symbol = "BTCUSDT";
        let url = format!(
            "wss://fstream.asterdex.com/ws/{}@depth20@100ms",
            symbol.to_lowercase()
        );
        assert_eq!(
            url,
            "wss://fstream.asterdex.com/ws/btcusdt@depth20@100ms"
        );
    }
}
