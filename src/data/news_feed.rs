// src/data/news_feed.rs
// Data types for the live crypto news feed widget.
//
// Articles come from cryptocurrency.cv's REST and SSE APIs.
// Each article has a title, source, category, link, and timestamp.

use chrono::DateTime;
use serde::Deserialize;
use std::collections::{HashSet, VecDeque};

/// Maximum number of articles kept in the feed.
const MAX_ARTICLES: usize = 500;

/// Maximum number of seen links for deduplication.
const MAX_SEEN_LINKS: usize = 1000;

/// A single news article with pre-parsed fields.
#[derive(Debug, Clone)]
pub struct NewsArticle {
    pub title: String,
    pub source: String,
    #[allow(dead_code)]
    pub category: String,
    pub link: String,
    /// Milliseconds since epoch
    pub timestamp: i64,
    pub is_breaking: bool,
}

/// SSE event payload from the cryptocurrency.cv SSE stream.
#[derive(Debug, Deserialize)]
pub(crate) struct SseNewsPayload {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub event_type: String,
    pub articles: Vec<SseArticle>,
    #[allow(dead_code)]
    pub timestamp: String,
}

/// A single article within an SSE event payload.
#[derive(Debug, Deserialize)]
pub(crate) struct SseArticle {
    pub title: String,
    pub link: String,
    #[allow(dead_code)]
    pub description: Option<String>,
    #[serde(rename = "pubDate")]
    pub pub_date: String,
    pub source: String,
    pub category: Option<String>,
}

/// Parse SSE event data JSON into a Vec of NewsArticle.
///
/// Deserializes the JSON payload, converts each SseArticle to NewsArticle
/// with RFC 3339 timestamp parsing (falls back to current time on parse failure).
pub fn parse_sse_articles(data: &str, is_breaking: bool) -> Vec<NewsArticle> {
    let payload: SseNewsPayload = match serde_json::from_str(data) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse SSE news payload");
            return Vec::new();
        }
    };

    payload
        .articles
        .into_iter()
        .map(|a| {
            let timestamp = DateTime::parse_from_rfc3339(&a.pub_date)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

            NewsArticle {
                title: a.title,
                source: a.source,
                category: a.category.unwrap_or_default(),
                link: a.link,
                timestamp,
                is_breaking,
            }
        })
        .collect()
}

/// Accumulator for news articles with deduplication and bounded storage.
///
/// Articles are stored newest-first in a VecDeque bounded at 500.
/// Deduplication uses a HashSet of link URLs bounded at 1000.
pub struct NewsFeed {
    pub articles: VecDeque<NewsArticle>,
    seen_links: HashSet<String>,
    pub total_count: u64,
    pub breaking_count: u64,
}

impl NewsFeed {
    /// Create a new empty news feed.
    pub fn new() -> Self {
        Self {
            articles: VecDeque::new(),
            seen_links: HashSet::new(),
            total_count: 0,
            breaking_count: 0,
        }
    }

    /// Push a new article into the feed.
    ///
    /// Returns false if the article's link was already seen (duplicate).
    /// Inserts at the front of the deque (newest first).
    /// Enforces MAX_ARTICLES bound by popping from back.
    /// Rebuilds seen_links from current articles if it exceeds MAX_SEEN_LINKS.
    pub fn push_article(&mut self, article: NewsArticle) -> bool {
        if !self.seen_links.insert(article.link.clone()) {
            return false;
        }

        self.total_count += 1;
        if article.is_breaking {
            self.breaking_count += 1;
        }

        self.articles.push_front(article);

        // Enforce article bound
        if self.articles.len() > MAX_ARTICLES {
            self.articles.pop_back();
        }

        // Rebuild seen_links if it grows too large
        if self.seen_links.len() > MAX_SEEN_LINKS {
            self.seen_links.clear();
            for a in &self.articles {
                self.seen_links.insert(a.link.clone());
            }
        }

        true
    }

    /// Push multiple articles, processing in reverse order so newest ends up at front.
    pub fn push_articles(&mut self, articles: Vec<NewsArticle>) {
        for article in articles.into_iter().rev() {
            self.push_article(article);
        }
    }

    /// Getter for articles deque.
    pub fn articles(&self) -> &VecDeque<NewsArticle> {
        &self.articles
    }

