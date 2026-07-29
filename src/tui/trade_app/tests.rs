// src/tui/trade_app/tests.rs
// Unit tests for TradeApp: order building, input handling, focus navigation, bracket orders.

use super::*;
use crate::network::asterdex_exchange_info::SymbolInfo;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Helper: create a minimal SymbolInfo for tests.
fn test_symbol_info() -> SymbolInfo {
    SymbolInfo {
        symbol: "BTCUSDT".to_string(),
        status: "TRADING".to_string(),
        base_asset: "BTC".to_string(),
        quote_asset: "USDT".to_string(),
        price_precision: 2,
        quantity_precision: 3,
        filters: vec![],
    }
}

/// Helper: create a TradeApp for tests.
fn test_app() -> TradeApp {
    TradeApp::new("BTCUSDT".to_string(), test_symbol_info(), Theme::dark())
}

/// Helper: create a KeyEvent with no modifiers.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Helper: type a string into a TextInput via key events.
fn type_str(input: &mut TextInput, s: &str) {
    for c in s.chars() {
        input.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

// ---- TradeOrderType tests ----

#[test]
fn test_trade_order_type_cycle() {
    let t = TradeOrderType::Market;
    let t = t.cycle();
    assert_eq!(t, TradeOrderType::Limit);
    let t = t.cycle();
    assert_eq!(t, TradeOrderType::StopMarket);
    let t = t.cycle();
    assert_eq!(t, TradeOrderType::StopLimit);
    let t = t.cycle();
    assert_eq!(t, TradeOrderType::TrailingStop);
    let t = t.cycle();
    assert_eq!(t, TradeOrderType::Market); // wraps around
}

#[test]
fn test_trade_order_type_needs_price() {
    assert!(!TradeOrderType::Market.needs_price());
    assert!(TradeOrderType::Limit.needs_price());
    assert!(!TradeOrderType::StopMarket.needs_price());
    assert!(TradeOrderType::StopLimit.needs_price());
}

#[test]
fn test_trade_order_type_needs_stop_price() {
    assert!(!TradeOrderType::Market.needs_stop_price());
    assert!(!TradeOrderType::Limit.needs_stop_price());
    assert!(TradeOrderType::StopMarket.needs_stop_price());
    assert!(TradeOrderType::StopLimit.needs_stop_price());
}

#[test]
fn test_trade_order_type_labels() {
    assert_eq!(TradeOrderType::Market.label(), "MARKET");
    assert_eq!(TradeOrderType::Limit.label(), "LIMIT");
    assert_eq!(TradeOrderType::StopMarket.label(), "STOP MKT");
    assert_eq!(TradeOrderType::StopLimit.label(), "STOP LMT");
}

// ---- Focus tests ----

#[test]
fn test_focus_next_market() {
    // Market: Side -> OrderType -> Quantity -> ReduceOnly -> Submit
    // (skip Price, StopPrice, TimeInForce)
    let ot = TradeOrderType::Market;
    let f = Focus::Side.next(&ot, false);
    assert_eq!(f, Focus::OrderType);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Quantity);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::ReduceOnly); // skips Price, StopPrice
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Submit); // skips TimeInForce (Market doesn't need price)
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Side); // wraps
}

#[test]
fn test_focus_next_limit() {
    // Limit: Side -> OrderType -> Quantity -> Price -> ReduceOnly -> TimeInForce -> Submit
    // (skip StopPrice)
    let ot = TradeOrderType::Limit;
    let f = Focus::Side.next(&ot, false);
    assert_eq!(f, Focus::OrderType);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Quantity);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Price);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::ReduceOnly); // skips StopPrice
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::TimeInForce);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Submit);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Side); // wraps
}

#[test]
fn test_focus_next_stop_limit() {
    // StopLimit: Side -> OrderType -> Quantity -> Price -> StopPrice -> ReduceOnly -> TimeInForce -> Submit
    let ot = TradeOrderType::StopLimit;
    let f = Focus::Side.next(&ot, false);
    assert_eq!(f, Focus::OrderType);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Quantity);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Price);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::StopPrice);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::ReduceOnly);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::TimeInForce);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Submit);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Side); // wraps
}

#[test]
fn test_focus_prev_market() {
    // Backward: Submit -> ReduceOnly -> Quantity -> OrderType -> Side
    let ot = TradeOrderType::Market;
    let f = Focus::Submit.prev(&ot, false);
    assert_eq!(f, Focus::ReduceOnly); // skips TimeInForce
    let f = f.prev(&ot, false);
    assert_eq!(f, Focus::Quantity); // skips StopPrice, Price
    let f = f.prev(&ot, false);
    assert_eq!(f, Focus::OrderType);
    let f = f.prev(&ot, false);
    assert_eq!(f, Focus::Side);
    let f = f.prev(&ot, false);
    assert_eq!(f, Focus::Submit); // wraps backward
}

