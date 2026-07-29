// src/config.rs
// CLI argument parsing and configuration validation

use crate::tui::ThemeName;
use clap::{Parser, Subcommand, ValueEnum};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::env;
use std::str::FromStr;

/// Candle building mode - determines how candles are aggregated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandleMode {
    /// Aggregate by trade count (N trades per candle)
    TickBased,
    /// Aggregate by time interval (1m, 5m, 15m candles)
    TimeBased,
}

impl std::fmt::Display for CandleMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CandleMode::TickBased => write!(f, "Tick-Based"),
            CandleMode::TimeBased => write!(f, "Time-Based"),
        }
    }
}

/// Time-based candle resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Resolution {
    /// 1-minute candles
    #[value(name = "1m")]
    OneMinute,
    /// 3-minute candles
    #[value(name = "3m")]
    ThreeMinutes,
    /// 5-minute candles
    #[value(name = "5m")]
    FiveMinutes,
    /// 15-minute candles
    #[value(name = "15m")]
    FifteenMinutes,
    /// 30-minute candles
    #[value(name = "30m")]
    ThirtyMinutes,
    /// 1-hour candles
    #[value(name = "1h")]
    OneHour,
    /// 2-hour candles
    #[value(name = "2h")]
    TwoHours,
    /// 4-hour candles
    #[value(name = "4h")]
    FourHours,
    /// 6-hour candles
    #[value(name = "6h")]
    SixHours,
    /// 8-hour candles
    #[value(name = "8h")]
    EightHours,
    /// 12-hour candles
    #[value(name = "12h")]
    TwelveHours,
    /// 1-day candles
    #[value(name = "1d")]
    OneDay,
    /// 3-day candles
    #[value(name = "3d")]
    ThreeDays,
    /// 1-week candles
    #[value(name = "1w")]
    OneWeek,
    /// 1-month candles
    #[value(name = "1M")]
    OneMonth,
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_interval_str())
    }
}

/// Curated tick size cycle for +/- hotkey (trader-friendly values).
pub const TICK_SIZE_CYCLE: &[u32] = &[
    10, 25, 50, 100, 200, 500, 1000, 2000, 5000,
];

/// Curated resolution cycle for 'r' hotkey (8 of 15 variants, most commonly used).
pub const RESOLUTION_CYCLE: &[Resolution] = &[
    Resolution::OneMinute,
    Resolution::ThreeMinutes,
    Resolution::FiveMinutes,
    Resolution::FifteenMinutes,
    Resolution::ThirtyMinutes,
    Resolution::OneHour,
    Resolution::FourHours,
    Resolution::OneDay,
];

impl Resolution {
    /// Returns the API interval string for this resolution (e.g., "1m", "5m", "1h")
    pub fn as_interval_str(&self) -> &'static str {
        match self {
            Resolution::OneMinute => "1m",
            Resolution::ThreeMinutes => "3m",
            Resolution::FiveMinutes => "5m",
            Resolution::FifteenMinutes => "15m",
            Resolution::ThirtyMinutes => "30m",
            Resolution::OneHour => "1h",
            Resolution::TwoHours => "2h",
            Resolution::FourHours => "4h",
            Resolution::SixHours => "6h",
            Resolution::EightHours => "8h",
            Resolution::TwelveHours => "12h",
            Resolution::OneDay => "1d",
            Resolution::ThreeDays => "3d",
            Resolution::OneWeek => "1w",
            Resolution::OneMonth => "1M",
        }
    }

    /// Returns the interval in milliseconds
    pub fn interval_ms(&self) -> u64 {
        match self {
            Resolution::OneMinute => 60_000,
            Resolution::ThreeMinutes => 180_000,
            Resolution::FiveMinutes => 300_000,
            Resolution::FifteenMinutes => 900_000,
            Resolution::ThirtyMinutes => 1_800_000,
            Resolution::OneHour => 3_600_000,
            Resolution::TwoHours => 7_200_000,
            Resolution::FourHours => 14_400_000,
            Resolution::SixHours => 21_600_000,
            Resolution::EightHours => 28_800_000,
            Resolution::TwelveHours => 43_200_000,
            Resolution::OneDay => 86_400_000,
            Resolution::ThreeDays => 259_200_000,
            Resolution::OneWeek => 604_800_000,
            Resolution::OneMonth => 2_592_000_000, // 30 days
        }
    }
}

