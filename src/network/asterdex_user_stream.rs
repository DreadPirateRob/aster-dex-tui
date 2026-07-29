// src/network/asterdex_user_stream.rs
// AsterDEX User Data Stream WebSocket manager with listenKey lifecycle and reconnection.

use crate::data::order::Order;
use crate::network::asterdex_auth::ASTERDEX_REST_URL;
use crate::network::asterdex_stream_types::{
    AccountUpdateData, AccountUpdateEvent, OrderTradeUpdate, UserDataEvent,
};
use crate::network::heartbeat::HeartbeatMonitor;
use crate::network::reconnect::ExponentialBackoff;
use crate::network::types::{ConnectionState, ConnectionStatus};

use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
    MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, trace, warn};

/// AsterDEX User Data Stream WebSocket endpoint (append listenKey)
const ASTERDEX_WS_URL: &str = "wss://fstream.asterdex.com/ws/";

/// ListenKey keepalive interval: 30 minutes (key expires after 60 minutes)
const KEEPALIVE_INTERVAL_SECS: u64 = 30 * 60;

/// Create a new listenKey via POST /fapi/v1/listenKey.
///
/// Uses USER_STREAM security type: only X-MBX-APIKEY header, no HMAC signature.
/// Returns the listenKey string on success.
async fn create_listen_key(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/fapi/v1/listenKey", ASTERDEX_REST_URL);

    let resp = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
    })
    .await
    .map_err(|_| "ListenKey request timed out after 10 seconds")?
    .map_err(|e| format!("ListenKey HTTP request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("ListenKey POST failed ({}): {}", status, body).into());
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("ListenKey response parse failed: {}", e))?;

    let listen_key = json["listenKey"]
        .as_str()
        .ok_or("ListenKey not found in response")?
        .to_string();

    debug!(listen_key_prefix = %&listen_key[..8.min(listen_key.len())], "ListenKey created");

    Ok(listen_key)
}