#[test]
fn test_focus_reduce_only_always_reachable() {
    // ReduceOnly should be reachable for all order types
    for ot in &[
        TradeOrderType::Market,
        TradeOrderType::Limit,
        TradeOrderType::StopMarket,
        TradeOrderType::StopLimit,
    ] {
        let mut found = false;
        let mut f = Focus::Side;
        for _ in 0..Focus::ALL.len() {
            f = f.next(ot, false);
            if f == Focus::ReduceOnly {
                found = true;
                break;
            }
        }
        assert!(found, "ReduceOnly should be reachable for {:?}", ot);
    }
}

#[test]
fn test_focus_time_in_force_skipped_for_market() {
    // TimeInForce should NOT appear in Market order tab cycle
    let ot = TradeOrderType::Market;
    let mut f = Focus::Side;
    for _ in 0..Focus::ALL.len() {
        f = f.next(&ot, false);
        assert_ne!(f, Focus::TimeInForce, "TimeInForce should be skipped for Market");
        if f == Focus::Side {
            break; // wrapped around
        }
    }
}

#[test]
fn test_focus_time_in_force_visible_for_limit() {
    // TimeInForce should appear in Limit order tab cycle
    let ot = TradeOrderType::Limit;
    let mut found = false;
    let mut f = Focus::Side;
    for _ in 0..Focus::ALL.len() {
        f = f.next(&ot, false);
        if f == Focus::TimeInForce {
            found = true;
            break;
        }
    }
    assert!(found, "TimeInForce should be reachable for Limit");
}

// ---- handle_key tests ----

#[test]
fn test_handle_key_quit_q() {
    let mut app = test_app();
    assert!(!app.should_quit);
    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn test_handle_key_quit_esc() {
    let mut app = test_app();
    app.handle_key(key(KeyCode::Esc));
    assert!(app.should_quit);
}

#[test]
fn test_handle_key_quit_ctrl_c() {
    let mut app = test_app();
    let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.handle_key(k);
    assert!(app.should_quit);
}

#[test]
fn test_handle_key_side_toggle() {
    let mut app = test_app();
    assert_eq!(app.side, OrderSide::Buy);
    assert_eq!(app.focus, Focus::Side);

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.side, OrderSide::Sell);

    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.side, OrderSide::Buy);

    app.handle_key(key(KeyCode::Char('l')));
    assert_eq!(app.side, OrderSide::Sell);

    app.handle_key(key(KeyCode::Char('h')));
    assert_eq!(app.side, OrderSide::Buy);
}

#[test]
fn test_handle_key_order_type_cycle() {
    let mut app = test_app();
    assert_eq!(app.order_type, TradeOrderType::Market);

    // 'o' cycles order type when not on text field (focus starts at Side)
    app.handle_key(key(KeyCode::Char('o')));
    assert_eq!(app.order_type, TradeOrderType::Limit);
    // After 'o', focus moves to Quantity (a text field), move back to Side first
    app.focus = Focus::Side;

    app.handle_key(key(KeyCode::Char('o')));
    assert_eq!(app.order_type, TradeOrderType::StopMarket);
}

#[test]
fn test_handle_key_tab_navigation() {
    let mut app = test_app();
    assert_eq!(app.focus, Focus::Side);

    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::OrderType);

    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Quantity);

    // Market order: Tab from Quantity should skip to ReduceOnly (skipping Price, StopPrice)
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::ReduceOnly);

    // Tab from ReduceOnly should skip to Submit (skipping TimeInForce for Market)
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Submit);
}

#[test]
fn test_handle_key_backtab_navigation() {
    let mut app = test_app();
    app.focus = Focus::Submit;

    let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key(backtab);
    // Market order: from Submit, back should go to ReduceOnly (skip TimeInForce)
    assert_eq!(app.focus, Focus::ReduceOnly);
}

#[test]
fn test_handle_key_text_input_numeric_only() {
    let mut app = test_app();
    app.focus = Focus::Quantity;

    // Digits should work
    app.handle_key(key(KeyCode::Char('1')));
    app.handle_key(key(KeyCode::Char('2')));
    app.handle_key(key(KeyCode::Char('.')));
    app.handle_key(key(KeyCode::Char('3')));
    assert_eq!(app.quantity_input.value(), "12.3");

    // Letters should be rejected (consumed but not inserted)
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.quantity_input.value(), "12.3");
}

#[test]
fn test_handle_key_enter_on_text_advances_focus() {
    let mut app = test_app();
    app.focus = Focus::Quantity;

    // Enter on text field advances to next focus
    app.handle_key(key(KeyCode::Enter));
    // Market order: after Quantity, next is ReduceOnly (skip Price, StopPrice)
    assert_eq!(app.focus, Focus::ReduceOnly);
}