/// Normalizes a trading pair symbol to AsterDEX format.
///
/// Strips separators (-, /, _), uppercases, and converts USD -> USDT
/// for USDT-margined perpetuals.
pub fn normalize_symbol(symbol: &str) -> String {
    // Strip common separators and uppercase
    let cleaned: String = symbol
        .chars()
        .filter(|c| *c != '-' && *c != '/' && *c != '_')
        .collect::<String>()
        .to_uppercase();

    // Known quote currencies (longest first for greedy matching)
    let quotes = ["USDT", "USD", "EUR", "BTC", "ETH"];

    // Find the quote currency
    let (base, quote) = quotes
        .iter()
        .find_map(|q| {
            if cleaned.ends_with(q) {
                let base = &cleaned[..cleaned.len() - q.len()];
                if !base.is_empty() {
                    return Some((base.to_string(), q.to_string()));
                }
            }
            None
        })
        .unwrap_or_else(|| (cleaned.clone(), String::new()));

    // AsterDEX uses concatenated uppercase format, USDT-margined perps
    if quote == "USD" {
        format!("{}USDT", base)
    } else if quote.is_empty() {
        cleaned
    } else {
        format!("{}{}", base, quote)
    }
}

#[derive(Parser, Debug)]
#[command(name = "crypto-tui")]
#[command(version, about = "Real-time crypto market data TUI")]
#[command(subcommand_negates_reqs = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Trading pair symbol (e.g., BTC/USD, BTCUSDT, BTC-USD)
    #[arg(required = true)]
    pub symbol: Option<String>,

    /// Number of trades per candlestick (1-10000, mutually exclusive with --res)
    #[arg(short, long)]
    #[arg(value_parser = clap::value_parser!(u32).range(1..=10000))]
    #[arg(conflicts_with = "resolution")]
    pub tick_size: Option<u32>,

    /// Time-based candle resolution (mutually exclusive with --tick-size)
    #[arg(long, value_enum)]
    pub resolution: Option<Resolution>,

    /// Threshold for highlighting large trades (in quote currency)
    #[arg(short = 'l', long, default_value = "10000")]
    #[arg(value_parser = parse_decimal)]
    pub large_trade_threshold: Decimal,

    /// Color theme (dark, high-contrast, light)
    #[arg(long, default_value = "dark")]
    #[arg(value_enum)]
    pub theme: ThemeName,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// View order history for a trading pair (or all pairs with --all)
    Orders {
        /// Trading pair symbol (e.g., BTCUSDT). Omit or use --all for all pairs.
        #[arg(conflicts_with = "all")]
        symbol: Option<String>,

        /// Watch orders for all trading pairs
        #[arg(long)]
        all: bool,
    },
    /// Place an order for a trading pair
    Trade {
        /// Trading pair symbol (e.g., BTCUSDT) -- REQUIRED
        symbol: String,

        /// Hide recent orders table for more screen space
        #[arg(long)]
        no_orders: bool,
    },
    /// View account overview with margin, PnL, and positions summary
    Account,
    /// View open positions (single symbol or all pairs)
    Positions {
        /// Trading pair symbol (e.g., BTCUSDT). Omit or use --all for all pairs.
        #[arg(conflicts_with = "all")]
        symbol: Option<String>,

        /// Show positions for all trading pairs
        #[arg(long)]
        all: bool,
    },
    /// View DOM (Depth of Market) price ladder for a trading pair
    Dom {
        /// Trading pair symbol (e.g., BTCUSDT)
        symbol: String,
    },
    /// View real-time liquidation feed across all markets
    Liquidations {
        /// Optional symbol filter (e.g., BTCUSDT)
        #[arg(long)]
        symbol: Option<String>,

        /// USD threshold for highlighting large liquidations (default: 100000)
        #[arg(long, default_value = "100000")]
        #[arg(value_parser = parse_decimal)]
        large_threshold: Decimal,
    },
    /// View live session analytics with PnL tracking, win rate, fees, and drawdown
    Analytics {
        /// Optional symbol filter (e.g., BTCUSDT) - show only trades for this symbol
        #[arg(long)]
        symbol: Option<String>,
    },
    /// View live crypto news headlines from 130+ sources
    News {
        /// Optional symbol/ticker filter (e.g., BTC, ETH, SOL)
        #[arg(long)]
        symbol: Option<String>,

        /// Filter by news category (general, bitcoin, defi, nft, research, etc.)
        #[arg(long)]
        category: Option<String>,
    },
    /// View real-time funding rates across all perpetual pairs
    Funding {
        /// Optional symbol filter (e.g., BTCUSDT)
        #[arg(long)]
        symbol: Option<String>,
    },
    /// View real-time market overview with all-pair ticker data
    Overview {
        /// Optional symbol filter (e.g., BTC to show only BTC pairs)
        #[arg(long)]
        symbol: Option<String>,
    },
    /// View upcoming macro economic events (requires FINNHUB_API_KEY)
    Calendar {
        /// Show only high-impact events
        #[arg(long)]
        high_only: bool,

        /// Country filter (e.g., US, GB, EU)
        #[arg(long)]
        country: Option<String>,
    },
}

