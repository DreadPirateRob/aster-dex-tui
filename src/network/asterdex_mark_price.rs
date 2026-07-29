// src/network/asterdex_mark_price.rs
// Public mark price WebSocket stream for AsterDEX Futures.
// Connects to the @markPrice@1s stream, parses mark price updates,
// and sends the latest Decimal value via a watch channel.
//
// Also provides multi-symbol combined streams support via
// run_multi_mark_price_stream() for position tracking.

use crate::network::reconnect::ExponentialBackoff;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Deserialization type for the Binance-compatible mark price update event.
///
/// Stream: `<symbol>@markPrice@1s` (public, no auth required)
/// Example payload:
/// ```json
/// {
///   "e": "markPriceUpdate",
///   "E": 1700000000000,
///   "s": "BTCUSDT",
///   "p": "50000.12345678",
///   "i": "50000.00000000",
///   "P": "50000.00000000",
///   "r": "0.00010000",
///   "T": 1700003600000
/// }
/// ```
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct MarkPriceEvent {
    /// Event type: "markPriceUpdate"
    pub e: String,
    /// Event time (ms)
    #[serde(rename = "E")]
    pub event_time: u64,
    /// Symbol
    pub s: String,
    /// Mark price (string, parse to Decimal)
    pub p: String,
    /// Index price
    pub i: String,
    /// Estimated settle price
    #[serde(rename = "P")]
    pub estimated_settle_price: String,
    /// Funding rate
    pub r: String,
    /// Next funding time (ms)
    #[serde(rename = "T")]
    pub next_funding_time: u64,
}