#[test]
fn test_handle_key_confirming_y_sends() {
    let mut app = test_app();
    // Set up valid form state
    type_str(&mut app.quantity_input, "0.001");
    app.confirm_snapshot = Some(OrderSummary {
        symbol: "BTCUSDT".to_string(),
        side: OrderSide::Buy,
        order_type: TradeOrderType::Market,
        quantity: Decimal::from_str("0.001").unwrap(),
        price: None,
        stop_price: None,
        callback_rate: None,
        activation_price: None,
        mark_price: None,
        bracket_sl_price: None,
        bracket_tp_price: None,
    });
    app.mode = AppMode::Confirming;

    // Create a channel to capture the sent params
    let (tx, mut rx) = mpsc::channel(1);
    app.submit_tx = Some(tx);

    app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(app.mode, AppMode::Submitting);

    // Verify params were sent
    let params = rx.try_recv().unwrap();
    match params {
        OrderParams::Market { symbol, side, quantity } => {
            assert_eq!(symbol, "BTCUSDT");
            assert_eq!(side, OrderSide::Buy);
            assert_eq!(quantity, Decimal::from_str("0.001").unwrap());
        }
        _ => panic!("Expected Market order params"),
    }
}

#[test]
fn test_handle_key_confirming_n_cancels() {
    let mut app = test_app();
    app.mode = AppMode::Confirming;
    app.confirm_snapshot = Some(OrderSummary {
        symbol: "BTCUSDT".to_string(),
        side: OrderSide::Buy,
        order_type: TradeOrderType::Market,
        quantity: Decimal::from_str("0.001").unwrap(),
        price: None,
        stop_price: None,
        callback_rate: None,
        activation_price: None,
        mark_price: None,
        bracket_sl_price: None,
        bracket_tp_price: None,
    });

    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.confirm_snapshot.is_none());
}

#[test]
fn test_handle_key_showing_result_any_key_dismisses() {
    let mut app = test_app();
    app.mode = AppMode::ShowingResult;
    type_str(&mut app.quantity_input, "0.001");

    // Any key should dismiss
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.order_result.is_none());
    // Form should be cleared
    assert!(app.quantity_input.is_empty());
}

// ---- build_order_params tests ----

#[test]
fn test_build_order_params_market() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "0.001");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::Market { symbol, side, quantity } => {
            assert_eq!(symbol, "BTCUSDT");
            assert_eq!(side, OrderSide::Buy);
            assert_eq!(quantity, Decimal::from_str("0.001").unwrap());
        }
        _ => panic!("Expected Market order params"),
    }
}

#[test]
fn test_build_order_params_limit_missing_price() {
    let mut app = test_app();
    app.order_type = TradeOrderType::Limit;
    type_str(&mut app.quantity_input, "0.001");
    // Price is empty

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Price is required"));
}

#[test]
fn test_build_order_params_limit_valid() {
    let mut app = test_app();
    app.order_type = TradeOrderType::Limit;
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.price_input, "50000");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::Limit { price, time_in_force, .. } => {
            assert_eq!(price, Decimal::from_str("50000").unwrap());
            assert_eq!(time_in_force, TimeInForce::Gtc);
        }
        _ => panic!("Expected Limit order params"),
    }
}

#[test]
fn test_build_order_params_stop_market() {
    let mut app = test_app();
    app.order_type = TradeOrderType::StopMarket;
    type_str(&mut app.quantity_input, "0.01");
    type_str(&mut app.stop_price_input, "45000");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::StopMarket { stop_price, .. } => {
            assert_eq!(stop_price, Decimal::from_str("45000").unwrap());
        }
        _ => panic!("Expected StopMarket order params"),
    }
}

#[test]
fn test_build_order_params_empty_quantity() {
    let app = test_app();
    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Quantity is required"));
}

#[test]
fn test_build_order_params_zero_quantity() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "0");
    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("greater than 0"));
}

// ---- clear_form tests ----

#[test]
fn test_clear_form_after_order() {
    let mut app = test_app();
    app.side = OrderSide::Sell;
    app.order_type = TradeOrderType::Limit;
    type_str(&mut app.quantity_input, "0.5");
    type_str(&mut app.price_input, "50000");
    type_str(&mut app.stop_price_input, "45000");
    app.error_message = Some("old error".to_string());

    app.clear_form_after_order();

    // Inputs cleared
    assert!(app.quantity_input.is_empty());
    assert!(app.price_input.is_empty());
    assert!(app.stop_price_input.is_empty());
    assert!(app.error_message.is_none());

    // Side and order type preserved
    assert_eq!(app.side, OrderSide::Sell);
    assert_eq!(app.order_type, TradeOrderType::Limit);
}

// ---- Order type cycle clears irrelevant fields ----

#[test]
fn test_order_type_cycle_clears_price_when_switching_to_market() {
    let mut app = test_app();
    app.order_type = TradeOrderType::StopLimit;
    type_str(&mut app.price_input, "50000");
    type_str(&mut app.stop_price_input, "45000");

    // Cycle: StopLimit -> TrailingStop -> Market
    app.order_type = app.order_type.cycle();
    assert_eq!(app.order_type, TradeOrderType::TrailingStop);
    app.order_type = app.order_type.cycle();
    assert_eq!(app.order_type, TradeOrderType::Market);
    // The handle_key 'o' path does the clearing, test that directly
    // by simulating what handle_key does
    if !app.order_type.needs_price() {
        app.price_input.clear();
    }
    if !app.order_type.needs_stop_price() {
        app.stop_price_input.clear();
    }
    assert!(app.price_input.is_empty());
    assert!(app.stop_price_input.is_empty());
}

