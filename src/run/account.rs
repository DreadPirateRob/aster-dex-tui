// src/run/account.rs
// Account subcommand event loop (extracted from main.rs)

use crate::data::account::build_account_summary;
use crate::data::order::Order;
use crate::data::position::{Position, PositionSide};
use crate::helpers;
use crate::network::{
    self, compute_margin_ratio, fetch_account_info, fetch_positions, fetch_user_trades,
    run_multi_mark_price_stream, start_of_today_utc_ms, AccountInfo, AsterDexUserStreamManager,
    AsterDexUserTrade, MarkPriceData, TradingError,
};
use crate::tui::{AccountApp, Event, Theme};
use crate::tui::widgets::ui_account;
use rust_decimal::Decimal;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

/// AccountData uses Option for trades (only fetched every refresh tick).
struct AccountData {
    account_info: Result<AccountInfo, TradingError>,
    positions: Result<Vec<network::AsterDexPosition>, TradingError>,
    trades: Option<Result<Vec<AsterDexUserTrade>, TradingError>>,
}

pub async fn run(theme: Theme, http_client: reqwest::Client) -> io::Result<()> {
    // Step 1: Credential validation (fail fast)
    let credentials = helpers::require_credentials();

    // Step 2: Initial data fetch at startup (concurrent via tokio::join!)
    // NOTE: /fapi/v2/balance eliminated -- wallet_balance and available_balance
    // come from AccountInfo fields (ACCT-M3, ACCT-L7)
    let (account_res, positions_res, trades_res) = tokio::join!(
        fetch_account_info(&credentials, &http_client),
        fetch_positions(&credentials, &http_client),
        fetch_user_trades(start_of_today_utc_ms(), &credentials, &http_client),
    );

    // Create AccountApp
    let mut app = AccountApp::new(theme);

    // Process initial results with failure counting (ACCT-M1)
    let mut failure_count: u8 = 0;

    // Extract account info and compute margin ratio
    let margin_ratio = match &account_res {
        Ok(info) => {
            let maint = info.total_maint_margin.unwrap_or(Decimal::ZERO);
            let balance = info.total_margin_balance.unwrap_or(Decimal::ZERO);
            compute_margin_ratio(maint, balance)
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch account info at startup");
            failure_count += 1;
            Decimal::ZERO
        }
    };

    // Extract wallet_balance and available_balance from AccountInfo (replaces separate /fapi/v2/balance call)
    let (wallet_balance, available_balance) = match &account_res {
        Ok(info) => (info.total_wallet_balance, info.available_balance),
        Err(_) => (Decimal::ZERO, Decimal::ZERO), // already logged above
    };

    // Extract positions (filter zero-size)
    let positions: Vec<Position> = match positions_res {
        Ok(api_positions) => {
            let mut positions: Vec<Position> = api_positions
                .into_iter()
                .map(Position::from)
                .collect();
            positions.retain(|p| !p.position_amt.is_zero());
            tracing::info!(count = positions.len(), "Fetched positions for account overview");
            positions
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch positions at startup");
            failure_count += 1;
            Vec::new()
        }
    };

    // Extract realized_pnl values from trades
    let realized_pnls: Vec<Decimal> = match &trades_res {
        Ok(trades) => trades.iter().map(|t| t.realized_pnl).collect(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch user trades at startup");
            failure_count += 1;
            Vec::new()
        }
    };

    // Build summary and update app
    let summary = build_account_summary(
        wallet_balance,
        available_balance,
        &positions,
        &realized_pnls,
    );

    let total_endpoints: u8 = 3; // account_info, positions, trades
    if failure_count == total_endpoints {
        // ALL endpoints failed -- truly unreachable
        app.set_unreachable();
        app.set_error_message("All data sources failed".to_string());
    } else if failure_count > 0 {
        app.set_error_message("Some data could not be fetched".to_string());
        // Still populate what we have
        app.summary = Some(summary);
        app.positions = positions.clone();
        app.margin_ratio = margin_ratio;
    } else {
        app.update_data(summary, positions.clone(), margin_ratio);
    }

    // Spawn mark price WebSocket stream for position symbols (ACCT-C1)
    let (mark_tx, mut mark_rx) = mpsc::channel::<MarkPriceData>(64);
    let mut current_stream_symbols: Vec<String> = {
        let mut syms: Vec<String> = app.positions.iter().map(|p| p.symbol.clone()).collect();
        syms.sort();
        syms.dedup();
        syms
    };
    let mut mark_price_handle: Option<tokio::task::JoinHandle<()>> = None;
    if !current_stream_symbols.is_empty() {
        let symbols = current_stream_symbols.clone();
        let tx = mark_tx.clone();
        mark_price_handle = Some(tokio::spawn(async move {
            run_multi_mark_price_stream(symbols, tx).await;
        }));
    }

    // Create ACCOUNT_UPDATE channel for real-time balance/position updates (RT-02)
    let (account_update_tx, mut account_update_rx) =
        mpsc::channel::<network::AccountUpdateData>(64);

    // Create order channel (user stream requires it, but account view doesn't use orders)
    let (ws_order_tx_acct, _ws_order_rx_acct) = mpsc::channel::<Order>(32);

    // Spawn user data stream for ACCOUNT_UPDATE events
    let (mut user_stream, _user_stream_status_rx) = AsterDexUserStreamManager::new(
        credentials.api_key.clone(),
        None, // Account view monitors all symbols
        ws_order_tx_acct,
        Some(account_update_tx),
        http_client.clone(),
    );
    tokio::spawn(async move {
        user_stream.run_with_reconnect().await;
    });

    // Step 3: Terminal init
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Step 4: Spawn auto-refresh background task
    // Create data channel outside spawn so we can clone data_tx for manual refresh
    let (data_tx, mut data_rx) = mpsc::channel::<AccountData>(4);
    {
        let refresh_creds = credentials.clone();
        let client_for_refresh = http_client.clone();
        let auto_data_tx = data_tx.clone();
        tokio::spawn(async move {
            let mut timer = interval(Duration::from_secs(30));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Skip first immediate tick (initial fetch already done at startup)
            timer.tick().await;

            loop {
                timer.tick().await;
                let fetch_trades = true; // every 30s (RT-03: reduced from 10s auto-refresh)
                tracing::debug!(fetch_trades, "Starting account auto-refresh");

                let (account_info, positions) = tokio::join!(
                    fetch_account_info(&refresh_creds, &client_for_refresh),
                    fetch_positions(&refresh_creds, &client_for_refresh),
                );

                let trades = if fetch_trades {
                    Some(
                        fetch_user_trades(
                            start_of_today_utc_ms(),
                            &refresh_creds,
                            &client_for_refresh,
                        )
                        .await,
                    )
                } else {
                    None
                };

                let data = AccountData {
                    account_info,
                    positions,
                    trades,
                };

                if auto_data_tx.send(data).await.is_err() {
                    break; // Receiver dropped, main loop closed
                }
            }
        });
    }

    // Create manual refresh channel and assign to app (ACCT-L1)
    let (refresh_tx, mut refresh_rx) = mpsc::channel::<()>(4);
    app.refresh_tx = Some(refresh_tx);

    // Step 5: Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        ui_account(frame, &app);
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Resize(_w, _h) => {}
                Event::Tick => {
                    // Drain mark price updates (ACCT-C1)
                    while let Ok(data) = mark_rx.try_recv() {
                        app.update_mark_price(&data);
                    }

                    // Drain ACCOUNT_UPDATE events for real-time balance/position updates (RT-02)
                    let mut account_ws_changed = false;
                    while let Ok(update) = account_update_rx.try_recv() {
                        // Apply balance deltas
                        for ws_bal in &update.balances {
                            if ws_bal.asset == "USDT" {
                                if let Some(ref mut summary) = app.summary {
                                    summary.wallet_balance = ws_bal.wallet_balance;
                                    tracing::debug!(
                                        wallet_balance = %ws_bal.wallet_balance,
                                        balance_change = %ws_bal.balance_change,
                                        reason = %update.reason,
                                        "Account balance updated via ACCOUNT_UPDATE"
                                    );
                                    account_ws_changed = true;
                                }
                            }
                        }

                        // Apply position deltas (same logic as positions view)
                        for ws_pos in &update.positions {
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
                                // Position closed
                                let before = app.positions.len();
                                app.positions.retain(|p| {
                                    !(p.symbol == ws_pos.symbol && p.position_side == side)
                                });
                                if app.positions.len() != before {
                                    account_ws_changed = true;
                                    tracing::info!(symbol = %ws_pos.symbol, "Position closed via ACCOUNT_UPDATE (account view)");
                                }
                            } else if let Some(existing) = app
                                .positions
                                .iter_mut()
                                .find(|p| p.symbol == ws_pos.symbol && p.position_side == side)
                            {
                                existing.position_amt = ws_pos.position_amt;
                                existing.entry_price = ws_pos.entry_price;
                                existing.unrealized_profit = ws_pos.unrealized_pnl;
                                existing.isolated_margin = ws_pos.isolated_wallet;
                                existing.margin_type = ws_pos.margin_type.clone();
                                existing.notional = existing.mark_price * ws_pos.position_amt;
                                account_ws_changed = true;
                            } else {
                                // New position
                                let new_pos = Position::from_ws_update(ws_pos);
                                app.positions.push(new_pos);
                                account_ws_changed = true;
                                tracing::info!(symbol = %ws_pos.symbol, "New position from ACCOUNT_UPDATE (account view)");
                            }
                        }
                    }

                    // If account data changed from WebSocket, recompute derived summary fields
                    if account_ws_changed {
                        if let Some(ref summary) = app.summary {
                            let new_summary = build_account_summary(
                                summary.wallet_balance,
                                summary.available_balance,
                                &app.positions,
                                &[summary.daily_realized_pnl],
                            );
                            app.summary = Some(new_summary);
                        }

                        // Check symbol set change for mark price respawn
                        let mut new_syms: Vec<String> =
                            app.positions.iter().map(|p| p.symbol.clone()).collect();
                        new_syms.sort();
                        new_syms.dedup();
                        if new_syms != current_stream_symbols {
                            tracing::info!(
                                old_count = current_stream_symbols.len(),
                                new_count = new_syms.len(),
                                "Account position symbol set changed from ACCOUNT_UPDATE, respawning mark price stream"
                            );
                            if let Some(handle) = mark_price_handle.take() {
                                handle.abort();
                            }
                            current_stream_symbols = new_syms.clone();
                            if !new_syms.is_empty() {
                                let tx = mark_tx.clone();
                                mark_price_handle = Some(tokio::spawn(async move {
                                    run_multi_mark_price_stream(new_syms, tx).await;
                                }));
                            }
                        }
                    }

                    // Check for manual refresh request (ACCT-L1)
                    if let Ok(()) = refresh_rx.try_recv() {
                        let refresh_creds = credentials.clone();
                        let client = http_client.clone();
                        let tx = data_tx.clone();
                        let trades_start = start_of_today_utc_ms();
                        tokio::spawn(async move {
                            let (account_info, positions, trades) = tokio::join!(
                                fetch_account_info(&refresh_creds, &client),
                                fetch_positions(&refresh_creds, &client),
                                fetch_user_trades(trades_start, &refresh_creds, &client),
                            );
                            let data = AccountData {
                                account_info,
                                positions,
                                trades: Some(trades),
                            };
                            let _ = tx.send(data).await;
                        });
                    }

                    // Drain auto-refresh channel
                    while let Ok(data) = data_rx.try_recv() {
                        let mut failure_count: u8 = 0;

                        // Process account info -> margin_ratio + wallet/available balance
                        let (mr, wb, ab) = match &data.account_info {
                            Ok(info) => {
                                let maint = info.total_maint_margin.unwrap_or(Decimal::ZERO);
                                let bal = info.total_margin_balance.unwrap_or(Decimal::ZERO);
                                (
                                    compute_margin_ratio(maint, bal),
                                    info.total_wallet_balance,
                                    info.available_balance,
                                )
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Account info refresh failed");
                                failure_count += 1;
                                // Keep existing values
                                let (prev_wb, prev_ab) = match &app.summary {
                                    Some(s) => (s.wallet_balance, s.available_balance),
                                    None => (Decimal::ZERO, Decimal::ZERO),
                                };
                                (app.margin_ratio, prev_wb, prev_ab)
                            }
                        };

                        // Process positions
                        let pos: Vec<Position> = match data.positions {
                            Ok(api_positions) => {
                                let mut positions: Vec<Position> = api_positions
                                    .into_iter()
                                    .map(Position::from)
                                    .collect();
                                positions.retain(|p| !p.position_amt.is_zero());
                                positions
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Positions refresh failed");
                                failure_count += 1;
                                app.positions.clone() // keep existing
                            }
                        };

                        // Process trades -> realized pnls (only when present)
                        let trades_failed = matches!(&data.trades, Some(Err(_)));
                        let rpnls: Vec<Decimal> = match &data.trades {
                            Some(Ok(trades)) => {
                                trades.iter().map(|t| t.realized_pnl).collect()
                            }
                            Some(Err(e)) => {
                                tracing::warn!(error = %e, "Trades refresh failed");
                                failure_count += 1;
                                Vec::new() // patched below
                            }
                            None => Vec::new(), // intentionally skipped this tick
                        };

                        // Build summary
                        let mut summary = build_account_summary(wb, ab, &pos, &rpnls);

                        // Preserve daily_realized_pnl if trades fetch failed or was skipped (ACCT-C2 fix)
                        if trades_failed || data.trades.is_none() {
                            if let Some(ref prev_summary) = app.summary {
                                summary.daily_realized_pnl = prev_summary.daily_realized_pnl;
                            }
                        }

                        // Apply partial failure handling (ACCT-M1)
                        let total_endpoints: u8 =
                            if data.trades.is_some() { 3 } else { 2 };
                        if failure_count == total_endpoints {
                            // ALL endpoints failed -- truly unreachable
                            app.set_unreachable();
                            app.set_error_message("All data sources failed".to_string());
                            // Don't update data -- keep previous values entirely
                        } else if failure_count > 0 {
                            // Partial failure -- API is reachable but some data stale
                            app.update_data(summary, pos.clone(), mr);
                            app.set_error_message(format!(
                                "{} of {} data sources failed",
                                failure_count, total_endpoints
                            ));
                        } else {
                            // Full success
                            app.update_data(summary, pos.clone(), mr);
                            app.status_message = None; // clear "Refreshing..." feedback
                        }

                        // Check if symbol set changed and respawn mark price stream (Pitfall 2)
                        let mut new_syms: Vec<String> =
                            app.positions.iter().map(|p| p.symbol.clone()).collect();
                        new_syms.sort();
                        new_syms.dedup();
                        if new_syms != current_stream_symbols {
                            tracing::info!(
                                old_count = current_stream_symbols.len(),
                                new_count = new_syms.len(),
                                "Account position symbol set changed, respawning mark price stream"
                            );
                            if let Some(handle) = mark_price_handle.take() {
                                handle.abort();
                            }
                            current_stream_symbols = new_syms.clone();
                            if !new_syms.is_empty() {
                                let tx = mark_tx.clone();
                                mark_price_handle = Some(tokio::spawn(async move {
                                    run_multi_mark_price_stream(new_syms, tx).await;
                                }));
                            }
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

    // Step 6: Terminal cleanup
    tui_instance.restore();

    Ok(())
}
