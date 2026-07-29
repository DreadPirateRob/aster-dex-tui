// src/run/dom.rs
// DOM (Depth of Market) subcommand event loop
// Displays a real-time price ladder with bid/ask quantities from the depth stream.
// Supports authenticated order placement with graceful degradation to read-only mode.

use crate::config::AsterDexCredentials;
use crate::data::order::{Order, OrderSide};
use crate::data::types::Trade;
use crate::network::asterdex_exchange_info::SymbolFilter;
use crate::network::asterdex_stream_types::AccountUpdateData;
use crate::network::{
    cancel_all_open_orders, cancel_order, fetch_depth, fetch_open_orders, fetch_positions,
    place_order, run_depth_stream, run_dom_trade_stream, validate_symbol,
    AsterDexUserStreamManager, DepthUpdate, OrderParams, OrderResponse, PositionMode,
    TradingError,
};
use crate::tui::dom_app::CancelRequest;
use crate::tui::widgets::FlowImbalance;
use crate::tui::{DomApp, DomLadder, Event, Theme};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::io;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub async fn run(
    symbol: String,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // 1. Validate symbol and get tick size (fail fast, before TUI)
    let symbol_info = match validate_symbol(&symbol, &http_client).await {
        Ok(info) => {
            tracing::info!(
                symbol = %info.symbol,
                price_precision = info.price_precision,
                quantity_precision = info.quantity_precision,
                "Symbol validated for DOM"
            );
            info
        }
        Err(e) => {
            eprintln!("Error: Invalid symbol '{}': {}", symbol, e);
            std::process::exit(1);
        }
    };

    let tick_size = symbol_info
        .filters
        .iter()
        .find_map(|f| match f {
            SymbolFilter::PriceFilter { tick_size, .. } => Some(*tick_size),
            _ => None,
        })
        .unwrap_or_else(|| Decimal::new(1, 2)); // fallback 0.01

    // 1b. Try credentials (graceful degradation -- no require_credentials())
    let credentials = AsterDexCredentials::from_env();

    // 1c. Fetch position mode if authenticated
    let position_mode = if let Some(ref creds) = credentials {
        crate::network::fetch_position_mode(creds, &http_client).await
    } else {
        PositionMode::OneWay
    };

    // 2. Initialize DomApp state with exchange info
    let mut app = DomApp::new(
        symbol.clone(),
        theme,
        tick_size,
        symbol_info.quantity_precision as u32,
        symbol_info.filters.clone(),
    );
    app.credentials_present = credentials.is_some();

    // 2b. Create submit/result mpsc channels for order placement
    let (submit_tx, mut submit_rx) = mpsc::channel::<OrderParams>(1);
    let (result_tx, mut result_rx) = mpsc::channel::<Result<OrderResponse, TradingError>>(1);
    let (bracket_leg_tx, mut bracket_leg_rx) =
        mpsc::channel::<(String, Result<OrderResponse, TradingError>)>(2);
    app.submit_tx = Some(submit_tx);

    // 2c. Spawn startup balance + leverage fetch if authenticated
    let mut startup_rx: Option<mpsc::Receiver<(Option<Decimal>, Option<u32>)>> = None;
    if let Some(ref creds) = credentials {
        let startup_creds = creds.clone();
        let startup_client = http_client.clone();
        let startup_symbol = symbol.clone();
        let (startup_tx, rx) = mpsc::channel::<(Option<Decimal>, Option<u32>)>(1);
        startup_rx = Some(rx);
        tokio::spawn(async move {
            let balance = match crate::network::fetch_account_balance(&startup_creds, &startup_client).await {
                Ok(balances) => balances.iter().find(|b| b.asset == "USDT").map(|b| b.available_balance),
                Err(e) => {
                    tracing::warn!("DOM balance fetch failed: {}", e);
                    None
                }
            };
            let leverage = match fetch_positions(&startup_creds, &startup_client).await {
                Ok(positions) => positions
                    .iter()
                    .find(|p| p.symbol == startup_symbol)
                    .map(|p| p.leverage.to_u32().unwrap_or(1)),
                Err(e) => {
                    tracing::warn!("DOM leverage fetch failed: {}", e);
                    None
                }
            };
            let _ = startup_tx.send((balance, leverage)).await;
        });
    }

    // 2d. Spawn startup open orders + position fetch if authenticated
    let mut startup_orders_rx: Option<mpsc::Receiver<(Vec<Order>, Option<(Decimal, Decimal)>)>> = None;
    if let Some(ref creds) = credentials {
        let creds_cl = creds.clone();
        let client_cl = http_client.clone();
        let sym = symbol.clone();
        let (tx, rx) = mpsc::channel(1);
        startup_orders_rx = Some(rx);
        tokio::spawn(async move {
            // Fetch open orders
            let orders: Vec<Order> = match fetch_open_orders(&creds_cl, &client_cl).await {
                Ok(aster_orders) => aster_orders
                    .into_iter()
                    .filter(|o| o.symbol.eq_ignore_ascii_case(&sym))
                    .map(|o| o.into())
                    .collect(),
                Err(e) => {
                    tracing::warn!("DOM open orders fetch failed: {}", e);
                    Vec::new()
                }
            };
            // Fetch position for this symbol (hedge-mode aware: find the active side)
            let position = match fetch_positions(&creds_cl, &client_cl).await {
                Ok(positions) => positions
                    .iter()
                    .find(|p| p.symbol.eq_ignore_ascii_case(&sym) && p.position_amt != Decimal::ZERO)
                    .map(|p| (p.entry_price, p.position_amt)),
                Err(e) => {
                    tracing::warn!("DOM position fetch failed: {}", e);
                    None
                }
            };
            let _ = tx.send((orders, position)).await;
        });
    }

    // 2e. Create cancel/flatten channels
    let (cancel_tx, mut cancel_rx) = mpsc::channel::<CancelRequest>(4);
    let (flatten_tx, mut flatten_rx) = mpsc::channel::<()>(1);
    let (cancel_result_tx, mut cancel_result_rx) = mpsc::channel::<(String, Result<(), TradingError>)>(4);
    let (flatten_result_tx, mut flatten_result_rx) = mpsc::channel::<Result<OrderResponse, TradingError>>(1);
    app.cancel_tx = Some(cancel_tx);
    app.flatten_tx = Some(flatten_tx);

    // 3. Bootstrap order book with REST depth (50 levels per side), then spawn WebSocket stream
    match fetch_depth(&symbol, 1000, &http_client).await {
        Ok(snapshot) => {
            app.order_book.apply_partial_snapshot(
                snapshot.bids,
                snapshot.asks,
                snapshot.last_update_id,
            );
            tracing::info!(symbol = %symbol, "REST depth bootstrap: 50 levels per side");
        }
        Err(e) => {
            tracing::warn!(error = %e, "REST depth bootstrap failed, will rely on WebSocket");
        }
    }
    let (mut depth_tx, mut depth_rx) = tokio::sync::mpsc::channel::<DepthUpdate>(32);
    let mut depth_handle = tokio::spawn(run_depth_stream(symbol.clone(), depth_tx.clone()));

    // Periodic REST depth refresh timer (every 5 seconds for deeper levels)
    let mut depth_refresh_time = Instant::now();
    let depth_refresh_client = http_client.clone();
    let mut depth_refresh_symbol = symbol.clone();

    // 3a. Set up trade stream for volume profile (always runs; data accumulates for instant-on toggle)
    let (trade_tx, mut trade_rx) = tokio::sync::mpsc::channel::<Trade>(64);
    let mut trade_handle = tokio::spawn(run_dom_trade_stream(symbol.clone(), trade_tx));

    // Mutable symbol binding for pair switch reassignment
    let mut symbol = symbol;

    // 3b. Spawn user data stream if authenticated (for real-time order/position updates)
    let mut order_rx: Option<mpsc::Receiver<Order>> = None;
    let mut account_update_rx: Option<mpsc::Receiver<AccountUpdateData>> = None;
    if let Some(ref creds) = credentials {
        let (order_tx_stream, ord_rx) = mpsc::channel::<Order>(64);
        let (acct_tx, acct_rx) = mpsc::channel::<AccountUpdateData>(16);
        order_rx = Some(ord_rx);
        account_update_rx = Some(acct_rx);

        let (mut user_stream, _status_rx) = AsterDexUserStreamManager::new(
            creds.api_key.clone(),
            None, // Pass all events; DomApp filters by symbol (supports pair switching)
            order_tx_stream,
            Some(acct_tx),
            http_client.clone(),
        );
        tokio::spawn(async move {
            user_stream.run_with_reconnect().await;
        });
    }

    // 4. Initialize TUI using the shared helper
    let (mut tui, mut events) = crate::helpers::init_tui()?;

    // Pair switch: async symbol validation result channel
    let mut pending_switch_rx: Option<
        mpsc::Receiver<(
            String,
            Result<crate::network::asterdex_exchange_info::SymbolInfo, String>,
        )>,
    > = None;

    // 5. Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Tick => {
                    // Drain depth channel (consume all pending WebSocket updates)
                    // Uses apply_top_levels to preserve deeper REST levels
                    let mut depth_updated = false;
                    while let Ok(depth_update) = depth_rx.try_recv() {
                        app.order_book.apply_top_levels(
                            depth_update.bids,
                            depth_update.asks,
                            depth_update.last_update_id,
                        );
                        depth_updated = true;
                    }

                    // Periodic REST depth refresh (50 levels) every 5 seconds
                    if depth_refresh_time.elapsed() >= Duration::from_secs(5) {
                        depth_refresh_time = Instant::now();
                        let client = depth_refresh_client.clone();
                        let sym = depth_refresh_symbol.clone();
                        let tx = depth_tx.clone();
                        tokio::spawn(async move {
                            if let Ok(snapshot) = fetch_depth(&sym, 1000, &client).await {
                                let _ = tx.try_send(snapshot);
                            }
                        });
                    }

                    // Feed heatmap tracker with fresh quantity data when enabled
                    if depth_updated && app.show_heatmap {
                        app.heatmap_tracker.push_snapshot(app.order_book.all_quantities());
                    }

                    // Drain incoming trades into volume profile accumulator
                    // Runs unconditionally so data is ready when user toggles overlay on
                    while let Ok(trade) = trade_rx.try_recv() {
                        app.trade_volume_profile.record_trade(trade.price, trade.quantity, trade.side);
                        app.trade_imbalance.record_trade(trade.quantity, trade.side);
                    }

                    // Drain startup data (balance + leverage) -- consumed once
                    if let Some(ref mut rx) = startup_rx {
                        if let Ok((balance, leverage)) = rx.try_recv() {
                            app.available_balance = balance;
                            app.leverage = leverage;
                            startup_rx = None; // consumed, no need to poll again
                        }
                    }

                    // Drain order submission requests from submit_rx
                    if let Ok(params) = submit_rx.try_recv() {
                        if let Some(ref creds) = credentials {
                            let creds = creds.clone();
                            let tx = result_tx.clone();
                            let pm = position_mode;
                            let client_clone = http_client.clone();
                            tokio::spawn(async move {
                                let result = place_order(&params, &creds, false, pm, &client_clone).await;
                                let _ = tx.send(result).await;
                            });
                        }
                    }

                    // Drain order results from result_rx
                    if let Ok(result) = result_rx.try_recv() {
                        app.order_in_flight = false;
                        match result {
                            Ok(resp) => {
                                // Check if this was a bracket entry
                                if let Some(bracket) = app.pending_bracket.take() {
                                    // Entry accepted — spawn SL and TP legs
                                    let opposite_side = match bracket.entry_side {
                                        OrderSide::Buy => OrderSide::Sell,
                                        OrderSide::Sell => OrderSide::Buy,
                                    };

                                    // Spawn SL
                                    let sl_params = OrderParams::StopMarket {
                                        symbol: app.symbol.clone(),
                                        side: opposite_side,
                                        quantity: bracket.quantity,
                                        stop_price: bracket.sl_price,
                                    };
                                    if let Some(ref creds) = credentials {
                                        let creds_cl = creds.clone();
                                        let pm = position_mode;
                                        let client_cl = http_client.clone();
                                        let tx = bracket_leg_tx.clone();
                                        tokio::spawn(async move {
                                            let result = place_order(&sl_params, &creds_cl, true, pm, &client_cl).await;
                                            let _ = tx.send(("SL".to_string(), result)).await;
                                        });
                                    }

                                    // Spawn TP
                                    let tp_params = OrderParams::TakeProfitMarket {
                                        symbol: app.symbol.clone(),
                                        side: opposite_side,
                                        quantity: bracket.quantity,
                                        stop_price: bracket.tp_price,
                                    };
                                    if let Some(ref creds) = credentials {
                                        let creds_cl = creds.clone();
                                        let pm = position_mode;
                                        let client_cl = http_client.clone();
                                        let tx = bracket_leg_tx.clone();
                                        tokio::spawn(async move {
                                            let result = place_order(&tp_params, &creds_cl, true, pm, &client_cl).await;
                                            let _ = tx.send(("TP".to_string(), result)).await;
                                        });
                                    }

                                    app.status_message = Some(format!(
                                        "OK: bracket entry {} {} @ {} (#{}), placing SL/TP...",
                                        resp.side, resp.orig_qty, resp.price, resp.order_id
                                    ));
                                } else {
                                    // Normal (non-bracket) order result
                                    app.status_message = Some(format!(
                                        "OK: {} {} @ {} (#{})",
                                        resp.side, resp.orig_qty, resp.price, resp.order_id
                                    ));
                                }
                            }
                            Err(e) => {
                                app.pending_bracket = None; // Clear bracket on entry failure
                                app.status_message = Some(format!("FAILED: {}", e));
                            }
                        }
                        app.status_message_time = Some(Instant::now());
                    }

                    // Drain bracket leg results (SL/TP placed after entry)
                    while let Ok((leg_name, result)) = bracket_leg_rx.try_recv() {
                        match (leg_name.as_str(), &result) {
                            ("SL", Ok(resp)) => {
                                app.status_message = Some(format!(
                                    "SL placed @ {} (#{})",
                                    resp.stop_price, resp.order_id
                                ));
                            }
                            ("SL", Err(e)) => {
                                app.status_message = Some(format!("SL FAILED: {}", e));
                            }
                            ("TP", Ok(resp)) => {
                                app.status_message = Some(format!(
                                    "TP placed @ {} (#{})",
                                    resp.stop_price, resp.order_id
                                ));
                            }
                            ("TP", Err(e)) => {
                                app.status_message = Some(format!("TP FAILED: {}", e));
                            }
                            _ => {}
                        }
                        app.status_message_time = Some(Instant::now());
                    }

                    // Drain startup open orders + position (consumed once)
                    if let Some(ref mut rx) = startup_orders_rx {
                        if let Ok((orders, position)) = rx.try_recv() {
                            for order in &orders {
                                app.update_working_orders(order);
                            }
                            if let Some((entry_price, qty)) = position {
                                app.position_entry_price = Some(entry_price);
                                app.position_qty = qty;
                            }
                            startup_orders_rx = None;
                        }
                    }

                    // Drain real-time order updates from user stream
                    if let Some(ref mut rx) = order_rx {
                        while let Ok(order) = rx.try_recv() {
                            app.update_working_orders(&order);
                        }
                    }

                    // Drain account updates from user stream (position changes)
                    if let Some(ref mut rx) = account_update_rx {
                        while let Ok(update) = rx.try_recv() {
                            app.update_position_from_ws(&update.positions);
                            // Also update balance from ACCOUNT_UPDATE wallet balance
                            for bal in &update.balances {
                                if bal.asset == "USDT" {
                                    app.available_balance = Some(bal.wallet_balance);
                                }
                            }
                        }
                    }

                    // Drain cancel requests
                    if let Ok(cancel_req) = cancel_rx.try_recv() {
                        if let Some(ref creds) = credentials {
                            let creds_cl = creds.clone();
                            let client_cl = http_client.clone();
                            let sym = symbol.clone();
                            let tx = cancel_result_tx.clone();
                            match cancel_req {
                                CancelRequest::Single(order_id) => {
                                    tokio::spawn(async move {
                                        let result = cancel_order(&sym, order_id, &creds_cl, &client_cl).await;
                                        let _ = tx.send((format!("#{}", order_id), result.map(|_| ()))).await;
                                    });
                                }
                                CancelRequest::All => {
                                    tokio::spawn(async move {
                                        let result = cancel_all_open_orders(&sym, &creds_cl, &client_cl).await;
                                        let _ = tx.send(("all orders".to_string(), result)).await;
                                    });
                                }
                            }
                        }
                    }

                    // Drain cancel results
                    while let Ok((desc, result)) = cancel_result_rx.try_recv() {
                        match result {
                            Ok(()) => {
                                app.status_message = Some(format!("Cancelled {}", desc));
                            }
                            Err(e) => {
                                app.status_message = Some(format!("Cancel {} failed: {}", desc, e));
                            }
                        }
                        app.status_message_time = Some(Instant::now());
                    }

                    // Drain flatten requests
                    if let Ok(()) = flatten_rx.try_recv() {
                        if let Some(ref creds) = credentials {
                            if app.position_qty != Decimal::ZERO {
                                let close_side = if app.position_qty > Decimal::ZERO {
                                    OrderSide::Sell
                                } else {
                                    OrderSide::Buy
                                };
                                let close_qty = app.position_qty.abs();
                                let params = OrderParams::Market {
                                    symbol: symbol.clone(),
                                    side: close_side,
                                    quantity: close_qty,
                                };
                                let creds_cl = creds.clone();
                                let pm = position_mode;
                                let client_cl = http_client.clone();
                                let tx = flatten_result_tx.clone();
                                tokio::spawn(async move {
                                    let result = place_order(&params, &creds_cl, true, pm, &client_cl).await;
                                    let _ = tx.send(result).await;
                                });
                            }
                        }
                    }

                    // Drain flatten results
                    if let Ok(result) = flatten_result_rx.try_recv() {
                        match result {
                            Ok(resp) => {
                                app.status_message = Some(format!(
                                    "Position flattened: {} {} @ {}",
                                    resp.side, resp.orig_qty, resp.avg_price
                                ));
                            }
                            Err(e) => {
                                app.status_message = Some(format!("Flatten FAILED: {}", e));
                            }
                        }
                        app.status_message_time = Some(Instant::now());
                    }

                    // Auto-dismiss status messages: success after 3s, errors after 5s
                    if let Some(time) = app.status_message_time {
                        let is_error = app
                            .status_message
                            .as_ref()
                            .map_or(false, |m| m.starts_with("FAILED"));
                        let timeout = if is_error {
                            Duration::from_secs(5)
                        } else {
                            Duration::from_secs(3)
                        };
                        if time.elapsed() > timeout {
                            app.status_message = None;
                            app.status_message_time = None;
                        }
                    }

                    // --- Pair Switch: Phase 1 — Spawn async validation ---
                    if let Some(new_symbol) = app.pair_switch_requested.take() {
                        if app.order_in_flight {
                            app.status_message = Some("Cannot switch while order is in-flight".to_string());
                            app.status_message_time = Some(Instant::now());
                        } else if pending_switch_rx.is_some() {
                            app.status_message = Some("Switch already in progress...".to_string());
                            app.status_message_time = Some(Instant::now());
                        } else {
                            let client_cl = http_client.clone();
                            let (switch_tx, switch_rx) = mpsc::channel(1);
                            pending_switch_rx = Some(switch_rx);
                            tokio::spawn(async move {
                                let result = validate_symbol(&new_symbol, &client_cl)
                                    .await
                                    .map_err(|e| e.to_string());
                                let _ = switch_tx.send((new_symbol, result)).await;
                            });
                            app.status_message = Some("Validating symbol...".to_string());
                            app.status_message_time = Some(Instant::now());
                        }
                    }

                    // --- Pair Switch: Phase 2 — Handle validation result ---
                    if let Some(ref mut rx) = pending_switch_rx {
                        if let Ok((new_symbol, result)) = rx.try_recv() {
                            pending_switch_rx = None;
                            match result {
                                Ok(symbol_info) => {
                                    // Extract tick size from symbol info filters
                                    let new_tick_size = symbol_info
                                        .filters
                                        .iter()
                                        .find_map(|f| match f {
                                            SymbolFilter::PriceFilter { tick_size, .. } => Some(*tick_size),
                                            _ => None,
                                        })
                                        .unwrap_or_else(|| Decimal::new(1, 2));

                                    let new_qty_prec = symbol_info.quantity_precision as u32;

                                    // 1. Abort old stream tasks
                                    depth_handle.abort();
                                    trade_handle.abort();

                                    // 2. Reset app state
                                    app.switch_symbol(
                                        new_symbol.clone(),
                                        new_tick_size,
                                        new_qty_prec,
                                        symbol_info.filters,
                                    );

                                    // 3. Create new channels
                                    let (new_depth_tx, new_depth_rx) = mpsc::channel(32);
                                    let (new_trade_tx, new_trade_rx) = mpsc::channel(64);
                                    depth_tx = new_depth_tx;
                                    depth_rx = new_depth_rx;
                                    trade_rx = new_trade_rx;

                                    // 4. Bootstrap REST depth for new symbol
                                    match fetch_depth(&new_symbol, 1000, &http_client).await {
                                        Ok(snapshot) => {
                                            app.order_book.apply_partial_snapshot(
                                                snapshot.bids,
                                                snapshot.asks,
                                                snapshot.last_update_id,
                                            );
                                            tracing::info!(symbol = %new_symbol, "Pair switch: REST depth bootstrap OK");
                                        }
                                        Err(e) => {
                                            tracing::warn!(error = %e, "Pair switch: REST depth bootstrap failed");
                                        }
                                    }

                                    // 5. Spawn new stream tasks
                                    depth_handle = tokio::spawn(run_depth_stream(new_symbol.clone(), depth_tx.clone()));
                                    trade_handle = tokio::spawn(run_dom_trade_stream(new_symbol.clone(), new_trade_tx));

                                    // 6. Update event loop local variables
                                    symbol = new_symbol.clone();
                                    depth_refresh_symbol = new_symbol.clone();
                                    depth_refresh_time = Instant::now();

                                    // 7. Re-fetch startup data for new symbol (balance + leverage)
                                    if let Some(ref creds) = credentials {
                                        let startup_creds = creds.clone();
                                        let startup_client = http_client.clone();
                                        let startup_symbol = new_symbol.clone();
                                        let (tx, rx) = mpsc::channel::<(Option<Decimal>, Option<u32>)>(1);
                                        startup_rx = Some(rx);
                                        tokio::spawn(async move {
                                            let balance = match crate::network::fetch_account_balance(&startup_creds, &startup_client).await {
                                                Ok(balances) => balances.iter().find(|b| b.asset == "USDT").map(|b| b.available_balance),
                                                Err(e) => {
                                                    tracing::warn!("Pair switch balance fetch failed: {}", e);
                                                    None
                                                }
                                            };
                                            let leverage = match fetch_positions(&startup_creds, &startup_client).await {
                                                Ok(positions) => positions
                                                    .iter()
                                                    .find(|p| p.symbol == startup_symbol)
                                                    .map(|p| p.leverage.to_u32().unwrap_or(1)),
                                                Err(e) => {
                                                    tracing::warn!("Pair switch leverage fetch failed: {}", e);
                                                    None
                                                }
                                            };
                                            let _ = tx.send((balance, leverage)).await;
                                        });
                                    }

                                    // 8. Re-fetch open orders + position for new symbol
                                    if let Some(ref creds) = credentials {
                                        let creds_cl = creds.clone();
                                        let client_cl = http_client.clone();
                                        let sym = new_symbol.clone();
                                        let (tx, rx) = mpsc::channel(1);
                                        startup_orders_rx = Some(rx);
                                        tokio::spawn(async move {
                                            let orders: Vec<Order> = match fetch_open_orders(&creds_cl, &client_cl).await {
                                                Ok(aster_orders) => aster_orders
                                                    .into_iter()
                                                    .filter(|o| o.symbol.eq_ignore_ascii_case(&sym))
                                                    .map(|o| o.into())
                                                    .collect(),
                                                Err(e) => {
                                                    tracing::warn!("Pair switch open orders fetch failed: {}", e);
                                                    Vec::new()
                                                }
                                            };
                                            let position = match fetch_positions(&creds_cl, &client_cl).await {
                                                Ok(positions) => positions
                                                    .iter()
                                                    .find(|p| p.symbol.eq_ignore_ascii_case(&sym) && p.position_amt != Decimal::ZERO)
                                                    .map(|p| (p.entry_price, p.position_amt)),
                                                Err(e) => {
                                                    tracing::warn!("Pair switch position fetch failed: {}", e);
                                                    None
                                                }
                                            };
                                            let _ = tx.send((orders, position)).await;
                                        });
                                    }

                                    app.status_message = Some(format!("Switched to {}", new_symbol));
                                    app.status_message_time = Some(Instant::now());
                                    tracing::info!(symbol = %new_symbol, "Pair switch complete");
                                }
                                Err(e) => {
                                    app.status_message = Some(format!("Invalid symbol: {}", e));
                                    app.status_message_time = Some(Instant::now());
                                }
                            }
                        }
                    }

                    // Set initial center price once (static center, no auto-centering)
                    if app.center_price.is_none() {
                        app.center_price = app.order_book.mid_price();
                    }
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Render => {
                    tui.terminal().draw(|frame| ui(frame, &app))?;
                }
                Event::Resize(_w, h) => {
                    app.visible_rows = h.saturating_sub(6); // border(2) + header(2) + status(1) + margin(1)
                    app.clamp_cursor();
                }
                Event::Error => {
                    tracing::error!("Terminal I/O error, exiting DOM");
                    break;
                }
            }
        } else {
            break;
        }

        if app.should_quit {
            break;
        }
    }

    tui.restore();
    Ok(())
}