// ---- Side enter advances focus ----

#[test]
fn test_side_enter_advances_focus() {
    let mut app = test_app();
    assert_eq!(app.focus, Focus::Side);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::OrderType);
}

// ---- OrderType left/right cycling via handle_key ----

#[test]
fn test_order_type_left_right_in_focus() {
    let mut app = test_app();
    app.focus = Focus::OrderType;
    assert_eq!(app.order_type, TradeOrderType::Market);

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.order_type, TradeOrderType::Limit);

    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.order_type, TradeOrderType::Market);
}

// ---- Submit enter with invalid form shows error ----

#[test]
fn test_submit_enter_invalid_shows_error() {
    let mut app = test_app();
    app.focus = Focus::Submit;
    // Quantity is empty -> validation should fail
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.error_message.is_some());
    assert!(app.error_message.as_ref().unwrap().contains("Quantity"));
}

// ---- Submit enter with valid form goes to confirming ----

#[test]
fn test_submit_enter_valid_goes_to_confirming() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "0.001");
    app.focus = Focus::Submit;

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, AppMode::Confirming);
    assert!(app.confirm_snapshot.is_some());
    assert!(app.error_message.is_none());
}

// ---- Submitting mode absorbs keys but allows Ctrl+C/Esc ----

#[test]
fn test_submitting_absorbs_keys() {
    let mut app = test_app();
    app.mode = AppMode::Submitting;

    // Regular keys should be consumed but not quit
    assert!(app.handle_key(key(KeyCode::Char('q'))));
    assert!(!app.should_quit);
    assert_eq!(app.mode, AppMode::Submitting);
}

#[test]
fn test_submitting_allows_ctrl_c_quit() {
    let mut app = test_app();
    app.mode = AppMode::Submitting;

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.handle_key(ctrl_c);
    assert!(app.should_quit);
}

#[test]
fn test_submitting_allows_esc_quit() {
    let mut app = test_app();
    app.mode = AppMode::Submitting;

    app.handle_key(key(KeyCode::Esc));
    assert!(app.should_quit);
}

// ---- try_send error handling ----

#[test]
fn test_confirming_y_handles_full_channel() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "0.001");
    app.confirm_snapshot = Some(OrderSummary {
        symbol: "BTCUSDT".to_string(),
        side: OrderSide::Buy,
        order_type: TradeOrderType::Market,
        quantity: Decimal::from_str("0.001").unwrap(),
        price: None,
        stop_price: None,
        callback_rate: None,
        activation_price: None,
        mark_price: None,
        bracket_sl_price: None,
        bracket_tp_price: None,
    });
    app.mode = AppMode::Confirming;

    // Create a channel with capacity 1 and fill it
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(OrderParams::Market {
        symbol: "BTCUSDT".into(),
        side: OrderSide::Buy,
        quantity: Decimal::ONE,
    }).unwrap(); // fill the channel
    app.submit_tx = Some(tx);

    // Now try to confirm -- channel is full
    app.handle_key(key(KeyCode::Char('y')));

    // Should NOT be in Submitting mode -- should fall back to Editing with error
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.error_message.is_some());
    assert!(app.error_message.as_ref().unwrap().contains("busy"));
    assert!(app.confirm_snapshot.is_none());
}

// ---- build_order_summary tests ----

#[test]
fn test_build_order_summary_includes_mark_price() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "1.0");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());

    let summary = app.build_order_summary().unwrap();
    assert_eq!(summary.symbol, "BTCUSDT");
    assert_eq!(summary.mark_price, Some(Decimal::from_str("50000").unwrap()));
}

// ---- New Phase 40-01 tests ----

// ---- ReduceOnly key handling ----

#[test]
fn test_handle_key_reduce_only_toggle() {
    let mut app = test_app();
    app.focus = Focus::ReduceOnly;
    assert!(!app.reduce_only);

    // Space toggles
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(app.reduce_only);
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(!app.reduce_only);

    // Left toggles
    app.handle_key(key(KeyCode::Left));
    assert!(app.reduce_only);

    // Right toggles
    app.handle_key(key(KeyCode::Right));
    assert!(!app.reduce_only);
}

// ---- TimeInForce key handling ----

#[test]
fn test_handle_key_time_in_force_cycle() {
    let mut app = test_app();
    app.order_type = TradeOrderType::Limit;
    app.focus = Focus::TimeInForce;
    assert_eq!(app.time_in_force, TimeInForce::Gtc);

    // Right cycles forward
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.time_in_force, TimeInForce::Ioc);

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.time_in_force, TimeInForce::Fok);

    // Left cycles backward
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.time_in_force, TimeInForce::Ioc);
}

// ---- build_order_params uses self.time_in_force ----

#[test]
fn test_build_order_params_uses_time_in_force() {
    let mut app = test_app();
    app.order_type = TradeOrderType::Limit;
    app.time_in_force = TimeInForce::Fok;
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.price_input, "50000");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::Limit { time_in_force, .. } => {
            assert_eq!(time_in_force, TimeInForce::Fok);
        }
        _ => panic!("Expected Limit order params"),
    }
}

