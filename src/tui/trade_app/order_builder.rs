// src/tui/trade_app/order_builder.rs
// Order building, validation, and notional/margin computation methods for TradeApp.

use super::{OrderSummary, TradeApp, TradeOrderType};
use crate::data::order::OrderSide;
use crate::network::asterdex_trading::OrderParams;
use rust_decimal::Decimal;

impl TradeApp {
    /// Validate all form fields and construct the correct OrderParams variant.
    ///
    /// Returns a descriptive error string on validation failure.
    /// Uses `self.time_in_force` for Limit/StopLimit orders.
    pub fn build_order_params(&self) -> Result<OrderParams, String> {
        // Parse and validate quantity
        let quantity = self
            .quantity_input
            .as_decimal()
            .ok_or_else(|| "Quantity is required".to_string())?;
        if quantity <= Decimal::ZERO {
            return Err("Quantity must be greater than 0".to_string());
        }

        let params = match self.order_type {
            TradeOrderType::Market => Ok(OrderParams::Market {
                symbol: self.symbol.clone(),
                side: self.side,
                quantity,
            }),
            TradeOrderType::Limit => {
                let price = self
                    .price_input
                    .as_decimal()
                    .ok_or_else(|| "Price is required for Limit orders".to_string())?;
                if price <= Decimal::ZERO {
                    return Err("Price must be greater than 0".to_string());
                }
                Ok(OrderParams::Limit {
                    symbol: self.symbol.clone(),
                    side: self.side,
                    quantity,
                    price,
                    time_in_force: self.time_in_force,
                })
            }
            TradeOrderType::StopMarket => {
                let stop_price = self
                    .stop_price_input
                    .as_decimal()
                    .ok_or_else(|| "Stop price is required for Stop Market orders".to_string())?;
                if stop_price <= Decimal::ZERO {
                    return Err("Stop price must be greater than 0".to_string());
                }
                Ok(OrderParams::StopMarket {
                    symbol: self.symbol.clone(),
                    side: self.side,
                    quantity,
                    stop_price,
                })
            }
            TradeOrderType::StopLimit => {
                let price = self
                    .price_input
                    .as_decimal()
                    .ok_or_else(|| "Price is required for Stop Limit orders".to_string())?;
                if price <= Decimal::ZERO {
                    return Err("Price must be greater than 0".to_string());
                }
                let stop_price = self
                    .stop_price_input
                    .as_decimal()
                    .ok_or_else(|| "Stop price is required for Stop Limit orders".to_string())?;
                if stop_price <= Decimal::ZERO {
                    return Err("Stop price must be greater than 0".to_string());
                }
                Ok(OrderParams::StopLimit {
                    symbol: self.symbol.clone(),
                    side: self.side,
                    quantity,
                    price,
                    stop_price,
                    time_in_force: self.time_in_force,
                })
            }
            TradeOrderType::TrailingStop => {
                let callback_rate = self
                    .callback_rate_input
                    .as_decimal()
                    .ok_or_else(|| "Callback rate is required for Trailing Stop orders".to_string())?;
                if callback_rate < Decimal::new(1, 1) || callback_rate > Decimal::new(5, 0) {
                    return Err("Callback rate must be between 0.1 and 5.0".to_string());
                }
                let activation_price = if self.activation_price_input.is_empty() {
                    None
                } else {
                    let ap = self
                        .activation_price_input
                        .as_decimal()
                        .ok_or_else(|| "Invalid activation price".to_string())?;
                    if ap <= Decimal::ZERO {
                        return Err("Activation price must be greater than 0".to_string());
                    }
                    // Direction validation: BUY trailing stop needs activation < mark, SELL needs activation > mark
                    if let Some(mark) = self.mark_price {
                        match self.side {
                            OrderSide::Buy => {
                                if ap >= mark {
                                    return Err("Activation price must be below mark price for BUY trailing stop".to_string());
                                }
                            }
                            OrderSide::Sell => {
                                if ap <= mark {
                                    return Err("Activation price must be above mark price for SELL trailing stop".to_string());
                                }
                            }
                        }
                    }
                    Some(ap)
                };
                Ok(OrderParams::TrailingStopMarket {
                    symbol: self.symbol.clone(),
                    side: self.side,
                    quantity,
                    callback_rate,
                    activation_price,
                })
            }
        };

        // Bracket price validation (if bracket mode enabled)
        if self.bracket_enabled {
            let has_sl = !self.bracket_sl_price.is_empty();
            let has_tp = !self.bracket_tp_price.is_empty();

            if !has_sl && !has_tp {
                return Err("Bracket mode requires at least one of SL or TP price".to_string());
            }

            if has_sl {
                let sl = self
                    .bracket_sl_price
                    .as_decimal()
                    .ok_or_else(|| "Invalid bracket SL price".to_string())?;
                if sl <= Decimal::ZERO {
                    return Err("Bracket SL price must be greater than 0".to_string());
                }
                if let Some(mark) = self.mark_price {
                    match self.side {
                        OrderSide::Buy => {
                            if sl >= mark {
                                return Err("Bracket SL must be below mark price for BUY entry".to_string());
                            }
                        }
                        OrderSide::Sell => {
                            if sl <= mark {
                                return Err("Bracket SL must be above mark price for SELL entry".to_string());
                            }
                        }
                    }
                }
            }

            if has_tp {
                let tp = self
                    .bracket_tp_price
                    .as_decimal()
                    .ok_or_else(|| "Invalid bracket TP price".to_string())?;
                if tp <= Decimal::ZERO {
                    return Err("Bracket TP price must be greater than 0".to_string());
                }
                if let Some(mark) = self.mark_price {
                    match self.side {
                        OrderSide::Buy => {
                            if tp <= mark {
                                return Err("Bracket TP must be above mark price for BUY entry".to_string());
                            }
                        }
                        OrderSide::Sell => {
                            if tp >= mark {
                                return Err("Bracket TP must be below mark price for SELL entry".to_string());
                            }
                        }
                    }
                }
            }
        }

        params
    }

