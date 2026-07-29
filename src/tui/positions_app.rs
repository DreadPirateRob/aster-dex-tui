// src/tui/positions_app.rs
// Application state for the positions TUI view with management action state machine.
//
// PositionsMode controls the current interaction state:
//   Normal -> PriceInput (S/T keys) -> Confirming (Enter) -> Submitting (Y) -> ShowingResult
//   Normal -> Confirming (C/X keys) -> Submitting (Y) -> ShowingResult
//   Any overlay mode -> Normal (Esc)

use crate::data::order::OrderSide;
use crate::data::position::{Position, PositionSide};
use crate::network::asterdex_trading::{OrderParams, OrderResponse, TradingError};
use crate::network::MarkPriceData;
use crate::tui::text_input::TextInput;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use rust_decimal::Decimal;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// PositionsMode
// ---------------------------------------------------------------------------

/// Current interaction mode for the positions view.
///
/// Overlays (PriceInput, Confirming, Submitting, ShowingResult) capture all
/// keys -- quit keys only work in Normal mode (Pitfall 6 from research).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionsMode {
    /// Normal table browsing (navigation, quit)
    Normal,
    /// Price input overlay for SL or TP (TextInput active)
    PriceInput,
    /// Confirmation dialog before order submission (Y/N)
    Confirming,
    /// Waiting for async order placement
    Submitting,
    /// Showing order result (success or error)
    ShowingResult,
}

// ---------------------------------------------------------------------------
// ManagementAction
// ---------------------------------------------------------------------------

/// Which management action is in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagementAction {
    StopLoss,
    TakeProfit,
    ClosePosition,
    ReversePosition,
}

// ---------------------------------------------------------------------------
// PositionSnapshot
// ---------------------------------------------------------------------------

/// Snapshot of selected position data at the moment of action initiation.
///
/// Captures immutable state for the confirmation dialog, preventing stale
/// data issues if mark price updates while the user is confirming
/// (Pitfall 2 from research).
#[derive(Debug, Clone)]
pub struct PositionSnapshot {
    pub symbol: String,
    pub side: PositionSide,
    pub quantity: Decimal,     // abs(position_amt)
    pub _entry_price: Decimal,
    pub mark_price: Decimal,
}

// ---------------------------------------------------------------------------
// PositionsApp
// ---------------------------------------------------------------------------

/// Application state for the positions TUI view.
///
/// Holds all state needed for rendering and event handling of the
/// scrollable position table. Simpler than OrdersApp -- no status/side
/// filters since all displayed positions are open (zero-size filtered at ingest).
pub struct PositionsApp {
    /// Loaded position data from fetch_positions(), filtered to non-zero position_amt
    pub positions: Vec<Position>,
    /// ratatui's built-in selection/scroll state for the table
    pub table_state: TableState,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,
    /// Current trading pair (Some for single-pair, None for all-pairs mode)
    pub symbol: Option<String>,
    /// Color theme (from existing Theme system)
    pub theme: Theme,
    /// Rows per page for PgUp/PgDn (default 20, updated from terminal area on render)
    pub page_size: u16,
    /// Channel sender to request a position refresh from the event loop
    pub refresh_tx: Option<mpsc::Sender<()>>,
    /// Timestamp of last successful refresh request (for debounce)
    pub last_refresh: Instant,
    /// Track freshness of mark price data for status display
    pub last_mark_update: Option<Instant>,

    // -- Management action state (Phase 43) --

    /// Current interaction mode (Normal for read-only browsing, overlays for management)
    pub mode: PositionsMode,
    /// Which management action is being performed (set when entering PriceInput or Confirming)
    pub current_action: Option<ManagementAction>,
    /// Snapshot of selected position for confirmation (prevents stale data per Pitfall 2)
    pub position_snapshot: Option<PositionSnapshot>,
    /// TextInput for stop-loss / take-profit price entry
    pub price_input: TextInput,
    /// Channel to send (OrderParams, reduce_only) to event loop for async placement
    pub submit_tx: Option<mpsc::Sender<(OrderParams, bool)>>,
    /// Result of the last management order placement attempt
    pub order_result: Option<Result<OrderResponse, TradingError>>,
    /// Error message for inline display (e.g., "Price must be below mark price")
    pub error_message: Option<String>,
    /// Status message for transient feedback (e.g., "Refresh in 3s")
    pub status_message: Option<String>,
}