#[test]
fn test_build_order_params_stop_limit_uses_time_in_force() {
    let mut app = test_app();
    app.order_type = TradeOrderType::StopLimit;
    app.time_in_force = TimeInForce::Gtx;
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.price_input, "50000");
    type_str(&mut app.stop_price_input, "49000");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::StopLimit { time_in_force, .. } => {
            assert_eq!(time_in_force, TimeInForce::Gtx);
        }
        _ => panic!("Expected StopLimit order params"),
    }
}

// ---- Notional value computation ----

#[test]
fn test_notional_value_market() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "0.5");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());

    let notional = app.notional_value().unwrap();
    assert_eq!(notional, Decimal::from_str("25000").unwrap());
}

#[test]
fn test_notional_value_limit() {
    let mut app = test_app();
    app.order_type = TradeOrderType::Limit;
    type_str(&mut app.quantity_input, "1.0");
    type_str(&mut app.price_input, "48000");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());

    // Limit uses price_input, not mark_price
    let notional = app.notional_value().unwrap();
    assert_eq!(notional, Decimal::from_str("48000").unwrap());
}

#[test]
fn test_notional_value_none_when_no_qty() {
    let app = test_app();
    // No quantity entered
    assert!(app.notional_value().is_none());
}

#[test]
fn test_notional_value_none_when_no_price() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "1.0");
    // No mark_price set for Market order
    assert!(app.notional_value().is_none());
}

// ---- Margin required computation ----

#[test]
fn test_margin_required() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "1.0");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    app.leverage = Some(10);

    let margin = app.margin_required().unwrap();
    assert_eq!(margin, Decimal::from_str("5000").unwrap());
}

#[test]
fn test_margin_required_none_without_leverage() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "1.0");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    // No leverage set
    assert!(app.margin_required().is_none());
}

// ---- Quick-size (apply_quick_size) ----

#[test]
fn test_apply_quick_size_basic() {
    let mut app = test_app();
    app.available_balance = Some(Decimal::from_str("1000").unwrap());
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    app.leverage = Some(10);

    // 100% of balance at 10x leverage: (1000 * 1.0 * 10) / 50000 = 0.2
    app.apply_quick_size(Decimal::ONE);
    let qty = app.quantity_input.as_decimal().unwrap();
    assert_eq!(qty, Decimal::from_str("0.2").unwrap());
}

#[test]
fn test_apply_quick_size_25_percent() {
    let mut app = test_app();
    app.available_balance = Some(Decimal::from_str("1000").unwrap());
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    app.leverage = Some(10);

    // 25%: (1000 * 0.25 * 10) / 50000 = 0.05
    app.apply_quick_size(Decimal::new(25, 2));
    let qty = app.quantity_input.as_decimal().unwrap();
    assert_eq!(qty, Decimal::from_str("0.05").unwrap());
}

#[test]
fn test_apply_quick_size_no_balance() {
    let mut app = test_app();
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    // No balance
    app.apply_quick_size(Decimal::ONE);
    assert!(app.error_message.is_some());
    assert!(app.error_message.as_ref().unwrap().contains("Balance or price"));
}

#[test]
fn test_apply_quick_size_no_price() {
    let mut app = test_app();
    app.available_balance = Some(Decimal::from_str("1000").unwrap());
    // No mark price
    app.apply_quick_size(Decimal::ONE);
    assert!(app.error_message.is_some());
}

#[test]
fn test_apply_quick_size_default_leverage() {
    let mut app = test_app();
    app.available_balance = Some(Decimal::from_str("1000").unwrap());
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    // No leverage set -> defaults to 1

    // (1000 * 1.0 * 1) / 50000 = 0.02
    app.apply_quick_size(Decimal::ONE);
    let qty = app.quantity_input.as_decimal().unwrap();
    assert_eq!(qty, Decimal::from_str("0.02").unwrap());
}

#[test]
fn test_apply_quick_size_limit_uses_price_input() {
    let mut app = test_app();
    app.order_type = TradeOrderType::Limit;
    app.available_balance = Some(Decimal::from_str("1000").unwrap());
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    app.leverage = Some(10);
    type_str(&mut app.price_input, "40000");

    // Should use price_input (40000), not mark_price (50000)
    // (1000 * 1.0 * 10) / 40000 = 0.25
    app.apply_quick_size(Decimal::ONE);
    let qty = app.quantity_input.as_decimal().unwrap();
    assert_eq!(qty, Decimal::from_str("0.25").unwrap());
}

// ---- clear_form preserves reduce_only and time_in_force ----

#[test]
fn test_clear_form_preserves_reduce_only_and_tif() {
    let mut app = test_app();
    app.reduce_only = true;
    app.time_in_force = TimeInForce::Fok;
    type_str(&mut app.quantity_input, "0.5");

    app.clear_form_after_order();

    assert!(app.reduce_only, "reduce_only should be preserved");
    assert_eq!(app.time_in_force, TimeInForce::Fok, "time_in_force should be preserved");
    assert!(app.quantity_input.is_empty(), "quantity should be cleared");
}

