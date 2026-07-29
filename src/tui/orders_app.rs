// src/tui/orders_app.rs
// Application state for the orders TUI view

use crate::data::order::{Order, OrderSide, OrderStatus, OrderType};
use crate::network::types::ConnectionStatus;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use rust_decimal::Decimal;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Filter for order status column.
///
/// Cycles through ALL -> OPEN -> FILLED -> CANCELLED -> ALL.
/// `Open` matches both `New` and `PartiallyFilled` statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Open,
    Filled,
    Cancelled,
}

impl StatusFilter {
    /// Advance to the next filter in the cycle.
    pub fn cycle(self) -> Self {
        match self {
            StatusFilter::All => StatusFilter::Open,
            StatusFilter::Open => StatusFilter::Filled,
            StatusFilter::Filled => StatusFilter::Cancelled,
            StatusFilter::Cancelled => StatusFilter::All,
        }
    }

    /// Human-readable label for display in the status bar.
    pub fn label(&self) -> &'static str {
        match self {
            StatusFilter::All => "ALL",
            StatusFilter::Open => "OPEN",
            StatusFilter::Filled => "FILLED",
            StatusFilter::Cancelled => "CANCELLED",
        }
    }

    /// Returns true if the given order status passes this filter.
    pub fn matches(&self, status: &OrderStatus) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Open => matches!(status, OrderStatus::New | OrderStatus::PartiallyFilled),
            StatusFilter::Filled => matches!(status, OrderStatus::Filled),
            StatusFilter::Cancelled => matches!(status, OrderStatus::Canceled),
        }
    }
}

/// Filter for order side column.
///
/// Cycles through ALL -> BUY -> SELL -> ALL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    #[default]
    All,
    Buy,
    Sell,
}

impl SideFilter {
    /// Advance to the next filter in the cycle.
    pub fn cycle(self) -> Self {
        match self {
            SideFilter::All => SideFilter::Buy,
            SideFilter::Buy => SideFilter::Sell,
            SideFilter::Sell => SideFilter::All,
        }
    }

    /// Human-readable label for display in the status bar.
    pub fn label(&self) -> &'static str {
        match self {
            SideFilter::All => "ALL",
            SideFilter::Buy => "BUY",
            SideFilter::Sell => "SELL",
        }
    }

    /// Returns true if the given order side passes this filter.
    pub fn matches(&self, side: &OrderSide) -> bool {
        match self {
            SideFilter::All => true,
            SideFilter::Buy => matches!(side, OrderSide::Buy),
            SideFilter::Sell => matches!(side, OrderSide::Sell),
        }
    }
}

/// Current interaction mode for the orders view.
///
/// Simpler than PositionsMode -- no PriceInput needed since cancel has no price.
/// Overlays (Confirming, Submitting, ShowingResult) capture all keys --
/// quit keys only work in Normal mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdersMode {
    /// Normal table browsing (navigation, quit)
    Normal,
    /// Y/N confirmation dialog showing order details before cancel
    Confirming,
    /// Waiting for cancel API response
    Submitting,
    /// Success or error display (any key dismisses)
    ShowingResult,
}

/// Snapshot of selected order data at the moment 'd' is pressed.
///
/// Captures immutable state for the confirmation dialog, preventing
/// cancel-wrong-order race if the table updates while confirming
/// (Pitfall 1 from research: selection-by-ID).
#[derive(Debug, Clone)]
pub struct OrderSnapshot {
    pub order_id: u64,
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub price: Decimal,
    pub orig_qty: Decimal,
    pub executed_qty: Decimal,
    pub status: OrderStatus,
}

/// Application state for the orders TUI view.
///
/// Holds all state needed for rendering and event handling of the
/// scrollable order history table. Separate from the chart `App` to
/// keep the orders subcommand cleanly isolated.
pub struct OrdersApp {
    /// Loaded order data from fetch_orders()
    pub orders: Vec<Order>,
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
    /// Active status filter (ALL / OPEN / FILLED / CANCELLED)
    pub status_filter: StatusFilter,
    /// Active side filter (ALL / BUY / SELL)
    pub side_filter: SideFilter,
    /// Channel sender to request an order refresh from the event loop
    pub refresh_tx: Option<mpsc::Sender<()>>,
    /// Timestamp of last successful refresh request (for debounce)
    pub last_refresh: Instant,
    /// WebSocket connection status for status bar display
    pub connection_status: ConnectionStatus,

    // -- Cancel action state (Phase 51) --

