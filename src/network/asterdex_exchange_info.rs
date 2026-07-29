// src/network/asterdex_exchange_info.rs
// AsterDEX exchange info types and symbol validation
// Fetches public /fapi/v1/exchangeInfo endpoint (no auth required)

use crate::network::asterdex_auth::ASTERDEX_REST_URL;
use crate::network::asterdex_trading::TradingError;
use rust_decimal::Decimal;
use serde::Deserialize;

/// Exchange information response from GET /fapi/v1/exchangeInfo
#[derive(Debug, Deserialize)]
pub struct ExchangeInfo {
    pub symbols: Vec<SymbolInfo>,
}

/// Information about a single trading symbol
#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInfo {
    pub symbol: String,
    pub status: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub filters: Vec<SymbolFilter>,
}

/// Symbol filter types from exchange info
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "filterType")]
pub enum SymbolFilter {
    #[serde(rename = "PRICE_FILTER")]
    PriceFilter {
        #[serde(rename = "minPrice", with = "rust_decimal::serde::str")]
        min_price: Decimal,
        #[serde(rename = "maxPrice", with = "rust_decimal::serde::str")]
        max_price: Decimal,
        #[serde(rename = "tickSize", with = "rust_decimal::serde::str")]
        tick_size: Decimal,
    },
    #[serde(rename = "LOT_SIZE")]
    LotSize {
        #[serde(rename = "minQty", with = "rust_decimal::serde::str")]
        min_qty: Decimal,
        #[serde(rename = "maxQty", with = "rust_decimal::serde::str")]
        max_qty: Decimal,
        #[serde(rename = "stepSize", with = "rust_decimal::serde::str")]
        step_size: Decimal,
    },
    #[serde(rename = "MIN_NOTIONAL")]
    MinNotional {
        #[serde(with = "rust_decimal::serde::str")]
        notional: Decimal,
    },
    #[serde(other)]
    Other,
}

/// Fetch exchange information from AsterDEX (public endpoint, no auth required).
pub async fn fetch_exchange_info(client: &reqwest::Client) -> Result<ExchangeInfo, TradingError> {
    let url = format!("{}/fapi/v1/exchangeInfo", ASTERDEX_REST_URL);

    let response = client.get(&url).send().await?;
    let status = response.status();

    if status.is_success() {
        response
            .json::<ExchangeInfo>()
            .await
            .map_err(|e| TradingError::Parse(e.to_string()))
    } else {
        let status_code = status.as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(TradingError::from_api_response(status_code, &body))
    }
}

/// Validate that a symbol exists and is actively trading on AsterDEX.
///
/// Fetches exchange info and searches for the symbol (case-insensitive).
/// Returns the SymbolInfo if found with TRADING status, or InvalidSymbol error.
pub async fn validate_symbol(symbol: &str, client: &reqwest::Client) -> Result<SymbolInfo, TradingError> {
    let info = fetch_exchange_info(client).await?;
    info.symbols
        .into_iter()
        .find(|s| s.symbol.eq_ignore_ascii_case(symbol) && s.status == "TRADING")
        .ok_or_else(|| TradingError::InvalidSymbol(symbol.to_string()))
}

