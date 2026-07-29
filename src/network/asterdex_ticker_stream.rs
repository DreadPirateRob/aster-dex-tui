// src/network/asterdex_ticker_stream.rs
// AsterDEX all-symbols 24hr ticker stream for market overview widget.
//
// Connects to wss://fstream.asterdex.com/ws/!ticker@arr,
// parses 24hr ticker events for ALL symbols, extracts price/volume data,
// and sends through an mpsc channel for the ticker table.
//
// Also provides REST bootstrap via /fapi/v1/ticker/24hr.

use crate::data::ticker::TickerEntry;
use crate::network::reconnect::ExponentialBackoff;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// AsterDEX all-symbols 24hr ticker WebSocket endpoint.
const TICKER_STREAM_URL: &str = "wss://fstream.asterdex.com/ws/!ticker@arr";

/// REST endpoint for 24hr ticker data (all symbols).
const TICKER_24HR_URL: &str = "https://fapi.asterdex.com/fapi/v1/ticker/24hr";

/// Deserialization type for the /fapi/v1/ticker/24hr REST response.
#[derive(Debug, Deserialize)]
#[allow(dead_code, non_snake_case)]
struct Ticker24hrResponse {
    symbol: String,
    priceChange: String,
    priceChangePercent: String,
    weightedAvgPrice: String,
    lastPrice: String,
    lastQty: String,
    openPrice: String,
    highPrice: String,
    lowPrice: String,
    volume: String,
    quoteVolume: String,
    openTime: u64,
    closeTime: u64,
    firstId: i64,
    lastId: i64,
    count: u64,
}

/// Deserialization type for the !ticker@arr WebSocket event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TickerEvent {
    s: String,           // symbol
    p: String,           // price change (absolute)
    #[serde(rename = "P")]
    price_change_pct: String, // price change percent
    c: String,           // last price
    h: String,           // 24h high
    l: String,           // 24h low
    q: String,           // quote volume (USD)
    n: u64,              // trade count
}

/// Fetch initial 24hr ticker data for all pairs via REST.
///
/// Calls GET /fapi/v1/ticker/24hr (no symbol param = all pairs).
/// Returns Vec<TickerEntry> on success, empty Vec on failure.
pub async fn fetch_ticker_24hr(client: &reqwest::Client) -> Vec<TickerEntry> {
    let response = match client.get(TICKER_24HR_URL).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch ticker/24hr");
            return Vec::new();
        }
    };

    if !response.status().is_success() {
        tracing::warn!(
            status = %response.status(),
            "ticker/24hr returned non-success status"
        );
        return Vec::new();
    }

    let entries: Vec<Ticker24hrResponse> = match response.json().await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse ticker/24hr response");
            return Vec::new();
        }
    };

    let result: Vec<TickerEntry> = entries
        .into_iter()
        .filter_map(|entry| {
            let last_price = Decimal::from_str(&entry.lastPrice).ok()?;
            let price_change = Decimal::from_str(&entry.priceChange).ok()?;
            let price_change_percent = Decimal::from_str(&entry.priceChangePercent).ok()?;
            let high_price = Decimal::from_str(&entry.highPrice).ok()?;
            let low_price = Decimal::from_str(&entry.lowPrice).ok()?;
            let quote_volume = Decimal::from_str(&entry.quoteVolume).ok()?;

            Some(TickerEntry {
                symbol: entry.symbol,
                last_price,
                price_change,
                price_change_percent,
                high_price,
                low_price,
                quote_volume,
                trade_count: entry.count,
            })
        })
        .collect();

    tracing::info!(count = result.len(), "Fetched ticker/24hr data");
    result
}

/// Run the all-symbols 24hr ticker WebSocket stream with automatic reconnection.
///
/// Connects to `wss://fstream.asterdex.com/ws/!ticker@arr` which delivers
/// 24hr ticker updates for ALL symbols.
///
/// Follows the `run_all_mark_price_stream` pattern: double-loop ExponentialBackoff,
/// 10s connect timeout, 30s read timeout, Ping/Pong handling.
///
/// Events are sent as `TickerEntry` via the provided `tx` channel.
/// Designed to be spawned with `tokio::spawn`.
pub async fn run_ticker_stream(tx: mpsc::Sender<TickerEntry>) {
    loop {
        let mut backoff = ExponentialBackoff::new();

        loop {
            tracing::info!("Connecting to all-symbols ticker stream");

            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(TICKER_STREAM_URL),
            )
            .await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!("All-symbols ticker stream connected");
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "All-symbols ticker stream connection failed"
                    );
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!("All-symbols ticker stream connection timed out");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break;
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
                        // The !ticker@arr stream sends arrays of ticker events
                        let events: Vec<TickerEvent> =
                            match serde_json::from_str::<Vec<TickerEvent>>(&text) {
                                Ok(events) => events,
                                Err(_) => {
                                    // Try single event fallback
                                    match serde_json::from_str::<TickerEvent>(&text) {
                                        Ok(event) => vec![event],
                                        Err(e) => {
                                            tracing::debug!(
                                                error = %e,
                                                "Failed to parse ticker arr event"
                                            );
                                            continue;
                                        }
                                    }
                                }
                            };

                        for event in &events {
                            let last_price = match Decimal::from_str(&event.c) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let price_change = match Decimal::from_str(&event.p) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let price_change_percent =
                                match Decimal::from_str(&event.price_change_pct) {
                                    Ok(p) => p,
                                    Err(_) => continue,
                                };
                            let high_price = match Decimal::from_str(&event.h) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let low_price = match Decimal::from_str(&event.l) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let quote_volume = match Decimal::from_str(&event.q) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };

                            let entry = TickerEntry {
                                symbol: event.s.clone(),
                                last_price,
                                price_change,
                                price_change_percent,
                                high_price,
                                low_price,
                                quote_volume,
                                trade_count: event.n,
                            };

                            match tx.try_send(entry) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    tracing::trace!(
                                        "Ticker channel full, dropping update"
                                    );
                                }
                                Err(TrySendError::Closed(_)) => {
                                    tracing::info!(
                                        "Ticker receiver dropped, exiting"
                                    );
                                    return;
                                }
                            }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        if let Err(e) = write.send(Message::Pong(data)).await {
                            tracing::warn!(
                                error = %e,
                                "All-symbols ticker stream failed to send pong"
                            );
                            break;
                        }
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        tracing::info!("All-symbols ticker stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {} // Binary, Pong, Frame - ignore
                    Ok(Some(Err(e))) => {
                        tracing::warn!(
                            error = %e,
                            "All-symbols ticker stream WebSocket error"
                        );
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("All-symbols ticker stream ended");
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(
                            "All-symbols ticker stream read timeout (30s)"
                        );
                        break;
                    }
                }
            }

            tracing::info!(
                "All-symbols ticker stream disconnected, reconnecting..."
            );
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break;
            }
        }

        // Backoff exhausted -- cooldown then restart
        tracing::warn!(
            "All-symbols ticker stream backoff exhausted, restarting after 60s"
        );
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
