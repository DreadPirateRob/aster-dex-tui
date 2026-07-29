// src/helpers.rs
// Shared helper functions extracted from main.rs command handlers (SHARED-M2, M3, M4, M5)

use crate::config::AsterDexCredentials;
use crate::data::position::Position;
use crate::network::{fetch_positions, TradingError};
use crate::tui::{EventHandler, Tui};
use std::io;

/// Validate AsterDEX credentials or exit with a user-facing error (SHARED-M4).
///
/// Replaces the 4 duplicated `match AsterDexCredentials::validate()` blocks
/// in Orders, Trade, Account, and Positions subcommands.
#[allow(dead_code)] // Used by binary crate (src/run/)
pub(crate) fn require_credentials() -> AsterDexCredentials {
    match AsterDexCredentials::validate() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Fetch positions from AsterDEX, filter zero-size, and optionally filter by symbol (SHARED-M2).
///
/// Replaces the 5 duplicated fetch+filter+retain blocks in Positions and Account subcommands.
#[allow(dead_code)] // Used by binary crate (src/run/)
pub(crate) async fn fetch_and_filter_positions(
    credentials: &AsterDexCredentials,
    symbol_filter: Option<&str>,
    client: &reqwest::Client,
) -> Result<Vec<Position>, TradingError> {
    let api_positions = fetch_positions(credentials, client).await?;
    let mut positions: Vec<Position> = api_positions.into_iter().map(Position::from).collect();
    positions.retain(|p| !p.position_amt.is_zero());
    if let Some(sym) = symbol_filter {
        positions.retain(|p| p.symbol.eq_ignore_ascii_case(sym));
    }
    Ok(positions)
}

/// Initialize TUI terminal and event handler with standard tick/frame rates (SHARED-M5).
///
/// Replaces the 4 duplicated `Tui::new()` + `EventHandler::new(TICK_RATE_MS, FRAME_RATE)` blocks
/// in Orders, Trade, Account, and Positions subcommands.
#[allow(dead_code)] // Used by binary crate (src/run/)
pub(crate) fn init_tui() -> io::Result<(Tui, EventHandler)> {
    let tui_instance = Tui::new()?;
    let events = EventHandler::new(
        std::time::Duration::from_millis(crate::TICK_RATE_MS),
        crate::FRAME_RATE,
    );
    Ok((tui_instance, events))
}

/// Capitalize the first character of a string, lowercase the rest.
///
/// Returns an empty string for empty input.
/// Extracted from account_app.rs and account_view.rs (ACCT-L6 deduplication).
pub(crate) fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
    }
}