/// Validate order parameters against exchange symbol filters.
///
/// Checks LOT_SIZE (quantity range and step), PRICE_FILTER (price range and tick),
/// and MIN_NOTIONAL (minimum order value). Returns Ok(()) if all filters pass,
/// or Err with a human-readable message for the first violation found.
///
/// - `quantity`: order quantity (always required)
/// - `price`: limit/stop-limit price (None for market/stop-market orders -- skips PRICE_FILTER)
/// - `notional`: estimated order notional value (qty * price or qty * mark_price)
pub fn validate_filters(
    filters: &[SymbolFilter],
    quantity: Decimal,
    price: Option<Decimal>,
    notional: Option<Decimal>,
) -> Result<(), String> {
    for filter in filters {
        match filter {
            SymbolFilter::LotSize { min_qty, max_qty, step_size } => {
                if quantity < *min_qty {
                    return Err(format!("Qty below minimum {}", min_qty));
                }
                if quantity > *max_qty {
                    return Err(format!("Qty above maximum {}", max_qty));
                }
                if *step_size > Decimal::ZERO && !(quantity % *step_size).is_zero() {
                    return Err(format!("Qty must be multiple of {}", step_size));
                }
            }
            SymbolFilter::PriceFilter { min_price, max_price, tick_size } => {
                if let Some(p) = price {
                    if *min_price > Decimal::ZERO && p < *min_price {
                        return Err(format!("Price below minimum {}", min_price));
                    }
                    if *max_price > Decimal::ZERO && p > *max_price {
                        return Err(format!("Price above maximum {}", max_price));
                    }
                    if *tick_size > Decimal::ZERO && !(p % *tick_size).is_zero() {
                        return Err(format!("Price must be multiple of {}", tick_size));
                    }
                }
                // Skip PRICE_FILTER entirely when price is None (Market/StopMarket orders)
            }
            SymbolFilter::MinNotional { notional: min_notional } => {
                if let Some(n) = notional {
                    if n < *min_notional {
                        return Err(format!("Notional below minimum {} USDT", min_notional));
                    }
                }
            }
            _ => {} // Ignore unknown filter types (Other variant)
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    /// Sample exchange info JSON with one symbol containing known filters + unknown filter
    const SAMPLE_EXCHANGE_INFO: &str = r#"{
        "symbols": [
            {
                "symbol": "BTCUSDT",
                "status": "TRADING",
                "baseAsset": "BTC",
                "quoteAsset": "USDT",
                "pricePrecision": 2,
                "quantityPrecision": 3,
                "filters": [
                    {
                        "filterType": "PRICE_FILTER",
                        "minPrice": "0.01",
                        "maxPrice": "1000000.00",
                        "tickSize": "0.01"
                    },
                    {
                        "filterType": "LOT_SIZE",
                        "minQty": "0.001",
                        "maxQty": "1000.000",
                        "stepSize": "0.001"
                    },
                    {
                        "filterType": "MIN_NOTIONAL",
                        "notional": "5.00"
                    },
                    {
                        "filterType": "PERCENT_PRICE",
                        "multiplierUp": "1.1500",
                        "multiplierDown": "0.8500",
                        "multiplierDecimal": "4"
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn test_exchange_info_deserialization() {
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        assert_eq!(info.symbols.len(), 1);
        assert_eq!(info.symbols[0].symbol, "BTCUSDT");
        assert_eq!(info.symbols[0].status, "TRADING");
    }

    #[test]
    fn test_symbol_info_fields() {
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        let sym = &info.symbols[0];
        assert_eq!(sym.base_asset, "BTC");
        assert_eq!(sym.quote_asset, "USDT");
        assert_eq!(sym.price_precision, 2);
        assert_eq!(sym.quantity_precision, 3);
        assert_eq!(sym.filters.len(), 4);
    }

    #[test]
    fn test_price_filter_deserialization() {
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        let filter = &info.symbols[0].filters[0];
        match filter {
            SymbolFilter::PriceFilter {
                min_price,
                max_price,
                tick_size,
            } => {
                assert_eq!(*min_price, Decimal::from_str("0.01").unwrap());
                assert_eq!(*max_price, Decimal::from_str("1000000.00").unwrap());
                assert_eq!(*tick_size, Decimal::from_str("0.01").unwrap());
            }
            _ => panic!("Expected PriceFilter variant"),
        }
    }

    #[test]
    fn test_lot_size_deserialization() {
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        let filter = &info.symbols[0].filters[1];
        match filter {
            SymbolFilter::LotSize {
                min_qty,
                max_qty,
                step_size,
            } => {
                assert_eq!(*min_qty, Decimal::from_str("0.001").unwrap());
                assert_eq!(*max_qty, Decimal::from_str("1000.000").unwrap());
                assert_eq!(*step_size, Decimal::from_str("0.001").unwrap());
            }
            _ => panic!("Expected LotSize variant"),
        }
    }

    #[test]
    fn test_min_notional_deserialization() {
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        let filter = &info.symbols[0].filters[2];
        match filter {
            SymbolFilter::MinNotional { notional } => {
                assert_eq!(*notional, Decimal::from_str("5.00").unwrap());
            }
            _ => panic!("Expected MinNotional variant"),
        }
    }

    #[test]
    fn test_unknown_filter_type_deserializes_as_other() {
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        // PERCENT_PRICE is the 4th filter, unknown to our enum
        let filter = &info.symbols[0].filters[3];
        assert!(
            matches!(filter, SymbolFilter::Other),
            "Unknown filter type should deserialize as Other, got: {:?}",
            filter
        );
    }

    #[test]
    fn test_multiple_unknown_filters_handled() {
        let json = r#"{
            "symbols": [
                {
                    "symbol": "ETHUSDT",
                    "status": "TRADING",
                    "baseAsset": "ETH",
                    "quoteAsset": "USDT",
                    "pricePrecision": 2,
                    "quantityPrecision": 3,
                    "filters": [
                        {"filterType": "MARKET_LOT_SIZE", "minQty": "0", "maxQty": "100", "stepSize": "0.001"},
                        {"filterType": "MAX_NUM_ORDERS", "limit": 200},
                        {"filterType": "PRICE_FILTER", "minPrice": "0.01", "maxPrice": "500000.00", "tickSize": "0.01"}
                    ]
                }
            ]
        }"#;

        let info: ExchangeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.symbols.len(), 1);
        // MARKET_LOT_SIZE and MAX_NUM_ORDERS should be Other
        assert!(matches!(info.symbols[0].filters[0], SymbolFilter::Other));
        assert!(matches!(info.symbols[0].filters[1], SymbolFilter::Other));
        // PRICE_FILTER should parse correctly
        assert!(matches!(
            info.symbols[0].filters[2],
            SymbolFilter::PriceFilter { .. }
        ));
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires network access to AsterDEX)
    async fn test_fetch_exchange_info_real_api() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let info = fetch_exchange_info(&client)
            .await
            .expect("Should fetch exchange info");

        assert!(
            !info.symbols.is_empty(),
            "Exchange info should have at least one symbol"
        );
        println!("Fetched {} symbols. First 5:", info.symbols.len());
        for sym in info.symbols.iter().take(5) {
            println!(
                "  {} ({}/{}) status={} price_prec={} qty_prec={}",
                sym.symbol,
                sym.base_asset,
                sym.quote_asset,
                sym.status,
                sym.price_precision,
                sym.quantity_precision,
            );
        }
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires network access to AsterDEX)
    async fn test_validate_symbol_real_api() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let sym_info = validate_symbol("BTCUSDT", &client)
            .await
            .expect("BTCUSDT should be a valid trading symbol");

        assert_eq!(sym_info.symbol, "BTCUSDT");
        assert_eq!(sym_info.status, "TRADING");
        println!(
            "BTCUSDT: price_precision={}, quantity_precision={}, filters={}",
            sym_info.price_precision,
            sym_info.quantity_precision,
            sym_info.filters.len(),
        );
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored (requires network access to AsterDEX)
    async fn test_validate_invalid_symbol_real_api() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let result = validate_symbol("NOSUCHSYMBOLXYZ", &client).await;
        assert!(
            result.is_err(),
            "Non-existent symbol should return error"
        );
        match result.unwrap_err() {
            TradingError::InvalidSymbol(sym) => {
                assert_eq!(sym, "NOSUCHSYMBOLXYZ");
            }
            other => panic!("Expected InvalidSymbol error, got: {:?}", other),
        }
    }

    // ---- validate_filters tests ----

    #[test]
    fn test_validate_filters_lot_size_below_min() {
        let filters = vec![SymbolFilter::LotSize {
            min_qty: Decimal::from_str("0.001").unwrap(),
            max_qty: Decimal::from_str("1000").unwrap(),
            step_size: Decimal::from_str("0.001").unwrap(),
        }];
        let result = validate_filters(&filters, Decimal::from_str("0.0001").unwrap(), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("below minimum"));
    }

    #[test]
    fn test_validate_filters_lot_size_above_max() {
        let filters = vec![SymbolFilter::LotSize {
            min_qty: Decimal::from_str("0.001").unwrap(),
            max_qty: Decimal::from_str("1000").unwrap(),
            step_size: Decimal::from_str("0.001").unwrap(),
        }];
        let result = validate_filters(&filters, Decimal::from_str("1001").unwrap(), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("above maximum"));
    }

    #[test]
    fn test_validate_filters_lot_size_step_violation() {
        let filters = vec![SymbolFilter::LotSize {
            min_qty: Decimal::from_str("0.001").unwrap(),
            max_qty: Decimal::from_str("1000").unwrap(),
            step_size: Decimal::from_str("0.001").unwrap(),
        }];
        // 0.0015 is not a multiple of 0.001
        let result = validate_filters(&filters, Decimal::from_str("0.0015").unwrap(), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("multiple of"));
    }

    #[test]
    fn test_validate_filters_lot_size_valid() {
        let filters = vec![SymbolFilter::LotSize {
            min_qty: Decimal::from_str("0.001").unwrap(),
            max_qty: Decimal::from_str("1000").unwrap(),
            step_size: Decimal::from_str("0.001").unwrap(),
        }];
        let result = validate_filters(&filters, Decimal::from_str("0.005").unwrap(), None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_filters_price_filter() {
        let filters = vec![SymbolFilter::PriceFilter {
            min_price: Decimal::from_str("0.01").unwrap(),
            max_price: Decimal::from_str("1000000").unwrap(),
            tick_size: Decimal::from_str("0.01").unwrap(),
        }];
        // Valid price
        let result = validate_filters(&filters, Decimal::ONE, Some(Decimal::from_str("50000.01").unwrap()), None);
        assert!(result.is_ok());
        // Price below minimum
        let result = validate_filters(&filters, Decimal::ONE, Some(Decimal::from_str("0.001").unwrap()), None);
        assert!(result.is_err());
        // Tick size violation
        let result = validate_filters(&filters, Decimal::ONE, Some(Decimal::from_str("50000.015").unwrap()), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("multiple of"));
    }

    #[test]
    fn test_validate_filters_price_filter_skipped_when_no_price() {
        let filters = vec![SymbolFilter::PriceFilter {
            min_price: Decimal::from_str("0.01").unwrap(),
            max_price: Decimal::from_str("1000000").unwrap(),
            tick_size: Decimal::from_str("0.01").unwrap(),
        }];
        // No price (Market order) -- should pass
        let result = validate_filters(&filters, Decimal::ONE, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_filters_min_notional() {
        let filters = vec![SymbolFilter::MinNotional {
            notional: Decimal::from_str("5.00").unwrap(),
        }];
        // Below minimum
        let result = validate_filters(&filters, Decimal::ONE, None, Some(Decimal::from_str("4.99").unwrap()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Notional below minimum"));
        // At minimum
        let result = validate_filters(&filters, Decimal::ONE, None, Some(Decimal::from_str("5.00").unwrap()));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_filters_all_pass() {
        // Realistic filter set from SAMPLE_EXCHANGE_INFO
        let info: ExchangeInfo = serde_json::from_str(SAMPLE_EXCHANGE_INFO).unwrap();
        let filters = &info.symbols[0].filters;
        // qty=0.001, price=50000.00, notional=50.00 -- all valid
        let result = validate_filters(
            filters,
            Decimal::from_str("0.001").unwrap(),
            Some(Decimal::from_str("50000.00").unwrap()),
            Some(Decimal::from_str("50.00").unwrap()),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_filters_unknown_filter_ignored() {
        let filters = vec![SymbolFilter::Other];
        let result = validate_filters(&filters, Decimal::ONE, Some(Decimal::ONE), Some(Decimal::ONE));
        assert!(result.is_ok());
    }
}
