// src/network/asterdex_liquidation_stream.rs
// AsterDEX forceOrder WebSocket stream for live liquidation data.
//
// Connects to wss://fstream.asterdex.com/ws/!forceOrder@arr,
// parses force order events, converts to LiquidationEvent objects,
// and sends through an mpsc channel for the liquidation feed.

use crate::data::liquidation::{LiquidationEvent, LiquidationSide};
use crate::network::reconnect::ExponentialBackoff;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// AsterDEX forceOrder WebSocket endpoint (all symbols).
const FORCE_ORDER_STREAM_URL: &str = "wss://fstream.asterdex.com/ws/!forceOrder@arr";

/// Wrapper event from the forceOrder stream.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ForceOrderEvent {
    /// Event type (e.g., "forceOrder")
    e: String,
    /// Event time (ms since epoch)
    #[serde(rename = "E")]
    event_time: u64,
    /// Force order payload
    o: ForceOrderPayload,
}

/// Payload within a forceOrder event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ForceOrderPayload {
    /// Symbol (e.g., "BTCUSDT")
    s: String,
    /// Side ("BUY" or "SELL")
    #[serde(rename = "S")]
    side: String,
    /// Order type (e.g., "LIMIT")
    o: String,
    /// Time in force (e.g., "IOC")
    f: String,
    /// Original quantity
    q: String,
    /// Price
    p: String,
    /// Average price
    ap: String,
    /// Order status (e.g., "FILLED")
    #[serde(rename = "X")]
    status: String,
    /// Last filled quantity
    l: String,
    /// Accumulated filled quantity
    z: String,
    /// Trade time (ms since epoch)
    #[serde(rename = "T")]
    trade_time: u64,
}

/// Convert a ForceOrderPayload into a LiquidationEvent.
///
/// Parses string numeric fields into Decimal, with fallback logic:
/// - quantity: prefer `z` (accumulated filled qty), fall back to `q` (original qty)
/// - price: prefer `ap` (average price), fall back to `p` (order price)
fn payload_to_event(payload: &ForceOrderPayload) -> Option<LiquidationEvent> {
    let side = if payload.side == "SELL" {
        LiquidationSide::LongLiquidated
    } else {
        LiquidationSide::ShortLiquidated
    };

    // Parse quantity: prefer accumulated filled qty (z), fall back to original qty (q)
    let quantity = Decimal::from_str(&payload.z)
        .ok()
        .filter(|d| !d.is_zero())
        .or_else(|| Decimal::from_str(&payload.q).ok())?;

    // Parse price: prefer average price (ap), fall back to order price (p)
    let price = Decimal::from_str(&payload.ap)
        .ok()
        .filter(|d| !d.is_zero())
        .or_else(|| Decimal::from_str(&payload.p).ok())?;

    let usd_value = quantity * price;

    Some(LiquidationEvent {
        symbol: payload.s.clone(),
        side,
        quantity,
        price,
        usd_value,
        timestamp: payload.trade_time,
    })
}

/// Run the forceOrder WebSocket stream with automatic reconnection.
///
/// Follows the `run_dom_trade_stream()` pattern: mpsc-based delivery,
/// double-loop reconnection with ExponentialBackoff, 30s read timeout
/// for stale detection.
///
/// Events are sent as `LiquidationEvent` via the provided `tx` channel.
/// Designed to be spawned with `tokio::spawn`.
pub async fn run_liquidation_stream(tx: mpsc::Sender<LiquidationEvent>) {
    loop {
        let mut backoff = ExponentialBackoff::new();

        loop {
            tracing::info!("Connecting to forceOrder liquidation stream");

            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(FORCE_ORDER_STREAM_URL),
            )
            .await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!("ForceOrder liquidation stream connected");
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "ForceOrder liquidation stream connection failed");
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!("ForceOrder liquidation stream connection timed out");
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
                        // The !forceOrder@arr stream sends arrays of events
                        match serde_json::from_str::<Vec<ForceOrderEvent>>(&text) {
                            Ok(events) => {
                                for event in &events {
                                    if let Some(liq_event) = payload_to_event(&event.o) {
                                        tracing::debug!(
                                            symbol = %liq_event.symbol,
                                            side = ?liq_event.side,
                                            usd = %liq_event.usd_value,
                                            "Liquidation event"
                                        );
                                        match tx.try_send(liq_event) {
                                            Ok(()) => {}
                                            Err(TrySendError::Full(_)) => {
                                                tracing::debug!(
                                                    "Liquidation channel full, dropping event"
                                                );
                                            }
                                            Err(TrySendError::Closed(_)) => {
                                                tracing::info!(
                                                    "Liquidation receiver dropped, exiting"
                                                );
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                // Try parsing as a single event (some streams send individual objects)
                                match serde_json::from_str::<ForceOrderEvent>(&text) {
                                    Ok(event) => {
                                        if let Some(liq_event) = payload_to_event(&event.o) {
                                            tracing::debug!(
                                                symbol = %liq_event.symbol,
                                                side = ?liq_event.side,
                                                usd = %liq_event.usd_value,
                                                "Liquidation event"
                                            );
                                            match tx.try_send(liq_event) {
                                                Ok(()) => {}
                                                Err(TrySendError::Full(_)) => {
                                                    tracing::debug!(
                                                        "Liquidation channel full, dropping event"
                                                    );
                                                }
                                                Err(TrySendError::Closed(_)) => {
                                                    tracing::info!(
                                                        "Liquidation receiver dropped, exiting"
                                                    );
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::debug!(
                                            error = %e,
                                            "Failed to parse forceOrder event"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        if let Err(e) = write.send(Message::Pong(data)).await {
                            tracing::warn!(error = %e, "ForceOrder stream failed to send pong");
                            break;
                        }
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        tracing::info!("ForceOrder liquidation stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {} // Binary, Pong, Frame - ignore
                    Ok(Some(Err(e))) => {
                        tracing::warn!(error = %e, "ForceOrder liquidation stream WebSocket error");
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("ForceOrder liquidation stream ended");
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("ForceOrder liquidation stream read timeout (30s)");
                        break;
                    }
                }
            }

            tracing::info!("ForceOrder liquidation stream disconnected, reconnecting...");
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break;
            }
        }

        // Backoff exhausted -- cooldown then restart
        tracing::warn!("ForceOrder liquidation stream backoff exhausted, restarting after 60s");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
