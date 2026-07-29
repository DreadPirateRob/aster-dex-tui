// src/network/asterdex_auth.rs
// AsterDEX HMAC-SHA256 authentication for Binance-compatible REST API
// Source: AsterDEX/Binance FAPI authentication docs

use ring::hmac;
use std::time::{SystemTime, UNIX_EPOCH};

/// AsterDEX Futures REST API base URL
pub const ASTERDEX_REST_URL: &str = "https://fapi.asterdex.com";

/// Default receive window in milliseconds for signed requests
pub const DEFAULT_RECV_WINDOW: u64 = 5000;

/// Compute HMAC-SHA256 signature over a query string using the given secret.
///
/// The secret is used as raw ASCII bytes (not hex-decoded), per Binance convention.
///
/// Returns the hex-encoded signature string.
pub fn sign_query(query_string: &str, secret: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, query_string.as_bytes());
    tag.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Build a signed query string for AsterDEX authenticated requests.
///
/// Appends `recvWindow` and `timestamp` to the given parameters, signs the full
/// query string with HMAC-SHA256, and appends the signature.
///
/// # Arguments
/// * `params` - Existing query parameters (may be empty)
/// * `secret` - The HMAC secret key (ASCII)
/// * `recv_window` - Receive window in milliseconds
///
/// # Returns
/// A tuple of (signed_query_string, timestamp_u64)
pub fn sign_request(params: &str, secret: &str, recv_window: u64) -> (String, u64) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System clock before UNIX epoch")
        .as_millis() as u64;

    // Build full query: existing params + recvWindow + timestamp
    let query = if params.is_empty() {
        format!("recvWindow={}&timestamp={}", recv_window, timestamp)
    } else {
        format!(
            "{}&recvWindow={}&timestamp={}",
            params, recv_window, timestamp
        )
    };

    // Sign and append signature
    let signature = sign_query(&query, secret);
    let signed_query = format!("{}&signature={}", query, signature);

    (signed_query, timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_query_binance_test_vector() {
        // Known test vector from AsterDEX/Binance docs
        let secret = "2b5eb11e18796d12d88f13dc27dbbd02c2cc51ff7059765ed9821957d82bb4d9";
        let query = "symbol=BTCUSDT&side=BUY&type=LIMIT&quantity=1&price=9000&timeInForce=GTC&recvWindow=5000&timestamp=1591702613943";
        let expected = "3c661234138461fcc7a7d8746c6558c9842d4e10870d2ecbedf7777cad694af9";
        assert_eq!(sign_query(query, secret), expected);
    }

    #[test]
    fn test_sign_request_appends_recv_window_and_timestamp() {
        let secret = "testsecret";
        let (signed, ts) = sign_request("symbol=BTCUSDT", secret, 5000);

        // Must contain original param, recvWindow, timestamp, and signature
        assert!(signed.starts_with("symbol=BTCUSDT&recvWindow=5000&timestamp="));
        assert!(signed.contains("&signature="));
        assert!(ts > 0);
    }

    #[test]
    fn test_sign_request_empty_params() {
        let secret = "testsecret";
        let (signed, _ts) = sign_request("", secret, 5000);

        // When no params, recvWindow should come first
        assert!(signed.starts_with("recvWindow=5000&timestamp="));
        assert!(signed.contains("&signature="));
    }

    #[test]
    fn test_sign_query_empty_string() {
        // Signing an empty string should produce a valid 64-char hex signature
        let secret = "testsecret";
        let sig = sign_query("", secret);
        assert_eq!(sig.len(), 64, "HMAC-SHA256 hex signature must be 64 chars");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
