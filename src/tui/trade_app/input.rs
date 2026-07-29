// src/tui/trade_app/input.rs
// Keyboard input handling methods for TradeApp: handle_key dispatcher and per-focus handlers.

use super::{AppMode, Focus, TradeApp};
use crate::data::order::OrderSide;
use crate::network::asterdex_exchange_info::validate_filters;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rust_decimal::Decimal;

/// Internal enum for identifying which text field to delegate to.
enum TextField {
    Quantity,
    Price,
    StopPrice,
    CallbackRate,
    ActivationPrice,
    BracketSLPrice,
    BracketTPPrice,
}

impl TradeApp {
    /// Handle a keyboard event. Returns true if the key was consumed.
    ///
    /// Dispatches based on current AppMode:
    /// - Editing: form navigation, text input, quit keys
    /// - Confirming: Y/N to confirm or cancel order
    /// - Submitting: all keys ignored (waiting for async result)
    /// - ShowingResult: any key dismisses and returns to Editing
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match self.mode {
            AppMode::Editing => self.handle_key_editing(key),
            AppMode::Confirming => self.handle_key_confirming(key),
            AppMode::Submitting => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                        self.should_quit = true;
                    }
                    _ => {} // absorb all other keys
                }
                true
            }
            AppMode::ShowingResult => {
                // Any key dismisses the result and returns to editing
                self.clear_form_after_order();
                self.order_result = None;
                self.mode = AppMode::Editing;
                self.focus = Focus::Side;
                true
            }
            AppMode::BracketWaiting => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        self.should_quit = true;
                    }
                    (KeyCode::Esc, _) | (KeyCode::Char('n'), _) => {
                        // Cancel bracket wait -- don't place SL/TP
                        self.bracket_state = None;
                        self.mode = AppMode::Editing;
                    }
                    _ => {} // absorb all other keys while waiting
                }
                true
            }
            AppMode::ShowingBracketResult => {
                // Any key dismisses bracket result and returns to editing
                self.clear_form_after_order();
                self.bracket_state = None;
                self.mode = AppMode::Editing;
                self.focus = Focus::Side;
                true
            }
        }
    }

    /// Handle keys when leverage editing mode is active.
    ///
    /// Accepts digits, Enter to validate and send, Esc to cancel, Backspace to delete.
    /// All other keys are consumed and ignored (prevents leaking to form fields).
    fn handle_leverage_input(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.leverage_input.handle_key(key);
                true
            }
            KeyCode::Backspace => {
                if self.leverage_input.is_empty() {
                    self.leverage_editing = false;
                } else {
                    self.leverage_input.handle_key(key);
                }
                true
            }
            KeyCode::Esc => {
                self.leverage_editing = false;
                self.leverage_input.clear();
                true
            }
            KeyCode::Enter => {
                let value_str = self.leverage_input.value().to_string();
                self.leverage_editing = false;
                self.leverage_input.clear();

                if let Ok(new_lev) = value_str.parse::<u32>() {
                    if new_lev < 1 {
                        self.error_message = Some("Leverage must be at least 1".into());
                        return true;
                    }
                    if let Some(max) = self.max_leverage {
                        if new_lev > max {
                            self.error_message = Some(format!("Leverage must be 1-{}", max));
                            return true;
                        }
                    }
                    if let Some(tx) = &self.leverage_change_tx {
                        match tx.try_send(new_lev) {
                            Ok(()) => {
                                // Optimistic: don't update app.leverage yet; wait for async response
                            }
                            Err(_) => {
                                self.error_message = Some("Leverage change busy".into());
                            }
                        }
                    }
                } else {
                    self.error_message = Some("Invalid leverage value".into());
                }
                true
            }
            _ => true, // consume all other keys while editing leverage
        }
    }

    /// Handle keys in Editing mode.
    fn handle_key_editing(&mut self, key: KeyEvent) -> bool {
        // Ctrl+C always quits, even during leverage editing
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            self.should_quit = true;
            return true;
        }

        // Leverage input captures all keys when active (including Esc)
        if self.leverage_editing {
            return self.handle_leverage_input(key);
        }

        // Esc quits when not in leverage editing mode
        if key.code == KeyCode::Esc {
            self.should_quit = true;
            return true;
        }

        // 'q' quits only when NOT focused on a text field (so user can type 'q' in theory,
        // but numeric fields won't have 'q' anyway -- still, be consistent)
        if key.code == KeyCode::Char('q')
            && key.modifiers == KeyModifiers::NONE
            && !self.focus.is_text_field()
        {
            self.should_quit = true;
            return true;
        }

        // Tab / Shift-Tab navigation
        if key.code == KeyCode::Tab {
            self.focus = self.focus.next(&self.order_type, self.bracket_enabled);
            return true;
        }
        if key.code == KeyCode::BackTab {
            self.focus = self.focus.prev(&self.order_type, self.bracket_enabled);
            return true;
        }

        // Shift+O toggles orders table visibility (works regardless of focus)
        if key.code == KeyCode::Char('O') && key.modifiers == KeyModifiers::SHIFT {
            self.show_orders = !self.show_orders;
            return true;
        }

        // Shift+B toggles bracket mode (SL/TP after entry fill)
        if key.code == KeyCode::Char('B') && key.modifiers == KeyModifiers::SHIFT {
            self.bracket_enabled = !self.bracket_enabled;
            if !self.bracket_enabled {
                self.bracket_sl_price.clear();
                self.bracket_tp_price.clear();
            }
            return true;
        }

        // Global hotkeys when not on a text field
        if key.modifiers == KeyModifiers::NONE && !self.focus.is_text_field() {
            match key.code {
                // 'o' cycles order type
                KeyCode::Char('o') => {
                    self.order_type = self.order_type.cycle();
                    if !self.order_type.needs_price() {
                        self.price_input.clear();
                    }
                    if !self.order_type.needs_stop_price() {
                        self.stop_price_input.clear();
                    }
                    if !self.order_type.needs_callback_rate() {
                        self.callback_rate_input.clear();
                    }
                    if !self.order_type.needs_activation_price() {
                        self.activation_price_input.clear();
                    }
                    self.focus = Focus::Quantity;
                    return true;
                }
                // 'b'/'s' toggle side
                KeyCode::Char('b') => {
                    self.side = OrderSide::Buy;
                    return true;
                }
                KeyCode::Char('s') => {
                    self.side = OrderSide::Sell;
                    return true;
                }
                // 'r' toggles reduce-only
                KeyCode::Char('r') => {
                    self.reduce_only = !self.reduce_only;
                    return true;
                }
                // 'l' activates leverage editing (but not when Side is focused, where 'l' toggles side)
                KeyCode::Char('l') if self.focus != Focus::Side => {
                    self.leverage_editing = true;
                    self.leverage_input.clear();
                    return true;
                }
                _ => {}
            }
        }

        // Focus-specific key handling
        match self.focus {
            Focus::Side => self.handle_key_side(key),
            Focus::OrderType => self.handle_key_order_type(key),
            Focus::Quantity => self.handle_key_quantity(key),
            Focus::Price => self.handle_key_text_field(key, TextField::Price),
            Focus::StopPrice => self.handle_key_text_field(key, TextField::StopPrice),
            Focus::CallbackRate => self.handle_key_text_field(key, TextField::CallbackRate),
            Focus::ActivationPrice => self.handle_key_text_field(key, TextField::ActivationPrice),
            Focus::BracketSLPrice => self.handle_key_text_field(key, TextField::BracketSLPrice),
            Focus::BracketTPPrice => self.handle_key_text_field(key, TextField::BracketTPPrice),
            Focus::ReduceOnly => self.handle_key_reduce_only(key),
            Focus::TimeInForce => self.handle_key_time_in_force(key),
            Focus::Submit => self.handle_key_submit(key),
        }
    }

    /// Handle keys when Side field is focused.
    fn handle_key_side(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left | KeyCode::Right => {
                self.side = match self.side {
                    OrderSide::Buy => OrderSide::Sell,
                    OrderSide::Sell => OrderSide::Buy,
                };
                true
            }
            KeyCode::Char('h') | KeyCode::Char('l') => {
                self.side = match self.side {
                    OrderSide::Buy => OrderSide::Sell,
                    OrderSide::Sell => OrderSide::Buy,
                };
                true
            }
            KeyCode::Enter => {
                self.focus = self.focus.next(&self.order_type, self.bracket_enabled);
                true
            }
            _ => false,
        }
    }

    /// Handle keys when OrderType field is focused.
    fn handle_key_order_type(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left => {
                self.order_type = self.order_type.cycle_back();
                if !self.order_type.needs_price() {
                    self.price_input.clear();
                }
                if !self.order_type.needs_stop_price() {
                    self.stop_price_input.clear();
                }
                if !self.order_type.needs_callback_rate() {
                    self.callback_rate_input.clear();
                }
                if !self.order_type.needs_activation_price() {
                    self.activation_price_input.clear();
                }
                true
            }
            KeyCode::Right => {
                self.order_type = self.order_type.cycle();
                if !self.order_type.needs_price() {
                    self.price_input.clear();
                }
                if !self.order_type.needs_stop_price() {
                    self.stop_price_input.clear();
                }
                if !self.order_type.needs_callback_rate() {
                    self.callback_rate_input.clear();
                }
                if !self.order_type.needs_activation_price() {
                    self.activation_price_input.clear();
                }
                true
            }
            KeyCode::Enter => {
                self.focus = self.focus.next(&self.order_type, self.bracket_enabled);
                true
            }
            _ => false,
        }
    }

    /// Handle keys when Quantity field is focused.
    ///
    /// Extends handle_key_text_field with F1-F4 quick-size hotkeys.
    fn handle_key_quantity(&mut self, key: KeyEvent) -> bool {
        // Quick-size hotkeys: F1=25%, F2=50%, F3=75%, F4=100%
        let pct = match key.code {
            KeyCode::F(1) => Some(Decimal::new(25, 2)), // 0.25
            KeyCode::F(2) => Some(Decimal::new(50, 2)), // 0.50
            KeyCode::F(3) => Some(Decimal::new(75, 2)), // 0.75
            KeyCode::F(4) => Some(Decimal::ONE),        // 1.00
            _ => None,
        };
        if let Some(pct) = pct {
            self.apply_quick_size(pct);
            return true;
        }
        // Delegate to normal text field handling
        self.handle_key_text_field(key, TextField::Quantity)
    }

    /// Handle keys when ReduceOnly field is focused.
    fn handle_key_reduce_only(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                self.reduce_only = !self.reduce_only;
                true
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.focus = self.focus.next(&self.order_type, self.bracket_enabled);
                true
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev(&self.order_type, self.bracket_enabled);
                true
            }
            KeyCode::Esc => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }

    /// Handle keys when TimeInForce field is focused.
    fn handle_key_time_in_force(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Right => {
                self.time_in_force = self.time_in_force.cycle();
                true
            }
            KeyCode::Left => {
                self.time_in_force = self.time_in_force.cycle_back();
                true
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.focus = self.focus.next(&self.order_type, self.bracket_enabled);
                true
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev(&self.order_type, self.bracket_enabled);
                true
            }
            KeyCode::Esc => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }

    /// Handle keys when a text input field is focused.
    ///
    /// Only allows digits, '.', and '-' characters. Rejects other chars.
    /// Enter advances to the next focus field.
    fn handle_key_text_field(&mut self, key: KeyEvent, field: TextField) -> bool {
        let input = match field {
            TextField::Quantity => &mut self.quantity_input,
            TextField::Price => &mut self.price_input,
            TextField::StopPrice => &mut self.stop_price_input,
            TextField::CallbackRate => &mut self.callback_rate_input,
            TextField::ActivationPrice => &mut self.activation_price_input,
            TextField::BracketSLPrice => &mut self.bracket_sl_price,
            TextField::BracketTPPrice => &mut self.bracket_tp_price,
        };

        match key.code {
            KeyCode::Enter => {
                self.focus = self.focus.next(&self.order_type, self.bracket_enabled);
                true
            }
            KeyCode::Char(c) => {
                // Only allow numeric input: digits, decimal point, minus sign
                if c.is_ascii_digit() || c == '.' || c == '-' {
                    let char_key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
                    let handled = input.handle_key(char_key);
                    if handled {
                        self.error_message = None;
                    }
                    handled
                } else {
                    true // consume but reject non-numeric chars
                }
            }
            // Delegate editing keys (Backspace, Delete, Left, Right, Home, End)
            _ => {
                let handled = input.handle_key(key);
                if handled {
                    self.error_message = None;
                }
                handled
            }
        }
    }

    /// Handle keys when Submit button is focused.
    fn handle_key_submit(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Enter {
            match self.build_order_summary() {
                Ok(summary) => {
                    // Validate against exchange filters before showing confirmation
                    let quantity = summary.quantity;
                    let price = summary.price; // None for Market/StopMarket
                    let notional = self.notional_value();

                    if let Err(msg) = validate_filters(
                        &self.symbol_info.filters,
                        quantity,
                        price,
                        notional,
                    ) {
                        self.error_message = Some(msg);
                        return true;
                    }

                    // Validate notional against leverage bracket cap
                    if self.notional_exceeds_cap() {
                        if let Some(cap) = self.notional_cap() {
                            self.error_message = Some(format!(
                                "Notional exceeds max {} for {}x leverage",
                                cap,
                                self.leverage.unwrap_or(0)
                            ));
                            return true;
                        }
                    }

                    self.confirm_snapshot = Some(summary);
                    self.mode = AppMode::Confirming;
                    self.error_message = None;
                }
                Err(msg) => {
                    self.error_message = Some(msg);
                }
            }
            return true;
        }
        false
    }

    /// Handle keys in Confirming mode.
    fn handle_key_confirming(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                // Build order params from the confirmed form state and send
                if let Ok(params) = self.build_order_params() {
                    if let Some(tx) = &self.submit_tx {
                        match tx.try_send(params) {
                            Ok(()) => {
                                self.mode = AppMode::Submitting;
                            }
                            Err(_) => {
                                self.error_message = Some("Order submission busy, try again".into());
                                self.mode = AppMode::Editing;
                                self.confirm_snapshot = None;
                            }
                        }
                    } else {
                        self.mode = AppMode::Submitting;
                    }
                }
                true
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.mode = AppMode::Editing;
                self.confirm_snapshot = None;
                true
            }
            _ => true, // absorb all other keys in confirming mode
        }
    }
}