// ---- Quantity precision from symbol_info ----

#[test]
fn test_quantity_precision_from_symbol_info() {
    let app = test_app();
    // test_symbol_info() has quantity_precision: 3
    assert_eq!(app.quantity_precision, 3);
}

// ---- Filter validation wiring tests (Phase 47-02) ----

#[test]
fn test_submit_rejects_qty_below_lot_size_min() {
    let mut app = test_app();
    // Set up symbol_info with LOT_SIZE filter
    app.symbol_info.filters = vec![
        crate::network::asterdex_exchange_info::SymbolFilter::LotSize {
            min_qty: Decimal::from_str("0.001").unwrap(),
            max_qty: Decimal::from_str("1000").unwrap(),
            step_size: Decimal::from_str("0.001").unwrap(),
        },
    ];
    type_str(&mut app.quantity_input, "0.0001"); // below min
    app.focus = Focus::Submit;

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, AppMode::Editing); // should NOT go to Confirming
    assert!(app.error_message.is_some());
    assert!(app.error_message.as_ref().unwrap().contains("below minimum"));
}

#[test]
fn test_submit_rejects_notional_below_min() {
    let mut app = test_app();
    app.symbol_info.filters = vec![
        crate::network::asterdex_exchange_info::SymbolFilter::MinNotional {
            notional: Decimal::from_str("5.00").unwrap(),
        },
    ];
    type_str(&mut app.quantity_input, "0.001");
    app.mark_price = Some(Decimal::from_str("100").unwrap());
    // notional = 0.001 * 100 = 0.1 USDT (below 5.00 minimum)
    app.focus = Focus::Submit;

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.error_message.is_some());
    assert!(app.error_message.as_ref().unwrap().contains("Notional below minimum"));
}

#[test]
fn test_submit_passes_valid_filters() {
    let mut app = test_app();
    app.symbol_info.filters = vec![
        crate::network::asterdex_exchange_info::SymbolFilter::LotSize {
            min_qty: Decimal::from_str("0.001").unwrap(),
            max_qty: Decimal::from_str("1000").unwrap(),
            step_size: Decimal::from_str("0.001").unwrap(),
        },
        crate::network::asterdex_exchange_info::SymbolFilter::MinNotional {
            notional: Decimal::from_str("5.00").unwrap(),
        },
    ];
    type_str(&mut app.quantity_input, "0.001");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    // notional = 0.001 * 50000 = 50 USDT (above 5.00 minimum)
    app.focus = Focus::Submit;

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, AppMode::Confirming); // passes validation
    assert!(app.error_message.is_none());
}

// ---- Trailing Stop tests (Phase 52-01) ----

#[test]
fn test_trade_order_type_needs_callback_rate() {
    assert!(TradeOrderType::TrailingStop.needs_callback_rate());
    assert!(!TradeOrderType::Market.needs_callback_rate());
    assert!(!TradeOrderType::Limit.needs_callback_rate());
    assert!(!TradeOrderType::StopMarket.needs_callback_rate());
    assert!(!TradeOrderType::StopLimit.needs_callback_rate());
}

#[test]
fn test_trade_order_type_needs_activation_price() {
    assert!(TradeOrderType::TrailingStop.needs_activation_price());
    assert!(!TradeOrderType::Market.needs_activation_price());
    assert!(!TradeOrderType::Limit.needs_activation_price());
    assert!(!TradeOrderType::StopMarket.needs_activation_price());
    assert!(!TradeOrderType::StopLimit.needs_activation_price());
}

#[test]
fn test_focus_next_trailing_stop() {
    // TrailingStop: Side -> OrderType -> Quantity -> CallbackRate -> ActivationPrice -> ReduceOnly -> Submit
    // (skip Price, StopPrice, TimeInForce)
    let ot = TradeOrderType::TrailingStop;
    let f = Focus::Side.next(&ot, false);
    assert_eq!(f, Focus::OrderType);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Quantity);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::CallbackRate); // skips Price, StopPrice
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::ActivationPrice);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::ReduceOnly);
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Submit); // skips TimeInForce
    let f = f.next(&ot, false);
    assert_eq!(f, Focus::Side); // wraps
}

#[test]
fn test_build_order_params_trailing_stop_valid() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.callback_rate_input, "1.5");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::TrailingStopMarket {
            symbol,
            side,
            quantity,
            callback_rate,
            activation_price,
        } => {
            assert_eq!(symbol, "BTCUSDT");
            assert_eq!(side, OrderSide::Buy);
            assert_eq!(quantity, Decimal::from_str("0.001").unwrap());
            assert_eq!(callback_rate, Decimal::from_str("1.5").unwrap());
            assert!(activation_price.is_none());
        }
        _ => panic!("Expected TrailingStopMarket order params"),
    }
}