    /// Build an OrderSummary snapshot for the confirmation dialog.
    ///
    /// Validates all fields first, then captures the current mark_price.
    pub fn build_order_summary(&self) -> Result<OrderSummary, String> {
        // Validate fields first (reuse build_order_params validation)
        let _ = self.build_order_params()?;

        let quantity = self.quantity_input.as_decimal().unwrap();
        let price = if self.order_type.needs_price() {
            self.price_input.as_decimal()
        } else {
            None
        };
        let stop_price = if self.order_type.needs_stop_price() {
            self.stop_price_input.as_decimal()
        } else {
            None
        };
        let callback_rate = if self.order_type.needs_callback_rate() {
            self.callback_rate_input.as_decimal()
        } else {
            None
        };
        let activation_price = if self.order_type.needs_activation_price() {
            self.activation_price_input.as_decimal()
        } else {
            None
        };

        let bracket_sl_price = if self.bracket_enabled {
            self.bracket_sl_price.as_decimal()
        } else {
            None
        };
        let bracket_tp_price = if self.bracket_enabled {
            self.bracket_tp_price.as_decimal()
        } else {
            None
        };

        Ok(OrderSummary {
            symbol: self.symbol.clone(),
            side: self.side,
            order_type: self.order_type,
            quantity,
            price,
            stop_price,
            callback_rate,
            activation_price,
            mark_price: self.mark_price,
            bracket_sl_price,
            bracket_tp_price,
        })
    }

    /// Clear form inputs after a completed order.
    ///
    /// Clears quantity, price, and stop_price inputs.
    /// Preserves side, order_type, reduce_only, and time_in_force
    /// per Pitfall 5 (user likely wants same settings for repeated orders).
    pub fn clear_form_after_order(&mut self) {
        self.quantity_input.clear();
        self.price_input.clear();
        self.stop_price_input.clear();
        self.callback_rate_input.clear();
        self.activation_price_input.clear();
        self.bracket_sl_price.clear();
        self.bracket_tp_price.clear();
        self.bracket_enabled = false;
        self.bracket_state = None;
        self.error_message = None;
        // reduce_only and time_in_force are intentionally preserved
    }

