// src/run/positions.rs
// Positions subcommand event loop (extracted from main.rs)

use crate::data::order::{Order, OrderSide};
use crate::data::position::{Position, PositionSide, reconcile_positions};
use crate::helpers;
use crate::network::{
    self, place_order, run_multi_mark_price_stream, AsterDexUserStreamManager,
    MarkPriceData, OrderParams, OrderResponse, TradingError,
};
use crate::tui::{self, Event, PositionsApp, Theme};
use crate::tui::positions_app::{ManagementAction, PositionsMode};
use rust_decimal::Decimal;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

pub async fn run(
    effective_symbol: Option<String>,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // Validate AsterDEX credentials (fail fast)
    let credentials = helpers::require_credentials();

    // Detect account position mode (hedge vs one-way)
    let position_mode = network::fetch_position_mode(&credentials, &http_client).await;

    // Fetch positions from AsterDEX REST API
    let positions: Vec<Position> = match helpers::fetch_and_filter_positions(
        &credentials,
        effective_symbol.as_deref(),
        &http_client,
    )
    .await
    {
        Ok(positions) => {
            tracing::info!(count = positions.len(), "Fetched positions from AsterDEX");
            positions
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch positions");
            eprintln!("Error fetching positions: {}", e);
            std::process::exit(1);
        }
    };

    // Extract symbols for mark price stream
    let stream_symbols: Vec<String> = if let Some(ref sym) = effective_symbol {
        vec![sym.clone()]
    } else {
        let mut syms: Vec<String> = positions.iter().map(|p| p.symbol.clone()).collect();
        syms.sort();
        syms.dedup();
        syms
    };

    // Create mark price channel and spawn multi-symbol stream
    let (mark_tx, mut mark_rx) = mpsc::channel::<MarkPriceData>(64);
    let mut mark_price_handle: Option<tokio::task::JoinHandle<()>> = None;
    if !stream_symbols.is_empty() {
        let symbols = stream_symbols.clone();
        let tx = mark_tx.clone();
        mark_price_handle = Some(tokio::spawn(async move {
            run_multi_mark_price_stream(symbols, tx).await;
        }));
    }
    let mut current_stream_symbols = stream_symbols;

    // Create PositionsApp
    let mut app = PositionsApp::new(effective_symbol.clone(), positions, theme);

    // Order submission channels for position management actions
    let (submit_tx, mut submit_rx) = mpsc::channel::<(OrderParams, bool)>(1);
    let (order_result_tx, mut order_result_rx) =
        mpsc::channel::<Result<OrderResponse, TradingError>>(1);
    app.submit_tx = Some(submit_tx);

    // Create refresh and result channels
    let (refresh_tx, mut refresh_rx) = mpsc::channel::<()>(1);
    let (position_result_tx, mut position_result_rx) = mpsc::channel::<Vec<Position>>(1);
    app.refresh_tx = Some(refresh_tx);

    // Create ACCOUNT_UPDATE channel for real-time position updates (RT-01)
    let (account_update_tx, mut account_update_rx) =
        mpsc::channel::<network::AccountUpdateData>(64);

    // Create order channel (user stream requires it, but positions view doesn't use it)
    let (ws_order_tx, _ws_order_rx) = mpsc::channel::<Order>(32);

    // Spawn user data stream for ACCOUNT_UPDATE events
    let (mut user_stream, _user_stream_status_rx) = AsterDexUserStreamManager::new(
        credentials.api_key.clone(),
        effective_symbol.clone(),
        ws_order_tx,
        Some(account_update_tx),
        http_client.clone(),
    );
    tokio::spawn(async move {
        user_stream.run_with_reconnect().await;
    });

    // Spawn periodic REST reconciliation (RT-03: reduced from 30s to 60s)
    {
        let recon_creds = credentials.clone();
        let recon_symbol = effective_symbol.clone();
        let recon_tx = mpsc::Sender::clone(&position_result_tx);
        let client_for_recon = http_client.clone();
        tokio::spawn(async move {
            let mut timer = interval(Duration::from_secs(60));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            timer.tick().await; // Skip first immediate tick

            loop {
                timer.tick().await;
                tracing::debug!("Starting position reconciliation");
                match helpers::fetch_and_filter_positions(
                    &recon_creds,
                    recon_symbol.as_deref(),
                    &client_for_recon,
                )
                .await
                {
                    Ok(positions) => {
                        tracing::info!(
                            count = positions.len(),
                            "Reconciliation fetched positions"
                        );
                        if recon_tx.send(positions).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Position reconciliation failed, will retry in 60s");
                    }
                }
            }
        });
    }

    // Initialize terminal and event handler
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Positions event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        tui::ui_positions(frame, &mut app);
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Resize(_w, _h) => {}
                Event::Tick => {
                    // 1. Drain mark price mpsc channel (while-let to process all pending)
                    while let Ok(data) = mark_rx.try_recv() {
                        app.update_mark_price(&data);
                    }

                    // 2. Drain ACCOUNT_UPDATE events for real-time position updates (RT-01)
                    let mut positions_changed_from_ws = false;
                    while let Ok(update) = account_update_rx.try_recv() {
                        for ws_pos in &update.positions {
                            // Filter: in single-pair mode, skip deltas for other symbols
                            if let Some(ref target) = effective_symbol {
                                if !ws_pos.symbol.eq_ignore_ascii_case(target) {
                                    continue;
                                }
                            }

                            let side = if ws_pos.position_side == "LONG" {
                                PositionSide::Long
                            } else if ws_pos.position_side == "SHORT" {
                                PositionSide::Short
                            } else {
                                if ws_pos.position_amt < Decimal::ZERO {
                                    PositionSide::Short
                                } else {
                                    PositionSide::Long
                                }
                            };

                            if ws_pos.position_amt.is_zero() {
                                // Position closed: remove it
                                let before = app.positions.len();
                                app.positions.retain(|p| {
                                    !(p.symbol == ws_pos.symbol && p.position_side == side)
                                });
                                if app.positions.len() != before {
                                    positions_changed_from_ws = true;
                                    tracing::info!(symbol = %ws_pos.symbol, "Position closed via ACCOUNT_UPDATE");
                                }
                            } else if let Some(existing) = app
                                .positions
                                .iter_mut()
                                .find(|p| p.symbol == ws_pos.symbol && p.position_side == side)
                            {
                                // Update structural fields -- do NOT overwrite mark_price or funding
                                existing.position_amt = ws_pos.position_amt;
                                existing.entry_price = ws_pos.entry_price;
                                existing.unrealized_profit = ws_pos.unrealized_pnl;
                                existing.isolated_margin = ws_pos.isolated_wallet;
                                existing.margin_type = ws_pos.margin_type.clone();
                                existing.notional = existing.mark_price * ws_pos.position_amt;
                                positions_changed_from_ws = true;
                                tracing::debug!(symbol = %ws_pos.symbol, amt = %ws_pos.position_amt, "Position updated via ACCOUNT_UPDATE");
                            } else {
                                // Brand new position (first fill on new symbol)
                                let new_pos = Position::from_ws_update(ws_pos);
                                tracing::info!(symbol = %ws_pos.symbol, amt = %ws_pos.position_amt, "New position from ACCOUNT_UPDATE");
                                app.positions.push(new_pos);
                                positions_changed_from_ws = true;
                            }
                        }
                    }

                    // If positions changed from WebSocket, fix table selection and check mark price respawn
                    if positions_changed_from_ws {
                        if !app.positions.is_empty() {
                            if let Some(selected) = app.table_state.selected() {
                                if selected >= app.positions.len() {
                                    app.table_state.select(Some(0));
                                }
                            }
                        } else {
                            app.table_state.select(None);
                        }

                        let mut new_symbols: Vec<String> =
                            app.positions.iter().map(|p| p.symbol.clone()).collect();
                        new_symbols.sort();
                        new_symbols.dedup();
                        if new_symbols != current_stream_symbols {
                            tracing::info!(
                                old_count = current_stream_symbols.len(),
                                new_count = new_symbols.len(),
                                "Position symbol set changed from ACCOUNT_UPDATE, respawning mark price stream"
                            );
                            if let Some(handle) = mark_price_handle.take() {
                                handle.abort();
                            }
                            if !new_symbols.is_empty() {
                                let tx = mark_tx.clone();
                                let syms = new_symbols.clone();
                                mark_price_handle = Some(tokio::spawn(async move {
                                    run_multi_mark_price_stream(syms, tx).await;
                                }));
                            }
                            current_stream_symbols = new_symbols;
                        }
                    }

                    // 3. Check for manual refresh requests
                    if let Ok(()) = refresh_rx.try_recv() {
                        let refresh_creds = credentials.clone();
                        let refresh_symbol = effective_symbol.clone();
                        let tx = position_result_tx.clone();
                        let client_clone = http_client.clone();
                        tokio::spawn(async move {
                            match helpers::fetch_and_filter_positions(
                                &refresh_creds,
                                refresh_symbol.as_deref(),
                                &client_clone,
                            )
                            .await
                            {
                                Ok(positions) => {
                                    let _ = tx.send(positions).await;
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "Manual position refresh failed");
                                }
                            }
                        });
                    }

                    // 4. Receive refreshed/reconciled positions and merge
                    while let Ok(fresh_positions) = position_result_rx.try_recv() {
                        let merged = reconcile_positions(&app.positions, fresh_positions);
                        tracing::info!(count = merged.len(), "Applied position reconciliation");
                        app.positions = merged;
                        if !app.positions.is_empty() {
                            // Preserve selection if still valid, otherwise select first
                            if let Some(selected) = app.table_state.selected() {
                                if selected >= app.positions.len() {
                                    app.table_state.select(Some(0));
                                }
                            }
                        } else {
                            app.table_state.select(None);
                        }

                        // Check if symbol set changed (POS-C1)
                        let mut new_symbols: Vec<String> =
                            app.positions.iter().map(|p| p.symbol.clone()).collect();
                        new_symbols.sort();
                        new_symbols.dedup();

                        if new_symbols != current_stream_symbols {
                            tracing::info!(
                                old_count = current_stream_symbols.len(),
                                new_count = new_symbols.len(),
                                "Position symbol set changed, respawning mark price stream"
                            );
                            // Abort old mark price task
                            if let Some(handle) = mark_price_handle.take() {
                                handle.abort();
                            }
                            // Spawn new stream with updated symbols (if any)
                            if !new_symbols.is_empty() {
                                let tx = mark_tx.clone();
                                let syms = new_symbols.clone();
                                mark_price_handle = Some(tokio::spawn(async move {
                                    run_multi_mark_price_stream(syms, tx).await;
                                }));
                            }
                            current_stream_symbols = new_symbols;
                        }
                    }

                    // 5. Check for order submission from position management
                    if let Ok((params, reduce_only)) = submit_rx.try_recv() {
                        let creds = credentials.clone();
                        let tx = order_result_tx.clone();
                        let is_reverse = matches!(
                            app.current_action,
                            Some(ManagementAction::ReversePosition)
                        );
                        let client_clone = http_client.clone();

                        if is_reverse {
                            // Reverse position: two sequential orders in one spawned task
                            // Step 1: Close current position (reduceOnly=true)
                            // Step 2: Open opposite (same qty from step 1 response, reduceOnly=false)
                            let snapshot_symbol = app
                                .position_snapshot
                                .as_ref()
                                .map(|s| s.symbol.clone())
                                .unwrap_or_default();
                            let close_params = params;
                            let pm = position_mode;

                            tokio::spawn(async move {
                                // Step 1: Close at market with reduceOnly
                                let close_result =
                                    place_order(&close_params, &creds, true, pm, &client_clone)
                                        .await;
                                match close_result {
                                    Ok(close_response) => {
                                        // Use executed_qty from close response for the open leg
                                        // (handles partial fills per Pitfall 4 from research)
                                        let open_qty = close_response.executed_qty;
                                        // Open leg uses SAME side as close leg:
                                        // Close LONG = SELL -> open SHORT = SELL (positionSide=SHORT in hedge mode)
                                        // Close SHORT = BUY -> open LONG = BUY (positionSide=LONG in hedge mode)
                                        let open_side = if close_response.side == "BUY" {
                                            OrderSide::Buy
                                        } else {
                                            OrderSide::Sell
                                        };
                                        let open_params = OrderParams::Market {
                                            symbol: snapshot_symbol,
                                            side: open_side,
                                            quantity: open_qty,
                                        };
                                        // Step 2: Open opposite position (NOT reduceOnly)
                                        let open_result = place_order(
                                            &open_params,
                                            &creds,
                                            false,
                                            pm,
                                            &client_clone,
                                        )
                                        .await;
                                        let _ = tx.send(open_result).await;
                                    }
                                    Err(e) => {
                                        // Close failed -- do NOT open opposite
                                        // (per research: two orders are not atomic)
                                        let _ = tx.send(Err(e)).await;
                                    }
                                }
                            });
                        } else {
                            // Single order: SL, TP, or Close
                            let pm = position_mode;
                            tokio::spawn(async move {
                                let result =
                                    place_order(&params, &creds, reduce_only, pm, &client_clone)
                                        .await;
                                let _ = tx.send(result).await;
                            });
                        }
                    }

                    // 6. Receive order results
                    if let Ok(result) = order_result_rx.try_recv() {
                        let is_success = result.is_ok();
                        app.order_result = Some(result);
                        app.mode = PositionsMode::ShowingResult;

                        // Trigger position refresh on success (POS-M1)
                        if is_success {
                            let refresh_creds = credentials.clone();
                            let refresh_symbol = effective_symbol.clone();
                            let tx = position_result_tx.clone();
                            let client_clone = http_client.clone();
                            tokio::spawn(async move {
                                // Brief delay to let exchange process the order (Pitfall 3)
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                match helpers::fetch_and_filter_positions(
                                    &refresh_creds,
                                    refresh_symbol.as_deref(),
                                    &client_clone,
                                )
                                .await
                                {
                                    Ok(positions) => {
                                        let _ = tx.send(positions).await;
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            error = %e,
                                            "Post-order position refresh failed"
                                        );
                                    }
                                }
                            });
                        }
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
