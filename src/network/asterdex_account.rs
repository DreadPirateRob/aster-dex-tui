// src/network/asterdex_account.rs
// AsterDEX authenticated REST fetch for account data (GET /fapi/v2/account)

use crate::config::AsterDexCredentials;
use crate::network::asterdex_auth::{sign_request, ASTERDEX_REST_URL, DEFAULT_RECV_WINDOW};
use crate::network::asterdex_trading::TradingError;
use crate::network::reconnect::ExponentialBackoff;
use rust_decimal::Decimal;
use serde::Deserialize;

/// Maximum retry attempts for transient failures
const MAX_RETRIES: u32 = 3;

/// AsterDEX account response from GET /fapi/v2/account
///
/// Field names use camelCase per the Binance-compatible API.
/// Decimal fields are returned as JSON strings.
///
/// NOTE: `total_margin_balance` and `total_maint_margin` have `#[serde(default)]`
/// as a safety net in case AsterDEX omits them (Research Open Question 1).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    #[serde(with = "rust_decimal::serde::str")]
    pub total_wallet_balance: Decimal,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub total_margin_balance: Option<Decimal>,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub total_maint_margin: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str")]
    pub available_balance: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub total_unrealized_profit: Decimal,
}

/// Compute margin ratio as a percentage: (maint_margin / margin_balance) * 100.
///
/// Returns `Decimal::ZERO` when `margin_balance` is zero to avoid division by zero
/// (Research Pitfall 5).
pub fn compute_margin_ratio(maint_margin: Decimal, margin_balance: Decimal) -> Decimal {
    if margin_balance.is_zero() {
        return Decimal::ZERO;
    }
    (maint_margin / margin_balance) * Decimal::from(100)
}

/// Fetch account info from AsterDEX REST API with retry and authentication.
///
/// Sends an authenticated GET to /fapi/v2/account with HMAC-SHA256 signed query.
/// Retries transient failures (timeouts, 429, 5xx) with exponential backoff.
///
/// CRITICAL: The signed query is regenerated on each retry attempt because the
/// timestamp expires after the recvWindow (5 seconds), and backoff delays can
/// exceed this window.
///
/// # Arguments
/// * `credentials` - AsterDEX API key and secret key
///
/// # Returns
/// * `Ok(AccountInfo)` - Account data from the API
/// * `Err(TradingError)` - If request fails after retries
pub async fn fetch_account_info(
    credentials: &AsterDexCredentials,
    client: &reqwest::Client,
) -> Result<AccountInfo, TradingError> {
    let mut backoff = ExponentialBackoff::new();
    let mut attempts = 0;

    loop {
        attempts += 1;

        // No params -- returns full account info
        let params = "";
        let (signed_query, _timestamp) =
            sign_request(params, &credentials.secret_key, DEFAULT_RECV_WINDOW);
        let url = format!("{}/fapi/v2/account?{}", ASTERDEX_REST_URL, signed_query);

        match client
            .get(&url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let account: AccountInfo = response
                        .json()
                        .await
                        .map_err(|e| TradingError::Parse(e.to_string()))?;
                    return Ok(account);
                } else {
                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let error = TradingError::from_api_response(status_code, &body);

                    if error.is_retriable() && attempts < MAX_RETRIES {
                        if let Some(delay) = backoff.next_delay() {
                            tracing::warn!(
                                "Account fetch failed (attempt {}/{}): {}. Retrying in {:?}",
                                attempts,
                                MAX_RETRIES,
                                error,
                                delay
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    return Err(error);
                }
            }
            Err(e) => {
                let error = if e.is_timeout() {
                    TradingError::Timeout
                } else {
                    TradingError::Http(e)
                };

                if error.is_retriable() && attempts < MAX_RETRIES {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::warn!(
                            "Account fetch failed (attempt {}/{}): {}. Retrying in {:?}",
                            attempts,
                            MAX_RETRIES,
                            error,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_compute_margin_ratio_normal() {
        // 50 / 1000 * 100 = 5%
        let result = compute_margin_ratio(dec!(50), dec!(1000));
        assert_eq!(result, dec!(5));
    }

    #[test]
    fn test_compute_margin_ratio_zero_balance() {
        // Division by zero guard
        let result = compute_margin_ratio(dec!(0), dec!(0));
        assert_eq!(result, Decimal::ZERO);
    }

    #[test]
    fn test_compute_margin_ratio_high() {
        // 800 / 1000 * 100 = 80%
        let result = compute_margin_ratio(dec!(800), dec!(1000));
        assert_eq!(result, dec!(80));
    }

    #[test]
    fn test_deserialize_account_info() {
        let json = r#"{
            "totalWalletBalance": "10000.50",
            "totalMarginBalance": "9500.25",
            "totalMaintMargin": "475.01",
            "availableBalance": "5000.00",
            "totalUnrealizedProfit": "250.75"
        }"#;

        let info: AccountInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.total_wallet_balance, dec!(10000.50));
        assert_eq!(info.total_margin_balance, Some(dec!(9500.25)));
        assert_eq!(info.total_maint_margin, Some(dec!(475.01)));
        assert_eq!(info.available_balance, dec!(5000.00));
        assert_eq!(info.total_unrealized_profit, dec!(250.75));
    }

    #[test]
    fn test_deserialize_account_info_missing_optional_fields() {
        // Verify #[serde(default)] works when fields are omitted
        let json = r#"{
            "totalWalletBalance": "10000.50",
            "availableBalance": "5000.00",
            "totalUnrealizedProfit": "250.75"
        }"#;

        let info: AccountInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.total_wallet_balance, dec!(10000.50));
        assert_eq!(info.total_margin_balance, None);
        assert_eq!(info.total_maint_margin, None);
        assert_eq!(info.available_balance, dec!(5000.00));
    }
}
