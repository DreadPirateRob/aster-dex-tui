// src/tui/dom_app.rs
// DOM (Depth of Market) application state

use crate::data::heatmap_tracker::HeatmapTracker;
use crate::data::order::{Order, OrderSide, OrderStatus, OrderType, TimeInForce};
use crate::data::order_book::OrderBook;
use crate::data::trade_imbalance::TradeImbalance;
use crate::data::trade_volume_profile::TradeVolumeProfile;
use crate::network::asterdex_exchange_info::{validate_filters, SymbolFilter};
use crate::network::asterdex_stream_types::PositionUpdateData;
use crate::network::{ConnectionStatus, OrderParams};
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyModifiers};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tick size multipliers for DOM ladder cycling.
/// Applied to the exchange's native tick size (base_tick_size).
/// e.g., if base is 0.01: [0.01, 0.02, 0.05, 0.10, 0.25, 0.50, 1.00, 2.00, 5.00, 10.00]
const TICK_MULTIPLIERS: &[u32] = &[1, 2, 5, 10, 25, 50, 100, 200, 500, 1000];
use tokio::sync::mpsc;

/// Per-price cooldown to prevent rapid duplicate submissions at the same price level.
const ORDER_COOLDOWN: Duration = Duration::from_millis(500);

/// A working (open) order at a price level, displayed on the DOM ladder.
pub struct WorkingOrder {
    pub order_id: u64,
    pub side: OrderSide,
    pub price: Decimal,
    pub orig_qty: Decimal,
    #[allow(dead_code)]
    pub order_type: OrderType,
}

/// Interaction mode for the DOM: normal trading, confirmation prompt, or symbol input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomMode {
    Normal,
    ConfirmCancel(u64),    // order_id to cancel
    ConfirmCancelAll,
    ConfirmFlatten,
    SymbolInput(String),   // character buffer for pair switch input
}

/// Request sent from DomApp key handler to the event loop for cancel execution.
pub enum CancelRequest {
    Single(u64),   // order_id
    All,
}

/// Volume profile display mode: source of volume data.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VolumeProfileMode {
    /// Volume from aggTrade stream over rolling window
    TradeVolume,
    /// Volume from current order book snapshot (resting depth)
    RestingDepth,
}

pub struct DomApp {
    pub symbol: String,
    pub theme: Theme,
    pub should_quit: bool,
    pub order_book: OrderBook,
    pub tick_size: Decimal,
    pub center_price: Option<Decimal>,
    pub visible_rows: u16,
    #[allow(dead_code)] // Available for future connection status display
    pub connection_status: ConnectionStatus,
    /// Signed offset from center row (positive = up/higher price, negative = down/lower price)
    pub cursor_offset: i16,
    /// Index 0-3 for F1-F4 quick-size presets (25%/50%/75%/100%), None if unset
    pub selected_qty_preset: Option<usize>,
    /// Status message displayed in the bottom status bar
    pub status_message: Option<String>,
    /// Timestamp when status_message was last set (for auto-dismiss)
    pub status_message_time: Option<Instant>,
    /// Channel to send order submissions to the event loop for async execution
    pub submit_tx: Option<mpsc::Sender<OrderParams>>,
    /// Per-price cooldown tracker: maps price -> last submission time
    pub cooldowns: HashMap<Decimal, Instant>,
    /// True while an order is being submitted (blocks new submissions)
    pub order_in_flight: bool,
    /// Available USDT balance for quantity computation
    pub available_balance: Option<Decimal>,
    /// Current leverage multiplier for quantity computation
    pub leverage: Option<u32>,
    /// Quantity decimal precision from exchange info
    pub quantity_precision: u32,
    /// Exchange symbol filters for pre-submission validation
    pub filters: Vec<SymbolFilter>,
    /// Whether API credentials are present (false = read-only mode)
    pub credentials_present: bool,
    /// Whether bracket mode is active (SL/TP ghost markers shown, Shift+B/S places bracket)
    pub bracket_mode: bool,
    /// Stop-loss distance in ticks from entry price (always positive, minimum 1)
    pub sl_offset_ticks: i16,
    /// Take-profit distance in ticks from entry price (always positive, minimum 1)
    pub tp_offset_ticks: i16,
    /// Bracket context stored after entry submission, consumed by event loop to spawn SL/TP.
    pub pending_bracket: Option<PendingBracket>,
    /// Current interaction mode (Normal, or confirmation prompt)
    pub mode: DomMode,
    /// Working orders indexed by price for O(1) ladder lookup
    pub working_orders: HashMap<Decimal, Vec<WorkingOrder>>,
    /// Current position entry price (None if flat)
    pub position_entry_price: Option<Decimal>,
    /// Current position quantity (signed: positive=long, negative=short)
    pub position_qty: Decimal,
    /// Channel to send cancel requests to event loop
    pub cancel_tx: Option<mpsc::Sender<CancelRequest>>,
    /// Channel to send flatten requests to event loop
    pub flatten_tx: Option<mpsc::Sender<()>>,
    /// Whether heatmap overlay is active on DOM ladder
    pub show_heatmap: bool,
    /// Whether volume profile overlay is active (placeholder for Plan 02)
    pub show_volume_profile: bool,
    /// Whether cumulative depth overlay is active (placeholder for Plan 02)
    pub show_cumulative_depth: bool,
    /// Heatmap rolling-window quantity tracker
    pub heatmap_tracker: HeatmapTracker,
    /// Current volume profile mode (trade volume vs resting depth)
    pub volume_profile_mode: VolumeProfileMode,
    /// Trade volume accumulator for volume profile overlay
    pub trade_volume_profile: TradeVolumeProfile,
    /// Multi-window trade imbalance accumulator for flow imbalance panel
    pub trade_imbalance: TradeImbalance,
    /// Whether flow imbalance overlay is active (toggled with 'i')
    pub show_flow_imbalance: bool,
    /// Exchange's native minimum tick size (immutable after init)
    pub base_tick_size: Decimal,
    /// Current index into TICK_MULTIPLIERS for tick size cycling
    pub tick_multiplier_index: usize,
    /// Flag set by SymbolInput mode when user confirms a new symbol; consumed by event loop
    pub pair_switch_requested: Option<String>,
}

