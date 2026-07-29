// src/network/asterdex_trade_stream.rs
// AsterDEX aggTrade WebSocket stream for live trade data.
// Connects to wss://fstream.asterdex.com/ws/{symbol}@aggTrade,
// parses aggregate trade events, converts to Trade objects, and
// sends through a ring_channel for the chart runner.

use crate::data::types::{Trade, TradeSide};
use crate::network::reconnect::ExponentialBackoff;
use crate::network::types::{ConnectionState, ConnectionStatus};
use futures_util::{SinkExt, StreamExt};
use ring_channel::RingSender;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// AsterDEX aggregate trade WebSocket event.
///
/// Stream: `<symbol>@aggTrade` (public, no auth required)
/// Example payload:
/// ```json
/// {
///   "e": "aggTrade",
///   "E": 1700000000000,
///   "s": "BTCUSDT",
///   "a": 12345678,
///   "p": "50000.50",
///   "q": "0.123",
///   "f": 100000,
///   "l": 100001,
///   "T": 1700000000123,
///   "m": true
/// }
/// ```
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields present for serde deserialization
pub struct AsterDexAggTradeEvent {
    #[serde(rename = "e")]
    pub event_type: String,
    #[serde(rename = "E")]
    pub event_time: u64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "a")]
    pub agg_trade_id: u64,
    #[serde(rename = "p")]
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(rename = "q")]
    #[serde(with = "rust_decimal::serde::str")]
    pub quantity: Decimal,
    #[serde(rename = "f")]
    pub first_trade_id: u64,
    #[serde(rename = "l")]
    pub last_trade_id: u64,
    #[serde(rename = "T")]
    pub trade_time: u64,
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,
}

impl From<AsterDexAggTradeEvent> for Trade {
    fn from(event: AsterDexAggTradeEvent) -> Self {
        Trade {
            id: event.agg_trade_id,
            price: event.price,
            quantity: event.quantity,
            timestamp: event.trade_time,
            // is_buyer_maker = true means the buyer is the maker,
            // so the trade was initiated by a sell order
            side: if event.is_buyer_maker {
                TradeSide::Sell
            } else {
                TradeSide::Buy
            },
            is_buyer_maker: event.is_buyer_maker,
        }
    }
}

/// Build the WebSocket URL for an AsterDEX aggTrade stream.
///
/// Constructs `wss://fstream.asterdex.com/ws/{symbol_lowercase}@aggTrade`
fn build_agg_trade_url(symbol: &str) -> String {
    format!(
        "wss://fstream.asterdex.com/ws/{}@aggTrade",
        symbol.to_lowercase()
    )
}

/// Manages the AsterDEX aggTrade WebSocket connection with automatic reconnection.
///
/// Uses the same double-loop pattern as `run_mark_price_stream`:
/// - Outer restart loop with fresh backoff each iteration
/// - Inner backoff loop for connection attempts
/// - Inner message loop with 30s read timeout for stale detection
///
/// Trade events are converted to `Trade` and sent via `ring_channel`.
/// Connection status is reported via `watch` channel.
pub struct AsterDexTradeConnectionManager {
    symbol: String,
    trade_tx: RingSender<Trade>,
    status_tx: watch::Sender<ConnectionStatus>,
}

impl AsterDexTradeConnectionManager {
    /// Create a new AsterDEX trade connection manager.
    ///
    /// Returns the manager and a receiver for connection status updates.
    pub fn new(
        symbol: String,
        trade_tx: RingSender<Trade>,
    ) -> (Self, watch::Receiver<ConnectionStatus>) {
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::default());

        let manager = Self {
            symbol,
            trade_tx,
            status_tx,
        };