    /// Current interaction mode (Normal for read-only browsing, overlays for cancel flow)
    pub mode: OrdersMode,
    /// Snapshot of selected order for confirmation (prevents cancel-wrong-order race)
    pub order_snapshot: Option<OrderSnapshot>,
    /// Result of the last cancel attempt: Ok = success msg, Err = error msg
    pub cancel_result: Option<Result<String, String>>,
    /// Channel to send (symbol, order_id) to event loop for async cancellation
    pub cancel_tx: Option<mpsc::Sender<(String, u64)>>,
    /// Inline error message (e.g., "Only open orders can be cancelled")
    pub error_message: Option<String>,
}

impl OrdersApp {
    /// Create a new OrdersApp with the given symbol, orders, and theme.
    ///
    /// Pass `Some(symbol)` for single-pair mode or `None` for all-pairs mode.
    /// Initializes table selection to the first row if orders are non-empty,
    /// or no selection if the orders list is empty.
    pub fn new(symbol: Option<String>, orders: Vec<Order>, theme: Theme) -> Self {
        let mut table_state = TableState::default();
        if !orders.is_empty() {
            table_state.select(Some(0));
        }

        Self {
            orders,
            table_state,
            should_quit: false,
            symbol,
            theme,
            page_size: 20,
            status_filter: StatusFilter::default(),
            side_filter: SideFilter::default(),
            refresh_tx: None,
            // Initialize to 10 seconds in the past so the first 'r' press is always allowed
            last_refresh: Instant::now() - Duration::from_secs(10),
            connection_status: ConnectionStatus::default(),
            // Cancel action state
            mode: OrdersMode::Normal,
            order_snapshot: None,
            cancel_result: None,
            cancel_tx: None,
            error_message: None,
        }
    }

    // -----------------------------------------------------------------------
    // Key handling: mode-aware dispatch
    // -----------------------------------------------------------------------