/// Bracket context stored after entry submission, consumed by event loop to spawn SL/TP.
pub struct PendingBracket {
    pub sl_price: Decimal,
    pub tp_price: Decimal,
    pub quantity: Decimal,
    pub entry_side: OrderSide,
}

impl DomApp {
    pub fn new(symbol: String, theme: Theme, tick_size: Decimal, quantity_precision: u32, filters: Vec<SymbolFilter>) -> Self {
        Self {
            symbol,
            theme,
            should_quit: false,
            order_book: OrderBook::new(),
            tick_size,
            center_price: None,
            visible_rows: 40, // default, updated from terminal height
            connection_status: ConnectionStatus::default(),
            cursor_offset: 0,
            selected_qty_preset: None,
            status_message: None,
            status_message_time: None,
            submit_tx: None,
            cooldowns: HashMap::new(),
            order_in_flight: false,
            available_balance: None,
            leverage: None,
            quantity_precision,
            filters,
            credentials_present: false,
            bracket_mode: false,
            sl_offset_ticks: 10,
            tp_offset_ticks: 20,
            pending_bracket: None,
            mode: DomMode::Normal,
            working_orders: HashMap::new(),
            position_entry_price: None,
            position_qty: Decimal::ZERO,
            cancel_tx: None,
            flatten_tx: None,
            show_heatmap: false,
            show_volume_profile: false,
            show_cumulative_depth: false,
            heatmap_tracker: HeatmapTracker::new(),
            volume_profile_mode: VolumeProfileMode::TradeVolume,
            trade_volume_profile: TradeVolumeProfile::new(),
            trade_imbalance: TradeImbalance::new(),
            show_flow_imbalance: false,
            base_tick_size: tick_size,
            tick_multiplier_index: 0, // 1x = exchange native tick size
            pair_switch_requested: None,
        }
    }

    /// Move cursor up one row (higher price).
    pub fn move_cursor_up(&mut self) {
        let limit = self.visible_rows.saturating_sub(1) as i16;
        if self.cursor_offset < limit {
            self.cursor_offset += 1;
        }
    }

    /// Move cursor down one row (lower price).
    pub fn move_cursor_down(&mut self) {
        let limit = self.visible_rows.saturating_sub(1) as i16;
        if self.cursor_offset > -limit {
            self.cursor_offset -= 1;
        }
    }

    /// Re-center the ladder on the current mid-price and reset cursor to center.
    pub fn recenter(&mut self) {
        if let Some(mid) = self.order_book.mid_price() {
            self.center_price = Some(mid);
            self.cursor_offset = 0;
        }
    }

    /// Cycle tick size forward (larger). Wraps around to 1x.
    pub fn cycle_tick_size_forward(&mut self) {
        let next = self.tick_multiplier_index + 1;
        self.tick_multiplier_index = if next >= TICK_MULTIPLIERS.len() { 0 } else { next };
        self.tick_size = self.base_tick_size * Decimal::from(TICK_MULTIPLIERS[self.tick_multiplier_index]);
        self.cursor_offset = 0;
    }