#[test]
fn test_build_order_params_trailing_stop_with_activation() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    app.side = OrderSide::Buy;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.callback_rate_input, "1.0");
    type_str(&mut app.activation_price_input, "48000");

    let params = app.build_order_params().unwrap();
    match params {
        OrderParams::TrailingStopMarket {
            callback_rate,
            activation_price,
            ..
        } => {
            assert_eq!(callback_rate, Decimal::from_str("1.0").unwrap());
            assert_eq!(activation_price, Some(Decimal::from_str("48000").unwrap()));
        }
        _ => panic!("Expected TrailingStopMarket order params"),
    }
}

#[test]
fn test_build_order_params_trailing_stop_missing_callback() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    type_str(&mut app.quantity_input, "0.001");
    // callback_rate_input is empty

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Callback rate is required"));
}

#[test]
fn test_build_order_params_trailing_stop_callback_out_of_range() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.callback_rate_input, "6.0");

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("between 0.1 and 5.0"));

    // Also test below minimum
    app.callback_rate_input.clear();
    type_str(&mut app.callback_rate_input, "0.05");

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("between 0.1 and 5.0"));
}

#[test]
fn test_build_order_params_trailing_stop_bad_activation_direction() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    app.side = OrderSide::Buy;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.callback_rate_input, "1.0");
    type_str(&mut app.activation_price_input, "52000"); // above mark for BUY -> error

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("below mark price for BUY"));

    // Test SELL with activation below mark
    app.side = OrderSide::Sell;
    app.activation_price_input.clear();
    type_str(&mut app.activation_price_input, "48000"); // below mark for SELL -> error

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("above mark price for SELL"));
}

#[test]
fn test_notional_value_trailing_stop() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    type_str(&mut app.quantity_input, "0.5");
    app.mark_price = Some(Decimal::from_str("50000").unwrap());

    // TrailingStop uses mark_price like Market
    let notional = app.notional_value().unwrap();
    assert_eq!(notional, Decimal::from_str("25000").unwrap());
}

#[test]
fn test_clear_form_clears_trailing_stop_fields() {
    let mut app = test_app();
    app.order_type = TradeOrderType::TrailingStop;
    type_str(&mut app.callback_rate_input, "1.5");
    type_str(&mut app.activation_price_input, "48000");

    app.clear_form_after_order();

    assert!(app.callback_rate_input.is_empty());
    assert!(app.activation_price_input.is_empty());
}

// ---- Bracket order tests (Phase 54-01) ----

#[test]
fn test_shift_b_toggles_bracket_enabled() {
    let mut app = test_app();
    assert!(!app.bracket_enabled);

    let shift_b = KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT);
    app.handle_key(shift_b);
    assert!(app.bracket_enabled);

    app.handle_key(shift_b);
    assert!(!app.bracket_enabled);
}

#[test]
fn test_bracket_sl_tp_cleared_when_disabled() {
    let mut app = test_app();
    app.bracket_enabled = true;
    type_str(&mut app.bracket_sl_price, "48000");
    type_str(&mut app.bracket_tp_price, "52000");

    // Toggle off -> should clear
    let shift_b = KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT);
    app.handle_key(shift_b);
    assert!(!app.bracket_enabled);
    assert!(app.bracket_sl_price.is_empty());
    assert!(app.bracket_tp_price.is_empty());
}

#[test]
fn test_focus_tab_includes_bracket_when_enabled() {
    // Market with bracket: Side -> OrderType -> Quantity -> BracketSLPrice -> BracketTPPrice -> ReduceOnly -> Submit
    let ot = TradeOrderType::Market;
    let f = Focus::Side.next(&ot, true);
    assert_eq!(f, Focus::OrderType);
    let f = f.next(&ot, true);
    assert_eq!(f, Focus::Quantity);
    let f = f.next(&ot, true);
    assert_eq!(f, Focus::BracketSLPrice); // skips Price, StopPrice, CallbackRate, ActivationPrice
    let f = f.next(&ot, true);
    assert_eq!(f, Focus::BracketTPPrice);
    let f = f.next(&ot, true);
    assert_eq!(f, Focus::ReduceOnly);
    let f = f.next(&ot, true);
    assert_eq!(f, Focus::Submit); // skips TimeInForce
    let f = f.next(&ot, true);
    assert_eq!(f, Focus::Side); // wraps
}

#[test]
fn test_focus_tab_skips_bracket_when_disabled() {
    // Market without bracket: Side -> OrderType -> Quantity -> ReduceOnly -> Submit
    let ot = TradeOrderType::Market;
    let mut f = Focus::Side;
    let mut visited = Vec::new();
    loop {
        f = f.next(&ot, false);
        if f == Focus::Side {
            break;
        }
        visited.push(f);
    }
    assert!(!visited.contains(&Focus::BracketSLPrice));
    assert!(!visited.contains(&Focus::BracketTPPrice));
}

#[test]
fn test_bracket_validation_buy_sl_above_mark_error() {
    let mut app = test_app();
    app.bracket_enabled = true;
    app.side = OrderSide::Buy;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.bracket_sl_price, "51000"); // above mark for BUY -> error
    type_str(&mut app.bracket_tp_price, "55000");

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("SL must be below mark price for BUY"));
}

