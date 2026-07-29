// src/main.rs
// Entry point for crypto-tui application — thin dispatcher

mod config;
mod data;
mod helpers;
mod logging;
mod network;
mod run;
mod tui;

use clap::Parser;
use config::{Cli, Command};
use logging::init_logging;
use std::io;

/// Tick rate for state updates (ms)
const TICK_RATE_MS: u64 = 250;

/// Target frame rate (FPS)
const FRAME_RATE: f64 = 30.0;

#[tokio::main]
async fn main() -> io::Result<()> {
    // Load .env file if present (before any env var reads)
    // Silently ignore if .env doesn't exist
    let _ = dotenvy::dotenv();

    // Initialize logging FIRST (before any tracing calls)
    // Keep _guard alive for entire program - dropping it stops log flushing
    let _guard = init_logging("./logs");

    // Initialize rustls crypto provider (required for TLS connections)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Build shared HTTP client (reused across all REST calls)
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client");

    // Parse CLI arguments
    let cli = Cli::parse();

    // Dispatch on subcommand
    match cli.command {
        Some(Command::Orders { symbol, all }) => {
            let effective_symbol = match (symbol, all) {
                (Some(s), false) => Some(s.to_uppercase()),
                (None, true) | (None, false) => None,
                (Some(_), true) => unreachable!(),
            };
            let theme = cli.theme.to_theme();
            run::orders::run(effective_symbol, theme, http_client).await
        }
        Some(Command::Trade { symbol, no_orders }) => {
            let symbol = symbol.to_uppercase();
            let theme = cli.theme.to_theme();
            run::trade::run(symbol, no_orders, theme, http_client).await
        }
        Some(Command::Account) => {
            let theme = cli.theme.to_theme();
            run::account::run(theme, http_client).await
        }
        Some(Command::Positions { symbol, all }) => {
            let effective_symbol = match (symbol, all) {
                (Some(s), false) => Some(s.to_uppercase()),
                (None, true) | (None, false) => None,
                (Some(_), true) => unreachable!(),
            };
            let theme = cli.theme.to_theme();
            run::positions::run(effective_symbol, theme, http_client).await
        }
        Some(Command::Dom { symbol }) => {
            let symbol = symbol.to_uppercase();
            let theme = cli.theme.to_theme();
            run::dom::run(symbol, theme, http_client).await
        }
        Some(Command::Liquidations { symbol, large_threshold }) => {
            let theme = cli.theme.to_theme();
            run::liquidations::run(symbol, large_threshold, theme).await
        }
        Some(Command::Analytics { symbol }) => {
            let theme = cli.theme.to_theme();
            run::analytics::run(symbol, theme, http_client).await
        }
        Some(Command::News { symbol, category }) => {
            let theme = cli.theme.to_theme();
            run::news::run(symbol, category, theme, http_client).await
        }
        Some(Command::Funding { symbol }) => {
            let theme = cli.theme.to_theme();
            run::funding::run(symbol, theme, http_client).await
        }
        Some(Command::Overview { symbol }) => {
            let theme = cli.theme.to_theme();
            run::overview::run(symbol, theme, http_client).await
        }
        Some(Command::Calendar { high_only, country }) => {
            let theme = cli.theme.to_theme();
            run::calendar::run(high_only, country, theme, http_client).await
        }
        None => {
            // Default: chart mode
            run::chart::run(
                cli.symbol,
                cli.tick_size,
                cli.resolution,
                cli.large_trade_threshold,
                cli.theme,
                http_client,
            ).await
        }
    }
}
