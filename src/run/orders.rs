// src/run/orders.rs
// Orders subcommand event loop (extracted from main.rs)

use crate::data::order::Order;
use crate::helpers;
use crate::network::{
    cancel_order, fetch_open_orders, fetch_orders, user_friendly_cancel_error,
    AsterDexUserStreamManager,
};
use crate::tui::{self, Event, Theme};
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

    // Fetch orders from AsterDEX REST API (mode-aware)
    let orders = match &effective_symbol {
        Some(sym) => match fetch_orders(sym, &credentials, &http_client).await {
            Ok(api_orders) => {
                let mut orders: Vec<Order> = api_orders.into_iter().map(Order::from).collect();
                orders.sort_by(|a, b| b.time.cmp(&a.time));
                tracing::info!(count = orders.len(), symbol = %sym, "Fetched orders from AsterDEX");
                orders
            }
            Err(e) => {
                tracing::error!(error = %e, symbol = %sym, "Failed to fetch orders");
                eprintln!("Error fetching orders for {}: {}", sym, e);
                std::process::exit(1);
            }
        },
        None => match fetch_open_orders(&credentials, &http_client).await {
            Ok(api_orders) => {
                let mut orders: Vec<Order> = api_orders.into_iter().map(Order::from).collect();
                orders.sort_by(|a, b| b.time.cmp(&a.time));
                tracing::info!(count = orders.len(), "Fetched open orders for all pairs");
                orders
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to fetch open orders");
                eprintln!("Error fetching open orders: {}", e);
                std::process::exit(1);
            }
        },
    };

    // Create WebSocket order channel (before app takes ownership of symbol)
    let (ws_order_tx, mut ws_order_rx) = mpsc::channel::<Order>(32);

    // Create and spawn the user stream manager
    let (mut stream_manager, ws_status_rx) = AsterDexUserStreamManager::new(
        credentials.api_key.clone(),
        effective_symbol.clone(),
        ws_order_tx,
        None, // No ACCOUNT_UPDATE handler needed for orders view
        http_client.clone(),
    );
    tokio::spawn(async move {
        stream_manager.run_with_reconnect().await;
    });

    // Create OrdersApp with fetched data
    let mut app = tui::OrdersApp::new(effective_symbol.clone(), orders, theme);

    // Refresh channel: app sends () via refresh_tx, event loop receives and spawns fetch
    let (refresh_tx, mut refresh_rx) = mpsc::channel::<()>(1);
    let (order_result_tx, mut order_result_rx) = mpsc::channel::<Vec<Order>>(1);
    app.refresh_tx = Some(refresh_tx);

    // Cancel channels: orders_app sends (symbol, order_id), event loop spawns cancel_order()
    let (cancel_tx, mut cancel_rx) = mpsc::channel::<(String, u64)>(1);
    let (cancel_result_tx, mut cancel_result_rx) = mpsc::channel::<Result<String, String>>(1);
    app.cancel_tx = Some(cancel_tx);

    // Reconciliation channel: background task sends Vec<Order> every 60s
    let (recon_order_tx, mut recon_order_rx) = mpsc::channel::<Vec<Order>>(1);

    // Spawn periodic REST reconciliation task (every 60 seconds)
    {
        let recon_symbol = app.symbol.clone();
        let recon_creds = credentials.clone();
        let client_for_recon = http_client.clone();
        tokio::spawn(async move {
            let mut timer = interval(Duration::from_secs(60));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Skip first immediate tick (initial fetch already done at startup)
            timer.tick().await;

            loop {
                timer.tick().await;
                tracing::debug!("Starting order reconciliation");

                let result = match &recon_symbol {
                    Some(sym) => fetch_orders(sym, &recon_creds, &client_for_recon).await,
                    None => fetch_open_orders(&recon_creds, &client_for_recon).await,
                };
                match result {
                    Ok(api_orders) => {
                        let orders: Vec<Order> =
                            api_orders.into_iter().map(Order::from).collect();
                        tracing::info!(count = orders.len(), "Reconciliation fetched orders");
                        if recon_order_tx.send(orders).await.is_err() {
                            break; // Main loop closed
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Order reconciliation failed, will retry in 60s");
                    }
                }
            }
        });
    }

    // Initialize terminal and event handler
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Orders event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        tui::ui_orders(frame, &mut app);
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Resize(_width, _height) => {
                    // page_size is updated on each render from frame area
                }
                Event::Tick => {
                    // Check for manual refresh requests from 'r' key
                    if let Ok(()) = refresh_rx.try_recv() {
                        let refresh_symbol = app.symbol.clone();
                        let creds_clone = credentials.clone();
                        let tx = order_result_tx.clone();
                        let client_clone = http_client.clone();
                        tokio::spawn(async move {
                            let result = match &refresh_symbol {
                                Some(sym) => fetch_orders(sym, &creds_clone, &client_clone).await,
                                None => fetch_open_orders(&creds_clone, &client_clone).await,
                            };
                            match result {
                                Ok(api_orders) => {
                                    let mut orders: Vec<Order> =
                                        api_orders.into_iter().map(Order::from).collect();
                                    orders.sort_by(|a, b| b.time.cmp(&a.time));
                                    let _ = tx.send(orders).await;
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "Manual refresh failed");
                                }
                            }
                        });
                    }

                    // Receive refreshed orders
                    while let Ok(new_orders) = order_result_rx.try_recv() {
                        tracing::info!(count = new_orders.len(), "Refreshed orders");
                        app.orders = new_orders;
                        if !app.orders.is_empty() {
                            app.table_state.select(Some(0));
                        } else {
                            app.table_state.select(None);
                        }
                    }

                    // Receive WebSocket order updates and merge by order_id
                    while let Ok(incoming) = ws_order_rx.try_recv() {
                        // Find existing order by order_id
                        if let Some(existing) =
                            app.orders.iter_mut().find(|o| o.order_id == incoming.order_id)
                        {
                            // Monotonic guard: only update if incoming is newer
                            if incoming.update_time >= existing.update_time {
                                *existing = incoming;
                            }
                        } else {
                            // New order: insert at beginning (newest first)
                            app.orders.insert(0, incoming);
                            // Maintain selection on first row for new orders
                            app.table_state.select(Some(0));
                        }
                    }

                    // Receive reconciliation orders and merge by order_id
                    while let Ok(recon_orders) = recon_order_rx.try_recv() {
                        for incoming in recon_orders {
                            if let Some(existing) =
                                app.orders.iter_mut().find(|o| o.order_id == incoming.order_id)
                            {
                                if incoming.update_time >= existing.update_time {
                                    *existing = incoming;
                                }
                            } else {
                                app.orders.insert(0, incoming);
                            }
                        }
                        tracing::debug!("Applied order reconciliation updates");
                    }

                    // Update WebSocket connection status
                    app.connection_status = ws_status_rx.borrow().clone();

                    // Check for cancel requests from 'd' hotkey
                    if let Ok((symbol, order_id)) = cancel_rx.try_recv() {
                        let creds = credentials.clone();
                        let tx = cancel_result_tx.clone();
                        let client_clone = http_client.clone();
                        tokio::spawn(async move {
                            match cancel_order(&symbol, order_id, &creds, &client_clone).await {
                                Ok(response) => {
                                    let msg = format!(
                                        "Order {} cancelled (status: {})",
                                        response.order_id, response.status
                                    );
                                    let _ = tx.send(Ok(msg)).await;
                                }
                                Err(e) => {
                                    let msg = user_friendly_cancel_error(&e);
                                    let _ = tx.send(Err(msg)).await;
                                }
                            }
                        });
                    }

                    // Receive cancel results
                    if let Ok(result) = cancel_result_rx.try_recv() {
                        app.cancel_result = Some(result);
                        app.mode = tui::orders_app::OrdersMode::ShowingResult;
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

    // Restore terminal
    tui_instance.restore();

    Ok(())
}