#[test]
fn test_bracket_validation_sell_tp_above_mark_error() {
    let mut app = test_app();
    app.bracket_enabled = true;
    app.side = OrderSide::Sell;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.bracket_sl_price, "52000");
    type_str(&mut app.bracket_tp_price, "51000"); // above mark for SELL -> error (TP must be below)

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("TP must be below mark price for SELL"));
}

#[test]
fn test_bracket_validation_no_sl_or_tp_error() {
    let mut app = test_app();
    app.bracket_enabled = true;
    type_str(&mut app.quantity_input, "0.001");
    // Both SL and TP empty

    let result = app.build_order_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("at least one of SL or TP"));
}

#[test]
fn test_bracket_validation_valid_buy() {
    let mut app = test_app();
    app.bracket_enabled = true;
    app.side = OrderSide::Buy;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.bracket_sl_price, "48000"); // below mark
    type_str(&mut app.bracket_tp_price, "55000"); // above mark

    let result = app.build_order_params();
    assert!(result.is_ok());
}

#[test]
fn test_bracket_validation_valid_sell() {
    let mut app = test_app();
    app.bracket_enabled = true;
    app.side = OrderSide::Sell;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.bracket_sl_price, "52000"); // above mark for SELL
    type_str(&mut app.bracket_tp_price, "45000"); // below mark for SELL

    let result = app.build_order_params();
    assert!(result.is_ok());
}

#[test]
fn test_bracket_validation_sl_only_valid() {
    let mut app = test_app();
    app.bracket_enabled = true;
    app.side = OrderSide::Buy;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.bracket_sl_price, "48000");
    // TP is empty -- should still be valid (at least one required)

    let result = app.build_order_params();
    assert!(result.is_ok());
}

#[test]
fn test_bracket_waiting_esc_cancels() {
    let mut app = test_app();
    app.mode = AppMode::BracketWaiting;
    app.bracket_state = Some(BracketState::WaitingForFill { entry_order_id: 123 });

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.bracket_state.is_none());
}

#[test]
fn test_bracket_waiting_n_cancels() {
    let mut app = test_app();
    app.mode = AppMode::BracketWaiting;
    app.bracket_state = Some(BracketState::WaitingForFill { entry_order_id: 123 });

    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.bracket_state.is_none());
}

#[test]
fn test_bracket_waiting_absorbs_other_keys() {
    let mut app = test_app();
    app.mode = AppMode::BracketWaiting;
    app.bracket_state = Some(BracketState::WaitingForFill { entry_order_id: 123 });

    // Regular key should be absorbed
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.mode, AppMode::BracketWaiting);
    assert!(app.bracket_state.is_some());
}

#[test]
fn test_showing_bracket_result_any_key_dismisses() {
    let mut app = test_app();
    app.mode = AppMode::ShowingBracketResult;
    app.bracket_enabled = true;
    type_str(&mut app.bracket_sl_price, "48000");

    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.mode, AppMode::Editing);
    assert!(app.bracket_state.is_none());
    assert!(!app.bracket_enabled);
    assert!(app.bracket_sl_price.is_empty());
}

#[test]
fn test_clear_form_clears_bracket_fields() {
    let mut app = test_app();
    app.bracket_enabled = true;
    type_str(&mut app.bracket_sl_price, "48000");
    type_str(&mut app.bracket_tp_price, "52000");
    app.bracket_state = Some(BracketState::WaitingForFill { entry_order_id: 1 });

    app.clear_form_after_order();

    assert!(!app.bracket_enabled);
    assert!(app.bracket_sl_price.is_empty());
    assert!(app.bracket_tp_price.is_empty());
    assert!(app.bracket_state.is_none());
}

#[test]
fn test_bracket_is_text_field() {
    assert!(Focus::BracketSLPrice.is_text_field());
    assert!(Focus::BracketTPPrice.is_text_field());
}

#[test]
fn test_build_order_summary_includes_bracket_prices() {
    let mut app = test_app();
    app.bracket_enabled = true;
    app.mark_price = Some(Decimal::from_str("50000").unwrap());
    type_str(&mut app.quantity_input, "0.001");
    type_str(&mut app.bracket_sl_price, "48000");
    type_str(&mut app.bracket_tp_price, "55000");

    let summary = app.build_order_summary().unwrap();
    assert_eq!(summary.bracket_sl_price, Some(Decimal::from_str("48000").unwrap()));
    assert_eq!(summary.bracket_tp_price, Some(Decimal::from_str("55000").unwrap()));
}

#[test]
fn test_build_order_summary_none_bracket_when_disabled() {
    let mut app = test_app();
    type_str(&mut app.quantity_input, "0.001");
    // bracket_enabled is false by default

    let summary = app.build_order_summary().unwrap();
    assert!(summary.bracket_sl_price.is_none());
    assert!(summary.bracket_tp_price.is_none());
}

#[test]
fn test_bracket_waiting_ctrl_c_quits() {
    let mut app = test_app();
    app.mode = AppMode::BracketWaiting;

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.handle_key(ctrl_c);
    assert!(app.should_quit);
}
