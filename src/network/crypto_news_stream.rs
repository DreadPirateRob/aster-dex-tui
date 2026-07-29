// src/network/crypto_news_stream.rs
// SSE stream and REST bootstrap for live crypto news from cryptocurrency.cv.
//
// Connects to the SSE endpoint for live news events with automatic
// reconnection using ExponentialBackoff. Also provides REST bootstrap
// to fetch initial headlines on startup.

use crate::data::news_feed::{parse_sse_articles, NewsArticle, SseArticle};
use crate::network::reconnect::ExponentialBackoff;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;

/// Base URL for the cryptocurrency.cv news REST API.
const NEWS_API_URL: &str = "https://cryptocurrency.cv/api/news?limit=50";

/// Base URL for the cryptocurrency.cv SSE stream.
const SSE_BASE_URL: &str = "https://cryptocurrency.cv/api/sse";

/// User-Agent header value (required — API blocks default UAs with BOT_BLOCKED).
const USER_AGENT: &str = "crypto-tui/0.1";

/// Bootstrap response from the REST /api/news endpoint.
#[derive(Debug, Deserialize)]
struct BootstrapResponse {
    articles: Vec<SseArticle>,
}

/// Fetch initial headlines from the REST API for news feed bootstrap.
///
/// Returns up to 50 articles from the /api/news endpoint.
/// On error, logs a warning and returns an empty Vec (non-fatal).
pub async fn fetch_news_bootstrap(http_client: &reqwest::Client) -> Vec<NewsArticle> {
    let resp = http_client
        .get(NEWS_API_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await;

    match resp {
        Ok(r) => match r.json::<BootstrapResponse>().await {
            Ok(body) => {
                let articles: Vec<NewsArticle> = body
                    .articles
                    .into_iter()
                    .map(|a| {
                        let timestamp =
                            chrono::DateTime::parse_from_rfc3339(&a.pub_date)
                                .map(|dt| dt.timestamp_millis())
                                .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

                        NewsArticle {
                            title: a.title,
                            source: a.source,
                            category: a.category.unwrap_or_default(),
                            link: a.link,
                            timestamp,
                            is_breaking: false,
                        }
                    })
                    .collect();

                tracing::info!(count = articles.len(), "News bootstrap complete");
                articles
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse news bootstrap response");
                Vec::new()
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch news bootstrap");
            Vec::new()
        }
    }
}

/// Run the SSE news stream with automatic reconnection.
///
/// Connects to cryptocurrency.cv/api/sse, parses SSE events for "news"
/// and "breaking" event types, and sends parsed articles through the
/// mpsc channel. Uses ExponentialBackoff for reconnection with a 60s
/// cooldown after backoff exhaustion.
///
/// If `category` is Some, appends `?categories={category}` to the SSE URL.
///
/// Designed to be spawned with `tokio::spawn`.
pub async fn run_news_stream(
    tx: mpsc::Sender<NewsArticle>,
    http_client: reqwest::Client,
    category: Option<String>,
) {
    let sse_url = match &category {
        Some(cat) => format!("{}?categories={}", SSE_BASE_URL, cat),
        None => SSE_BASE_URL.to_string(),
    };

    loop {
        let mut backoff = ExponentialBackoff::new();

        loop {
            tracing::info!(url = %sse_url, "Connecting to news SSE stream");

            let resp = http_client
                .get(&sse_url)
                .header("User-Agent", USER_AGENT)
                .header("Accept", "text/event-stream")
                .send()
                .await;

            match resp {
                Ok(response) => {
                    if !response.status().is_success() {
                        tracing::warn!(
                            status = %response.status(),
                            "News SSE stream returned non-success status"
                        );
                        if let Some(delay) = backoff.next_delay() {
                            tokio::time::sleep(delay).await;
                        } else {
                            break;
                        }
                        continue;
                    }

                    backoff.reset();
                    tracing::info!("News SSE stream connected");

                    let mut stream = response.bytes_stream().eventsource();

                    // Inner event loop
                    loop {
                        let event_result = tokio::time::timeout(
                            Duration::from_secs(60),
                            stream.next(),
                        )
                        .await;

                        match event_result {
                            Err(_) => {
                                // Timeout
                                tracing::warn!("SSE read timeout (60s), reconnecting");
                                break;
                            }
                            Ok(Some(Ok(event))) => {
                                match event.event.as_str() {
                                    "news" => {
                                        let articles =
                                            parse_sse_articles(&event.data, false);
                                        for article in articles {
                                            if tx.send(article).await.is_err() {
                                                tracing::info!(
                                                    "News receiver dropped, exiting"
                                                );
                                                return;
                                            }
                                        }
                                    }
                                    "breaking" => {
                                        let articles =
                                            parse_sse_articles(&event.data, true);
                                        for article in articles {
                                            if tx.send(article).await.is_err() {
                                                tracing::info!(
                                                    "News receiver dropped, exiting"
                                                );
                                                return;
                                            }
                                        }
                                    }
                                    "heartbeat" | "connected" => {
                                        tracing::trace!(
                                            event = event.event.as_str(),
                                            "SSE control event"
                                        );
                                    }
                                    _ => {
                                        // Ignore unknown event types
                                    }
                                }
                            }
                            Ok(Some(Err(e))) => {
                                tracing::warn!(error = %e, "SSE parse error");
                                break;
                            }
                            Ok(None) => {
                                tracing::info!("SSE stream ended");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "News SSE stream connection failed");
                }
            }

            // Backoff before reconnecting
            if let Some(delay) = backoff.next_delay() {
                tokio::time::sleep(delay).await;
            } else {
                break;
            }
        }

        // Backoff exhausted -- cooldown then restart
        tracing::warn!("News SSE stream backoff exhausted, restarting after 60s");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