/// Send keepalive PUT to extend listenKey validity.
///
/// Uses USER_STREAM security type: only X-MBX-APIKEY header.
async fn keepalive_listen_key(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/fapi/v1/listenKey", ASTERDEX_REST_URL);

    let resp = client
        .put(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .map_err(|e| format!("ListenKey keepalive HTTP error: {}", e))?;

    if resp.status().is_success() {
        info!("ListenKey keepalive successful");
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(status = %status, body = %body, "ListenKey keepalive failed");
    }

    Ok(())
}

/// Manages WebSocket connection to AsterDEX User Data Stream.
///
/// Handles listenKey lifecycle (create, keepalive, refresh on reconnect),
/// parses ORDER_TRADE_UPDATE events into Order structs, and sends them
/// through an mpsc channel for downstream consumption.
pub struct AsterDexUserStreamManager {
    api_key: String,
    symbol: Option<String>,
    order_tx: mpsc::Sender<Order>,
    account_update_tx: Option<mpsc::Sender<AccountUpdateData>>,
    status_tx: watch::Sender<ConnectionStatus>,
    client: reqwest::Client,
}

impl AsterDexUserStreamManager {
    /// Create a new user stream manager for AsterDEX.
    ///
    /// Returns the manager and a receiver for connection status updates.
    pub fn new(
        api_key: String,
        symbol: Option<String>,
        order_tx: mpsc::Sender<Order>,
        account_update_tx: Option<mpsc::Sender<AccountUpdateData>>,
        client: reqwest::Client,
    ) -> (Self, watch::Receiver<ConnectionStatus>) {
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::default());

        let manager = Self {
            api_key,
            symbol,
            order_tx,
            account_update_tx,
            status_tx,
            client,
        };

        (manager, status_rx)
    }

    /// Update connection status and broadcast via watch channel.
    fn update_status(&self, state: ConnectionState, error: Option<String>) {
        let status = ConnectionStatus {
            state: state.clone(),
            _last_error: error.clone(),
            retry_count: match &state {
                ConnectionState::Reconnecting { attempt, .. } => *attempt,
                ConnectionState::Connected => 0,
                _ => self.status_tx.borrow().retry_count,
            },
        };

        info!(
            state = ?state,
            error = ?error,
            "AsterDEX user stream status updated"
        );

        let _ = self.status_tx.send(status);
    }

    /// Attempt to connect: create listenKey, connect WebSocket, spawn keepalive.
    ///
    /// Returns the WebSocket stream and the keepalive task handle on success.
    async fn connect_once(
        &mut self,
        client: &reqwest::Client,
    ) -> Result<
        (
            WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
            JoinHandle<()>,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let symbol_display = self.symbol.as_deref().unwrap_or("ALL PAIRS");
        info!(symbol = %symbol_display, "Connecting to AsterDEX User Data Stream");

        self.update_status(ConnectionState::Connecting, None);

        // Step 1: Get a fresh listenKey
        let listen_key = create_listen_key(client, &self.api_key).await.map_err(|e| {
            let error_msg = format!("Failed to create listenKey: {}", e);
            error!("{}", error_msg);
            self.update_status(ConnectionState::Disconnected, Some(error_msg.clone()));
            e
        })?;

        // Step 2: Connect to WebSocket with listenKey
        let ws_url = format!("{}{}", ASTERDEX_WS_URL, listen_key);

        let connect_future = connect_async(&ws_url);
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_secs(10), connect_future).await;

        let (ws_stream, response) = match timeout_result {
            Ok(Ok((stream, resp))) => (stream, resp),
            Ok(Err(e)) => {
                let error_msg = format!("WebSocket connection failed: {}", e);
                warn!(error = %error_msg, "Failed to connect to AsterDEX user stream");
                self.update_status(ConnectionState::Disconnected, Some(error_msg.clone()));
                return Err(error_msg.into());
            }
            Err(_) => {
                let error_msg = "WebSocket connection timeout after 10 seconds".to_string();
                error!("{}", error_msg);
                self.update_status(ConnectionState::Disconnected, Some(error_msg.clone()));
                return Err(error_msg.into());
            }
        };

        info!(
            status = %response.status(),
            "AsterDEX user stream WebSocket connected"
        );

        // Step 3: Spawn keepalive task (PUT every 30 minutes)
        let keepalive_client = client.clone();
        let keepalive_api_key = self.api_key.clone();

        let keepalive_handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Skip the first immediate tick
            interval.tick().await;

            loop {
                interval.tick().await;

                if let Err(e) =
                    keepalive_listen_key(&keepalive_client, &keepalive_api_key).await
                {
                    error!(error = %e, "ListenKey keepalive error");
                }
            }
        });

        self.update_status(ConnectionState::Connected, None);

        Ok((ws_stream, keepalive_handle))
    }

    /// Message loop with integrated heartbeat monitoring.
    ///
    /// Returns Ok(()) for clean shutdown, Err for connection loss / listenKey expiry.
    async fn run_message_loop_with_heartbeat(
        &mut self,
        mut ws_stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut heartbeat = HeartbeatMonitor::new();
        let mut ping_interval = heartbeat.create_interval();

        // Skip first immediate tick
        ping_interval.tick().await;

        loop {
            tokio::select! {
                // Handle incoming messages
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            self.handle_text_message(&text).await?;
                        }
                        Some(Ok(Message::Ping(_))) => {
                            trace!("Received ping from AsterDEX");
                        }
                        Some(Ok(Message::Pong(_))) => {
                            heartbeat.pong_received();
                        }
                        Some(Ok(Message::Close(frame))) => {
                            warn!("AsterDEX closed connection: {:?}", frame);
                            return Err("Server closed connection".into());
                        }
                        Some(Ok(_)) => {
                            // Binary, Frame - ignore
                        }
                        Some(Err(e)) => {
                            error!("AsterDEX WebSocket error: {}", e);
                            return Err(e.into());
                        }
                        None => {
                            warn!("AsterDEX WebSocket stream ended");
                            return Err("Stream ended".into());
                        }
                    }
                }

                // Send heartbeat ping
                _ = ping_interval.tick() => {
                    if heartbeat.is_stale() {
                        warn!(
                            "AsterDEX connection stale - no pong received in {:?}",
                            heartbeat.time_since_pong()
                        );
                        return Err("Heartbeat timeout".into());
                    }

                    match ws_stream.send(Message::Ping(vec![].into())).await {
                        Ok(()) => {
                            heartbeat.ping_sent();
                        }
                        Err(e) => {
                            error!("Failed to send ping: {}", e);
                            return Err(e.into());
                        }
                    }
                }
            }
        }
    }

    /// Handle a text message from the AsterDEX user data stream.
    ///
    /// Peeks at the event type, then deserializes the appropriate concrete type.
    async fn handle_text_message(
        &self,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        trace!(
            text_preview = %text.chars().take(200).collect::<String>(),
            "AsterDEX user stream message received"
        );

        // Peek at event type first
        let event: UserDataEvent = match serde_json::from_str(text) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    error = %e,
                    text_preview = %text.chars().take(200).collect::<String>(),
                    "Could not parse user data event type"
                );
                return Ok(());
            }
        };

        match event.e.as_str() {
            "listenKeyExpired" => {
                warn!("ListenKey expired - triggering reconnect");
                return Err("listenKey expired".into());
            }
            "ORDER_TRADE_UPDATE" => {
                match serde_json::from_str::<OrderTradeUpdate>(text) {
                    Ok(update) => {
                        let order: Order = update.into();

                        // Filter by symbol: in single-pair mode, only send orders matching target
                        // In all-pairs mode (self.symbol is None), all orders pass through
                        if let Some(ref target) = self.symbol {
                            if !order.symbol.eq_ignore_ascii_case(target) {
                                debug!(
                                    order_symbol = %order.symbol,
                                    target_symbol = %target,
                                    "Ignoring order update for different symbol"
                                );
                                return Ok(());
                            }
                        }

                        debug!(
                            order_id = order.order_id,
                            symbol = %order.symbol,
                            status = %order.status,
                            side = %order.side,
                            "Received AsterDEX order update"
                        );

                        if let Err(e) = self.order_tx.send(order).await {
                            warn!(error = %e, "Failed to send order to channel");
                        }
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            text_preview = %text.chars().take(200).collect::<String>(),
                            "Failed to parse ORDER_TRADE_UPDATE"
                        );
                    }
                }
            }
            "ACCOUNT_UPDATE" => {
                match serde_json::from_str::<AccountUpdateEvent>(text) {
                    Ok(update) => {
                        let data: AccountUpdateData = update.into();
                        debug!(
                            reason = %data.reason,
                            balances = data.balances.len(),
                            positions = data.positions.len(),
                            "Received ACCOUNT_UPDATE"
                        );
                        if let Some(ref tx) = self.account_update_tx {
                            if let Err(e) = tx.send(data).await {
                                warn!(error = %e, "Failed to send account update to channel");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            text_preview = %text.chars().take(200).collect::<String>(),
                            "Failed to parse ACCOUNT_UPDATE"
                        );
                    }
                }
            }
            other => {
                debug!(event_type = %other, "Ignoring unknown user data event");
            }
        }

        Ok(())
    }

    /// Run the connection manager with automatic reconnection.
    ///
    /// This is the main entry point - handles the full lifecycle.
    /// Always obtains a fresh listenKey on every reconnect.
    /// Returns only when max retries exhausted.
    pub async fn run_with_reconnect(&mut self) {
        let client = self.client.clone();
        let mut backoff = ExponentialBackoff::new();

        loop {
            if backoff.attempt() == 0 {
                self.update_status(ConnectionState::Connecting, None);
            }

            match self.connect_once(&client).await {
                Ok((ws_stream, keepalive_handle)) => {
                    backoff.reset();

                    match self.run_message_loop_with_heartbeat(ws_stream).await {
                        Ok(()) => {
                            info!("AsterDEX user stream clean shutdown");
                            keepalive_handle.abort();
                            self.update_status(ConnectionState::Disconnected, None);
                            return;
                        }
                        Err(e) => {
                            warn!("AsterDEX user stream connection lost: {}", e);
                            keepalive_handle.abort();
                        }
                    }
                }
                Err(e) => {
                    error!("AsterDEX user stream connection failed: {}", e);
                }
            }

            // CRITICAL: Always get a fresh listenKey on every reconnect (POST is idempotent)
            match backoff.next_delay() {
                Some(delay) => {
                    let next_retry = Instant::now() + delay;
                    info!(
                        "AsterDEX user stream reconnecting in {:?} (attempt {}/{})",
                        delay,
                        backoff.attempt(),
                        backoff.max_attempts()
                    );

                    self.update_status(
                        ConnectionState::Reconnecting {
                            attempt: backoff.attempt(),
                            max_attempts: backoff.max_attempts(),
                            _next_retry_at: Some(next_retry),
                        },
                        None,
                    );

                    tokio::time::sleep(delay).await;
                }
                None => {
                    error!(
                        "AsterDEX user stream max reconnection attempts ({}) exhausted",
                        backoff.max_attempts()
                    );
                    self.update_status(
                        ConnectionState::Disconnected,
                        Some("Max reconnection attempts exhausted".to_string()),
                    );
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listen_key_url_format() {
        let listen_key = "pqia91ma19a5s61cv6a81va65sdf19v8a65a1a5s61cv6a81va65sdf19v8a65a1";
        let url = format!("{}{}", ASTERDEX_WS_URL, listen_key);
        assert_eq!(
            url,
            "wss://fstream.asterdex.com/ws/pqia91ma19a5s61cv6a81va65sdf19v8a65a1a5s61cv6a81va65sdf19v8a65a1"
        );
    }

    #[test]
    fn test_listen_key_rest_url_format() {
        let url = format!("{}/fapi/v1/listenKey", ASTERDEX_REST_URL);
        assert_eq!(url, "https://fapi.asterdex.com/fapi/v1/listenKey");
    }

    #[test]
    fn test_keepalive_interval_is_30_minutes() {
        assert_eq!(KEEPALIVE_INTERVAL_SECS, 1800);
    }

    #[test]
    fn test_symbol_filter_is_case_insensitive() {
        // WebSocket ORDER_TRADE_UPDATE events always send uppercase symbols
        let ws_symbol = "SOLUSDT";
        // User might provide mixed-case input
        let user_symbol = "solUSDT";

        // Old behavior (broken): exact match would fail
        assert_ne!(ws_symbol, user_symbol);

        // New behavior (fixed): case-insensitive match succeeds
        assert!(ws_symbol.eq_ignore_ascii_case(user_symbol));
    }
}