    /// Compute notional value: quantity * price.
    ///
    /// Price reference depends on order type:
    /// - Market: mark_price
    /// - Limit/StopLimit: price_input (parsed)
    /// - StopMarket: stop_price_input (parsed)
    ///
    /// Returns None if quantity or price is unavailable or zero.
    pub fn notional_value(&self) -> Option<Decimal> {
        let qty = self.quantity_input.as_decimal()?;
        if qty <= Decimal::ZERO {
            return None;
        }

        let price = match self.order_type {
            TradeOrderType::Market | TradeOrderType::TrailingStop => self.mark_price?,
            TradeOrderType::Limit | TradeOrderType::StopLimit => {
                self.price_input.as_decimal()?
            }
            TradeOrderType::StopMarket => {
                self.stop_price_input.as_decimal()?
            }
        };
        if price <= Decimal::ZERO {
            return None;
        }

        qty.checked_mul(price)
    }

    /// Compute margin required: notional_value / leverage.
    ///
    /// Returns None if notional_value or leverage is unavailable.
    pub fn margin_required(&self) -> Option<Decimal> {
        let notional = self.notional_value()?;
        let leverage = self.leverage?;
        if leverage == 0 {
            return None;
        }
        notional.checked_div(Decimal::from(leverage))
    }

    /// Returns the notional cap for the current leverage from bracket data.
    ///
    /// Finds the bracket whose initial_leverage matches (or is closest >=) the current leverage.
    /// Returns None if no brackets loaded or no leverage set.
    pub fn notional_cap(&self) -> Option<u64> {
        let leverage = self.leverage?;
        // Brackets are sorted by tier (highest leverage first).
        // Find the bracket whose initial_leverage >= our leverage (tightest matching tier).
        self.leverage_brackets
            .iter()
            .filter(|b| b.initial_leverage >= leverage)
            .last()
            .map(|b| b.notional_cap)
    }

    /// Returns true if current notional exceeds the bracket's notional cap.
    pub fn notional_exceeds_cap(&self) -> bool {
        if let (Some(notional), Some(cap)) = (self.notional_value(), self.notional_cap()) {
            notional > Decimal::from(cap)
        } else {
            false
        }
    }

    /// Apply quick-size: compute quantity from available_balance, price, leverage, and percentage.
    ///
    /// Formula: qty = (available_balance * pct * leverage) / price
    /// Rounded to `self.quantity_precision` decimal places.
    /// Sets the quantity_input content with the formatted result.
    pub fn apply_quick_size(&mut self, pct: Decimal) {
        let balance = match self.available_balance {
            Some(b) if b > Decimal::ZERO => b,
            _ => {
                self.error_message = Some("Balance or price not available".into());
                return;
            }
        };

        let leverage_dec = Decimal::from(self.leverage.unwrap_or(1));

        // Price reference: mark_price for Market/StopMarket/TrailingStop, price_input for Limit/StopLimit
        let price = match self.order_type {
            TradeOrderType::Market | TradeOrderType::StopMarket | TradeOrderType::TrailingStop => {
                match self.mark_price {
                    Some(p) if p > Decimal::ZERO => p,
                    _ => {
                        self.error_message = Some("Balance or price not available".into());
                        return;
                    }
                }
            }
            TradeOrderType::Limit | TradeOrderType::StopLimit => {
                match self.price_input.as_decimal() {
                    Some(p) if p > Decimal::ZERO => p,
                    _ => {
                        // Fall back to mark_price if price_input is empty
                        match self.mark_price {
                            Some(p) if p > Decimal::ZERO => p,
                            _ => {
                                self.error_message = Some("Balance or price not available".into());
                                return;
                            }
                        }
                    }
                }
            }
        };

        let raw_qty = (balance * pct * leverage_dec) / price;
        let rounded = raw_qty.round_dp(self.quantity_precision);
        self.quantity_input.set_content(rounded.to_string());
        self.error_message = None;
    }

}