    /// Check if a news article matches a symbol filter.
    ///
    /// Maps common tickers to aliases (e.g., BTC -> ["BTC", "Bitcoin"]).
    /// For short tickers (<=3 chars), uses word-boundary matching to avoid
    /// false positives (e.g., "SOL" inside "SOLUTION").
    /// For longer names, uses case-insensitive contains.
    pub fn matches_symbol_filter(article: &NewsArticle, filter: &str) -> bool {
        let filter_upper = filter.to_uppercase();

        // Map tickers to aliases
        let aliases: Vec<&str> = match filter_upper.as_str() {
            "BTC" | "BTCUSDT" => vec!["BTC", "Bitcoin"],
            "ETH" | "ETHUSDT" => vec!["ETH", "Ethereum"],
            "SOL" | "SOLUSDT" => vec!["SOL", "Solana"],
            "XRP" | "XRPUSDT" => vec!["XRP", "Ripple"],
            "DOGE" | "DOGEUSDT" => vec!["DOGE", "Dogecoin"],
            "ADA" | "ADAUSDT" => vec!["ADA", "Cardano"],
            "AVAX" | "AVAXUSDT" => vec!["AVAX", "Avalanche"],
            "DOT" | "DOTUSDT" => vec!["DOT", "Polkadot"],
            "LINK" | "LINKUSDT" => vec!["LINK", "Chainlink"],
            "MATIC" | "MATICUSDT" => vec!["MATIC", "Polygon"],
            other => {
                // Strip USDT suffix if present
                let base = other.strip_suffix("USDT").unwrap_or(other);
                // Return just the base ticker — we'll handle it below
                return Self::title_contains_word(&article.title, base);
            }
        };

        for alias in aliases {
            if alias.len() <= 3 {
                // Short tickers: word-boundary matching (case-sensitive)
                if Self::title_contains_word_case_sensitive(&article.title, alias) {
                    return true;
                }
            } else {
                // Longer names: case-insensitive contains
                if article.title.to_lowercase().contains(&alias.to_lowercase()) {
                    return true;
                }
            }
        }

        false
    }

    /// Check if title contains word with case-sensitive word-boundary matching.
    ///
    /// The word must be surrounded by non-alphanumeric characters (or at
    /// start/end of string) to avoid matching "SOL" inside "SOLUTION".
    fn title_contains_word_case_sensitive(title: &str, word: &str) -> bool {
        let title_bytes = title.as_bytes();
        let word_bytes = word.as_bytes();

        if word_bytes.len() > title_bytes.len() {
            return false;
        }

        for i in 0..=(title_bytes.len() - word_bytes.len()) {
            if &title_bytes[i..i + word_bytes.len()] == word_bytes {
                // Check left boundary
                let left_ok =
                    i == 0 || !title_bytes[i - 1].is_ascii_alphanumeric();
                // Check right boundary
                let right_ok = i + word_bytes.len() == title_bytes.len()
                    || !title_bytes[i + word_bytes.len()].is_ascii_alphanumeric();

                if left_ok && right_ok {
                    return true;
                }
            }
        }

        false
    }

    /// Check if title contains word with word-boundary matching (case-insensitive for unknown tickers).
    fn title_contains_word(title: &str, word: &str) -> bool {
        if word.len() <= 3 {
            // Short: case-sensitive word-boundary
            Self::title_contains_word_case_sensitive(title, word)
        } else {
            // Longer: case-insensitive contains
            title.to_lowercase().contains(&word.to_lowercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_article(title: &str, link: &str) -> NewsArticle {
        NewsArticle {
            title: title.to_string(),
            source: "TestSource".to_string(),
            category: "general".to_string(),
            link: link.to_string(),
            timestamp: 1700000000000,
            is_breaking: false,
        }
    }

    #[test]
    fn test_push_article_dedup() {
        let mut feed = NewsFeed::new();
        let a1 = make_article("Test Article", "https://example.com/1");
        let a2 = make_article("Test Article Duplicate", "https://example.com/1");

        assert!(feed.push_article(a1));
        assert!(!feed.push_article(a2)); // duplicate link
        assert_eq!(feed.articles.len(), 1);
        assert_eq!(feed.total_count, 1);
    }

    #[test]
    fn test_push_articles_order() {
        let mut feed = NewsFeed::new();
        let articles = vec![
            make_article("First", "https://example.com/1"),
            make_article("Second", "https://example.com/2"),
            make_article("Third", "https://example.com/3"),
        ];
        feed.push_articles(articles);

        // Newest (first in input) should be at front
        assert_eq!(feed.articles[0].title, "First");
        assert_eq!(feed.articles[1].title, "Second");
        assert_eq!(feed.articles[2].title, "Third");
    }

    #[test]
    fn test_symbol_filter_btc() {
        let article = make_article("Bitcoin surges past $100k", "https://example.com/1");
        assert!(NewsFeed::matches_symbol_filter(&article, "BTC"));
        assert!(NewsFeed::matches_symbol_filter(&article, "BTCUSDT"));
    }

    #[test]
    fn test_symbol_filter_sol_word_boundary() {
        let match_article = make_article("SOL hits new high", "https://example.com/1");
        let no_match = make_article("SOLUTION to scaling", "https://example.com/2");

        assert!(NewsFeed::matches_symbol_filter(&match_article, "SOL"));
        assert!(!NewsFeed::matches_symbol_filter(&no_match, "SOL"));
    }

    #[test]
    fn test_symbol_filter_ethereum() {
        let article = make_article("Ethereum upgrade complete", "https://example.com/1");
        assert!(NewsFeed::matches_symbol_filter(&article, "ETH"));
    }

    #[test]
    fn test_symbol_filter_no_match() {
        let article = make_article("Stocks rally on earnings", "https://example.com/1");
        assert!(!NewsFeed::matches_symbol_filter(&article, "BTC"));
    }
}
