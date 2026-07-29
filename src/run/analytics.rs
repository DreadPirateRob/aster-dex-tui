// src/run/analytics.rs
// Session analytics subcommand event loop
//
// Connects to authenticated user stream and mark price stream, bootstraps
// from REST (account info, positions, user trades), and processes events
// into SessionAnalytics for live PnL tracking. Renders the full dashboard
// via AnalyticsApp and AnalyticsDashboardWidget.

use crate::data::SessionAnalytics;
use crate::data::order::Order;
use crate::helpers;
use crate::network::{
    fetch_account_info, fetch_positions, fetch_user_trades, run_multi_mark_price_stream,
    AsterDexUserStreamManager, MarkPriceData,
};
use crate::tui::widgets::AnalyticsDashboardWidget;
use crate::tui::{AnalyticsApp, Event, Theme};
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::io;
use tokio::sync::mpsc;

pub async fn run(
    symbol_filter: Option<String>,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // Step 1: Credential validation (fail fast)
    let credentials = helpers::require_credentials();

    // Step 2: Record session start time for bootstrap trade window
    let session_start_ms = chrono::Utc::now().timestamp_millis() as u64;

    // Step 3: REST bootstrap (concurrent via tokio::join!)
    let (account_res, positions_res, trades_res) = tokio::join!(
        fetch_account_info(&credentials, &http_client),
        fetch_positions(&credentials, &http_client),
        fetch_user_trades(session_start_ms, &credentials, &http_client),
    );

    // Extract starting wallet balance from account info
    let starting_wallet_balance = match &account_res {
        Ok(info) => info.total_wallet_balance,
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch account info at startup");
            Decimal::ZERO
        }
    };

    // Initialize SessionAnalytics
    let mut analytics = SessionAnalytics::new(starting_wallet_balance);

    // Process bootstrap trades into SessionAnalytics
    let mut max_trade_time: u64 = session_start_ms;
    if let Ok(trades) = &trades_res {
        for trade in trades {
            if trade.realized_pnl != Decimal::ZERO {
                analytics.total_realized_pnl += trade.realized_pnl;
                analytics.trade_count += 1;
                if trade.realized_pnl > Decimal::ZERO {
                    analytics.winning_trades += 1;
                    analytics.total_win_amount += trade.realized_pnl;
                } else {
                    analytics.losing_trades += 1;
                    analytics.total_loss_amount += trade.realized_pnl.abs();
                }
            }
            analytics.total_commissions += trade.commission;

            if trade.time > max_trade_time {
                max_trade_time = trade.time;
            }
        }
        tracing::info!(
            trade_count = trades.len(),
            realized_pnl = %analytics.total_realized_pnl,
            commissions = %analytics.total_commissions,
            "Bootstrapped session analytics from REST trades"
        );
    } else if let Err(e) = &trades_res {
        tracing::error!(error = %e, "Failed to fetch user trades at startup");
    }

    // Set bootstrap cutoff to prevent double-counting from WS events
    analytics.bootstrap_cutoff_time = max_trade_time;

    // Compute initial unrealized PnL from bootstrap positions
    let mut position_symbols: HashSet<String> = HashSet::new();
    if let Ok(positions) = &positions_res {
        let mut total_unrealized = Decimal::ZERO;
        for pos in positions {
            if pos.position_amt.is_zero() {
                continue;
            }
            // Apply symbol filter if present
            if let Some(ref filter) = symbol_filter {
                if !pos.symbol.eq_ignore_ascii_case(filter) {
                    continue;
                }
            }
            total_unrealized += pos.un_realized_profit;
            position_symbols.insert(pos.symbol.clone());
        }
        analytics.current_unrealized_pnl = total_unrealized;
        tracing::info!(
            unrealized_pnl = %total_unrealized,
            position_count = position_symbols.len(),
            "Bootstrapped initial unrealized PnL from positions"
        );
    } else if let Err(e) = &positions_res {
        tracing::error!(error = %e, "Failed to fetch positions at startup");
    }

    // Create AnalyticsApp (replaces Plan 01 placeholder)
    let mut app = AnalyticsApp::new(analytics, theme, symbol_filter.clone());

    // Step 4: Spawn WebSocket streams

    // User stream (account-wide, no symbol filter)
    let (order_tx, mut order_rx) = mpsc::channel::<Order>(64);
    let (account_update_tx, mut account_update_rx) =
        mpsc::channel::<crate::network::AccountUpdateData>(64);

    let (mut user_stream, _user_stream_status_rx) = AsterDexUserStreamManager::new(
        credentials.api_key.clone(),
        None, // Account-wide for all symbols
        order_tx,
        Some(account_update_tx),
        http_client.clone(),
    );
    tokio::spawn(async move {
        user_stream.run_with_reconnect().await;
    });

    // Mark price stream for position symbols
    let (mark_tx, mut mark_rx) = mpsc::channel::<MarkPriceData>(64);
    let mut current_stream_symbols: Vec<String> = position_symbols.iter().cloned().collect();
    current_stream_symbols.sort();
    let mut mark_price_handle: Option<tokio::task::JoinHandle<()>> = None;
    if !current_stream_symbols.is_empty() {
        let symbols = current_stream_symbols.clone();
        let tx = mark_tx.clone();
        mark_price_handle = Some(tokio::spawn(async move {
            run_multi_mark_price_stream(symbols, tx).await;
        }));
    }

    // Step 5: Terminal init
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Step 6: Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        let widget = AnalyticsDashboardWidget::new(&app);
                        frame.render_widget(widget, frame.area());
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Tick => {
                    // Drain order events (ORDER_TRADE_UPDATE fills)
                    // Order now carries realized_pnl and commission from the WS payload
                    while let Ok(order) = order_rx.try_recv() {
                        // Only process FILLED or PARTIALLY_FILLED
                        if order.status != crate::data::order::OrderStatus::Filled
                            && order.status != crate::data::order::OrderStatus::PartiallyFilled
                        {
                            continue;
                        }

                        // Guard against bootstrap double-counting
                        if order.time <= app.analytics.bootstrap_cutoff_time {
                            continue;
                        }

                        // Apply symbol filter
                        if let Some(ref filter) = app.symbol_filter {
                            if !order.symbol.eq_ignore_ascii_case(filter) {
                                continue;
                            }
                        }

                        // Accumulate commission
                        app.analytics.total_commissions += order.commission;

                        // Track realized PnL (only non-zero rp counts as trade)
                        if order.realized_pnl != Decimal::ZERO {
                            app.analytics.total_realized_pnl += order.realized_pnl;
                            app.analytics.trade_count += 1;
                            if order.realized_pnl > Decimal::ZERO {
                                app.analytics.winning_trades += 1;
                                app.analytics.total_win_amount += order.realized_pnl;
                            } else {
                                app.analytics.losing_trades += 1;
                                app.analytics.total_loss_amount += order.realized_pnl.abs();
                            }
                        }

                        // Push to recent fills
                        use crate::data::SessionFill;
                        let side_str = match order.side {
                            crate::data::order::OrderSide::Buy => "BUY",
                            crate::data::order::OrderSide::Sell => "SELL",
                        };
                        app.analytics.recent_fills.push_back(SessionFill {
                            symbol: order.symbol.clone(),
                            side: side_str.to_string(),
                            price: order.avg_price,
                            quantity: order.executed_qty,
                            realized_pnl: order.realized_pnl,
                            commission: order.commission,
                            time: order.time,
                        });
                        if app.analytics.recent_fills.len() > 200 {
                            app.analytics.recent_fills.pop_front();
                        }

                        tracing::debug!(
                            symbol = %order.symbol,
                            realized_pnl = %order.realized_pnl,
                            commission = %order.commission,
                            "Processed fill in analytics"
                        );
                    }

                    // Drain ACCOUNT_UPDATE events for unrealized PnL
                    let mut symbols_changed = false;
                    while let Ok(update) = account_update_rx.try_recv() {
                        let mut total_unrealized = Decimal::ZERO;
                        let mut has_position_data = false;
                        for ws_pos in &update.positions {
                            // Apply symbol filter
                            if let Some(ref filter) = app.symbol_filter {
                                if !ws_pos.symbol.eq_ignore_ascii_case(filter) {
                                    continue;
                                }
                            }
                            has_position_data = true;
                            total_unrealized += ws_pos.unrealized_pnl;

                            // Track new position symbols
                            if !ws_pos.position_amt.is_zero()
                                && !position_symbols.contains(&ws_pos.symbol)
                            {
                                position_symbols.insert(ws_pos.symbol.clone());
                                symbols_changed = true;
                            }
                            // Remove closed positions
                            if ws_pos.position_amt.is_zero()
                                && position_symbols.contains(&ws_pos.symbol)
                            {
                                position_symbols.remove(&ws_pos.symbol);
                                symbols_changed = true;
                            }
                        }
                        if has_position_data {
                            app.analytics.current_unrealized_pnl = total_unrealized;
                        }
                    }

                    // Respawn mark price stream if position symbols changed
                    if symbols_changed {
                        let mut new_syms: Vec<String> =
                            position_symbols.iter().cloned().collect();
                        new_syms.sort();
                        if new_syms != current_stream_symbols {
                            tracing::info!(
                                old_count = current_stream_symbols.len(),
                                new_count = new_syms.len(),
                                "Position symbol set changed, respawning mark price stream"
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

                    // Drain mark price updates (ACCOUNT_UPDATE provides unrealized PnL directly)
                    while let Ok(_data) = mark_rx.try_recv() {}

                    // Equity snapshot
                    if app.analytics.should_snapshot_equity() {
                        app.analytics.snapshot_equity();
                    }
                }
                Event::Resize(_w, _h) => {}
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

    // Step 7: Clean shutdown
    if let Some(handle) = mark_price_handle.take() {
        handle.abort();
    }
    tui_instance.restore();

    Ok(())
}