    /// Cycle tick size backward (smaller). Wraps around to largest.
    pub fn cycle_tick_size_backward(&mut self) {
        self.tick_multiplier_index = if self.tick_multiplier_index == 0 {
            TICK_MULTIPLIERS.len() - 1
        } else {
            self.tick_multiplier_index - 1
        };
        self.tick_size = self.base_tick_size * Decimal::from(TICK_MULTIPLIERS[self.tick_multiplier_index]);
        self.cursor_offset = 0;
    }

    /// Compute the price at the current cursor position.
    /// Returns None if center_price is not set.
    pub fn cursor_price(&self) -> Option<Decimal> {
        self.center_price.map(|cp| {
            let aligned = (cp / self.tick_size).floor() * self.tick_size;
            aligned + Decimal::from(self.cursor_offset as i64) * self.tick_size
        })
    }

    /// Infer the order side from cursor position relative to the mid-price.
    /// Cursor above mid-price → Sell, at or below → Buy.
    pub fn inferred_side(&self) -> OrderSide {
        if let (Some(cursor_p), Some(mid)) = (self.cursor_price(), self.order_book.mid_price()) {
            if cursor_p > mid {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            }
        } else {
            OrderSide::Buy
        }
    }

    /// Clamp cursor_offset to the visible range. Called on terminal resize.
    pub fn clamp_cursor(&mut self) {
        let limit = self.visible_rows.saturating_sub(1) as i16;
        self.cursor_offset = self.cursor_offset.clamp(-limit, limit);
    }

    /// Compute the absolute SL price for a bracket order given entry price and side.
    /// For Buy: SL is below entry (entry - offset * tick_size).
    /// For Sell: SL is above entry (entry + offset * tick_size).
    pub fn bracket_sl_price(&self, entry_price: Decimal, side: OrderSide) -> Decimal {
        let offset = Decimal::from(self.sl_offset_ticks.abs() as i64) * self.tick_size;
        match side {
            OrderSide::Buy => entry_price - offset,
            OrderSide::Sell => entry_price + offset,
        }
    }

    /// Compute the absolute TP price for a bracket order given entry price and side.
    /// For Buy: TP is above entry (entry + offset * tick_size).
    /// For Sell: TP is below entry (entry - offset * tick_size).
    pub fn bracket_tp_price(&self, entry_price: Decimal, side: OrderSide) -> Decimal {
        let offset = Decimal::from(self.tp_offset_ticks.abs() as i64) * self.tick_size;
        match side {
            OrderSide::Buy => entry_price + offset,
            OrderSide::Sell => entry_price - offset,
        }
    }

    /// Update working_orders map from an ORDER_TRADE_UPDATE event.
    /// Removes stale entries, inserts/updates working orders, cleans empty price levels.
    pub fn update_working_orders(&mut self, order: &Order) {
        // Remove any existing entry with this order_id from all price levels
        for orders_at_price in self.working_orders.values_mut() {
            orders_at_price.retain(|o| o.order_id != order.order_id);
        }
        // Clean empty price levels
        self.working_orders.retain(|_, orders| !orders.is_empty());

        // If still working (New or PartiallyFilled), insert at current price
        match order.status {
            OrderStatus::New | OrderStatus::PartiallyFilled => {
                let wo = WorkingOrder {
                    order_id: order.order_id,
                    side: order.side,
                    price: order.price,
                    orig_qty: order.orig_qty,
                    order_type: order.order_type,
                };
                self.working_orders.entry(order.price).or_default().push(wo);
            }
            _ => {} // Filled, Canceled, Expired -- already removed
        }
    }

    /// Update position state from ACCOUNT_UPDATE position deltas.
    /// Filters by symbol (per project memory: ACCOUNT_UPDATE is account-wide).
    /// Hedge-mode aware: prefers the active (non-zero) side; only clears when
    /// ALL sides for this symbol are zero.
    pub fn update_position_from_ws(&mut self, positions: &[PositionUpdateData]) {
        let mut found_active = false;
        for pos in positions {
            if pos.symbol.eq_ignore_ascii_case(&self.symbol) && pos.position_amt != Decimal::ZERO {
                self.position_qty = pos.position_amt;
                self.position_entry_price = Some(pos.entry_price);
                found_active = true;
                break; // only one side can be active at a time in practice
            }
        }
        // If no active side found for our symbol, position is truly flat
        if !found_active {
            let symbol_present = positions.iter().any(|p| p.symbol.eq_ignore_ascii_case(&self.symbol));
            if symbol_present {
                self.position_qty = Decimal::ZERO;
                self.position_entry_price = None;
            }
        }
    }