/// Chart-specific arguments extracted from Cli (used by Config::from_args)
#[derive(Debug)]
pub struct ChartArgs {
    pub symbol: String,
    pub tick_size: Option<u32>,
    pub resolution: Option<Resolution>,
    pub large_trade_threshold: Decimal,
    pub theme: ThemeName,
}

/// AsterDEX API credentials for authenticated REST requests (HMAC-SHA256)
#[derive(Debug, Clone)]
pub struct AsterDexCredentials {
    pub api_key: String,
    pub secret_key: String,
}

impl AsterDexCredentials {
    /// Load credentials from ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY env vars.
    /// Returns None if either variable is missing.
    pub fn from_env() -> Option<Self> {
        let api_key = env::var("ASTERDEX_API_KEY").ok()?;
        let secret_key = env::var("ASTERDEX_SECRET_KEY").ok()?;
        Some(Self {
            api_key,
            secret_key,
        })
    }

    /// Validate that AsterDEX credentials are present.
    /// Returns a helpful error message mentioning both env var names when missing.
    pub fn validate() -> Result<Self, String> {
        Self::from_env().ok_or_else(|| {
            "AsterDEX order history requires API credentials.\n\
             Set ASTERDEX_API_KEY and ASTERDEX_SECRET_KEY environment variables."
                .to_string()
        })
    }
}

fn parse_decimal(s: &str) -> Result<Decimal, String> {
    Decimal::from_str(s).map_err(|e| format!("Invalid decimal '{}': {}", s, e))
}

/// Validated configuration
#[derive(Debug)]
pub struct Config {
    pub symbol: String,
    pub mode: CandleMode,
    pub tick_size: u32,
    pub large_trade_threshold: Decimal,
    pub theme: ThemeName,
    pub resolution: Option<Resolution>,
}

