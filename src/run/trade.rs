// src/run/trade.rs
// Trade subcommand event loop (extracted from main.rs)

use crate::config;
use crate::data::order::{Order, OrderSide, OrderStatus};
use crate::data::position::{Position, PositionSide};
use crate::helpers;
use crate::network::{
    self, asterdex_mark_price, fetch_orders, fetch_positions, place_order, validate_symbol,
    AccountUpdateData, AsterDexUserStreamManager, OrderParams, OrderResponse, PositionMode,
    TradingError,
};
use crate::tui::trade_app::{AppMode, BracketState, LegResult};
use crate::tui::{self, Event, Theme, TradeApp};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::io;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, MissedTickBehavior};

// ---------------------------------------------------------------------------
// Bracket order support (private to trade module)
// ---------------------------------------------------------------------------

/// Configuration captured from the trade form when a bracket order is submitted.
/// Stored across ticks until the entry order fills and SL/TP are placed.
struct BracketConfig {
    sl_price: Option<Decimal>,
    tp_price: Option<Decimal>,
    entry_side: OrderSide,
    symbol: String,
}

/// Spawn SL and/or TP orders as parallel async tasks after entry fills.
///
/// Each leg sends its result through `leg_tx` tagged with "SL" or "TP".
/// Uses `reduce_only=true` for both legs (closing the position, not opening).
fn spawn_bracket_legs(
    config: &BracketConfig,
    executed_qty: Decimal,
    credentials: &config::AsterDexCredentials,
    position_mode: PositionMode,
    client: &reqwest::Client,
    leg_tx: mpsc::Sender<(String, Result<OrderResponse, TradingError>)>,
) {
    let opposite_side = match config.entry_side {
        OrderSide::Buy => OrderSide::Sell,
        OrderSide::Sell => OrderSide::Buy,
    };

    // Spawn SL if configured
    if let Some(sl_price) = config.sl_price {
        let sl_params = OrderParams::StopMarket {
            symbol: config.symbol.clone(),
            side: opposite_side,
            quantity: executed_qty,
            stop_price: sl_price,
        };
        let creds = credentials.clone();
        let pm = position_mode;
        let client_clone = client.clone();
        let tx = leg_tx.clone();
        tokio::spawn(async move {
            let result = place_order(&sl_params, &creds, true, pm, &client_clone).await;
            let _ = tx.send(("SL".to_string(), result)).await;
        });
    }

    // Spawn TP if configured
    if let Some(tp_price) = config.tp_price {
        let tp_params = OrderParams::TakeProfitMarket {
            symbol: config.symbol.clone(),
            side: opposite_side,
            quantity: executed_qty,
            stop_price: tp_price,
        };
        let creds = credentials.clone();
        let pm = position_mode;
        let client_clone = client.clone();
        let tx = leg_tx.clone();
        tokio::spawn(async move {
            let result = place_order(&tp_params, &creds, true, pm, &client_clone).await;
            let _ = tx.send(("TP".to_string(), result)).await;
        });
    }
}

