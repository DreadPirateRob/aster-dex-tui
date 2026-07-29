// src/tui/account_app.rs
// Application state for the account overview TUI view.
//
// Read-only dashboard: no navigation, no overlays, no management modes.
// Just quit keys. Auto-refresh is handled by the event loop (Plan 02).

use crate::data::account::{
    compute_exposure_by_side, compute_total_unrealized_pnl, AccountSummary,
};
use crate::data::position::Position;
use crate::helpers;
use crate::network::MarkPriceData;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rust_decimal::Decimal;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Application state for the account overview TUI view.
///
/// Holds all state needed for rendering the account overview dashboard.
/// Much simpler than PositionsApp -- no TableState, no management modes,
/// no overlays. This is a read-only dashboard.
pub struct AccountApp {
    /// Aggregated account metrics (None until first successful fetch)
    pub summary: Option<AccountSummary>,
    /// Computed margin ratio percentage (0-100+)
    pub margin_ratio: Decimal,
    /// Margin mode string: "Cross", "Isolated", or "Mixed"
    pub margin_mode: String,
    /// Open positions for mini-table display
    pub positions: Vec<Position>,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Color theme
    pub theme: Theme,
    /// Timestamp of last successful data refresh
    pub last_refresh: Option<Instant>,
    /// Whether the API is reachable (set on successful fetch)
    pub api_reachable: bool,
    /// Error message for display (e.g., fetch failure)
    pub error_message: Option<String>,
    /// Channel sender to request a manual refresh from the event loop
    pub refresh_tx: Option<mpsc::Sender<()>>,
    /// Timestamp of last refresh request (for 5s debounce)
    pub last_refresh_request: Instant,
    /// Transient status message (e.g., "Refreshing...", "Refresh in Xs")
    pub status_message: Option<String>,
}

impl AccountApp {
    /// Create a new AccountApp with default (empty) state.
    pub fn new(theme: Theme) -> Self {
        Self {
            summary: None,
            margin_ratio: Decimal::ZERO,
            margin_mode: "--".to_string(),
            positions: Vec::new(),
            should_quit: false,
            theme,
            last_refresh: None,
            api_reachable: false,
            error_message: None,
            refresh_tx: None,
            last_refresh_request: Instant::now() - Duration::from_secs(10),
            status_message: None,
        }
    }