impl PositionsApp {
    /// Create a new PositionsApp with the given symbol, positions, and theme.
    ///
    /// Pass `Some(symbol)` for single-pair mode or `None` for all-pairs mode.
    /// Initializes table selection to the first row if positions are non-empty,
    /// or no selection if the positions list is empty.
    pub fn new(symbol: Option<String>, positions: Vec<Position>, theme: Theme) -> Self {
        let mut table_state = TableState::default();
        if !positions.is_empty() {
            table_state.select(Some(0));
        }

        Self {
            positions,
            table_state,
            should_quit: false,
            symbol,
            theme,
            page_size: 20,
            refresh_tx: None,
            // Initialize to 10 seconds in the past so the first 'r' press is always allowed
            last_refresh: Instant::now() - Duration::from_secs(10),
            last_mark_update: None,
            // Management action state
            mode: PositionsMode::Normal,
            current_action: None,
            position_snapshot: None,
            price_input: TextInput::new(),
            submit_tx: None,
            order_result: None,
            error_message: None,
            status_message: None,
        }
    }

    // -----------------------------------------------------------------------
    // Key handling: mode-aware dispatch
    // -----------------------------------------------------------------------

    /// Handle keyboard input. Dispatches on current mode FIRST.
    ///
    /// Returns true if the key was handled, false otherwise.
    ///
    /// In overlay modes (PriceInput, Confirming, Submitting, ShowingResult),
    /// ALL keys are captured -- quit keys only work in Normal mode.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match self.mode {
            PositionsMode::Normal => self.handle_key_normal(key),
            PositionsMode::PriceInput => self.handle_key_price_input(key),
            PositionsMode::Confirming => self.handle_key_confirming(key),
            PositionsMode::Submitting => true, // absorb all keys while submitting
            PositionsMode::ShowingResult => {
                // Any key dismisses result and returns to Normal
                self.order_result = None;
                self.error_message = None;
                self.mode = PositionsMode::Normal;
                true
            }
        }
    }

    /// Handle keys in Normal mode (table browsing, quit, management hotkeys).
    fn handle_key_normal(&mut self, key: KeyEvent) -> bool {
        // Quit keys are always active in Normal mode
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.should_quit = true;
                return true;
            }
            (KeyCode::Esc, _) => {
                self.should_quit = true;
                return true;
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
                return true;
            }
            _ => {}
        }

        // Refresh key is always active (even on empty table)
        match (key.code, key.modifiers) {
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                let elapsed = self.last_refresh.elapsed();
                if elapsed >= Duration::from_secs(5) {
                    if let Some(tx) = &self.refresh_tx {
                        let _ = tx.try_send(());
                        self.last_refresh = Instant::now();
                        self.status_message = None;
                    }
                } else {
                    let remaining = 5 - elapsed.as_secs();
                    self.status_message =
                        Some(format!("Refresh in {}s", remaining));
                }
                return true;
            }
            _ => {}
        }

        // Navigation keys require non-empty table
        if self.positions.is_empty() {
            return false;
        }

        // Management hotkeys: S/T/C/X require non-empty positions AND a selection
        if self.table_state.selected().is_some() {
            match (key.code, key.modifiers) {
                // S = Stop Loss (price input needed)
                (KeyCode::Char('s'), KeyModifiers::NONE) => {
                    if let Some(snap) = self.snapshot_selected() {
                        self.position_snapshot = Some(snap);
                        self.current_action = Some(ManagementAction::StopLoss);
                        self.price_input.clear();
                        self.error_message = None;
                        self.mode = PositionsMode::PriceInput;
                    }
                    return true;
                }
                // T = Take Profit (price input needed)
                (KeyCode::Char('t'), KeyModifiers::NONE) => {
                    if let Some(snap) = self.snapshot_selected() {
                        self.position_snapshot = Some(snap);
                        self.current_action = Some(ManagementAction::TakeProfit);
                        self.price_input.clear();
                        self.error_message = None;
                        self.mode = PositionsMode::PriceInput;
                    }
                    return true;
                }
                // x = Close Position (lowercase)
                (KeyCode::Char('x'), KeyModifiers::NONE) => {
                    if let Some(snap) = self.snapshot_selected() {
                        self.position_snapshot = Some(snap);
                        self.current_action = Some(ManagementAction::ClosePosition);
                        self.error_message = None;
                        self.mode = PositionsMode::Confirming;
                    }
                    return true;
                }
                // X = Reverse Position (Shift+X)
                (KeyCode::Char('X'), KeyModifiers::SHIFT) => {
                    if let Some(snap) = self.snapshot_selected() {
                        self.position_snapshot = Some(snap);
                        self.current_action = Some(ManagementAction::ReversePosition);
                        self.error_message = None;
                        self.mode = PositionsMode::Confirming;
                    }
                    return true;
                }
                _ => {}
            }
        }

        match (key.code, key.modifiers) {
            // Single row movement
            (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.table_state.select_next();
                true
            }
            (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.table_state.select_previous();
                true
            }
            // Page movement
            (KeyCode::PageDown, _) => {
                self.table_state.scroll_down_by(self.page_size);
                true
            }
            (KeyCode::PageUp, _) => {
                self.table_state.scroll_up_by(self.page_size);
                true
            }
            // Jump to extremes
            (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.table_state.select_first();
                true
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                self.table_state.select_last();
                true
            }
            _ => false,
        }
    }

    /// Handle keys in PriceInput mode (text editing for SL/TP price).
    fn handle_key_price_input(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.price_input.clear();
                self.error_message = None;
                self.current_action = None;
                self.position_snapshot = None;
                self.mode = PositionsMode::Normal;
                true
            }
            KeyCode::Enter => {
                // Validate the price
                let price = match self.price_input.as_decimal() {
                    Some(p) if p > Decimal::ZERO => p,
                    _ => {
                        self.error_message = Some("Enter a valid price".to_string());
                        return true;
                    }
                };

                // Directional validation (Pitfall 3)
                if let (Some(action), Some(snap)) = (&self.current_action, &self.position_snapshot) {
                    let mark = snap.mark_price;
                    let is_long = snap.side == PositionSide::Long;

                    let validation_error = match action {
                        ManagementAction::StopLoss if is_long && price >= mark => {
                            Some(format!("SL for long must be below mark ({})", mark))
                        }
                        ManagementAction::StopLoss if !is_long && price <= mark => {
                            Some(format!("SL for short must be above mark ({})", mark))
                        }
                        ManagementAction::TakeProfit if is_long && price <= mark => {
                            Some(format!("TP for long must be above mark ({})", mark))
                        }
                        ManagementAction::TakeProfit if !is_long && price >= mark => {
                            Some(format!("TP for short must be below mark ({})", mark))
                        }
                        _ => None,
                    };

                    if let Some(msg) = validation_error {
                        self.error_message = Some(msg);
                        return true;
                    }
                }

                // Price is valid -- transition to Confirming
                self.error_message = None;
                self.mode = PositionsMode::Confirming;
                true
            }
            KeyCode::Char(c) => {
                // Only allow numeric input: digits, decimal point, minus sign
                if c.is_ascii_digit() || c == '.' || c == '-' {
                    let char_key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
                    self.price_input.handle_key(char_key);
                    self.error_message = None;
                }
                true // consume all chars (reject non-numeric silently)
            }
            // Delegate editing keys (Backspace, Delete, Left, Right, Home, End)
            _ => {
                let handled = self.price_input.handle_key(key);
                if handled {
                    self.error_message = None;
                }
                // Always return true in PriceInput to absorb all keys
                true
            }
        }
    }

    /// Handle keys in Confirming mode (Y/N to confirm or cancel).
    fn handle_key_confirming(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.mode = PositionsMode::Normal;
                self.current_action = None;
                self.position_snapshot = None;
                self.error_message = None;
                true
            }
            KeyCode::Char('y') | KeyCode::Enter => {
                // Build OrderParams from snapshot + action + price, send via submit_tx
                if let (Some(action), Some(snap)) = (self.current_action, &self.position_snapshot) {
                    let close_side = Self::close_side(snap);
                    let symbol = snap.symbol.clone();
                    let quantity = snap.quantity;

                    let (params, reduce_only) = match action {
                        ManagementAction::StopLoss => {
                            let stop_price = self.price_input.as_decimal().unwrap_or(Decimal::ZERO);
                            (
                                OrderParams::StopMarket {
                                    symbol,
                                    side: close_side,
                                    quantity,
                                    stop_price,
                                },
                                true, // SL is a closing order; to_query_string handles hedge/one-way emission
                            )
                        }
                        ManagementAction::TakeProfit => {
                            let stop_price = self.price_input.as_decimal().unwrap_or(Decimal::ZERO);
                            (
                                OrderParams::TakeProfitMarket {
                                    symbol,
                                    side: close_side,
                                    quantity,
                                    stop_price,
                                },
                                true, // TP is a closing order; to_query_string handles hedge/one-way emission
                            )
                        }
                        ManagementAction::ClosePosition => (
                            OrderParams::Market {
                                symbol,
                                side: close_side,
                                quantity,
                            },
                            true,
                        ),
                        ManagementAction::ReversePosition => (
                            OrderParams::Market {
                                symbol,
                                side: close_side,
                                quantity,
                            },
                            false, // Event loop overrides reduce_only per leg (close=true, open=false)
                        ),
                    };

                    if let Some(tx) = &self.submit_tx {
                        let _ = tx.try_send((params, reduce_only));
                        self.mode = PositionsMode::Submitting;
                    } else {
                        self.error_message = Some("No submission channel available".to_string());
                        self.mode = PositionsMode::Normal;
                        self.current_action = None;
                        self.position_snapshot = None;
                    }
                }
                true
            }
            _ => true, // absorb all other keys in confirming mode
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Snapshot the currently selected position.
    ///
    /// Returns None if no position is selected or index is out of bounds.
    /// Captures abs(position_amt) for quantity.
    pub fn snapshot_selected(&self) -> Option<PositionSnapshot> {
        let idx = self.table_state.selected()?;
        let pos = self.positions.get(idx)?;
        Some(PositionSnapshot {
            symbol: pos.symbol.clone(),
            side: pos.position_side,
            quantity: pos.position_amt.abs(),
            _entry_price: pos.entry_price,
            mark_price: pos.mark_price,
        })
    }

    /// Return the opposite side for closing a position.
    ///
    /// Long -> Sell, Short -> Buy.
    pub fn close_side(snapshot: &PositionSnapshot) -> OrderSide {
        match snapshot.side {
            PositionSide::Long => OrderSide::Sell,
            PositionSide::Short => OrderSide::Buy,
        }
    }

    /// Update mark price and funding data for ALL matching positions.
    ///
    /// Called from the event loop tick handler when mark price WebSocket
    /// data arrives. Iterates all positions matching by symbol (case-insensitive)
    /// and updates mark_price, funding_rate, and next_funding_time.
    /// Uses `.filter()` instead of `.find()` to handle hedge mode (same symbol,
    /// both sides).
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
            self.last_mark_update = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::position::{Position, PositionSide};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn sample_position() -> Position {
        Position {
            symbol: "BTCUSDT".to_string(),
            position_side: PositionSide::Long,
            position_amt: Decimal::from_str("0.5").unwrap(),
            entry_price: Decimal::from_str("50000").unwrap(),
            mark_price: Decimal::from_str("51000").unwrap(),
            unrealized_profit: Decimal::from_str("500").unwrap(),
            liquidation_price: Decimal::from_str("45000").unwrap(),
            leverage: 10,
            margin_type: "isolated".to_string(),
            isolated_margin: Decimal::from_str("2500").unwrap(),
            notional: Decimal::from_str("25500").unwrap(),
            update_time: 1700000000000,
            funding_rate: Some(Decimal::from_str("0.0001").unwrap()),
            next_funding_time: Some(1700003600000),
        }
    }

    fn sample_short_position() -> Position {
        Position {
            symbol: "ETHUSDT".to_string(),
            position_side: PositionSide::Short,
            position_amt: Decimal::from_str("-2.0").unwrap(),
            entry_price: Decimal::from_str("3000").unwrap(),
            mark_price: Decimal::from_str("2900").unwrap(),
            unrealized_profit: Decimal::from_str("200").unwrap(),
            liquidation_price: Decimal::from_str("5000").unwrap(),
            leverage: 5,
            margin_type: "cross".to_string(),
            isolated_margin: Decimal::ZERO,
            notional: Decimal::from_str("-5800").unwrap(),
            update_time: 1700000001000,
            funding_rate: Some(Decimal::from_str("0.0001").unwrap()),
            next_funding_time: Some(1700003600000),
        }
    }

    fn test_theme() -> Theme {
        Theme::dark()
    }

    // ---- Construction tests ----

    #[test]
    fn test_new_with_positions_selects_first() {
        let app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        assert_eq!(app.table_state.selected(), Some(0));
        assert!(!app.should_quit);
        assert_eq!(app.symbol, Some("BTCUSDT".to_string()));
        assert_eq!(app.page_size, 20);
        assert_eq!(app.mode, PositionsMode::Normal);
        assert!(app.current_action.is_none());
        assert!(app.position_snapshot.is_none());
    }

    #[test]
    fn test_new_empty_no_selection() {
        let app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        assert_eq!(app.table_state.selected(), None);
    }

    #[test]
    fn test_new_all_pairs_mode() {
        let app = PositionsApp::new(None, vec![sample_position()], test_theme());
        assert!(app.symbol.is_none());
        assert_eq!(app.table_state.selected(), Some(0));
    }

    // ---- Quit tests (Normal mode only) ----

    #[test]
    fn test_quit_q() {
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_esc() {
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_ctrl_c() {
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    // ---- Navigation tests ----

    #[test]
    fn test_navigation_on_empty_returns_false() {
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(!app.handle_key(key));
    }

    #[test]
    fn test_navigation_down() {
        let positions = vec![sample_position(), sample_position()];
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), positions, test_theme());
        assert_eq!(app.table_state.selected(), Some(0));

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.table_state.selected(), Some(1));
    }

    #[test]
    fn test_navigation_j() {
        let positions = vec![sample_position(), sample_position()];
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), positions, test_theme());

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.table_state.selected(), Some(1));
    }

    #[test]
    fn test_navigation_up() {
        let positions = vec![sample_position(), sample_position()];
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), positions, test_theme());

        // Move down first, then up
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(down);
        assert_eq!(app.table_state.selected(), Some(1));

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert!(app.handle_key(up));
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_navigation_k() {
        let positions = vec![sample_position(), sample_position()];
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), positions, test_theme());

        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(down);

        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(app.handle_key(k));
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_navigation_home_g() {
        let positions = vec![sample_position(), sample_position(), sample_position()];
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), positions, test_theme());

        // Move to end first
        let end = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);
        app.handle_key(end);

        // g should go to first
        let g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        assert!(app.handle_key(g));
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_navigation_end_shift_g() {
        let positions = vec![sample_position(), sample_position(), sample_position()];
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), positions, test_theme());

        let g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert!(app.handle_key(g));
        // select_last() sets a sentinel value (usize::MAX) that gets clamped
        // to the actual last row during render. Verify selection is set.
        assert!(app.table_state.selected().is_some());
    }

    // ---- Refresh debounce ----

    #[test]
    fn test_key_r_debounce() {
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let (tx, mut rx) = mpsc::channel(1);
        app.refresh_tx = Some(tx);

        // First press: last_refresh was initialized to 10s ago, should send
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(rx.try_recv().is_ok(), "First refresh should send signal");

        // Second press immediately: within 5s debounce window, should NOT send
        assert!(app.handle_key(key));
        assert!(
            rx.try_recv().is_err(),
            "Debounced refresh should not send signal"
        );

        // Simulate time passing (set last_refresh to 10s ago)
        app.last_refresh = Instant::now() - Duration::from_secs(10);
        assert!(app.handle_key(key));
        assert!(
            rx.try_recv().is_ok(),
            "Refresh after debounce window should send signal"
        );
    }

    // ---- Mark price update ----

    #[test]
    fn test_update_mark_price_matches_symbol() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        assert!(app.last_mark_update.is_none());

        let data = MarkPriceData {
            symbol: "BTCUSDT".to_string(),
            mark_price: Decimal::from_str("52000").unwrap(),
            funding_rate: Decimal::from_str("0.0002").unwrap(),
            next_funding_time: 1700007200000,
        };

        app.update_mark_price(&data);

        assert_eq!(
            app.positions[0].mark_price,
            Decimal::from_str("52000").unwrap()
        );
        assert_eq!(
            app.positions[0].funding_rate,
            Some(Decimal::from_str("0.0002").unwrap())
        );
        assert_eq!(app.positions[0].next_funding_time, Some(1700007200000));
        assert!(app.last_mark_update.is_some());
    }

    #[test]
    fn test_update_mark_price_recomputes_notional() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // position_amt=0.5, notional=25500
            test_theme(),
        );

        let data = MarkPriceData {
            symbol: "BTCUSDT".to_string(),
            mark_price: Decimal::from_str("52000").unwrap(),
            funding_rate: Decimal::from_str("0.0002").unwrap(),
            next_funding_time: 1700007200000,
        };

        app.update_mark_price(&data);

        // notional should be recomputed: 52000 * 0.5 = 26000
        assert_eq!(
            app.positions[0].notional,
            Decimal::from_str("26000").unwrap(),
            "notional should be mark_price * position_amt after update"
        );
    }

    #[test]
    fn test_update_mark_price_no_match() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        let data = MarkPriceData {
            symbol: "ETHUSDT".to_string(),
            mark_price: Decimal::from_str("3000").unwrap(),
            funding_rate: Decimal::from_str("0.0001").unwrap(),
            next_funding_time: 1700007200000,
        };

        app.update_mark_price(&data);

        // BTC position should be unchanged
        assert_eq!(
            app.positions[0].mark_price,
            Decimal::from_str("51000").unwrap()
        );
        // last_mark_update should remain None since no position matched
        assert!(app.last_mark_update.is_none());
    }

    #[test]
    fn test_update_mark_price_hedge_mode_both_sides() {
        // Hedge mode: same symbol, both Long and Short positions
        let long_pos = sample_position(); // BTCUSDT Long
        let mut short_pos = sample_position();
        short_pos.position_side = PositionSide::Short;
        short_pos.position_amt = Decimal::from_str("-0.3").unwrap();

        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![long_pos, short_pos],
            test_theme(),
        );

        let data = MarkPriceData {
            symbol: "BTCUSDT".to_string(),
            mark_price: Decimal::from_str("52000").unwrap(),
            funding_rate: Decimal::from_str("0.0003").unwrap(),
            next_funding_time: 1700007200000,
        };

        app.update_mark_price(&data);

        // Both positions should be updated
        assert_eq!(
            app.positions[0].mark_price,
            Decimal::from_str("52000").unwrap(),
            "Long position mark_price should be updated"
        );
        assert_eq!(
            app.positions[1].mark_price,
            Decimal::from_str("52000").unwrap(),
            "Short position mark_price should be updated"
        );
        assert_eq!(
            app.positions[0].funding_rate,
            Some(Decimal::from_str("0.0003").unwrap())
        );
        assert_eq!(
            app.positions[1].funding_rate,
            Some(Decimal::from_str("0.0003").unwrap())
        );
        assert!(app.last_mark_update.is_some());
    }

    // ---- Mode transition tests (Phase 43) ----

    #[test]
    fn test_s_key_transitions_to_price_input_stop_loss() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        assert_eq!(app.mode, PositionsMode::Normal);

        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(app.handle_key(key));

        assert_eq!(app.mode, PositionsMode::PriceInput);
        assert_eq!(app.current_action, Some(ManagementAction::StopLoss));
        assert!(app.position_snapshot.is_some());
        let snap = app.position_snapshot.as_ref().unwrap();
        assert_eq!(snap.symbol, "BTCUSDT");
        assert_eq!(snap.side, PositionSide::Long);
        assert_eq!(snap.quantity, Decimal::from_str("0.5").unwrap());
    }

    #[test]
    fn test_t_key_transitions_to_price_input_take_profit() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        let key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        assert!(app.handle_key(key));

        assert_eq!(app.mode, PositionsMode::PriceInput);
        assert_eq!(app.current_action, Some(ManagementAction::TakeProfit));
        assert!(app.position_snapshot.is_some());
    }

    #[test]
    fn test_x_key_transitions_to_confirming_close() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.handle_key(key));

        assert_eq!(app.mode, PositionsMode::Confirming);
        assert_eq!(app.current_action, Some(ManagementAction::ClosePosition));
        assert!(app.position_snapshot.is_some());
    }

    #[test]
    fn test_shift_x_key_transitions_to_confirming_reverse() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        let key = KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT);
        assert!(app.handle_key(key));

        assert_eq!(app.mode, PositionsMode::Confirming);
        assert_eq!(app.current_action, Some(ManagementAction::ReversePosition));
        assert!(app.position_snapshot.is_some());
    }

    #[test]
    fn test_esc_in_price_input_returns_to_normal() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        // Enter PriceInput mode
        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);
        assert_eq!(app.mode, PositionsMode::PriceInput);

        // Esc returns to Normal
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);
        assert_eq!(app.mode, PositionsMode::Normal);
        assert!(app.current_action.is_none());
        assert!(app.position_snapshot.is_none());
    }

    #[test]
    fn test_esc_in_confirming_returns_to_normal() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        // Enter Confirming mode via 'x'
        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        app.handle_key(x_key);
        assert_eq!(app.mode, PositionsMode::Confirming);

        // Esc returns to Normal
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);
        assert_eq!(app.mode, PositionsMode::Normal);
        assert!(app.current_action.is_none());
        assert!(app.position_snapshot.is_none());
    }

    #[test]
    fn test_n_key_in_confirming_returns_to_normal() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        app.handle_key(x_key);
        assert_eq!(app.mode, PositionsMode::Confirming);

        let n_key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        app.handle_key(n_key);
        assert_eq!(app.mode, PositionsMode::Normal);
    }

    #[test]
    fn test_quit_keys_in_price_input_do_not_quit() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        // Enter PriceInput mode
        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);
        assert_eq!(app.mode, PositionsMode::PriceInput);

        // 'q' should NOT quit in PriceInput mode (absorbed by overlay)
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(q));
        assert!(!app.should_quit);
        assert_eq!(app.mode, PositionsMode::PriceInput);
    }

    #[test]
    fn test_quit_keys_in_confirming_do_not_quit() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        // Enter Confirming mode
        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        app.handle_key(x_key);
        assert_eq!(app.mode, PositionsMode::Confirming);

        // 'q' should NOT quit in Confirming mode (absorbed by overlay)
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(q));
        assert!(!app.should_quit);
    }

    #[test]
    fn test_quit_keys_in_normal_do_quit() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        app.handle_key(q);
        assert!(app.should_quit);
    }

    #[test]
    fn test_submitting_absorbs_all_keys() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        app.mode = PositionsMode::Submitting;

        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(q));
        assert!(!app.should_quit);
        assert_eq!(app.mode, PositionsMode::Submitting);
    }

    #[test]
    fn test_showing_result_any_key_dismisses() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        app.mode = PositionsMode::ShowingResult;
        app.error_message = Some("test error".to_string());

        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.mode, PositionsMode::Normal);
        assert!(app.order_result.is_none());
        assert!(app.error_message.is_none());
    }

    // ---- snapshot_selected tests ----

    #[test]
    fn test_snapshot_selected_long_position() {
        let app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        let snap = app.snapshot_selected().unwrap();
        assert_eq!(snap.symbol, "BTCUSDT");
        assert_eq!(snap.side, PositionSide::Long);
        assert_eq!(snap.quantity, Decimal::from_str("0.5").unwrap()); // abs(0.5)
        assert_eq!(snap._entry_price, Decimal::from_str("50000").unwrap());
        assert_eq!(snap.mark_price, Decimal::from_str("51000").unwrap());
    }

    #[test]
    fn test_snapshot_selected_short_position_abs_quantity() {
        let app = PositionsApp::new(
            Some("ETHUSDT".to_string()),
            vec![sample_short_position()],
            test_theme(),
        );
        let snap = app.snapshot_selected().unwrap();
        assert_eq!(snap.symbol, "ETHUSDT");
        assert_eq!(snap.side, PositionSide::Short);
        // position_amt is -2.0, snapshot quantity should be abs = 2.0
        assert_eq!(snap.quantity, Decimal::from_str("2.0").unwrap());
    }

    #[test]
    fn test_snapshot_selected_empty_returns_none() {
        let app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        assert!(app.snapshot_selected().is_none());
    }

    // ---- close_side tests ----

    #[test]
    fn test_close_side_long_returns_sell() {
        let snap = PositionSnapshot {
            symbol: "BTCUSDT".to_string(),
            side: PositionSide::Long,
            quantity: Decimal::from_str("0.5").unwrap(),
            _entry_price: Decimal::from_str("50000").unwrap(),
            mark_price: Decimal::from_str("51000").unwrap(),
        };
        assert_eq!(PositionsApp::close_side(&snap), OrderSide::Sell);
    }

    #[test]
    fn test_close_side_short_returns_buy() {
        let snap = PositionSnapshot {
            symbol: "ETHUSDT".to_string(),
            side: PositionSide::Short,
            quantity: Decimal::from_str("2.0").unwrap(),
            _entry_price: Decimal::from_str("3000").unwrap(),
            mark_price: Decimal::from_str("2900").unwrap(),
        };
        assert_eq!(PositionsApp::close_side(&snap), OrderSide::Buy);
    }

    // ---- Price input validation tests ----

    #[test]
    fn test_price_input_invalid_shows_error() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );

        // Enter PriceInput mode for SL
        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);

        // Press Enter without typing a price
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);

        assert_eq!(app.mode, PositionsMode::PriceInput); // stays in PriceInput
        assert_eq!(app.error_message, Some("Enter a valid price".to_string()));
    }

    #[test]
    fn test_sl_long_price_must_be_below_mark() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // Long, mark_price=51000
            test_theme(),
        );

        // Enter PriceInput for SL
        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);

        // Type price above mark (52000 > 51000)
        for c in "52000".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);

        // Should stay in PriceInput with directional error
        assert_eq!(app.mode, PositionsMode::PriceInput);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("below mark"));
    }

    #[test]
    fn test_sl_long_valid_price_transitions_to_confirming() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // Long, mark_price=51000
            test_theme(),
        );

        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);

        // Type price below mark (49000 < 51000)
        for c in "49000".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);

        assert_eq!(app.mode, PositionsMode::Confirming);
        assert!(app.error_message.is_none());
    }

    #[test]
    fn test_tp_long_price_must_be_above_mark() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // Long, mark_price=51000
            test_theme(),
        );

        let t_key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        app.handle_key(t_key);

        // Type price below mark (49000 < 51000)
        for c in "49000".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);

        assert_eq!(app.mode, PositionsMode::PriceInput);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("above mark"));
    }

    #[test]
    fn test_sl_short_price_must_be_above_mark() {
        let mut app = PositionsApp::new(
            Some("ETHUSDT".to_string()),
            vec![sample_short_position()], // Short, mark_price=2900
            test_theme(),
        );

        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);

        // Type price below mark (2800 < 2900)
        for c in "2800".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);

        assert_eq!(app.mode, PositionsMode::PriceInput);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("above mark"));
    }

    #[test]
    fn test_tp_short_price_must_be_below_mark() {
        let mut app = PositionsApp::new(
            Some("ETHUSDT".to_string()),
            vec![sample_short_position()], // Short, mark_price=2900
            test_theme(),
        );

        let t_key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        app.handle_key(t_key);

        // Type price above mark (3000 > 2900)
        for c in "3000".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);

        assert_eq!(app.mode, PositionsMode::PriceInput);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("below mark"));
    }

    // ---- Confirming mode order building tests ----

    #[test]
    fn test_confirming_y_sends_close_position_order() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // Long 0.5 BTC
            test_theme(),
        );

        // Create channel
        let (tx, mut rx) = mpsc::channel(4);
        app.submit_tx = Some(tx);

        // Enter Confirming via 'x' (close position)
        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        app.handle_key(x_key);
        assert_eq!(app.mode, PositionsMode::Confirming);

        // Confirm with 'y'
        let y_key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        app.handle_key(y_key);
        assert_eq!(app.mode, PositionsMode::Submitting);

        // Verify the sent order
        let (params, reduce_only) = rx.try_recv().unwrap();
        assert!(reduce_only);
        match params {
            OrderParams::Market { symbol, side, quantity } => {
                assert_eq!(symbol, "BTCUSDT");
                assert_eq!(side, OrderSide::Sell); // Long -> close = Sell
                assert_eq!(quantity, Decimal::from_str("0.5").unwrap());
            }
            _ => panic!("Expected Market order for close position"),
        }
    }

    #[test]
    fn test_confirming_y_sends_stop_loss_order() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // Long 0.5 BTC, mark=51000
            test_theme(),
        );

        let (tx, mut rx) = mpsc::channel(4);
        app.submit_tx = Some(tx);

        // Enter SL flow: S -> type price -> Enter -> Y
        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        app.handle_key(s_key);

        for c in "49000".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, PositionsMode::Confirming);

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.mode, PositionsMode::Submitting);

        let (params, reduce_only) = rx.try_recv().unwrap();
        assert!(reduce_only);
        match params {
            OrderParams::StopMarket { symbol, side, quantity, stop_price } => {
                assert_eq!(symbol, "BTCUSDT");
                assert_eq!(side, OrderSide::Sell);
                assert_eq!(quantity, Decimal::from_str("0.5").unwrap());
                assert_eq!(stop_price, Decimal::from_str("49000").unwrap());
            }
            _ => panic!("Expected StopMarket order for SL"),
        }
    }

    #[test]
    fn test_confirming_y_sends_take_profit_order() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()], // Long 0.5 BTC, mark=51000
            test_theme(),
        );

        let (tx, mut rx) = mpsc::channel(4);
        app.submit_tx = Some(tx);

        // Enter TP flow: T -> type price -> Enter -> Y
        let t_key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        app.handle_key(t_key);

        for c in "55000".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, PositionsMode::Confirming);

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.mode, PositionsMode::Submitting);

        let (params, reduce_only) = rx.try_recv().unwrap();
        assert!(reduce_only);
        match params {
            OrderParams::TakeProfitMarket { symbol, side, quantity, stop_price } => {
                assert_eq!(symbol, "BTCUSDT");
                assert_eq!(side, OrderSide::Sell);
                assert_eq!(quantity, Decimal::from_str("0.5").unwrap());
                assert_eq!(stop_price, Decimal::from_str("55000").unwrap());
            }
            _ => panic!("Expected TakeProfitMarket order for TP"),
        }
    }

    #[test]
    fn test_confirming_no_submit_tx_returns_to_normal_with_error() {
        let mut app = PositionsApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_position()],
            test_theme(),
        );
        // No submit_tx set

        // Enter Confirming via 'x'
        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        app.handle_key(x_key);
        assert_eq!(app.mode, PositionsMode::Confirming);

        // Y with no channel
        let y_key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        app.handle_key(y_key);

        assert_eq!(app.mode, PositionsMode::Normal);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("channel"));
    }

    // ---- Management hotkeys on empty table ----

    #[test]
    fn test_management_keys_ignored_on_empty_table() {
        let mut app = PositionsApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());

        let s_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(!app.handle_key(s_key));
        assert_eq!(app.mode, PositionsMode::Normal);
    }
}