impl Config {
    pub fn from_args(args: ChartArgs) -> Result<Self, String> {
        // Check for empty symbol first
        if args.symbol.is_empty() {
            return Err("Symbol cannot be empty".to_string());
        }

        // Normalize symbol for AsterDEX format
        let symbol = normalize_symbol(&args.symbol);

        // Validate normalized symbol is alphanumeric
        let valid_chars = symbol.chars().all(|c| c.is_ascii_alphanumeric());
        if !valid_chars {
            return Err(format!(
                "Invalid symbol '{}': must be alphanumeric",
                symbol
            ));
        }

        // Validate large trade threshold
        if args.large_trade_threshold <= Decimal::ZERO {
            return Err("Large trade threshold must be positive".to_string());
        }

        // Mode detection - exactly one must be present (BREAKING CHANGE)
        let mode = match (&args.resolution, &args.tick_size) {
            (Some(_), None) => CandleMode::TimeBased,
            (None, Some(_)) => CandleMode::TickBased,
            (None, None) => {
                return Err(
                    "Either --resolution or --tick-size must be specified".to_string()
                )
            }
            (Some(_), Some(_)) => {
                // clap conflicts_with should prevent this
                unreachable!("--resolution and --tick-size are mutually exclusive")
            }
        };

        // Tick size: use provided value or internal default (only matters for TickBased)
        let tick_size = args.tick_size.unwrap_or(100);

        Ok(Config {
            symbol,
            mode,
            tick_size,
            large_trade_threshold: args.large_trade_threshold,
            theme: args.theme,
            resolution: args.resolution,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: Create args for tick-based mode
    fn make_args_tick_based(tick_size: u32) -> ChartArgs {
        ChartArgs {
            symbol: "BTCUSDT".to_string(),
            tick_size: Some(tick_size),
            resolution: None,
            large_trade_threshold: Decimal::from(10000),
            theme: ThemeName::Dark,
        }
    }

    // Helper: Create args for time-based mode
    fn make_args_time_based(resolution: Resolution) -> ChartArgs {
        ChartArgs {
            symbol: "BTCUSDT".to_string(),
            tick_size: None,
            resolution: Some(resolution),
            large_trade_threshold: Decimal::from(10000),
            theme: ThemeName::Dark,
        }
    }

    // Helper: Create args with no mode specified (invalid)
    fn make_args_no_mode() -> ChartArgs {
        ChartArgs {
            symbol: "BTCUSDT".to_string(),
            tick_size: None,
            resolution: None,
            large_trade_threshold: Decimal::from(10000),
            theme: ThemeName::Dark,
        }
    }

    #[test]
    fn test_resolution_value_enum_parsing() {
        // Test that clap ValueEnum works correctly for Resolution
        use clap::ValueEnum;

        // Test from_str equivalent via value_variants - all 15 intervals
        let variants = Resolution::value_variants();
        assert_eq!(variants.len(), 15);

        // Test to_possible_value for each variant (sample check)
        assert_eq!(
            Resolution::OneMinute.to_possible_value().unwrap().get_name(),
            "1m"
        );
        assert_eq!(
            Resolution::FiveMinutes.to_possible_value().unwrap().get_name(),
            "5m"
        );
        assert_eq!(
            Resolution::FifteenMinutes.to_possible_value().unwrap().get_name(),
            "15m"
        );
        assert_eq!(
            Resolution::OneHour.to_possible_value().unwrap().get_name(),
            "1h"
        );
        assert_eq!(
            Resolution::OneDay.to_possible_value().unwrap().get_name(),
            "1d"
        );
        assert_eq!(
            Resolution::OneMonth.to_possible_value().unwrap().get_name(),
            "1M"
        );
    }

    #[test]
    fn test_config_creation_with_resolution() {
        let args = make_args_time_based(Resolution::OneMinute);
        let config = Config::from_args(args).expect("Config creation should succeed");

        assert_eq!(config.resolution, Some(Resolution::OneMinute));
        assert_eq!(config.mode, CandleMode::TimeBased);
        assert_eq!(config.symbol, "BTCUSDT");
    }

    #[test]
    fn test_config_creation_with_tick_size() {
        let args = make_args_tick_based(100);
        let config = Config::from_args(args).expect("Config creation should succeed");

        assert_eq!(config.resolution, None);
        assert_eq!(config.mode, CandleMode::TickBased);
        assert_eq!(config.tick_size, 100);
    }

    #[test]
    fn test_mode_detection_tick_based() {
        let args = make_args_tick_based(50);
        let config = Config::from_args(args).expect("Config creation should succeed");

        assert_eq!(config.mode, CandleMode::TickBased);
        assert_eq!(config.tick_size, 50);
    }

    #[test]
    fn test_mode_detection_time_based() {
        let args = make_args_time_based(Resolution::FiveMinutes);
        let config = Config::from_args(args).expect("Config creation should succeed");

        assert_eq!(config.mode, CandleMode::TimeBased);
        assert_eq!(config.resolution, Some(Resolution::FiveMinutes));
    }

    #[test]
    fn test_mode_detection_neither_provided_returns_error() {
        let args = make_args_no_mode();
        let result = Config::from_args(args);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Either --resolution or --tick-size must be specified"
        );
    }

    #[test]
    fn test_candle_mode_display() {
        assert_eq!(format!("{}", CandleMode::TickBased), "Tick-Based");
        assert_eq!(format!("{}", CandleMode::TimeBased), "Time-Based");
    }

    #[test]
    fn test_resolution_variants_are_distinct() {
        assert_ne!(Resolution::OneMinute, Resolution::FiveMinutes);
        assert_ne!(Resolution::FiveMinutes, Resolution::FifteenMinutes);
        assert_ne!(Resolution::OneMinute, Resolution::FifteenMinutes);
    }

    #[test]
    fn test_resolution_as_interval_str() {
        assert_eq!(Resolution::OneMinute.as_interval_str(), "1m");
        assert_eq!(Resolution::FiveMinutes.as_interval_str(), "5m");
        assert_eq!(Resolution::FifteenMinutes.as_interval_str(), "15m");
    }

    // --- AsterDexCredentials tests ---

    #[test]
    fn test_asterdex_credentials_from_env() {
        env::set_var("ASTERDEX_API_KEY", "test-api-key-123");
        env::set_var("ASTERDEX_SECRET_KEY", "test-secret-key-456");

        let creds = AsterDexCredentials::from_env();
        assert!(creds.is_some());
        let creds = creds.unwrap();
        assert_eq!(creds.api_key, "test-api-key-123");
        assert_eq!(creds.secret_key, "test-secret-key-456");

        env::remove_var("ASTERDEX_API_KEY");
        env::remove_var("ASTERDEX_SECRET_KEY");
    }

    #[test]
    fn test_asterdex_credentials_missing_key() {
        env::remove_var("ASTERDEX_API_KEY");
        env::set_var("ASTERDEX_SECRET_KEY", "test-secret");

        let creds = AsterDexCredentials::from_env();
        assert!(creds.is_none());

        env::remove_var("ASTERDEX_SECRET_KEY");
    }

    #[test]
    fn test_asterdex_credentials_missing_secret() {
        env::set_var("ASTERDEX_API_KEY", "test-key");
        env::remove_var("ASTERDEX_SECRET_KEY");

        let creds = AsterDexCredentials::from_env();
        assert!(creds.is_none());

        env::remove_var("ASTERDEX_API_KEY");
    }

    #[test]
    fn test_asterdex_credentials_validate_missing() {
        env::remove_var("ASTERDEX_API_KEY");
        env::remove_var("ASTERDEX_SECRET_KEY");

        let result = AsterDexCredentials::validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("ASTERDEX_API_KEY"), "Error should mention ASTERDEX_API_KEY");
        assert!(err.contains("ASTERDEX_SECRET_KEY"), "Error should mention ASTERDEX_SECRET_KEY");
    }

    #[test]
    fn test_asterdex_credentials_validate_present() {
        env::set_var("ASTERDEX_API_KEY", "valid-key");
        env::set_var("ASTERDEX_SECRET_KEY", "valid-secret");

        let result = AsterDexCredentials::validate();
        assert!(result.is_ok());
        let creds = result.unwrap();
        assert_eq!(creds.api_key, "valid-key");
        assert_eq!(creds.secret_key, "valid-secret");

        env::remove_var("ASTERDEX_API_KEY");
        env::remove_var("ASTERDEX_SECRET_KEY");
    }

    #[test]
    fn test_normalize_symbol() {
        // Concatenated uppercase, USD -> USDT
        assert_eq!(normalize_symbol("BTCUSDT"), "BTCUSDT");
        assert_eq!(normalize_symbol("BTC-USD"), "BTCUSDT");
        assert_eq!(normalize_symbol("btc/usdt"), "BTCUSDT");
        assert_eq!(normalize_symbol("ETHUSDT"), "ETHUSDT");
        assert_eq!(normalize_symbol("btcusd"), "BTCUSDT");
    }

    #[test]
    fn test_config_without_credentials_succeeds() {
        let args = ChartArgs {
            symbol: "BTCUSDT".to_string(),
            tick_size: Some(100),
            resolution: None,
            large_trade_threshold: Decimal::from(10000),
            theme: ThemeName::Dark,
        };
        let config = Config::from_args(args).expect("Config creation should succeed");
        assert_eq!(config.symbol, "BTCUSDT");
    }

    #[test]
    fn test_config_symbol_normalization() {
        let args = ChartArgs {
            symbol: "BTC-USD".to_string(),
            tick_size: Some(100),
            resolution: None,
            large_trade_threshold: Decimal::from(10000),
            theme: ThemeName::Dark,
        };
        let config = Config::from_args(args).expect("Config creation should succeed");
        assert_eq!(config.symbol, "BTCUSDT");
    }
}