/// Render the DOM UI layout.
fn ui(frame: &mut Frame, app: &DomApp) {
    let area = frame.area();

    // Full-screen layout: header row 1 + header row 2 + flow imbalance + ladder + status bar + hotkey hint
    let flow_height = if app.show_flow_imbalance { 4 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),           // [0] header row 1: symbol + status
            Constraint::Length(1),           // [1] header row 2: cursor price + quick-size
            Constraint::Length(flow_height), // [2] flow imbalance panel (0 when hidden)
            Constraint::Min(10),             // [3] ladder
            Constraint::Length(1),           // [4] status bar
            Constraint::Length(1),           // [5] hotkey reference
        ])
        .split(area);

    // Header row 1: symbol name + connection/mode indicator
    let mode_indicator = if app.order_book.is_empty() {
        "[Connecting...]"
    } else if app.credentials_present {
        "[Live]"
    } else {
        "[Read-Only]"
    };
    let order_count = app.working_orders.values().map(|v| v.len()).sum::<usize>();
    let order_indicator = if order_count > 0 {
        format!(" [{}ord]", order_count)
    } else {
        String::new()
    };
    // Overlay status indicators
    let mut overlay_indicators = String::new();
    if app.show_heatmap {
        overlay_indicators.push_str(" [H]");
    }
    if app.show_volume_profile {
        let mode_char = match app.volume_profile_mode {
            crate::tui::dom_app::VolumeProfileMode::TradeVolume => "T",
            crate::tui::dom_app::VolumeProfileMode::RestingDepth => "D",
        };
        overlay_indicators.push_str(&format!(" [V:{}]", mode_char));
    }
    if app.show_cumulative_depth {
        overlay_indicators.push_str(" [D]");
    }
    if app.show_flow_imbalance {
        overlay_indicators.push_str(" [I]");
    }

    let header_text = format!(
        " DOM: {} | Tick: {} {}{}{}",
        app.symbol, app.tick_size, mode_indicator, order_indicator, overlay_indicators
    );
    let header = Paragraph::new(Line::from(header_text))
        .style(Style::default().fg(app.theme.text_primary));
    frame.render_widget(header, chunks[0]);

    // Header row 2: cursor price + quick-size presets
    let cursor_info = match app.cursor_price() {
        Some(price) => format!(" Cursor: {}", price),
        None => String::new(),
    };

    let preset_labels = ["F1:25%", "F2:50%", "F3:75%", "F4:100%"];
    let quick_size_display: String = preset_labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            if app.selected_qty_preset == Some(i) {
                format!("[{}]", label)
            } else {
                label.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let bracket_display = if app.bracket_mode {
        let side = app.inferred_side();
        let side_label = match side {
            crate::data::order::OrderSide::Buy => "B",
            crate::data::order::OrderSide::Sell => "S",
        };
        format!(" [BRK:{} SL:{} TP:{}]", side_label, app.sl_offset_ticks, app.tp_offset_ticks)
    } else {
        String::new()
    };

    let base_style = Style::default().fg(app.theme.text_secondary);
    let prefix_text = format!("{} {}{}", cursor_info, quick_size_display, bracket_display);
    let mut info_spans: Vec<Span> = vec![Span::styled(prefix_text, base_style)];

    if app.position_qty != Decimal::ZERO {
        let is_long = app.position_qty > Decimal::ZERO;
        let side_label = if is_long { "LONG" } else { "SHORT" };
        let side_color = if is_long { app.theme.bullish } else { app.theme.bearish };
        let entry_str = app.position_entry_price
            .map(|p| format!("@{}", p))
            .unwrap_or_default();
        info_spans.push(Span::styled(" | ", base_style));
        info_spans.push(Span::styled(
            format!("{} {}{}", side_label, app.position_qty.abs(), entry_str),
            Style::default().fg(side_color).add_modifier(ratatui::style::Modifier::BOLD),
        ));
        // Compute unrealized PnL using center_price as mark price proxy
        if let (Some(entry), Some(mark)) = (app.position_entry_price, app.center_price) {
            let pnl = app.position_qty * (mark - entry);
            let pnl_text = if pnl >= Decimal::ZERO {
                format!(" +{:.2}", pnl)
            } else {
                format!(" {:.2}", pnl)
            };
            let pnl_color = if pnl >= Decimal::ZERO { app.theme.bullish } else { app.theme.bearish };
            info_spans.push(Span::styled(pnl_text, Style::default().fg(pnl_color)));
        }
    }

    let info_bar = Paragraph::new(Line::from(info_spans));
    frame.render_widget(info_bar, chunks[1]);

    // Flow imbalance panel (chunks[2])
    if app.show_flow_imbalance {
        let bid_total = app.order_book.total_bid_qty();
        let ask_total = app.order_book.total_ask_qty();
        let book_total = bid_total + ask_total;
        let book_bid_ratio = if book_total > Decimal::ZERO {
            (bid_total / book_total).to_f64().unwrap_or(0.5)
        } else {
            0.5
        };
        let snapshots = app.trade_imbalance.snapshots();
        let flow_widget = FlowImbalance {
            book_bid_ratio,
            book_has_data: book_total > Decimal::ZERO,
            trade_snapshots: &snapshots,
            theme: &app.theme,
        };
        frame.render_widget(flow_widget, chunks[2]);
    }

    // Compute ghost bracket preview prices based on inferred side from cursor position
    let (sl_preview, tp_preview) = if app.bracket_mode {
        if let Some(cursor_p) = app.cursor_price() {
            let side = app.inferred_side();
            let sl = app.bracket_sl_price(cursor_p, side);
            let tp = app.bracket_tp_price(cursor_p, side);
            (Some(sl), Some(tp))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // Ladder widget
    let ladder = DomLadder::new()
        .order_book(&app.order_book)
        .tick_size(app.tick_size)
        .center_price(app.center_price)
        .best_bid(app.order_book.best_bid())
        .best_ask(app.order_book.best_ask())
        .mid_price(app.order_book.mid_price())
        .cursor_offset(app.cursor_offset)
        .sl_preview_price(sl_preview)
        .tp_preview_price(tp_preview)
        .working_orders(&app.working_orders)
        .position_entry_price(app.position_entry_price)
        .position_qty(app.position_qty)
        .show_heatmap(app.show_heatmap)
        .heatmap_tracker(if app.show_heatmap { Some(&app.heatmap_tracker) } else { None })
        .show_volume_profile(app.show_volume_profile)
        .show_cumulative_depth(app.show_cumulative_depth)
        .volume_profile_mode(if app.show_volume_profile { Some(app.volume_profile_mode) } else { None })
        .trade_volume_profile(if app.show_volume_profile { Some(&app.trade_volume_profile) } else { None })
        .theme(&app.theme)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(format!(" Price Ladder ({}) ", app.tick_size)),
        );
    frame.render_widget(ladder, chunks[3]);

    // Status bar: order feedback with color coding, confirmation mode emphasis
    let status_text = if let Some(buffer) = app.symbol_input_buffer() {
        format!("Enter symbol: {}\u{2588}", buffer) // Block cursor character
    } else if app.mode != crate::tui::dom_app::DomMode::Normal {
        app.status_message.as_deref().unwrap_or("Confirm? (y/n)").to_string()
    } else {
        app.status_message.as_deref().unwrap_or("").to_string()
    };
    let status_style = if app.symbol_input_buffer().is_some() {
        Style::default().fg(app.theme.text_primary)
    } else if app.mode != crate::tui::dom_app::DomMode::Normal {
        Style::default().fg(app.theme.text_primary).add_modifier(ratatui::style::Modifier::BOLD)
    } else if status_text.starts_with("FAILED") || status_text.starts_with("Error") || status_text.starts_with("Flatten FAILED") {
        Style::default().fg(app.theme.bearish)
    } else if status_text.starts_with("OK:") || status_text.starts_with("Cancelled") || status_text.starts_with("Position flattened") {
        Style::default().fg(app.theme.bullish)
    } else {
        Style::default().fg(app.theme.text_secondary)
    };
    let status_bar = Paragraph::new(Line::from(format!(" {}", status_text))).style(status_style);
    frame.render_widget(status_bar, chunks[4]);

    // Bottom hotkey reference bar
    let hotkey_bar =
        Paragraph::new(Line::from(" 'q' quit | j/k nav | '/' switch | 'b'/'s' buy/sell | 'f' flatten | 'c' center | +/- tick | h v d i overlays | 't' bracket | F1-F4 size "))
            .style(Style::default().bg(app.theme.background_highlight));
    frame.render_widget(hotkey_bar, chunks[5]);
}