    /// Set a status message with auto-dismiss timestamp.
    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_message_time = Some(Instant::now());
    }

    /// Reset all symbol-dependent state for a pair switch while preserving user preferences.
    /// Called by the event loop after async symbol validation succeeds.
    pub fn switch_symbol(
        &mut self,
        new_symbol: String,
        tick_size: Decimal,
        quantity_precision: u32,
        filters: Vec<SymbolFilter>,
    ) {
        self.symbol = new_symbol;
        self.order_book = OrderBook::new();
        self.center_price = None;
        self.cursor_offset = 0;
        self.tick_size = tick_size;
        self.base_tick_size = tick_size;
        self.tick_multiplier_index = 0;
        self.quantity_precision = quantity_precision;
        self.filters = filters;
        self.heatmap_tracker = HeatmapTracker::new();
        self.trade_volume_profile = TradeVolumeProfile::new();
        self.trade_imbalance = TradeImbalance::new();
        self.working_orders.clear();
        self.position_entry_price = None;
        self.position_qty = Decimal::ZERO;
        self.leverage = None;
        self.cooldowns.clear();
        self.order_in_flight = false;
        self.pending_bracket = None;
        self.mode = DomMode::Normal;
        self.status_message = None;
        self.status_message_time = None;
    }

    /// Returns the symbol input buffer if in SymbolInput mode, for UI rendering.
    pub fn symbol_input_buffer(&self) -> Option<&str> {
        if let DomMode::SymbolInput(ref buffer) = self.mode {
            Some(buffer.as_str())
        } else {
            None
        }
    }

    /// Attempt to place a limit order at the current cursor price.
    ///
    /// Implements the full guard chain:
    /// 1. Credentials check (read-only mode)
    /// 2. Quantity preset selected
    /// 3. Price available (center_price set)
    /// 4. No order in flight
    /// 5. Per-price cooldown (500ms)
    /// 6. Quantity computation from balance/leverage/preset
    /// 7. Exchange filter validation
    /// 8. Build and send OrderParams
    pub fn try_place_limit_order(&mut self, side: OrderSide) {
        // 1. Credentials guard
        if !self.credentials_present {
            self.set_status("No API credentials (read-only mode)");
            return;
        }

        // 2. Quantity preset guard
        let preset_idx = match self.selected_qty_preset {
            Some(idx) => idx,
            None => {
                self.set_status("Press F1-F4 to select size first");
                return;
            }
        };

        // 3. Price guard
        let price = match self.cursor_price() {
            Some(p) => p,
            None => {
                self.set_status("No price available");
                return;
            }
        };

        // 4. In-flight guard
        if self.order_in_flight {
            self.set_status("Order in progress...");
            return;
        }

        // 5. Per-price cooldown guard
        self.cooldowns.retain(|_, t| t.elapsed() >= ORDER_COOLDOWN);
        if let Some(t) = self.cooldowns.get(&price) {
            if t.elapsed() < ORDER_COOLDOWN {
                self.set_status("Cooldown active");
                return;
            }
        }

        // 6. Quantity computation
        let pct = match preset_idx {
            0 => Decimal::new(25, 2),  // 25%
            1 => Decimal::new(50, 2),  // 50%
            2 => Decimal::new(75, 2),  // 75%
            _ => Decimal::ONE,         // 100%
        };

        let balance = match self.available_balance {
            Some(b) => b,
            None => {
                self.set_status("Balance not loaded yet");
                return;
            }
        };

        let leverage = Decimal::from(self.leverage.unwrap_or(1));
        let raw_qty = (balance * pct * leverage) / price;
        let quantity = raw_qty.round_dp(self.quantity_precision);

        if quantity <= Decimal::ZERO {
            self.set_status("Computed quantity is zero");
            return;
        }

        // 7. Exchange filter validation
        if let Err(msg) = validate_filters(&self.filters, quantity, Some(price), Some(quantity * price)) {
            self.set_status(msg);
            return;
        }

        // 8. Build and send order
        let params = OrderParams::Limit {
            symbol: self.symbol.clone(),
            side,
            quantity,
            price,
            time_in_force: TimeInForce::Gtc,
        };

        if let Some(ref tx) = self.submit_tx {
            match tx.try_send(params) {
                Ok(()) => {
                    self.cooldowns.insert(price, Instant::now());
                    self.order_in_flight = true;
                    self.set_status(format!("{} {} @ {} submitted", side, quantity, price));
                }
                Err(_) => {
                    self.set_status("Submission channel full");
                }
            }
        }
    }

    /// Attempt to place a bracket order (entry limit + SL + TP) at the current cursor price.
    ///
    /// Reuses the same guard chain as try_place_limit_order (steps 1-7) and adds
    /// bracket-specific validation: SL/TP price filter checks and side price ordering.
    /// On success, submits the entry leg and stores PendingBracket for the event loop
    /// to spawn SL/TP after the entry REST response succeeds.
    pub fn try_place_bracket_order(&mut self, side: OrderSide) {
        // 1. Credentials guard
        if !self.credentials_present {
            self.set_status("No API credentials (read-only mode)");
            return;
        }

        // 2. Quantity preset guard
        let preset_idx = match self.selected_qty_preset {
            Some(idx) => idx,
            None => {
                self.set_status("Press F1-F4 to select size first");
                return;
            }
        };

        // 3. Price guard (entry price)
        let price = match self.cursor_price() {
            Some(p) => p,
            None => {
                self.set_status("No price available");
                return;
            }
        };

        // 4. In-flight guard
        if self.order_in_flight {
            self.set_status("Order in progress...");
            return;
        }

        // 5. Per-price cooldown guard
        self.cooldowns.retain(|_, t| t.elapsed() >= ORDER_COOLDOWN);
        if let Some(t) = self.cooldowns.get(&price) {
            if t.elapsed() < ORDER_COOLDOWN {
                self.set_status("Cooldown active");
                return;
            }
        }

        // 6. Quantity computation
        let pct = match preset_idx {
            0 => Decimal::new(25, 2),  // 25%
            1 => Decimal::new(50, 2),  // 50%
            2 => Decimal::new(75, 2),  // 75%
            _ => Decimal::ONE,         // 100%
        };

        let balance = match self.available_balance {
            Some(b) => b,
            None => {
                self.set_status("Balance not loaded yet");
                return;
            }
        };

        let leverage = Decimal::from(self.leverage.unwrap_or(1));
        let raw_qty = (balance * pct * leverage) / price;
        let quantity = raw_qty.round_dp(self.quantity_precision);

        if quantity <= Decimal::ZERO {
            self.set_status("Computed quantity is zero");
            return;
        }

        // 7. Entry filter validation
        if let Err(msg) = validate_filters(&self.filters, quantity, Some(price), Some(quantity * price)) {
            self.set_status(msg);
            return;
        }

        // 8. Compute SL and TP prices
        let sl_price = self.bracket_sl_price(price, side);
        let tp_price = self.bracket_tp_price(price, side);

        // 9. Validate SL price against exchange filters
        if let Err(msg) = validate_filters(&self.filters, quantity, Some(sl_price), Some(quantity * sl_price)) {
            self.set_status(format!("SL: {}", msg));
            return;
        }

        // 10. Validate TP price against exchange filters
        if let Err(msg) = validate_filters(&self.filters, quantity, Some(tp_price), Some(quantity * tp_price)) {
            self.set_status(format!("TP: {}", msg));
            return;
        }

        // 11. Side price ordering validation
        match side {
            OrderSide::Buy => {
                if !(sl_price < price && price < tp_price) {
                    self.set_status("SL must be below entry, TP must be above entry for Buy");
                    return;
                }
            }
            OrderSide::Sell => {
                if !(tp_price < price && price < sl_price) {
                    self.set_status("TP must be below entry, SL must be above entry for Sell");
                    return;
                }
            }
        }

        // 12. Build and submit entry leg
        let params = OrderParams::Limit {
            symbol: self.symbol.clone(),
            side,
            quantity,
            price,
            time_in_force: TimeInForce::Gtc,
        };

        if let Some(ref tx) = self.submit_tx {
            match tx.try_send(params) {
                Ok(()) => {
                    self.cooldowns.insert(price, Instant::now());
                    self.order_in_flight = true;
                    self.set_status(format!(
                        "Bracket {} @ {}: entry submitted (SL:{} TP:{})",
                        side, price, sl_price, tp_price
                    ));
                    self.bracket_mode = false; // offsets preserved for quick re-enable
                    self.pending_bracket = Some(PendingBracket {
                        sl_price,
                        tp_price,
                        quantity,
                        entry_side: side,
                    });
                }
                Err(_) => {
                    self.set_status("Submission channel full");
                }
            }
        }
    }

    /// Handle keyboard input.
    /// Ctrl+C must be matched BEFORE plain 'c' to avoid re-center on quit.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // If in symbol input mode, handle text entry
        if let DomMode::SymbolInput(ref mut buffer) = self.mode {
            match key.code {
                KeyCode::Char(c) => {
                    if buffer.len() < 20 {
                        buffer.push(c.to_ascii_uppercase());
                    }
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Enter => {
                    if !buffer.is_empty() {
                        self.pair_switch_requested = Some(buffer.clone());
                    }
                    self.mode = DomMode::Normal;
                }
                KeyCode::Esc => {
                    self.mode = DomMode::Normal;
                    self.set_status("Symbol switch cancelled");
                }
                _ => {}
            }
            return;
        }

        // If in confirmation mode, handle Y/N/Esc only
        if self.mode != DomMode::Normal {
            match (key.code, key.modifiers) {
                (KeyCode::Char('y'), KeyModifiers::NONE) | (KeyCode::Char('Y'), KeyModifiers::SHIFT) => {
                    match self.mode {
                        DomMode::ConfirmCancel(order_id) => {
                            if let Some(ref tx) = self.cancel_tx {
                                let _ = tx.try_send(CancelRequest::Single(order_id));
                                self.set_status(format!("Cancelling order #{}...", order_id));
                            }
                        }
                        DomMode::ConfirmCancelAll => {
                            if let Some(ref tx) = self.cancel_tx {
                                let _ = tx.try_send(CancelRequest::All);
                                self.set_status("Cancelling all orders...");
                            }
                        }
                        DomMode::ConfirmFlatten => {
                            if let Some(ref tx) = self.flatten_tx {
                                let _ = tx.try_send(());
                                self.set_status("Flattening position...");
                            }
                        }
                        DomMode::Normal | DomMode::SymbolInput(_) => {} // unreachable
                    }
                    self.mode = DomMode::Normal;
                    return;
                }
                (KeyCode::Char('n'), KeyModifiers::NONE) | (KeyCode::Esc, _) => {
                    self.mode = DomMode::Normal;
                    self.set_status("Cancelled");
                    return;
                }
                _ => return, // Ignore all other keys while confirming
            }
        }

        match (key.code, key.modifiers) {
            // Quit (Ctrl+C must be before plain 'c')
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.should_quit = true;
            }
            (KeyCode::Esc, _) => {
                self.should_quit = true;
            }

            // Symbol input mode (pair switching)
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.mode = DomMode::SymbolInput(String::new());
                self.set_status("Enter symbol: ");
            }

            // Cursor navigation (arrow keys + vim-style j/k)
            (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.move_cursor_up();
            }
            (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.move_cursor_down();
            }

            // Context-sensitive: cancel order at cursor if one exists, otherwise recenter
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                if let Some(price) = self.cursor_price() {
                    if let Some(orders) = self.working_orders.get(&price) {
                        if let Some(first) = orders.first() {
                            self.mode = DomMode::ConfirmCancel(first.order_id);
                            self.set_status(format!(
                                "Cancel {} {} @ {}? (y/n)",
                                first.side, first.orig_qty, first.price
                            ));
                            return;
                        }
                    }
                }
                self.recenter();
            }

            // Quick-size presets (F1-F4)
            (KeyCode::F(1), _) => {
                self.selected_qty_preset = Some(0);
            }
            (KeyCode::F(2), _) => {
                self.selected_qty_preset = Some(1);
            }
            (KeyCode::F(3), _) => {
                self.selected_qty_preset = Some(2);
            }
            (KeyCode::F(4), _) => {
                self.selected_qty_preset = Some(3);
            }

            // Heatmap overlay toggle
            (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.show_heatmap = !self.show_heatmap;
            }

            // Volume profile overlay toggle
            (KeyCode::Char('v'), KeyModifiers::NONE) => {
                self.show_volume_profile = !self.show_volume_profile;
            }

            // Cumulative depth overlay toggle
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                self.show_cumulative_depth = !self.show_cumulative_depth;
            }

            // Flow imbalance overlay toggle
            (KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.show_flow_imbalance = !self.show_flow_imbalance;
                self.clamp_cursor();
            }

            // Bracket mode toggle
            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                self.bracket_mode = !self.bracket_mode;
            }

            // SL offset adjustment: [ decrease, ] increase
            (KeyCode::Char('['), KeyModifiers::NONE) => {
                self.sl_offset_ticks = (self.sl_offset_ticks - 1).max(1);
            }
            (KeyCode::Char(']'), KeyModifiers::NONE) => {
                self.sl_offset_ticks += 1;
            }

            // TP offset adjustment: { decrease, } increase
            (KeyCode::Char('{'), _) => {
                self.tp_offset_ticks = (self.tp_offset_ticks - 1).max(1);
            }
            (KeyCode::Char('}'), _) => {
                self.tp_offset_ticks += 1;
            }

            // Volume profile mode cycling (Shift+V) -- must be before modifiers==NONE guard
            (KeyCode::Char('V'), KeyModifiers::SHIFT) => {
                if self.show_volume_profile {
                    self.volume_profile_mode = match self.volume_profile_mode {
                        VolumeProfileMode::TradeVolume => VolumeProfileMode::RestingDepth,
                        VolumeProfileMode::RestingDepth => VolumeProfileMode::TradeVolume,
                    };
                }
            }

            // Cancel all working orders (Shift+X) -- must be before modifiers==NONE guard
            (KeyCode::Char('X'), KeyModifiers::SHIFT) => {
                if self.working_orders.is_empty() {
                    self.set_status("No working orders to cancel");
                } else {
                    let count = self.working_orders.values().map(|v| v.len()).sum::<usize>();
                    self.mode = DomMode::ConfirmCancelAll;
                    self.set_status(format!("Cancel ALL {} working orders? (y/n)", count));
                }
            }

            // Bracket buy/sell at cursor price (requires bracket mode)
            (KeyCode::Char('B'), KeyModifiers::SHIFT) => {
                if self.bracket_mode {
                    self.try_place_bracket_order(OrderSide::Buy);
                } else {
                    self.set_status("Enable bracket mode first (press 't')");
                }
            }
            (KeyCode::Char('S'), KeyModifiers::SHIFT) => {
                if self.bracket_mode {
                    self.try_place_bracket_order(OrderSide::Sell);
                } else {
                    self.set_status("Enable bracket mode first (press 't')");
                }
            }

            // Flatten position
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                if !self.credentials_present {
                    self.set_status("No API credentials (read-only mode)");
                } else if self.position_qty == Decimal::ZERO {
                    self.set_status("No position to flatten");
                } else {
                    let side_label = if self.position_qty > Decimal::ZERO { "LONG" } else { "SHORT" };
                    self.mode = DomMode::ConfirmFlatten;
                    self.set_status(format!(
                        "Flatten {} {} {}? (y/n)",
                        side_label, self.position_qty.abs(), self.symbol
                    ));
                }
            }

            // Buy/sell limit at cursor price
            (KeyCode::Char('b'), KeyModifiers::NONE) => {
                self.try_place_limit_order(OrderSide::Buy);
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) => {
                self.try_place_limit_order(OrderSide::Sell);
            }

            // Tick size cycling (+/- or =/-)
            (KeyCode::Char('+'), _) | (KeyCode::Char('='), _) => {
                self.cycle_tick_size_forward();
                self.set_status(format!("Tick size: {}", self.tick_size));
            }
            (KeyCode::Char('-'), _) => {
                self.cycle_tick_size_backward();
                self.set_status(format!("Tick size: {}", self.tick_size));
            }

            // No-op for unhandled keys
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_app(symbol: &str) -> DomApp {
        DomApp::new(symbol.to_string(), Theme::dark(), dec!(0.01), 2, vec![])
    }

    fn make_position(symbol: &str, side: &str, amt: Decimal, entry: Decimal) -> PositionUpdateData {
        PositionUpdateData {
            symbol: symbol.to_string(),
            position_amt: amt,
            entry_price: entry,
            accumulated_realized: Decimal::ZERO,
            unrealized_pnl: Decimal::ZERO,
            margin_type: "crossed".to_string(),
            isolated_wallet: Decimal::ZERO,
            position_side: side.to_string(),
        }
    }

    // --- Bug 1 regression tests: hedge-mode ACCOUNT_UPDATE overwrite ---

    #[test]
    fn hedge_mode_long_then_empty_short_keeps_position() {
        let mut app = make_app("SOLUSDT");
        // Hedge mode: LONG active, SHORT empty — LONG appears first
        let positions = vec![
            make_position("SOLUSDT", "LONG", dec!(6.74), dec!(86.23)),
            make_position("SOLUSDT", "SHORT", dec!(0), dec!(0)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, dec!(6.74));
        assert_eq!(app.position_entry_price, Some(dec!(86.23)));
    }

    #[test]
    fn hedge_mode_empty_short_then_long_keeps_position() {
        let mut app = make_app("SOLUSDT");
        // Hedge mode: SHORT empty appears FIRST, LONG active second
        let positions = vec![
            make_position("SOLUSDT", "SHORT", dec!(0), dec!(0)),
            make_position("SOLUSDT", "LONG", dec!(6.74), dec!(86.23)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, dec!(6.74));
        assert_eq!(app.position_entry_price, Some(dec!(86.23)));
    }

    #[test]
    fn hedge_mode_both_sides_zero_clears_position() {
        let mut app = make_app("SOLUSDT");
        // Start with an active position
        app.position_qty = dec!(6.74);
        app.position_entry_price = Some(dec!(86.23));
        // Both sides close
        let positions = vec![
            make_position("SOLUSDT", "LONG", dec!(0), dec!(0)),
            make_position("SOLUSDT", "SHORT", dec!(0), dec!(0)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, Decimal::ZERO);
        assert_eq!(app.position_entry_price, None);
    }

    #[test]
    fn hedge_mode_other_symbol_ignored() {
        let mut app = make_app("SOLUSDT");
        app.position_qty = dec!(6.74);
        app.position_entry_price = Some(dec!(86.23));
        // Update for a different symbol should not touch our state
        let positions = vec![
            make_position("BTCUSDT", "LONG", dec!(0), dec!(0)),
            make_position("BTCUSDT", "SHORT", dec!(0), dec!(0)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, dec!(6.74));
        assert_eq!(app.position_entry_price, Some(dec!(86.23)));
    }

    #[test]
    fn hedge_mode_mixed_symbols_filters_correctly() {
        let mut app = make_app("SOLUSDT");
        // Account-wide update with 3 symbols, our symbol has active LONG
        let positions = vec![
            make_position("BTCUSDT", "LONG", dec!(0.1), dec!(95000)),
            make_position("BTCUSDT", "SHORT", dec!(0), dec!(0)),
            make_position("SOLUSDT", "SHORT", dec!(0), dec!(0)),
            make_position("SOLUSDT", "LONG", dec!(6.74), dec!(86.23)),
            make_position("ETHUSDT", "LONG", dec!(0), dec!(0)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, dec!(6.74));
        assert_eq!(app.position_entry_price, Some(dec!(86.23)));
    }

    #[test]
    fn one_way_mode_single_entry_works() {
        let mut app = make_app("SOLUSDT");
        // One-way mode: single BOTH entry
        let positions = vec![
            make_position("SOLUSDT", "BOTH", dec!(6.74), dec!(86.23)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, dec!(6.74));
        assert_eq!(app.position_entry_price, Some(dec!(86.23)));
    }

    #[test]
    fn one_way_mode_zero_clears() {
        let mut app = make_app("SOLUSDT");
        app.position_qty = dec!(6.74);
        app.position_entry_price = Some(dec!(86.23));
        let positions = vec![
            make_position("SOLUSDT", "BOTH", dec!(0), dec!(0)),
        ];
        app.update_position_from_ws(&positions);
        assert_eq!(app.position_qty, Decimal::ZERO);
        assert_eq!(app.position_entry_price, None);
    }

    // --- Bug 2 regression test: startup find() returning wrong side ---

    #[test]
    fn startup_find_skips_empty_side() {
        use crate::network::asterdex_positions::AsterDexPosition;

        // Simulate REST API returning both hedge-mode sides, empty first
        let positions = vec![
            AsterDexPosition {
                symbol: "SOLUSDT".to_string(),
                position_amt: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                mark_price: dec!(86.50),
                un_realized_profit: Decimal::ZERO,
                liquidation_price: Decimal::ZERO,
                leverage: dec!(5),
                margin_type: "crossed".to_string(),
                isolated_margin: Decimal::ZERO,
                position_side: "SHORT".to_string(),
                notional: Decimal::ZERO,
                update_time: 0,
            },
            AsterDexPosition {
                symbol: "SOLUSDT".to_string(),
                position_amt: dec!(6.74),
                entry_price: dec!(86.23),
                mark_price: dec!(86.50),
                un_realized_profit: dec!(1.82),
                liquidation_price: dec!(70.00),
                leverage: dec!(5),
                margin_type: "crossed".to_string(),
                isolated_margin: Decimal::ZERO,
                position_side: "LONG".to_string(),
                notional: dec!(581.19),
                update_time: 1700000000000,
            },
        ];

        // This is the exact pattern used in dom.rs startup fetch
        let result = positions
            .iter()
            .find(|p| p.symbol.eq_ignore_ascii_case("SOLUSDT") && p.position_amt != Decimal::ZERO)
            .map(|p| (p.entry_price, p.position_amt));

        assert_eq!(result, Some((dec!(86.23), dec!(6.74))));
    }

    #[test]
    fn startup_find_returns_none_when_flat() {
        use crate::network::asterdex_positions::AsterDexPosition;

        let positions = vec![
            AsterDexPosition {
                symbol: "SOLUSDT".to_string(),
                position_amt: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                mark_price: dec!(86.50),
                un_realized_profit: Decimal::ZERO,
                liquidation_price: Decimal::ZERO,
                leverage: dec!(5),
                margin_type: "crossed".to_string(),
                isolated_margin: Decimal::ZERO,
                position_side: "LONG".to_string(),
                notional: Decimal::ZERO,
                update_time: 0,
            },
            AsterDexPosition {
                symbol: "SOLUSDT".to_string(),
                position_amt: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                mark_price: dec!(86.50),
                un_realized_profit: Decimal::ZERO,
                liquidation_price: Decimal::ZERO,
                leverage: dec!(5),
                margin_type: "crossed".to_string(),
                isolated_margin: Decimal::ZERO,
                position_side: "SHORT".to_string(),
                notional: Decimal::ZERO,
                update_time: 0,
            },
        ];

        let result = positions
            .iter()
            .find(|p| p.symbol.eq_ignore_ascii_case("SOLUSDT") && p.position_amt != Decimal::ZERO)
            .map(|p| (p.entry_price, p.position_amt));

        assert_eq!(result, None);
    }
}