pub async fn run(
    symbol: String,
    no_orders: bool,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // 1. Validate credentials (fail fast, before any network calls)
    let credentials = helpers::require_credentials();

    // 2. Validate symbol via exchangeInfo (fail fast, before TUI)
    let symbol_info = match validate_symbol(&symbol, &http_client).await {
        Ok(info) => {
            tracing::info!(
                symbol = %info.symbol,
                price_precision = info.price_precision,
                quantity_precision = info.quantity_precision,
                "Symbol validated"
            );
            info
        }
        Err(e) => {
            eprintln!("Error: Invalid symbol '{}': {}", symbol, e);
            std::process::exit(1);
        }
    };

    // 3. Detect account position mode (hedge vs one-way)
    let position_mode = network::fetch_position_mode(&credentials, &http_client).await;

    // 4. Create mark price channel and spawn stream
    let (mark_price_tx, mark_price_rx) = watch::channel::<Option<Decimal>>(None);
    let mark_symbol = symbol.clone();
    tokio::spawn(async move {
        asterdex_mark_price::run_mark_price_stream(mark_symbol, mark_price_tx).await;
    });

    // 5. Create order submission channels
    let (submit_tx, mut submit_rx) = mpsc::channel::<OrderParams>(1);
    let (result_tx, mut result_rx) = mpsc::channel::<Result<OrderResponse, TradingError>>(1);

    // 6. Create balance, recent orders, and leverage channels (capacity 4 for refresh reuse)
    let (balance_tx, mut balance_rx) = mpsc::channel::<Option<Decimal>>(4);
    let (orders_tx, mut orders_rx) = mpsc::channel::<Vec<Order>>(4);
    let (leverage_tx, mut leverage_rx) = mpsc::channel::<Option<u32>>(1);
    let (position_tx, mut position_rx) = mpsc::channel::<Option<Position>>(1);

    // Leverage change request/result channels
    let (leverage_change_tx, mut leverage_change_rx) = mpsc::channel::<u32>(1);
    let (leverage_result_tx, mut leverage_result_rx) =
        mpsc::channel::<Result<network::asterdex_trading::LeverageResponse, network::TradingError>>(1);

    // Bracket leg result channel (SL/TP placement results)
    let (bracket_leg_tx, mut bracket_leg_rx) =
        mpsc::channel::<(String, Result<OrderResponse, TradingError>)>(4);

    // 6d. Spawn user data stream for real-time ORDER_TRADE_UPDATE and ACCOUNT_UPDATE
    let (ws_order_tx, mut ws_order_rx) = mpsc::channel::<Order>(32);
    let (account_update_tx, mut account_update_rx) = mpsc::channel::<AccountUpdateData>(64);
    {
        let (mut user_stream, _user_stream_status_rx) = AsterDexUserStreamManager::new(
            credentials.api_key.clone(),
            Some(symbol.clone()),
            ws_order_tx,
            Some(account_update_tx),
            http_client.clone(),
        );
        tokio::spawn(async move {
            user_stream.run_with_reconnect().await;
        });
    }

    // 6e. Spawn periodic reconciliation task (30s for balance + orders)
    let (recon_balance_tx, mut recon_balance_rx) = mpsc::channel::<Option<Decimal>>(4);
    let (recon_orders_tx, mut recon_orders_rx) = mpsc::channel::<Vec<Order>>(4);
    {
        let recon_creds = credentials.clone();
        let recon_symbol = symbol.clone();
        let recon_client = http_client.clone();
        let recon_bal_tx = recon_balance_tx;
        let recon_ord_tx = recon_orders_tx;
        tokio::spawn(async move {
            let mut timer = interval(Duration::from_secs(30));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            timer.tick().await; // skip first immediate tick
            loop {
                timer.tick().await;
                // Balance reconciliation
                match network::fetch_account_balance(&recon_creds, &recon_client).await {
                    Ok(balances) => {
                        if let Some(usdt) = balances.iter().find(|b| b.asset == "USDT") {
                            let _ = recon_bal_tx.send(Some(usdt.available_balance)).await;
                        }
                    }
                    Err(e) => tracing::warn!("Trade ticket balance reconciliation failed: {}", e),
                }
                // Orders reconciliation
                match fetch_orders(&recon_symbol, &recon_creds, &recon_client).await {
                    Ok(asterdex_orders) => {
                        let mut orders: Vec<Order> = asterdex_orders.into_iter().map(Order::from).collect();
                        orders.sort_by(|a, b| b.time.cmp(&a.time));
                        orders.truncate(5);
                        let _ = recon_ord_tx.send(orders).await;
                    }
                    Err(e) => tracing::warn!("Trade ticket orders reconciliation failed: {}", e),
                }
            }
        });
    }

    // 6a. Spawn async balance fetch at startup (non-blocking per Pitfall 6)
    {
        let startup_balance_tx = balance_tx.clone();
        let balance_creds = credentials.clone();
        let client_clone = http_client.clone();
        tokio::spawn(async move {
            match network::fetch_account_balance(&balance_creds, &client_clone).await {
                Ok(balances) => {
                    if let Some(usdt) = balances.iter().find(|b| b.asset == "USDT") {
                        let _ = startup_balance_tx.send(Some(usdt.available_balance)).await;
                    } else {
                        let _ = startup_balance_tx.send(None).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch account balance: {}", e);
                    let _ = startup_balance_tx.send(None).await;
                }
            }
        });
    }

    // 6b. Spawn async recent orders fetch at startup (non-blocking)
    {
        let startup_orders_tx = orders_tx.clone();
        let orders_creds = credentials.clone();
        let orders_symbol = symbol.clone();
        let client_clone = http_client.clone();
        tokio::spawn(async move {
            match fetch_orders(&orders_symbol, &orders_creds, &client_clone).await {
                Ok(asterdex_orders) => {
                    let mut orders: Vec<Order> = asterdex_orders.into_iter().map(Order::from).collect();
                    orders.sort_by(|a, b| b.time.cmp(&a.time)); // newest first
                    orders.truncate(5); // keep last 5
                    let _ = startup_orders_tx.send(orders).await;
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch recent orders: {}", e);
                    let _ = startup_orders_tx.send(Vec::new()).await;
                }
            }
        });
    }

    // 6c. Spawn async leverage + position fetch at startup via positionRisk
    {
        let lev_creds = credentials.clone();
        let lev_symbol = symbol.clone();
        let lev_client = http_client.clone();
        tokio::spawn(async move {
            match fetch_positions(&lev_creds, &lev_client).await {
                Ok(positions) => {
                    if let Some(pos) = positions.iter().find(|p| p.symbol == lev_symbol) {
                        let lev = pos.leverage.to_u32().unwrap_or(1);
                        let _ = leverage_tx.send(Some(lev)).await;
                        // Extract position for trade ticket
                        if pos.position_amt.is_zero() {
                            let _ = position_tx.send(None).await;
                        } else {
                            let position = Position::from(pos.clone());
                            let _ = position_tx.send(Some(position)).await;
                        }
                    } else {
                        let _ = leverage_tx.send(Some(1)).await;
                        let _ = position_tx.send(None).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch leverage: {}", e);
                    let _ = leverage_tx.send(Some(1)).await;
                    let _ = position_tx.send(None).await;
                }
            }
        });
    }

    // 6f. Spawn async leverage brackets fetch at startup
    let (brackets_tx, mut brackets_rx) = mpsc::channel::<Vec<network::LeverageBracket>>(1);
    {
        let br_creds = credentials.clone();
        let br_symbol = symbol.clone();
        let br_client = http_client.clone();
        tokio::spawn(async move {
            match network::fetch_leverage_brackets(&br_symbol, &br_creds, &br_client).await {
                Ok(brackets) => {
                    let _ = brackets_tx.send(brackets).await;
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch leverage brackets: {}", e);
                    let _ = brackets_tx.send(Vec::new()).await;
                }
            }
        });
    }

    // 7. Create TradeApp
    let mut app = TradeApp::new(symbol.clone(), symbol_info, theme);
    app.show_orders = !no_orders;
    app.submit_tx = Some(submit_tx);
    app.leverage_change_tx = Some(leverage_change_tx);

    // 8. Initialize terminal
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Bracket order tracking state
    let mut pending_bracket: Option<BracketConfig> = None;
    let mut bracket_legs_expected: u8 = 0;
    let mut bracket_legs_received: u8 = 0;
    let mut bracket_sl_result: Option<LegResult> = None;
    let mut bracket_tp_result: Option<LegResult> = None;

    // 9. Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        tui::ui_trade(frame, &mut app);
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Resize(_w, _h) => {}
                Event::Tick => {
                    // Poll mark price from watch channel
                    app.mark_price = *mark_price_rx.borrow();

                    // Poll balance updates (from startup fetch or post-order refresh)
                    if let Ok(balance) = balance_rx.try_recv() {
                        app.available_balance = balance;
                    }

                    // Poll recent orders updates (from startup fetch or post-order refresh)
                    if let Ok(orders) = orders_rx.try_recv() {
                        app.recent_orders = orders;
                    }

                    // Poll leverage from startup fetch
                    if let Ok(lev) = leverage_rx.try_recv() {
                        app.leverage = lev;
                    }

                    // Poll position from startup fetch
                    if let Ok(pos) = position_rx.try_recv() {
                        app.current_position = pos;
                    }

                    // Poll leverage brackets from startup fetch
                    if let Ok(brackets) = brackets_rx.try_recv() {
                        app.max_leverage = brackets.first().map(|b| b.initial_leverage);
                        app.leverage_brackets = brackets;
                    }

                    // Check for leverage change requests from TradeApp
                    if let Ok(new_leverage) = leverage_change_rx.try_recv() {
                        let creds = credentials.clone();
                        let sym = symbol.clone();
                        let client_clone = http_client.clone();
                        let lev_tx = leverage_result_tx.clone();
                        tokio::spawn(async move {
                            let result = network::change_leverage(&sym, new_leverage, &creds, &client_clone).await;
                            let _ = lev_tx.send(result).await;
                        });
                    }

                    // Receive leverage change results
                    if let Ok(result) = leverage_result_rx.try_recv() {
                        match result {
                            Ok(resp) => {
                                app.leverage = Some(resp.leverage);
                                app.error_message = None;
                                tracing::info!(leverage = resp.leverage, "Leverage updated from trade ticket");
                            }
                            Err(e) => {
                                app.error_message = Some(format!("Leverage change failed: {}", e));
                                tracing::warn!("Leverage change failed: {}", e);
                            }
                        }
                    }

                    // Drain ACCOUNT_UPDATE for real-time balance + position updates
                    while let Ok(update) = account_update_rx.try_recv() {
                        for bal in &update.balances {
                            if bal.asset == "USDT" {
                                app.available_balance = Some(bal.wallet_balance);
                                tracing::debug!(wallet_balance = %bal.wallet_balance, "Trade ticket balance updated via ACCOUNT_UPDATE");
                            }
                        }
                        // Position updates from ACCOUNT_UPDATE
                        for ws_pos in &update.positions {
                            // Filter to trade ticket's symbol only (ACCOUNT_UPDATE is account-wide)
                            if !ws_pos.symbol.eq_ignore_ascii_case(&symbol) {
                                continue;
                            }
                            // Determine side of this update (hedge mode sends both LONG and SHORT)
                            let ws_side = if ws_pos.position_side == "LONG" {
                                PositionSide::Long
                            } else if ws_pos.position_side == "SHORT" {
                                PositionSide::Short
                            } else {
                                if ws_pos.position_amt < Decimal::ZERO { PositionSide::Short } else { PositionSide::Long }
                            };
                            if ws_pos.position_amt.is_zero() {
                                // Position closed: only clear if side matches (hedge mode sends zero for the other side)
                                if let Some(ref existing) = app.current_position {
                                    if existing.position_side == ws_side {
                                        app.current_position = None;
                                        tracing::info!(symbol = %ws_pos.symbol, "Position closed via ACCOUNT_UPDATE (trade ticket)");
                                    }
                                }
                            } else if let Some(ref mut existing) = app.current_position {
                                if existing.position_side == ws_side {
                                    // Delta update: patch structural fields only
                                    existing.position_amt = ws_pos.position_amt;
                                    existing.entry_price = ws_pos.entry_price;
                                    existing.unrealized_profit = ws_pos.unrealized_pnl;
                                    existing.isolated_margin = ws_pos.isolated_wallet;
                                    existing.margin_type = ws_pos.margin_type.clone();
                                    tracing::debug!(symbol = %ws_pos.symbol, amt = %ws_pos.position_amt, "Position updated via ACCOUNT_UPDATE (trade ticket)");
                                } else {
                                    // Different side: replace (e.g., opened SHORT while LONG existed)
                                    app.current_position = Some(Position::from_ws_update(ws_pos));
                                    tracing::info!(symbol = %ws_pos.symbol, amt = %ws_pos.position_amt, "Position replaced via ACCOUNT_UPDATE (trade ticket)");
                                }
                            } else {
                                // New position from first fill
                                app.current_position = Some(Position::from_ws_update(ws_pos));
                                tracing::info!(symbol = %ws_pos.symbol, amt = %ws_pos.position_amt, "New position from ACCOUNT_UPDATE (trade ticket)");
                            }
                        }
                    }

                    // Drain ORDER_TRADE_UPDATE for real-time order status updates
                    while let Ok(incoming) = ws_order_rx.try_recv() {
                        // Bracket fill detection: check before consuming incoming
                        if app.mode == AppMode::BracketWaiting {
                            if let Some(BracketState::WaitingForFill { entry_order_id }) =
                                &app.bracket_state
                            {
                                if incoming.order_id == *entry_order_id {
                                    match incoming.status {
                                        OrderStatus::Filled => {
                                            // Entry filled! Spawn SL/TP legs
                                            let qty = incoming.executed_qty;
                                            let avg = incoming.avg_price;
                                            if let Some(ref config) = pending_bracket {
                                                bracket_legs_expected =
                                                    config.sl_price.is_some() as u8
                                                        + config.tp_price.is_some() as u8;
                                                bracket_legs_received = 0;
                                                spawn_bracket_legs(
                                                    config,
                                                    qty,
                                                    &credentials,
                                                    position_mode,
                                                    &http_client,
                                                    bracket_leg_tx.clone(),
                                                );
                                                app.bracket_state =
                                                    Some(BracketState::PlacingSLTP {
                                                        _entry_qty: qty,
                                                        _entry_avg_price: avg,
                                                    });
                                                app.mode = AppMode::Submitting;
                                            }
                                        }
                                        OrderStatus::Canceled
                                        | OrderStatus::Expired
                                        | OrderStatus::Rejected => {
                                            // Entry cancelled/expired/rejected
                                            app.bracket_state =
                                                Some(BracketState::Complete {
                                                    entry: LegResult::Failed {
                                                        error: format!(
                                                            "Entry {}",
                                                            incoming.status
                                                        ),
                                                    },
                                                    sl: LegResult::Skipped,
                                                    tp: LegResult::Skipped,
                                                });
                                            app.mode = AppMode::ShowingBracketResult;
                                            pending_bracket = None;
                                        }
                                        _ => {
                                            // PARTIALLY_FILLED etc: update display but
                                            // don't trigger SL/TP (v1 decision)
                                        }
                                    }
                                }
                            }
                        }

                        // Update recent orders (existing logic, unchanged)
                        if let Some(existing) = app.recent_orders.iter_mut().find(|o| o.order_id == incoming.order_id) {
                            if incoming.update_time >= existing.update_time {
                                *existing = incoming;
                            }
                        } else {
                            app.recent_orders.insert(0, incoming);
                            app.recent_orders.truncate(5);
                        }
                    }

                    // Drain reconciliation results (balance + orders)
                    if let Ok(balance) = recon_balance_rx.try_recv() {
                        app.available_balance = balance;
                    }
                    if let Ok(orders) = recon_orders_rx.try_recv() {
                        app.recent_orders = orders;
                    }

                    // Check for order submission requests from TradeApp
                    if let Ok(params) = submit_rx.try_recv() {
                        // Capture bracket config BEFORE spawning entry order
                        if app.bracket_enabled {
                            pending_bracket = Some(BracketConfig {
                                sl_price: app.bracket_sl_price.as_decimal(),
                                tp_price: app.bracket_tp_price.as_decimal(),
                                entry_side: app.side,
                                symbol: app.symbol.clone(),
                            });
                            // Reset leg tracking
                            bracket_legs_expected = 0;
                            bracket_legs_received = 0;
                            bracket_sl_result = None;
                            bracket_tp_result = None;
                            // Set preliminary bracket state to prevent cleanup guard
                            // from clearing pending_bracket before REST response arrives.
                            // Order ID updated on fill.
                            app.bracket_state = Some(BracketState::WaitingForFill {
                                entry_order_id: 0,
                            });
                        } else {
                            pending_bracket = None;
                        }

                        let creds = credentials.clone();
                        let tx = result_tx.clone();
                        let reduce_only = app.reduce_only;
                        let pm = position_mode;
                        let client_clone = http_client.clone();
                        tokio::spawn(async move {
                            let result = place_order(&params, &creds, reduce_only, pm, &client_clone).await;
                            let _ = tx.send(result).await;
                        });
                    }

                    // Receive order results
                    if let Ok(result) = result_rx.try_recv() {
                        if pending_bracket.is_some() {
                            // --- Bracket entry result handling ---
                            match result {
                                Err(e) => {
                                    // Entry failed: show normal error, no SL/TP
                                    app.order_result = Some(Err(e));
                                    app.mode = AppMode::ShowingResult;
                                    app.bracket_state = None;
                                    pending_bracket = None;
                                }
                                Ok(response) => {
                                    if response.status == "FILLED" {
                                        // MARKET entry fills immediately (Pitfall 6)
                                        let qty = response.executed_qty;
                                        let avg = response.avg_price;
                                        app.order_result = Some(Ok(response));
                                        if let Some(ref config) = pending_bracket {
                                            bracket_legs_expected = config.sl_price.is_some() as u8
                                                + config.tp_price.is_some() as u8;
                                            bracket_legs_received = 0;
                                            spawn_bracket_legs(
                                                config,
                                                qty,
                                                &credentials,
                                                position_mode,
                                                &http_client,
                                                bracket_leg_tx.clone(),
                                            );
                                            app.bracket_state = Some(BracketState::PlacingSLTP {
                                                _entry_qty: qty,
                                                _entry_avg_price: avg,
                                            });
                                            // Stay in Submitting mode while SL/TP are in flight
                                        }
                                    } else if response.status == "NEW" {
                                        // LIMIT entry accepted but not filled yet
                                        let order_id = response.order_id;
                                        app.order_result = Some(Ok(response));
                                        app.bracket_state = Some(BracketState::WaitingForFill {
                                            entry_order_id: order_id,
                                        });
                                        app.mode = AppMode::BracketWaiting;
                                    } else {
                                        // EXPIRED, REJECTED, etc.
                                        app.bracket_state = Some(BracketState::Complete {
                                            entry: LegResult::Failed {
                                                error: format!("Entry status: {}", response.status),
                                            },
                                            sl: LegResult::Skipped,
                                            tp: LegResult::Skipped,
                                        });
                                        app.order_result = Some(Ok(response));
                                        app.mode = AppMode::ShowingBracketResult;
                                        pending_bracket = None;
                                    }
                                }
                            }
                        } else {
                            // --- Normal (non-bracket) order result handling ---
                            let is_success = result.is_ok();
                            app.order_result = Some(result);
                            app.mode = AppMode::ShowingResult;

                            // Post-order refresh: re-fetch balance and recent orders on success
                            if is_success {
                                // Refresh balance
                                let refresh_balance_tx = balance_tx.clone();
                                let refresh_creds = credentials.clone();
                                let client_clone = http_client.clone();
                                tokio::spawn(async move {
                                    match network::fetch_account_balance(&refresh_creds, &client_clone).await {
                                        Ok(balances) => {
                                            if let Some(usdt) = balances.iter().find(|b| b.asset == "USDT") {
                                                let _ = refresh_balance_tx.send(Some(usdt.available_balance)).await;
                                            }
                                        }
                                        Err(e) => tracing::warn!("Balance refresh failed: {}", e),
                                    }
                                });
                                // Refresh recent orders
                                let refresh_orders_tx = orders_tx.clone();
                                let refresh_orders_creds = credentials.clone();
                                let refresh_orders_symbol = symbol.clone();
                                let client_clone2 = http_client.clone();
                                tokio::spawn(async move {
                                    match fetch_orders(&refresh_orders_symbol, &refresh_orders_creds, &client_clone2).await {
                                        Ok(asterdex_orders) => {
                                            let mut orders: Vec<Order> = asterdex_orders.into_iter().map(Order::from).collect();
                                            orders.sort_by(|a, b| b.time.cmp(&a.time));
                                            orders.truncate(5);
                                            let _ = refresh_orders_tx.send(orders).await;
                                        }
                                        Err(e) => tracing::warn!("Orders refresh failed: {}", e),
                                    }
                                });
                            }
                        }
                    }

                    // Drain bracket leg results (SL/TP placement outcomes)
                    while let Ok((leg_name, result)) = bracket_leg_rx.try_recv() {
                        bracket_legs_received += 1;
                        let leg_result = match result {
                            Ok(resp) => LegResult::Success {
                                order_id: resp.order_id,
                            },
                            Err(e) => LegResult::Failed {
                                error: e.to_string(),
                            },
                        };
                        match leg_name.as_str() {
                            "SL" => bracket_sl_result = Some(leg_result),
                            "TP" => bracket_tp_result = Some(leg_result),
                            _ => {}
                        }

                        // Check if all legs resolved
                        if bracket_legs_received >= bracket_legs_expected {
                            // Build final BracketState::Complete
                            let entry_result = LegResult::Success {
                                order_id: app
                                    .order_result
                                    .as_ref()
                                    .and_then(|r| r.as_ref().ok())
                                    .map(|r| r.order_id)
                                    .unwrap_or(0),
                            };
                            let sl = bracket_sl_result
                                .take()
                                .unwrap_or(LegResult::Skipped);
                            let tp = bracket_tp_result
                                .take()
                                .unwrap_or(LegResult::Skipped);
                            app.bracket_state = Some(BracketState::Complete {
                                entry: entry_result,
                                sl,
                                tp,
                            });
                            app.mode = AppMode::ShowingBracketResult;
                            pending_bracket = None;

                            // Post-bracket refresh: re-fetch balance
                            let refresh_balance_tx = balance_tx.clone();
                            let refresh_creds = credentials.clone();
                            let client_clone = http_client.clone();
                            tokio::spawn(async move {
                                match network::fetch_account_balance(
                                    &refresh_creds,
                                    &client_clone,
                                )
                                .await
                                {
                                    Ok(balances) => {
                                        if let Some(usdt) =
                                            balances.iter().find(|b| b.asset == "USDT")
                                        {
                                            let _ = refresh_balance_tx
                                                .send(Some(usdt.available_balance))
                                                .await;
                                        }
                                    }
                                    Err(e) => tracing::warn!(
                                        "Post-bracket balance refresh failed: {}",
                                        e
                                    ),
                                }
                            });
                        }
                    }

                    // Clean up pending_bracket if BracketWaiting was cancelled from trade_app
                    // (Esc sets bracket_state to None and mode to Editing)
                    if app.bracket_state.is_none() && pending_bracket.is_some() {
                        pending_bracket = None;
                    }
                }
                Event::Error => {
                    tracing::error!("Terminal I/O error, exiting");
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

    tui_instance.restore();
    Ok(())
}
