// src/network/finnhub_calendar.rs
// Finnhub economic calendar REST fetch and polling task.
//
// Fetches economic calendar data from Finnhub's free-tier API.
// Polling task re-fetches at 30-minute intervals for background updates.

use crate::data::economic_calendar::{EconomicCalendarResponse, EconomicEvent};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

/// Fetch economic calendar events from Finnhub REST API.
///
/// Returns parsed events on success, empty Vec on failure (matching funding pattern).
pub async fn fetch_economic_calendar(
    http_client: &reqwest::Client,
    api_key: &str,
    from: &str,
    to: &str,
) -> Vec<EconomicEvent> {
    let url = format!(
        "https://finnhub.io/api/v1/calendar/economic?from={}&to={}&token={}",
        from, to, api_key
    );

    let response = match http_client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch economic calendar");
            return Vec::new();
        }
    };

    if !response.status().is_success() {
        tracing::warn!(
            status = %response.status(),
            "Economic calendar returned non-success status"
        );
        return Vec::new();
    }

    let calendar_response: EconomicCalendarResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse economic calendar response");
            return Vec::new();
        }
    };

    let events: Vec<EconomicEvent> = calendar_response
        .economic_calendar
        .unwrap_or_default()
        .into_iter()
        .map(EconomicEvent::from_raw)
        .collect();

    tracing::info!(count = events.len(), from = %from, to = %to, "Fetched economic calendar");
    events
}

/// Poll economic calendar at 30-minute intervals.
///
/// Skips the first tick (bootstrap already fetched). Sends new events through
/// the mpsc channel. Exits when the receiver is dropped.
pub async fn poll_economic_calendar(
    tx: mpsc::Sender<Vec<EconomicEvent>>,
    http_client: reqwest::Client,
    api_key: String,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(30 * 60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    // Skip first tick (bootstrap already fetched)
    interval.tick().await;

    loop {
        interval.tick().await;

        // Compute date range: today to 7 days out
        let today = chrono::Utc::now().date_naive();
        let end = today + chrono::Duration::days(7);
        let from = today.format("%Y-%m-%d").to_string();
        let to = end.format("%Y-%m-%d").to_string();

        let events = fetch_economic_calendar(&http_client, &api_key, &from, &to).await;

        if !events.is_empty() {
            if tx.send(events).await.is_err() {
                tracing::info!("Economic calendar receiver dropped, exiting poller");
                return;
            }
        }
    }
}