    /// Handle keyboard input.
    ///
    /// Returns true if the key was handled, false otherwise.
    /// Only quit keys are active -- this is a read-only view.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.should_quit = true;
                true
            }
            (KeyCode::Esc, _) => {
                self.should_quit = true;
                true
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
                true
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                let elapsed = self.last_refresh_request.elapsed();
                if elapsed >= Duration::from_secs(5) {
                    if let Some(tx) = &self.refresh_tx {
                        let _ = tx.try_send(());
                        self.last_refresh_request = Instant::now();
                        self.status_message = Some("Refreshing...".to_string());
                    }
                } else {
                    let remaining = 5 - elapsed.as_secs();
                    self.status_message = Some(format!("Refresh in {}s", remaining));
                }
                true
            }
            _ => false,
        }
    }

    /// Update account data after a successful fetch.
    ///
    /// Sets summary, positions, margin_ratio, determines margin_mode,
    /// and marks the API as reachable.
    pub fn update_data(
        &mut self,
        summary: AccountSummary,
        positions: Vec<Position>,
        margin_ratio: Decimal,
    ) {
        self.summary = Some(summary);
        self.margin_ratio = margin_ratio;

        // Determine margin mode from positions
        self.margin_mode = Self::determine_margin_mode(&positions);

        self.positions = positions;
        self.last_refresh = Some(Instant::now());
        self.api_reachable = true;
        self.error_message = None;
    }

    /// Set an error message without changing reachability status.
    ///
    /// The event loop controls when to mark unreachable (only when ALL endpoints fail).
    pub fn set_error_message(&mut self, msg: String) {
        self.error_message = Some(msg);
    }

    /// Mark the API as unreachable (separate from error messages).
    pub fn set_unreachable(&mut self) {
        self.api_reachable = false;
    }

    /// Update mark price and funding data for all matching positions.
    ///
    /// Also recomputes summary unrealized PnL and exposure metrics when any
    /// position is updated. Unlike PositionsApp, AccountApp must recompute
    /// summary.long_exposure and summary.short_exposure because the metrics
    /// panel displays these values.
    pub fn update_mark_price(&mut self, data: &MarkPriceData) {
        let mut updated = false;
        for pos in self
            .positions
            .iter_mut()
            .filter(|p| p.symbol.eq_ignore_ascii_case(&data.symbol))
        {
            pos.mark_price = data.mark_price;
            pos.notional = data.mark_price * pos.position_amt;
            pos.funding_rate = Some(data.funding_rate);
            pos.next_funding_time = Some(data.next_funding_time);
            updated = true;
        }
        if updated {
            if let Some(ref mut summary) = self.summary {
                summary.total_unrealized_pnl = compute_total_unrealized_pnl(&self.positions);
                let (long, short) = compute_exposure_by_side(&self.positions);
                summary.long_exposure = long;
                summary.short_exposure = short;
            }
        }
    }

    /// Determine margin mode from positions.
    ///
    /// - If no positions: "--"
    /// - If all same margin_type: that value capitalized
    /// - If mixed: "Mixed"
    fn determine_margin_mode(positions: &[Position]) -> String {
        if positions.is_empty() {
            return "--".to_string();
        }

        let first = &positions[0].margin_type;
        let all_same = positions.iter().all(|p| p.margin_type == *first);

        if all_same {
            helpers::capitalize_first(first)
        } else {
            "Mixed".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::account::{AccountSummary, SideExposure};
    use crate::data::position::{Position, PositionSide};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::str::FromStr;

    fn test_theme() -> Theme {
        Theme::dark()
    }

    fn make_position(symbol: &str, margin_type: &str) -> Position {
        Position {
            symbol: symbol.to_string(),
            position_side: PositionSide::Long,
            position_amt: Decimal::from_str("0.5").unwrap(),
            entry_price: Decimal::from_str("50000").unwrap(),
            mark_price: Decimal::from_str("51000").unwrap(),
            unrealized_profit: Decimal::ZERO,
            liquidation_price: Decimal::ZERO,
            leverage: 10,
            margin_type: margin_type.to_string(),
            isolated_margin: Decimal::ZERO,
            notional: Decimal::from_str("25500").unwrap(),
            update_time: 0,
            funding_rate: None,
            next_funding_time: None,
        }
    }

    fn make_summary() -> AccountSummary {
        AccountSummary {
            wallet_balance: dec!(10000),
            available_balance: dec!(5000),
            total_unrealized_pnl: dec!(500),
            open_position_count: 2,
            total_notional: dec!(28600),
            long_exposure: SideExposure {
                notional: dec!(25500),
                unrealized_pnl: dec!(500),
                count: 1,
            },
            short_exposure: SideExposure {
                notional: dec!(3100),
                unrealized_pnl: dec!(-100),
                count: 1,
            },
            daily_realized_pnl: dec!(150),
        }
    }

    #[test]
    fn test_handle_key_quit_q() {
        let mut app = AccountApp::new(test_theme());
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_key_quit_esc() {
        let mut app = AccountApp::new(test_theme());
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_key_quit_ctrl_c() {
        let mut app = AccountApp::new(test_theme());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_key_ignores_other() {
        let mut app = AccountApp::new(test_theme());
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(!app.handle_key(key));
        assert!(!app.should_quit);
    }

    #[test]
    fn test_update_data_sets_fields() {
        let mut app = AccountApp::new(test_theme());
        assert!(app.summary.is_none());
        assert!(!app.api_reachable);

        let positions = vec![
            make_position("BTCUSDT", "cross"),
            make_position("ETHUSDT", "cross"),
        ];
        let summary = make_summary();

        app.update_data(summary, positions, dec!(5));

        assert!(app.summary.is_some());
        assert_eq!(app.margin_ratio, dec!(5));
        assert!(app.api_reachable);
        assert!(app.last_refresh.is_some());
        assert!(app.error_message.is_none());
        assert_eq!(app.positions.len(), 2);
    }

    #[test]
    fn test_margin_mode_cross() {
        let mut app = AccountApp::new(test_theme());
        let positions = vec![
            make_position("BTCUSDT", "cross"),
            make_position("ETHUSDT", "cross"),
        ];
        app.update_data(make_summary(), positions, dec!(5));
        assert_eq!(app.margin_mode, "Cross");
    }

    #[test]
    fn test_margin_mode_isolated() {
        let mut app = AccountApp::new(test_theme());
        let positions = vec![
            make_position("BTCUSDT", "isolated"),
            make_position("ETHUSDT", "isolated"),
        ];
        app.update_data(make_summary(), positions, dec!(5));
        assert_eq!(app.margin_mode, "Isolated");
    }

    #[test]
    fn test_margin_mode_mixed() {
        let mut app = AccountApp::new(test_theme());
        let positions = vec![
            make_position("BTCUSDT", "cross"),
            make_position("ETHUSDT", "isolated"),
        ];
        app.update_data(make_summary(), positions, dec!(5));
        assert_eq!(app.margin_mode, "Mixed");
    }

    #[test]
    fn test_margin_mode_empty() {
        let mut app = AccountApp::new(test_theme());
        app.update_data(make_summary(), vec![], dec!(0));
        assert_eq!(app.margin_mode, "--");
    }

    #[test]
    fn test_set_error_message() {
        let mut app = AccountApp::new(test_theme());
        // First mark as reachable
        app.api_reachable = true;

        app.set_error_message("Connection failed".to_string());
        // set_error_message does NOT change api_reachable
        assert!(app.api_reachable);
        assert_eq!(app.error_message, Some("Connection failed".to_string()));
    }

    #[test]
    fn test_set_unreachable() {
        let mut app = AccountApp::new(test_theme());
        app.api_reachable = true;

        app.set_unreachable();
        assert!(!app.api_reachable);
    }

    #[test]
    fn test_new_defaults() {
        let app = AccountApp::new(test_theme());
        assert!(app.summary.is_none());
        assert_eq!(app.margin_ratio, Decimal::ZERO);
        assert_eq!(app.margin_mode, "--");
        assert!(app.positions.is_empty());
        assert!(!app.should_quit);
        assert!(app.last_refresh.is_none());
        assert!(!app.api_reachable);
        assert!(app.error_message.is_none());
        assert!(app.refresh_tx.is_none());
        assert!(app.status_message.is_none());
    }
}
