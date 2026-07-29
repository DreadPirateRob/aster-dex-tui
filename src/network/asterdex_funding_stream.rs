// src/network/asterdex_funding_stream.rs
// AsterDEX all-symbols mark price stream for funding rate dashboard.
//
// Connects to wss://fstream.asterdex.com/ws/!markPrice@arr@1s,
// parses mark price events for ALL symbols, extracts funding rate data,
// and sends through an mpsc channel for the funding rate table.
//
// Also provides REST bootstrap via /fapi/v1/premiumIndex.

use crate::data::funding_rates::FundingRateEntry;
use crate::network::asterdex_mark_price::MarkPriceEvent;
use crate::network::reconnect::ExponentialBackoff;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// AsterDEX all-symbols mark price WebSocket endpoint.
const ALL_MARK_PRICE_STREAM_URL: &str = "wss://fstream.asterdex.com/ws/!markPrice@arr@1s";

/// REST endpoint for premium index (funding rate bootstrap).
const PREMIUM_INDEX_URL: &str = "https://fapi.asterdex.com/fapi/v1/premiumIndex";

/// Deserialization type for the premiumIndex REST response.
#[derive(Debug, Deserialize)]
#[allow(dead_code, non_snake_case)]
struct PremiumIndexEntry {
    symbol: String,
    markPrice: String,
    #[serde(default)]
    indexPrice: String,
    #[serde(default)]
    estimatedSettlePrice: String,
    lastFundingRate: String,
    nextFundingTime: u64,
    #[serde(default)]
    interestRate: String,
    #[serde(default)]
    time: u64,
}

/// Fetch initial funding rate data for all pairs via REST.
///
/// Calls GET /fapi/v1/premiumIndex (no symbol param = all pairs).
/// Returns Vec<FundingRateEntry> on success, empty Vec on failure.
pub async fn fetch_premium_index(client: &reqwest::Client) -> Vec<FundingRateEntry> {
    let response = match client.get(PREMIUM_INDEX_URL).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch premiumIndex");
            return Vec::new();
        }
    };

    if !response.status().is_success() {
        tracing::warn!(
            status = %response.status(),
            "premiumIndex returned non-success status"
        );
        return Vec::new();
    }

    let entries: Vec<PremiumIndexEntry> = match response.json().await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse premiumIndex response");
            return Vec::new();
        }
    };

    let mut result = Vec::with_capacity(entries.len());
    for entry in entries {
        let mark_price = match Decimal::from_str(&entry.markPrice) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let funding_rate = match Decimal::from_str(&entry.lastFundingRate) {
            Ok(r) => r,
            Err(_) => continue,
        };

        result.push(FundingRateEntry {
            symbol: entry.symbol,
            mark_price,
            funding_rate,
            next_funding_time: entry.nextFundingTime,
        });
    }

    tracing::info!(count = result.len(), "Fetched premiumIndex data");
    result
}

/// Run the all-symbols mark price WebSocket stream with automatic reconnection.
///
/// Connects to `wss://fstream.asterdex.com/ws/!markPrice@arr@1s` which delivers
/// mark price updates for ALL symbols in a single array message every second.
///
/// Follows the `run_liquidation_stream` pattern: double-loop ExponentialBackoff,
/// 10s connect timeout, 30s read timeout, Ping/Pong handling.
///
/// Events are sent as `FundingRateEntry` via the provided `tx` channel.
/// Designed to be spawned with `tokio::spawn`.
pub async fn run_all_mark_price_stream(tx: mpsc::Sender<FundingRateEntry>) {
    loop {
        let mut backoff = ExponentialBackoff::new();

        loop {
            tracing::info!("Connecting to all-symbols mark price stream");

            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(ALL_MARK_PRICE_STREAM_URL),
            )
            .await;

            let ws_stream = match connect_result {
                Ok(Ok((stream, _response))) => {
                    backoff.reset();
                    tracing::info!("All-symbols mark price stream connected");
                    stream
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "All-symbols mark price stream connection failed"
                    );
                    if let Some(delay) = backoff.next_delay() {
                        tokio::time::sleep(delay).await;
                    } else {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    tracing::warn!("All-symbols mark price stream connection timed out");
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
                        // The !markPrice@arr stream sends arrays of events
                        let events: Vec<MarkPriceEvent> =
                            match serde_json::from_str::<Vec<MarkPriceEvent>>(&text) {
                                Ok(events) => events,
                                Err(_) => {
                                    // Try single event fallback
                                    match serde_json::from_str::<MarkPriceEvent>(&text) {
                                        Ok(event) => vec![event],
                                        Err(e) => {
                                            tracing::debug!(
                                                error = %e,
                                                "Failed to parse mark price arr event"
                                            );
                                            continue;
                                        }
                                    }
                                }
                            };

                        for event in &events {
                            let mark_price = match Decimal::from_str(&event.p) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let funding_rate = match Decimal::from_str(&event.r) {
                                Ok(r) => r,
                                Err(_) => continue,
                            };

                            let entry = FundingRateEntry {
                                symbol: event.s.clone(),
                                mark_price,
                                funding_rate,
                                next_funding_time: event.next_funding_time,
                            };

                            match tx.try_send(entry) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    tracing::debug!(
                                        "Funding rate channel full, dropping update"
                                    );
                                }
                                Err(TrySendError::Closed(_)) => {
                                    tracing::info!(
                                        "Funding rate receiver dropped, exiting"
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
                                "All-symbols mark price stream failed to send pong"
                            );
                            break;
                        }
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        tracing::info!("All-symbols mark price stream closed by server");
                        break;
                    }
                    Ok(Some(Ok(_))) => {} // Binary, Pong, Frame - ignore
                    Ok(Some(Err(e))) => {
                        tracing::warn!(
                            error = %e,
                            "All-symbols mark price stream WebSocket error"
                        );
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("All-symbols mark price stream ended");
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(
                            "All-symbols mark price stream read timeout (30s)"
                        );
                        break;
                    }
                }
            }

            tracing::info!(
                "All-symbols mark price stream disconnected, reconnecting..."
            );
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break;
            }
        }

        // Backoff exhausted -- cooldown then restart
        tracing::warn!(
            "All-symbols mark price stream backoff exhausted, restarting after 60s"
        );
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