/// Run the mark price WebSocket stream with automatic reconnection.
///
/// Connects to `wss://fstream.asterdex.com/ws/<symbol>@markPrice@1s`
/// and sends parsed mark price updates through the watch channel.
///
/// This function runs forever (until the sender is dropped).
/// Designed to be spawned with `tokio::spawn`.
///
/// # Arguments
/// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
/// * `tx` - Watch channel sender for mark price updates
pub async fn run_mark_price_stream(symbol: String, tx: watch::Sender<Option<Decimal>>) {
    let url = format!(
        "wss://fstream.asterdex.com/ws/{}@markPrice@1s",
        symbol.to_lowercase()
    );

    loop {
        // Outer restart loop: creates a fresh backoff each iteration
        let mut backoff = ExponentialBackoff::new();

        loop {
            // Inner backoff loop: reconnects with exponential backoff
            tracing::info!(symbol = %symbol, "Connecting to mark price stream");

            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(&url),
            )
            .await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!(symbol = %symbol, "Mark price stream connected");
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, symbol = %symbol, "Mark price stream connection failed");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break; // Backoff exhausted, break to outer loop
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!(symbol = %symbol, "Mark price stream connection timed out");
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
                        match serde_json::from_str::<MarkPriceEvent>(&text) {
                            Ok(event) => {
                                match Decimal::from_str(&event.p) {
                                    Ok(price) => {
                                        let _ = tx.send(Some(price));
                                    }
                                    Err(e) => {
                                        tracing::debug!(
                                            error = %e,
                                            raw = %event.p,
                                            "Failed to parse mark price as Decimal"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(
                                    error = %e,
                                    "Failed to parse mark price event"
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
                        tracing::info!(symbol = %symbol, "Mark price stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {
                        // Binary, Pong, Frame - ignore
                    }
                    Ok(Some(Err(e))) => {
                        tracing::warn!(error = %e, "Mark price stream WebSocket error");
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("Mark price stream ended");
                        break;
                    }
                    Err(_) => {
                        // Timeout: no message received in 30 seconds (stale connection)
                        tracing::warn!(symbol = %symbol, "Mark price stream read timeout (30s)");
                        break;
                    }
                }
            }

            tracing::info!(symbol = %symbol, "Mark price stream disconnected, reconnecting...");
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break; // Backoff exhausted, break to outer loop
            }
        }

        // Backoff exhausted -- cooldown then restart with fresh backoff
        tracing::warn!(symbol = %symbol, "Mark price stream backoff exhausted, restarting after 60s cooldown");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

/// Extracted mark price data for position tracking.
///
/// Carries per-symbol mark price, funding rate, and next funding time.
/// Sent via mpsc channel from `run_multi_mark_price_stream` -- mpsc is
/// appropriate because each message carries a different symbol.
#[derive(Debug, Clone)]
pub struct MarkPriceData {
    pub symbol: String,
    pub mark_price: Decimal,
    pub funding_rate: Decimal,
    pub next_funding_time: u64,
}

/// Combined streams wrapper from the AsterDEX combined streams endpoint.
///
/// The combined streams URL returns messages in the format:
/// ```json
/// { "stream": "btcusdt@markPrice@1s", "data": { ... MarkPriceEvent ... } }
/// ```
///
/// This is an implementation detail of `run_multi_mark_price_stream` -- kept private.
#[derive(Debug, Deserialize)]
struct CombinedStreamEvent {
    #[allow(dead_code)]
    pub stream: String,
    pub data: MarkPriceEvent,
}

/// Build the combined streams URL for multiple symbols.
///
/// Returns `wss://fstream.asterdex.com/stream?streams={streams}` where each
/// stream is formatted as `{symbol_lowercase}@markPrice@1s`, joined with `/`.
fn build_combined_streams_url(symbols: &[String]) -> String {
    let streams: Vec<String> = symbols
        .iter()
        .map(|s| format!("{}@markPrice@1s", s.to_lowercase()))
        .collect();
    format!(
        "wss://fstream.asterdex.com/stream?streams={}",
        streams.join("/")
    )
}

/// Run a multi-symbol mark price WebSocket stream with automatic reconnection.
///
/// Connects to the combined streams endpoint for all given symbols over a single
/// WebSocket connection. Each mark price update is parsed and sent as `MarkPriceData`
/// through the mpsc channel.
///
/// If `symbols` is empty, returns immediately (nothing to subscribe to).
/// If the mpsc receiver is dropped, the loop exits gracefully.
///
/// This function runs forever (until the sender cannot deliver).
/// Designed to be spawned with `tokio::spawn`.
///
/// # Arguments
/// * `symbols` - Trading pair symbols (e.g., `["BTCUSDT", "ETHUSDT"]`)
/// * `mark_data_tx` - mpsc sender for per-symbol mark price updates
pub async fn run_multi_mark_price_stream(
    symbols: Vec<String>,
    mark_data_tx: mpsc::Sender<MarkPriceData>,
) {
    if symbols.is_empty() {
        tracing::info!("No symbols for multi mark price stream, returning");
        return;
    }

    let url = build_combined_streams_url(&symbols);
    let symbol_count = symbols.len();

    loop {
        // Outer restart loop: creates a fresh backoff each iteration
        let mut backoff = ExponentialBackoff::new();

        loop {
            // Inner backoff loop: reconnects with exponential backoff
            tracing::info!(
                symbol_count = symbol_count,
                "Connecting to multi mark price stream"
            );

            let connect_result =
                tokio::time::timeout(Duration::from_secs(10), connect_async(&url)).await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!(
                        symbol_count = symbol_count,
                        "Multi mark price stream connected"
                    );
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "Multi mark price stream connection failed"
                    );
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break; // Backoff exhausted, break to outer loop
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!("Multi mark price stream connection timed out");
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
                        match serde_json::from_str::<CombinedStreamEvent>(&text) {
                            Ok(event) => {
                                let mark_price = match Decimal::from_str(&event.data.p) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::debug!(
                                            error = %e,
                                            raw = %event.data.p,
                                            symbol = %event.data.s,
                                            "Failed to parse mark price as Decimal"
                                        );
                                        continue;
                                    }
                                };
                                let funding_rate = match Decimal::from_str(&event.data.r) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        tracing::debug!(
                                            error = %e,
                                            raw = %event.data.r,
                                            symbol = %event.data.s,
                                            "Failed to parse funding rate as Decimal"
                                        );
                                        continue;
                                    }
                                };

                                let data = MarkPriceData {
                                    symbol: event.data.s,
                                    mark_price,
                                    funding_rate,
                                    next_funding_time: event.data.next_funding_time,
                                };

                                match mark_data_tx.try_send(data) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => {
                                        tracing::debug!(
                                            "Mark price channel full, dropping update"
                                        );
                                    }
                                    Err(TrySendError::Closed(_)) => {
                                        tracing::info!(
                                            "Multi mark price stream receiver dropped, exiting"
                                        );
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(
                                    error = %e,
                                    "Failed to parse combined stream event"
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
                        tracing::info!("Multi mark price stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {
                        // Binary, Pong, Frame - ignore
                    }
                    Ok(Some(Err(e))) => {
                        tracing::warn!(
                            error = %e,
                            "Multi mark price stream WebSocket error"
                        );
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("Multi mark price stream ended");
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("Multi mark price stream read timeout (30s)");
                        break;
                    }
                }
            }

            tracing::info!("Multi mark price stream disconnected, reconnecting...");
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break; // Backoff exhausted, break to outer loop
            }
        }

        // Backoff exhausted -- cooldown then restart with fresh backoff
        tracing::warn!("Multi mark price stream backoff exhausted, restarting after 60s cooldown");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mark_price_event_deserialization() {
        let json = r#"{
            "e": "markPriceUpdate",
            "E": 1700000000000,
            "s": "BTCUSDT",
            "p": "50000.12345678",
            "i": "50000.00000000",
            "P": "50000.00000000",
            "r": "0.00010000",
            "T": 1700003600000
        }"#;

        let event: MarkPriceEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.e, "markPriceUpdate");
        assert_eq!(event.event_time, 1700000000000);
        assert_eq!(event.s, "BTCUSDT");
        assert_eq!(event.p, "50000.12345678");
        assert_eq!(event.i, "50000.00000000");
        assert_eq!(event.estimated_settle_price, "50000.00000000");
        assert_eq!(event.r, "0.00010000");
        assert_eq!(event.next_funding_time, 1700003600000);
    }

    #[test]
    fn test_mark_price_decimal_parse() {
        let price_str = "50000.12345678";
        let decimal = Decimal::from_str(price_str).unwrap();
        assert_eq!(decimal.to_string(), "50000.12345678");
    }

    #[test]
    fn test_websocket_url_lowercase() {
        let symbol = "BTCUSDT";
        let url = format!(
            "wss://fstream.asterdex.com/ws/{}@markPrice@1s",
            symbol.to_lowercase()
        );
        assert_eq!(
            url,
            "wss://fstream.asterdex.com/ws/btcusdt@markPrice@1s"
        );
    }

    // --- Multi-symbol combined streams tests ---

    #[test]
    fn test_combined_stream_event_deserialization() {
        let json = r#"{
            "stream": "btcusdt@markPrice@1s",
            "data": {
                "e": "markPriceUpdate",
                "E": 1700000000000,
                "s": "BTCUSDT",
                "p": "50000.12345678",
                "i": "50000.00000000",
                "P": "50000.00000000",
                "r": "0.00010000",
                "T": 1700003600000
            }
        }"#;

        let event: CombinedStreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.stream, "btcusdt@markPrice@1s");
        assert_eq!(event.data.e, "markPriceUpdate");
        assert_eq!(event.data.s, "BTCUSDT");
        assert_eq!(event.data.p, "50000.12345678");
        assert_eq!(event.data.r, "0.00010000");
        assert_eq!(event.data.next_funding_time, 1700003600000);
    }

    #[test]
    fn test_mark_price_data_from_event() {
        // Simulate extracting MarkPriceData from a MarkPriceEvent
        let event = MarkPriceEvent {
            e: "markPriceUpdate".to_string(),
            event_time: 1700000000000,
            s: "ETHUSDT".to_string(),
            p: "3000.50".to_string(),
            i: "3000.00".to_string(),
            estimated_settle_price: "3000.00".to_string(),
            r: "0.00050000".to_string(),
            next_funding_time: 1700003600000,
        };

        let mark_price = Decimal::from_str(&event.p).unwrap();
        let funding_rate = Decimal::from_str(&event.r).unwrap();

        let data = MarkPriceData {
            symbol: event.s.clone(),
            mark_price,
            funding_rate,
            next_funding_time: event.next_funding_time,
        };

        assert_eq!(data.symbol, "ETHUSDT");
        assert_eq!(data.mark_price, Decimal::from_str("3000.50").unwrap());
        assert_eq!(data.funding_rate, Decimal::from_str("0.00050000").unwrap());
        assert_eq!(data.next_funding_time, 1700003600000);
    }

    #[test]
    fn test_combined_streams_url_construction() {
        let symbols = vec![
            "BTCUSDT".to_string(),
            "ETHUSDT".to_string(),
            "SOLUSDT".to_string(),
        ];
        let url = build_combined_streams_url(&symbols);
        assert_eq!(
            url,
            "wss://fstream.asterdex.com/stream?streams=btcusdt@markPrice@1s/ethusdt@markPrice@1s/solusdt@markPrice@1s"
        );
    }

    #[test]
    fn test_combined_streams_url_empty() {
        let symbols: Vec<String> = vec![];
        let url = build_combined_streams_url(&symbols);
        assert_eq!(
            url,
            "wss://fstream.asterdex.com/stream?streams="
        );
    }
}