        (manager, status_rx)
    }

    /// Update connection status and broadcast via watch channel.
    fn update_status(&self, state: ConnectionState) {
        let status = ConnectionStatus {
            state: state.clone(),
            _last_error: None,
            retry_count: match &state {
                ConnectionState::Reconnecting { attempt, .. } => *attempt,
                ConnectionState::Connected => 0,
                _ => self.status_tx.borrow().retry_count,
            },
        };
        let _ = self.status_tx.send(status);
    }

    /// Run the aggTrade WebSocket stream with automatic reconnection.
    ///
    /// This function runs forever (until the trade sender is dropped or the
    /// task is cancelled). Designed to be spawned with `tokio::spawn`.
    ///
    /// Uses the AsterDEX double-loop reconnection pattern:
    /// 1. Outer restart loop: creates fresh backoff each iteration
    /// 2. Inner backoff loop: attempts connection with exponential backoff
    /// 3. Inner message loop: processes messages with 30s read timeout
    /// 4. On backoff exhaustion: 60s cooldown then restart
    pub async fn run_with_reconnect(&mut self) {
        let url = build_agg_trade_url(&self.symbol);

        loop {
            // Outer restart loop: creates a fresh backoff each iteration
            let mut backoff = ExponentialBackoff::new();

            loop {
                // Inner backoff loop: reconnects with exponential backoff
                self.update_status(ConnectionState::Connecting);
                tracing::info!(symbol = %self.symbol, "Connecting to AsterDEX aggTrade stream");

                let connect_result = tokio::time::timeout(
                    Duration::from_secs(10),
                    connect_async(&url),
                )
                .await;

                let ws_stream = match connect_result {
                    Ok(Ok((stream, _response))) => {
                        backoff.reset();
                        self.update_status(ConnectionState::Connected);
                        tracing::info!(symbol = %self.symbol, "AsterDEX aggTrade stream connected");
                        stream
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, symbol = %self.symbol, "AsterDEX aggTrade stream connection failed");
                        self.update_status(ConnectionState::Disconnected);
                        if let Some(delay) = backoff.next_delay() {
                            tokio::time::sleep(delay).await;
                        } else {
                            break; // Backoff exhausted, break to outer loop
                        }
                        continue;
                    }
                    Err(_) => {
                        tracing::warn!(symbol = %self.symbol, "AsterDEX aggTrade stream connection timed out");
                        self.update_status(ConnectionState::Disconnected);
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
                            match serde_json::from_str::<AsterDexAggTradeEvent>(&text) {
                                Ok(event) => {
                                    let trade = Trade::from(event);
                                    tracing::debug!(
                                        symbol = %self.symbol,
                                        side = ?trade.side,
                                        price = %trade.price,
                                        qty = %trade.quantity,
                                        "AsterDEX aggTrade"
                                    );
                                    let _ = self.trade_tx.send(trade);
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        error = %e,
                                        "Failed to parse AsterDEX aggTrade event"
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
                            tracing::info!(symbol = %self.symbol, "AsterDEX aggTrade stream closed by server");
                            break;
                        }
                        Ok(Some(Ok(_))) => {
                            // Binary, Pong, Frame - ignore
                        }
                        Ok(Some(Err(e))) => {
                            tracing::warn!(error = %e, "AsterDEX aggTrade stream WebSocket error");
                            break;
                        }
                        Ok(None) => {
                            tracing::info!("AsterDEX aggTrade stream ended");
                            break;
                        }
                        Err(_) => {
                            // Timeout: no message received in 30 seconds (stale connection)
                            tracing::warn!(symbol = %self.symbol, "AsterDEX aggTrade stream read timeout (30s)");
                            break;
                        }
                    }
                }

                self.update_status(ConnectionState::Reconnecting {
                    attempt: backoff.attempt(),
                    max_attempts: backoff.max_attempts(),
                    _next_retry_at: None,
                });
                tracing::info!(symbol = %self.symbol, "AsterDEX aggTrade stream disconnected, reconnecting...");
                if let Some(delay) = backoff.next_delay() {
                    tokio::time::sleep(delay).await;
                } else {
                    break; // Backoff exhausted, break to outer loop
                }
            }

            // Backoff exhausted -- cooldown then restart with fresh backoff
            tracing::warn!(symbol = %self.symbol, "AsterDEX aggTrade stream backoff exhausted, restarting after 60s cooldown");
            self.update_status(ConnectionState::Disconnected);
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }
}