    /// Handle keyboard input. Dispatches on current mode FIRST.
    ///
    /// Returns true if the key was handled, false otherwise.
    ///
    /// In overlay modes (Confirming, Submitting, ShowingResult),
    /// ALL keys are captured -- quit keys only work in Normal mode.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match self.mode {
            OrdersMode::Normal => self.handle_key_normal(key),
            OrdersMode::Confirming => self.handle_key_confirming(key),
            OrdersMode::Submitting => true, // absorb all keys while submitting
            OrdersMode::ShowingResult => {
                // Any key dismisses result and returns to Normal
                self.cancel_result = None;
                self.error_message = None;
                self.mode = OrdersMode::Normal;
                true
            }
        }
    }

    /// Handle keys in Normal mode (table browsing, quit, cancel hotkey).
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

        // Filter and refresh keys are always active (even on empty table)
        match (key.code, key.modifiers) {
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                self.status_filter = self.status_filter.cycle();
                self.table_state.select(Some(0));
                return true;
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) => {
                self.side_filter = self.side_filter.cycle();
                self.table_state.select(Some(0));
                return true;
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                if self.last_refresh.elapsed() >= Duration::from_secs(5) {
                    if let Some(tx) = &self.refresh_tx {
                        let _ = tx.try_send(());
                        self.last_refresh = Instant::now();
                    }
                }
                return true;
            }
            _ => {}
        }

        // Navigation keys require non-empty table
        if self.orders.is_empty() {
            return false;
        }

        // Cancel hotkey: 'd' requires selection
        if self.table_state.selected().is_some() {
            if let (KeyCode::Char('d'), KeyModifiers::NONE) = (key.code, key.modifiers) {
                if let Some(snapshot) = self.snapshot_selected() {
                    // Gate on open order status
                    if snapshot.status != OrderStatus::New
                        && snapshot.status != OrderStatus::PartiallyFilled
                    {
                        self.error_message =
                            Some("Only open orders can be cancelled".to_string());
                        self.mode = OrdersMode::ShowingResult;
                        return true;
                    }
                    self.order_snapshot = Some(snapshot);
                    self.error_message = None;
                    self.mode = OrdersMode::Confirming;
                }
                return true;
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

    /// Handle keys in Confirming mode (Y/N/Esc to confirm or cancel).
    fn handle_key_confirming(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.mode = OrdersMode::Normal;
                self.order_snapshot = None;
                self.error_message = None;
                true
            }
            KeyCode::Char('y') | KeyCode::Enter => {
                if let Some(snapshot) = &self.order_snapshot {
                    if let Some(tx) = &self.cancel_tx {
                        let _ = tx.try_send((snapshot.symbol.clone(), snapshot.order_id));
                        self.mode = OrdersMode::Submitting;
                    } else {
                        self.error_message =
                            Some("No cancel channel available".to_string());
                        self.mode = OrdersMode::Normal;
                        self.order_snapshot = None;
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

    /// Snapshot the currently selected order through active filters.
    ///
    /// CRITICAL: Applies status_filter and side_filter before indexing to
    /// prevent filter-index mismatch (Pitfall 2 from research). The table
    /// selection index corresponds to the filtered view, not the raw orders vec.
    pub fn snapshot_selected(&self) -> Option<OrderSnapshot> {
        let idx = self.table_state.selected()?;
        let filtered: Vec<&Order> = self
            .orders
            .iter()
            .filter(|o| self.status_filter.matches(&o.status))
            .filter(|o| self.side_filter.matches(&o.side))
            .collect();
        let order = filtered.get(idx)?;
        Some(OrderSnapshot {
            order_id: order.order_id,
            symbol: order.symbol.clone(),
            side: order.side,
            order_type: order.order_type,
            price: order.price,
            orig_qty: order.orig_qty,
            executed_qty: order.executed_qty,
            status: order.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::order::{OrderSide, OrderStatus, OrderType};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn sample_open_order() -> Order {
        Order {
            order_id: 100,
            symbol: "BTCUSDT".to_string(),
            time: 1700000000000,
            update_time: 1700000001000,
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            price: Decimal::from_str("50000.00").unwrap(),
            orig_qty: Decimal::from_str("0.001").unwrap(),
            executed_qty: Decimal::ZERO,
            avg_price: Decimal::ZERO,
            cum_quote: Decimal::ZERO,
            status: OrderStatus::New,
            realized_pnl: Decimal::ZERO,
            commission: Decimal::ZERO,
        }
    }

    fn sample_order() -> Order {
        Order {
            order_id: 1,
            symbol: "BTCUSDT".to_string(),
            time: 1700000000000,
            update_time: 1700000001000,
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            price: Decimal::from_str("50000.00").unwrap(),
            orig_qty: Decimal::from_str("0.001").unwrap(),
            executed_qty: Decimal::from_str("0.001").unwrap(),
            avg_price: Decimal::from_str("49999.50").unwrap(),
            cum_quote: Decimal::from_str("49.9995").unwrap(),
            status: OrderStatus::Filled,
            realized_pnl: Decimal::ZERO,
            commission: Decimal::ZERO,
        }
    }

    fn test_theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn test_new_with_orders_selects_first() {
        let app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![sample_order()], test_theme());
        assert_eq!(app.table_state.selected(), Some(0));
        assert!(!app.should_quit);
        assert_eq!(app.symbol, Some("BTCUSDT".to_string()));
        assert_eq!(app.page_size, 20);
    }

    #[test]
    fn test_new_empty_no_selection() {
        let app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        assert_eq!(app.table_state.selected(), None);
    }

    #[test]
    fn test_new_all_pairs_mode() {
        let app = OrdersApp::new(None, vec![sample_order()], test_theme());
        assert!(app.symbol.is_none());
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_quit_q() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_esc() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_ctrl_c() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.handle_key(key));
        assert!(app.should_quit);
    }

    #[test]
    fn test_navigation_on_empty_table_returns_false() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(!app.handle_key(key));
    }

    #[test]
    fn test_navigation_down() {
        let orders = vec![sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());
        assert_eq!(app.table_state.selected(), Some(0));

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.table_state.selected(), Some(1));
    }

    #[test]
    fn test_navigation_j() {
        let orders = vec![sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.table_state.selected(), Some(1));
    }

    #[test]
    fn test_navigation_up() {
        let orders = vec![sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());

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
        let orders = vec![sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());

        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(down);

        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(app.handle_key(k));
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_navigation_home_g() {
        let orders = vec![sample_order(), sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());

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
        let orders = vec![sample_order(), sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());

        let g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert!(app.handle_key(g));
        // select_last() sets a sentinel value (usize::MAX) that gets clamped
        // to the actual last row during render. Verify selection is set.
        assert!(app.table_state.selected().is_some());
    }

    // ---- StatusFilter tests ----

    #[test]
    fn test_status_filter_cycle() {
        let f = StatusFilter::All;
        let f = f.cycle();
        assert_eq!(f, StatusFilter::Open);
        let f = f.cycle();
        assert_eq!(f, StatusFilter::Filled);
        let f = f.cycle();
        assert_eq!(f, StatusFilter::Cancelled);
        let f = f.cycle();
        assert_eq!(f, StatusFilter::All);
    }

    #[test]
    fn test_status_filter_labels() {
        assert_eq!(StatusFilter::All.label(), "ALL");
        assert_eq!(StatusFilter::Open.label(), "OPEN");
        assert_eq!(StatusFilter::Filled.label(), "FILLED");
        assert_eq!(StatusFilter::Cancelled.label(), "CANCELLED");
    }

    #[test]
    fn test_status_filter_matches_all() {
        let f = StatusFilter::All;
        assert!(f.matches(&OrderStatus::New));
        assert!(f.matches(&OrderStatus::PartiallyFilled));
        assert!(f.matches(&OrderStatus::Filled));
        assert!(f.matches(&OrderStatus::Canceled));
        assert!(f.matches(&OrderStatus::Rejected));
        assert!(f.matches(&OrderStatus::Expired));
    }

    #[test]
    fn test_status_filter_matches_open() {
        let f = StatusFilter::Open;
        assert!(f.matches(&OrderStatus::New));
        assert!(f.matches(&OrderStatus::PartiallyFilled));
        assert!(!f.matches(&OrderStatus::Filled));
        assert!(!f.matches(&OrderStatus::Canceled));
        assert!(!f.matches(&OrderStatus::Rejected));
        assert!(!f.matches(&OrderStatus::Expired));
    }

    #[test]
    fn test_status_filter_matches_filled() {
        let f = StatusFilter::Filled;
        assert!(!f.matches(&OrderStatus::New));
        assert!(!f.matches(&OrderStatus::PartiallyFilled));
        assert!(f.matches(&OrderStatus::Filled));
        assert!(!f.matches(&OrderStatus::Canceled));
        assert!(!f.matches(&OrderStatus::Rejected));
        assert!(!f.matches(&OrderStatus::Expired));
    }

    #[test]
    fn test_status_filter_matches_cancelled() {
        let f = StatusFilter::Cancelled;
        assert!(!f.matches(&OrderStatus::New));
        assert!(!f.matches(&OrderStatus::PartiallyFilled));
        assert!(!f.matches(&OrderStatus::Filled));
        assert!(f.matches(&OrderStatus::Canceled));
        assert!(!f.matches(&OrderStatus::Rejected));
        assert!(!f.matches(&OrderStatus::Expired));
    }

    // ---- SideFilter tests ----

    #[test]
    fn test_side_filter_cycle() {
        let f = SideFilter::All;
        let f = f.cycle();
        assert_eq!(f, SideFilter::Buy);
        let f = f.cycle();
        assert_eq!(f, SideFilter::Sell);
        let f = f.cycle();
        assert_eq!(f, SideFilter::All);
    }

    #[test]
    fn test_side_filter_labels() {
        assert_eq!(SideFilter::All.label(), "ALL");
        assert_eq!(SideFilter::Buy.label(), "BUY");
        assert_eq!(SideFilter::Sell.label(), "SELL");
    }

    #[test]
    fn test_side_filter_matches_all() {
        let f = SideFilter::All;
        assert!(f.matches(&OrderSide::Buy));
        assert!(f.matches(&OrderSide::Sell));
    }

    #[test]
    fn test_side_filter_matches_buy() {
        let f = SideFilter::Buy;
        assert!(f.matches(&OrderSide::Buy));
        assert!(!f.matches(&OrderSide::Sell));
    }

    #[test]
    fn test_side_filter_matches_sell() {
        let f = SideFilter::Sell;
        assert!(!f.matches(&OrderSide::Buy));
        assert!(f.matches(&OrderSide::Sell));
    }

    // ---- Key handler tests ----

    #[test]
    fn test_key_f_cycles_status_filter() {
        let orders = vec![sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());
        assert_eq!(app.status_filter, StatusFilter::All);

        let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.status_filter, StatusFilter::Open);
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_key_s_cycles_side_filter() {
        let orders = vec![sample_order(), sample_order()];
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), orders, test_theme());
        assert_eq!(app.side_filter, SideFilter::All);

        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.side_filter, SideFilter::Buy);
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_key_f_works_on_empty_table() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        assert_eq!(app.status_filter, StatusFilter::All);

        let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.status_filter, StatusFilter::Open);
    }

    #[test]
    fn test_key_r_without_refresh_tx() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        assert!(app.refresh_tx.is_none());

        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        // Should return true (handled) without panic, even with no refresh_tx
        assert!(app.handle_key(key));
    }

    #[test]
    fn test_key_r_debounce() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        let (tx, mut rx) = mpsc::channel(1);
        app.refresh_tx = Some(tx);

        // First press: last_refresh was initialized to 10s ago, should send
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert!(rx.try_recv().is_ok(), "First refresh should send signal");

        // Second press immediately: within 5s debounce window, should NOT send
        assert!(app.handle_key(key));
        assert!(rx.try_recv().is_err(), "Debounced refresh should not send signal");

        // Simulate time passing (set last_refresh to 10s ago)
        app.last_refresh = Instant::now() - Duration::from_secs(10);
        assert!(app.handle_key(key));
        assert!(rx.try_recv().is_ok(), "Refresh after debounce window should send signal");
    }

    // ---- OrdersMode / cancel state machine tests (Phase 51) ----

    #[test]
    fn test_new_initializes_cancel_state() {
        let app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![sample_order()], test_theme());
        assert_eq!(app.mode, OrdersMode::Normal);
        assert!(app.order_snapshot.is_none());
        assert!(app.cancel_result.is_none());
        assert!(app.cancel_tx.is_none());
        assert!(app.error_message.is_none());
    }

    #[test]
    fn test_d_key_on_open_order_transitions_to_confirming() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );
        assert_eq!(app.mode, OrdersMode::Normal);

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.handle_key(key));

        assert_eq!(app.mode, OrdersMode::Confirming);
        assert!(app.order_snapshot.is_some());
        let snap = app.order_snapshot.as_ref().unwrap();
        assert_eq!(snap.order_id, 100);
        assert_eq!(snap.symbol, "BTCUSDT");
        assert_eq!(snap.side, OrderSide::Buy);
        assert_eq!(snap.order_type, OrderType::Limit);
        assert_eq!(snap.status, OrderStatus::New);
    }

    #[test]
    fn test_d_key_on_filled_order_shows_error() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_order()], // sample_order() has status=Filled
            test_theme(),
        );
        assert_eq!(app.mode, OrdersMode::Normal);

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.handle_key(key));

        assert_eq!(app.mode, OrdersMode::ShowingResult);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("Only open orders"));
        // Should NOT have set snapshot
        assert!(app.order_snapshot.is_none());
    }

    #[test]
    fn test_d_key_on_empty_table_ignored() {
        let mut app = OrdersApp::new(Some("BTCUSDT".to_string()), vec![], test_theme());
        assert_eq!(app.mode, OrdersMode::Normal);

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        // Empty table => orders.is_empty() returns false, 'd' not reached
        assert!(!app.handle_key(key));
        assert_eq!(app.mode, OrdersMode::Normal);
    }

    #[test]
    fn test_confirming_y_sends_cancel_request() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );

        let (tx, mut rx) = mpsc::channel(4);
        app.cancel_tx = Some(tx);

        // Enter Confirming via 'd'
        let d_key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_key(d_key);
        assert_eq!(app.mode, OrdersMode::Confirming);

        // Confirm with 'y'
        let y_key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        app.handle_key(y_key);
        assert_eq!(app.mode, OrdersMode::Submitting);

        // Verify the sent message
        let (symbol, order_id) = rx.try_recv().unwrap();
        assert_eq!(symbol, "BTCUSDT");
        assert_eq!(order_id, 100);
    }

    #[test]
    fn test_confirming_enter_sends_cancel_request() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );

        let (tx, mut rx) = mpsc::channel(4);
        app.cancel_tx = Some(tx);

        let d_key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_key(d_key);

        // Confirm with Enter
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(enter);
        assert_eq!(app.mode, OrdersMode::Submitting);

        let (symbol, order_id) = rx.try_recv().unwrap();
        assert_eq!(symbol, "BTCUSDT");
        assert_eq!(order_id, 100);
    }

    #[test]
    fn test_confirming_n_returns_to_normal() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );

        let d_key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_key(d_key);
        assert_eq!(app.mode, OrdersMode::Confirming);
        assert!(app.order_snapshot.is_some());

        let n_key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        app.handle_key(n_key);
        assert_eq!(app.mode, OrdersMode::Normal);
        assert!(app.order_snapshot.is_none());
    }

    #[test]
    fn test_confirming_esc_returns_to_normal() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );

        let d_key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_key(d_key);
        assert_eq!(app.mode, OrdersMode::Confirming);

        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);
        assert_eq!(app.mode, OrdersMode::Normal);
        assert!(app.order_snapshot.is_none());
    }

    #[test]
    fn test_submitting_absorbs_all_keys() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );
        app.mode = OrdersMode::Submitting;

        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(q));
        assert!(!app.should_quit);
        assert_eq!(app.mode, OrdersMode::Submitting);
    }

    #[test]
    fn test_showing_result_any_key_dismisses() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );
        app.mode = OrdersMode::ShowingResult;
        app.error_message = Some("test error".to_string());
        app.cancel_result = Some(Ok("test success".to_string()));

        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.mode, OrdersMode::Normal);
        assert!(app.cancel_result.is_none());
        assert!(app.error_message.is_none());
    }

    #[test]
    fn test_quit_keys_in_confirming_do_not_quit() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );

        // Enter Confirming mode
        let d_key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_key(d_key);
        assert_eq!(app.mode, OrdersMode::Confirming);

        // 'q' should NOT quit in Confirming mode (absorbed by overlay)
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.handle_key(q));
        assert!(!app.should_quit);
    }

    #[test]
    fn test_confirming_no_cancel_tx_returns_to_normal_with_error() {
        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![sample_open_order()],
            test_theme(),
        );
        // No cancel_tx set

        let d_key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_key(d_key);
        assert_eq!(app.mode, OrdersMode::Confirming);

        // Y with no channel
        let y_key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        app.handle_key(y_key);

        assert_eq!(app.mode, OrdersMode::Normal);
        assert!(app.error_message.is_some());
        assert!(app.error_message.as_ref().unwrap().contains("channel"));
    }

    #[test]
    fn test_snapshot_selected_applies_filters() {
        // Create mixed orders: Open BUY, Filled SELL, Open SELL
        let mut open_buy = sample_open_order(); // id=100, New, Buy
        open_buy.order_id = 100;

        let mut filled_sell = sample_order(); // id=1, Filled, Buy
        filled_sell.order_id = 200;
        filled_sell.side = OrderSide::Sell;

        let mut open_sell = sample_open_order();
        open_sell.order_id = 300;
        open_sell.side = OrderSide::Sell;

        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![open_buy, filled_sell, open_sell],
            test_theme(),
        );

        // With StatusFilter::Open, only orders 100 (New, Buy) and 300 (New, Sell) are visible
        app.status_filter = StatusFilter::Open;
        app.table_state.select(Some(1)); // second in filtered view = order 300

        let snap = app.snapshot_selected().unwrap();
        assert_eq!(snap.order_id, 300, "Filtered index 1 should be order 300 (open sell)");
        assert_eq!(snap.side, OrderSide::Sell);
        assert_eq!(snap.status, OrderStatus::New);
    }

    #[test]
    fn test_snapshot_selected_with_side_filter() {
        let mut open_buy = sample_open_order();
        open_buy.order_id = 100;

        let mut open_sell = sample_open_order();
        open_sell.order_id = 200;
        open_sell.side = OrderSide::Sell;

        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![open_buy, open_sell],
            test_theme(),
        );

        // Filter to SELL only
        app.side_filter = SideFilter::Sell;
        app.table_state.select(Some(0)); // first in filtered view = order 200

        let snap = app.snapshot_selected().unwrap();
        assert_eq!(snap.order_id, 200, "Filtered index 0 with SELL filter should be order 200");
        assert_eq!(snap.side, OrderSide::Sell);
    }

    #[test]
    fn test_d_key_on_partially_filled_order_transitions_to_confirming() {
        let mut partially = sample_open_order();
        partially.status = OrderStatus::PartiallyFilled;
        partially.executed_qty = Decimal::from_str("0.0005").unwrap();

        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![partially],
            test_theme(),
        );

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.mode, OrdersMode::Confirming);
        assert!(app.order_snapshot.is_some());
    }

    #[test]
    fn test_d_key_on_cancelled_order_shows_error() {
        let mut cancelled = sample_open_order();
        cancelled.status = OrderStatus::Canceled;

        let mut app = OrdersApp::new(
            Some("BTCUSDT".to_string()),
            vec![cancelled],
            test_theme(),
        );

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.handle_key(key));
        assert_eq!(app.mode, OrdersMode::ShowingResult);
        assert!(app.error_message.as_ref().unwrap().contains("Only open orders"));
    }
}