/// Lightweight aggTrade stream for the DOM volume profile.
///
/// Follows the `run_depth_stream()` pattern: mpsc-based delivery, no ring_channel,
/// no connection status reporting. Simpler reconnection loop.
/// The DOM continues working without trade data if this stream fails.
pub async fn run_dom_trade_stream(
    symbol: String,
    tx: tokio::sync::mpsc::Sender<Trade>,
) {
    let url = build_agg_trade_url(&symbol);

    loop {
        let mut backoff = ExponentialBackoff::new();

        loop {
            tracing::info!(symbol = %symbol, "Connecting to DOM aggTrade stream");

            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(&url),
            )
            .await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!(symbol = %symbol, "DOM aggTrade stream connected");
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, symbol = %symbol, "DOM aggTrade connection failed");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!(symbol = %symbol, "DOM aggTrade connection timed out");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break;
                    }
                    continue;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            loop {
                let msg = tokio::time::timeout(Duration::from_secs(30), read.next()).await;

                match msg {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        match serde_json::from_str::<AsterDexAggTradeEvent>(&text) {
                            Ok(event) => {
                                let trade = Trade::from(event);
                                match tx.try_send(trade) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => {
                                        tracing::debug!("DOM trade channel full, dropping trade");
                                    }
                                    Err(TrySendError::Closed(_)) => {
                                        tracing::info!("DOM trade receiver dropped, exiting");
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, "Failed to parse DOM aggTrade event");
                            }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        if let Err(e) = write.send(Message::Pong(data)).await {
                            tracing::warn!(error = %e, "DOM aggTrade failed to send pong");
                            break;
                        }
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        tracing::info!(symbol = %symbol, "DOM aggTrade stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {} // Binary, Pong, Frame - ignore
                    Ok(Some(Err(e))) => {
                        tracing::warn!(error = %e, "DOM aggTrade WebSocket error");
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("DOM aggTrade stream ended");
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(symbol = %symbol, "DOM aggTrade read timeout (30s)");
                        break;
                    }
                }
            }

            tracing::info!(symbol = %symbol, "DOM aggTrade disconnected, reconnecting...");
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break;
            }
        }

        // Backoff exhausted — cooldown then restart
        tracing::warn!(symbol = %symbol, "DOM aggTrade backoff exhausted, restarting after 60s");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_agg_trade_event_deserialization() {
        let json = r#"{
            "e": "aggTrade",
            "E": 1700000000000,
            "s": "BTCUSDT",
            "a": 12345678,
            "p": "50000.50",
            "q": "0.123",
            "f": 100000,
            "l": 100001,
            "T": 1700000000123,
            "m": true
        }"#;

        let event: AsterDexAggTradeEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "aggTrade");
        assert_eq!(event.event_time, 1700000000000);
        assert_eq!(event.symbol, "BTCUSDT");
        assert_eq!(event.agg_trade_id, 12345678);
        assert_eq!(event.price, Decimal::from_str("50000.50").unwrap());
        assert_eq!(event.quantity, Decimal::from_str("0.123").unwrap());
        assert_eq!(event.first_trade_id, 100000);
        assert_eq!(event.last_trade_id, 100001);
        assert_eq!(event.trade_time, 1700000000123);
        assert!(event.is_buyer_maker);
    }

    #[test]
    fn test_agg_trade_event_to_trade_sell() {
        let event = AsterDexAggTradeEvent {
            event_type: "aggTrade".to_string(),
            event_time: 1700000000000,
            symbol: "BTCUSDT".to_string(),
            agg_trade_id: 999,
            price: Decimal::from_str("50000.00").unwrap(),
            quantity: Decimal::from_str("1.5").unwrap(),
            first_trade_id: 1,
            last_trade_id: 2,
            trade_time: 1700000000123,
            is_buyer_maker: true, // buyer is maker -> sell initiated
        };

        let trade = Trade::from(event);
        assert_eq!(trade.id, 999); // id = agg_trade_id
        assert_eq!(trade.price, Decimal::from_str("50000.00").unwrap());
        assert_eq!(trade.quantity, Decimal::from_str("1.5").unwrap());
        assert_eq!(trade.timestamp, 1700000000123); // timestamp = trade_time
        assert_eq!(trade.side, TradeSide::Sell);
        assert!(trade.is_buyer_maker);
    }

    #[test]
    fn test_agg_trade_event_to_trade_buy() {
        let event = AsterDexAggTradeEvent {
            event_type: "aggTrade".to_string(),
            event_time: 1700000000000,
            symbol: "ETHUSDT".to_string(),
            agg_trade_id: 1000,
            price: Decimal::from_str("3000.75").unwrap(),
            quantity: Decimal::from_str("0.5").unwrap(),
            first_trade_id: 3,
            last_trade_id: 4,
            trade_time: 1700000001000,
            is_buyer_maker: false, // buyer is taker -> buy initiated
        };

        let trade = Trade::from(event);
        assert_eq!(trade.id, 1000);
        assert_eq!(trade.side, TradeSide::Buy);
        assert!(!trade.is_buyer_maker);
    }

    #[test]
    fn test_websocket_url_construction() {
        assert_eq!(
            build_agg_trade_url("BTCUSDT"),
            "wss://fstream.asterdex.com/ws/btcusdt@aggTrade"
        );
        assert_eq!(
            build_agg_trade_url("ethusdt"),
            "wss://fstream.asterdex.com/ws/ethusdt@aggTrade"
        );
        assert_eq!(
            build_agg_trade_url("SoLuSdT"),
            "wss://fstream.asterdex.com/ws/solusdt@aggTrade"
        );
    }
}
