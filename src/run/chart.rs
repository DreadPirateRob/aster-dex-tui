// src/run/chart.rs
// Chart (default) subcommand event loop (extracted from main.rs)

use crate::config::{CandleMode, ChartArgs, Config, Resolution};
use crate::data::types::{Candle, Trade};
use crate::network::{
    fetch_asterdex_agg_trades,
    load_asterdex_historical_klines, validate_symbol,
    AsterDexTradeConnectionManager, ConnectionStatus,
};
use crate::tui::app::TickerStats;
use crate::tui::{
    App, CandlestickChart, CvdSparkline, Event, EventHandler, StatusBar, TradesList, Tui, ThemeName,
};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::Line,
    widgets::Paragraph,
    Frame,
};
use ring_channel::RingReceiver;
use rust_decimal::Decimal;
use std::io;
use std::num::NonZeroUsize;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

pub async fn run(
    symbol: Option<String>,
    tick_size: Option<u32>,
    resolution: Option<Resolution>,
    large_trade_threshold: Decimal,
    theme: ThemeName,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // Validate symbol is present
    let symbol = symbol.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Symbol is required. Usage: crypto-tui <SYMBOL> --resolution <RES> or crypto-tui orders <SYMBOL>",
        )
    })?;

    let chart_args = ChartArgs {
        symbol,
        tick_size,
        resolution,
        large_trade_threshold,
        theme,
    };

    let config = match Config::from_args(chart_args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // Bootstrap: fetch historical data based on mode
    let (bootstrap_trades, bootstrap_candles, bootstrap_error) = match config.mode {
        CandleMode::TickBased => {
            // Fetch recent aggregate trades for tick-based bootstrap
            match fetch_asterdex_agg_trades(&config.symbol, 1000, &http_client).await {
                Ok(agg_trades) => {
                    let trades: Vec<Trade> = agg_trades.into_iter().map(Trade::from).collect();
                    tracing::info!(count = trades.len(), "Fetched AsterDEX historical aggTrades");
                    (trades, Vec::new(), None)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to fetch AsterDEX aggTrades");
                    tracing::info!("Continuing with WebSocket only (chart will start empty)");
                    (Vec::new(), Vec::new(), Some(format!("Failed to fetch AsterDEX aggTrades: {}", e)))
                }
            }
        }
        CandleMode::TimeBased => {
            let interval = config.resolution
                .expect("TimeBased mode requires resolution")
                .as_interval_str();

            match load_asterdex_historical_klines(&config.symbol, interval, 500, &http_client).await {
                Ok(candles) => {
                    tracing::info!(count = candles.len(), interval = interval, "Fetched AsterDEX historical klines");
                    (Vec::new(), candles, None)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to fetch AsterDEX klines");
                    tracing::info!("Continuing with WebSocket only (chart will start empty)");
                    (Vec::new(), Vec::new(), Some(format!("Failed to fetch AsterDEX klines: {}", e)))
                }
            }
        }
    };

    // Derive 24h ticker stats from bootstrap candles (AsterDEX has no public 24h ticker endpoint for chart mode)
    let ticker_stats = if bootstrap_candles.is_empty() {
        tracing::info!("Skipping ticker stats (no candles)");
        None
    } else {
        let high_price = bootstrap_candles.iter().map(|c| c.high).max().unwrap_or(Decimal::ZERO);
        let low_price = bootstrap_candles.iter().map(|c| c.low).min().unwrap_or(Decimal::ZERO);
        let volume = bootstrap_candles.iter().map(|c| c.volume).sum();
        let earliest_open = bootstrap_candles.first().map(|c| c.open).unwrap_or(Decimal::ZERO);
        let latest_close = bootstrap_candles.last().map(|c| c.close).unwrap_or(Decimal::ZERO);
        let price_change = latest_close - earliest_open;
        let price_change_percent = if earliest_open != Decimal::ZERO {
            (price_change / earliest_open) * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        tracing::info!("Derived ticker stats from bootstrap candles");
        Some(TickerStats {
            price_change,
            price_change_percent,
            high_price,
            low_price,
            volume,
        })
    };

    // Create trade channel (bounded ring buffer)
    let (trade_tx, trade_rx) =
        ring_channel::ring_channel::<Trade>(NonZeroUsize::new(100).unwrap());

    // Create and spawn AsterDEX connection manager (store handle for pair switch teardown)
    let (mut connection_manager, status_rx) =
        AsterDexTradeConnectionManager::new(config.symbol.clone(), trade_tx);
    let connection_manager_handle = tokio::spawn(async move {
        connection_manager.run_with_reconnect().await;
    });

    // Initialize terminal (raw mode, alternate screen, panic hook)
    let mut tui = Tui::new()?;

    // Run the application
    let result = event_loop(tui.terminal(), config, trade_rx, status_rx, connection_manager_handle, bootstrap_trades, bootstrap_candles, bootstrap_error, ticker_stats, http_client).await;

    // Restore terminal (explicit, though Drop would also handle it)
    tui.restore();

    result
}

/// Main application loop.
async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    config: Config,
    trade_rx: RingReceiver<Trade>,
    status_rx: tokio::sync::watch::Receiver<ConnectionStatus>,
    connection_manager_handle: tokio::task::JoinHandle<()>,
    bootstrap_trades: Vec<Trade>,
    bootstrap_candles: Vec<Candle>,
    bootstrap_error: Option<String>,
    ticker_stats: Option<TickerStats>,
    http_client: reqwest::Client,
) -> io::Result<()> {
    let mut app = App::new(config);
    app.ticker_stats = ticker_stats;
    let mut events = EventHandler::new(Duration::from_millis(crate::TICK_RATE_MS), crate::FRAME_RATE);

    // Display bootstrap error if any (shows in status bar, auto-dismisses after 5s)
    if let Some(error_msg) = bootstrap_error {
        app.set_error(error_msg);
    }

    // Mutable trade/status receivers and connection manager handle (replaced on pair switch)
    let mut trade_rx = trade_rx;
    let mut status_rx = status_rx;
    let mut connection_manager_handle = connection_manager_handle;

    // Load historical trades (tick-based mode)
    let mut last_rest_trade_id = if !bootstrap_trades.is_empty() {
        app.load_historical_trades(bootstrap_trades)
    } else {
        0
    };

    // Load historical candles (time-based mode)
    if !bootstrap_candles.is_empty() {
        app.load_historical_candles(bootstrap_candles);
    }

    // Reconciliation channel (receives corrected candles from REST)
    let (mut reconcile_tx, mut reconcile_rx) = mpsc::channel::<Vec<Candle>>(1);

    // Backfill trigger channel (signals to fetch missing candles on reconnect)
    let (mut backfill_tx, mut backfill_rx) = mpsc::channel::<()>(1);

    // Resolution switch channels (Option-wrapped for TickBased mode exclusion)
    let mut switch_tx_opt: Option<mpsc::Sender<(Resolution, u64)>> = None;
    let mut resolution_watch_tx_opt: Option<tokio::sync::watch::Sender<Resolution>> = None;
    let mut switch_reconcile_rx_opt: Option<mpsc::Receiver<(Vec<Candle>, u64)>> = None;

    // Tick-size switch channels (Option-wrapped for TimeBased mode exclusion)
    let mut tick_switch_tx_opt: Option<mpsc::Sender<(u32, u64)>> = None;
    let mut tick_switch_rx_opt: Option<mpsc::Receiver<(Vec<Trade>, u64)>> = None;

    // Background task handles (stored for pair switch teardown)
    let mut reconciliation_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut backfill_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut resolution_switch_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut tick_switch_handle: Option<tokio::task::JoinHandle<()>> = None;

    // Pair switch state
    let mut pending_switch_rx: Option<mpsc::Receiver<(String, Result<(), String>)>> = None;
    let mut pending_bootstrap_rx: Option<mpsc::Receiver<(Vec<Trade>, Vec<Candle>)>> = None;

    // Spawn reconciliation and backfill tasks for time-based mode
    if matches!(app.mode, CandleMode::TimeBased) {
        let symbol = app.config.symbol.clone();
        let resolution = app.config.resolution.expect("TimeBased requires resolution");

        // Watch channel so reconciliation/backfill tasks always use current resolution
        let (resolution_watch_tx, resolution_watch_rx) =
            tokio::sync::watch::channel(resolution);

        // Resolution switch channel (sends new resolution + generation to background fetcher)
        let (switch_tx, mut switch_rx) = mpsc::channel::<(Resolution, u64)>(1);

        // Tagged reconciliation channel for switch responses (includes generation for stale rejection)
        let (switch_reconcile_tx, switch_reconcile_rx) = mpsc::channel::<(Vec<Candle>, u64)>(1);

        // Store Option-wrapped handles for use in main loop
        switch_tx_opt = Some(switch_tx);
        resolution_watch_tx_opt = Some(resolution_watch_tx);
        switch_reconcile_rx_opt = Some(switch_reconcile_rx);

        // Spawn periodic reconciliation task
        let tx = reconcile_tx.clone();
        let recon_symbol = symbol.clone();
        let client_for_recon = http_client.clone();
        let recon_watch_rx = resolution_watch_rx.clone();

        reconciliation_handle = Some(tokio::spawn(async move {
            let mut timer = interval(Duration::from_secs(300)); // 5 minutes
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Skip first immediate tick
            timer.tick().await;

            loop {
                timer.tick().await;
                let current_resolution = *recon_watch_rx.borrow();
                let interval_str = current_resolution.as_interval_str();
                tracing::debug!(interval = interval_str, "Starting REST reconciliation");

                match load_asterdex_historical_klines(&recon_symbol, interval_str, 10, &client_for_recon).await {
                    Ok(candles) => {
                        tracing::info!(count = candles.len(), "Reconciliation fetched candles");
                        if tx.send(candles).await.is_err() {
                            break; // Main loop closed
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Reconciliation failed, will retry in 5 minutes");
                    }
                }
            }
        }));

        // Spawn backfill task (triggered on reconnection)
        let backfill_reconcile_tx = reconcile_tx.clone();
        let client_for_backfill = http_client.clone();
        let backfill_watch_rx = resolution_watch_rx;

        backfill_handle = Some(tokio::spawn(async move {
            while let Some(()) = backfill_rx.recv().await {
                let current_resolution = *backfill_watch_rx.borrow();
                let interval_str = current_resolution.as_interval_str();
                tracing::info!(interval = interval_str, "Backfill triggered, fetching recent candles");

                match load_asterdex_historical_klines(&symbol, interval_str, 50, &client_for_backfill).await {
                    Ok(candles) => {
                        tracing::info!(count = candles.len(), "Backfill fetched candles");
                        if backfill_reconcile_tx.send(candles).await.is_err() {
                            break; // Main loop closed
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Backfill failed");
                    }
                }
            }
        }));

        // Spawn resolution switch handler background task
        let switch_symbol = app.config.symbol.clone();
        let client_for_switch = http_client.clone();

        resolution_switch_handle = Some(tokio::spawn(async move {
            while let Some((new_resolution, generation)) = switch_rx.recv().await {
                let interval_str = new_resolution.as_interval_str();
                tracing::info!(resolution = interval_str, generation, "Resolution switch: fetching klines");

                match load_asterdex_historical_klines(&switch_symbol, interval_str, 500, &client_for_switch).await {
                    Ok(candles) => {
                        tracing::info!(count = candles.len(), generation, "Resolution switch: klines fetched");
                        if switch_reconcile_tx.send((candles, generation)).await.is_err() {
                            break; // Main loop closed
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, generation, "Resolution switch: fetch failed");
                        // Send empty vec so main loop can clear loading state
                        if switch_reconcile_tx.send((Vec::new(), generation)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }));
    }

    // Spawn tick-size switch background task (tick-based mode only)
    if matches!(app.mode, CandleMode::TickBased) {
        let symbol = app.config.symbol.clone();
        let client_for_tick_switch = http_client.clone();

        let (tick_switch_tx, mut tick_switch_inner_rx) = mpsc::channel::<(u32, u64)>(1);
        let (tick_switch_response_tx, tick_switch_response_rx) = mpsc::channel::<(Vec<Trade>, u64)>(1);

        tick_switch_tx_opt = Some(tick_switch_tx);
        tick_switch_rx_opt = Some(tick_switch_response_rx);

        tick_switch_handle = Some(tokio::spawn(async move {
            while let Some((_new_tick_size, generation)) = tick_switch_inner_rx.recv().await {
                tracing::info!(generation, "Tick-size switch: fetching aggTrades");

                match fetch_asterdex_agg_trades(&symbol, 1000, &client_for_tick_switch).await {
                    Ok(agg_trades) => {
                        let trades: Vec<Trade> = agg_trades.into_iter().map(Trade::from).collect();
                        tracing::info!(count = trades.len(), generation, "Tick-size switch: aggTrades fetched");
                        if tick_switch_response_tx.send((trades, generation)).await.is_err() {
                            break; // Main loop closed
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, generation, "Tick-size switch: fetch failed");
                        // Send empty vec so main loop knows the fetch failed
                        if tick_switch_response_tx.send((Vec::new(), generation)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }));
    }

    // Main event loop
    loop {
        // Wait for next event
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    // Draw the UI
                    terminal.draw(|frame| ui(frame, &app))?;
                }
                Event::Key(key) => {
                    // Handle keyboard input
                    app.handle_key(key);

                    // Check if resolution was switched (flag set by cycle_resolution_forward)
                    if app.resolution_switched {
                        app.resolution_switched = false;
                        let new_res = app.config.resolution.expect("TimeBased has resolution");
                        let gen = app.resolution_generation;

                        // Update watch channel so reconciliation/backfill use new resolution
                        if let Some(ref tx) = resolution_watch_tx_opt {
                            let _ = tx.send(new_res);
                        }
                        // Send fetch request to switch handler
                        if let Some(ref tx) = switch_tx_opt {
                            let _ = tx.try_send((new_res, gen));
                        }
                    }

                    // Check if tick size was switched (flag set by cycle_tick_size_forward/backward)
                    if app.tick_size_switched {
                        app.tick_size_switched = false;
                        let tick_size = app.config.tick_size;
                        let gen = app.tick_size_generation;

                        // Send fetch request to tick-size switch handler
                        if let Some(ref tx) = tick_switch_tx_opt {
                            let _ = tx.try_send((tick_size, gen));
                        }
                    }

                    // --- Pair Switch: Phase 1 — Spawn async validation ---
                    if let Some(new_symbol) = app.pair_switch_requested.take() {
                        if pending_switch_rx.is_some() {
                            app.set_info("Switch already in progress...".to_string());
                        } else {
                            let client_cl = http_client.clone();
                            let (switch_result_tx, switch_result_rx) = mpsc::channel(1);
                            pending_switch_rx = Some(switch_result_rx);
                            tokio::spawn(async move {
                                let result = validate_symbol(&new_symbol, &client_cl)
                                    .await
                                    .map(|_| ())
                                    .map_err(|e| e.to_string());
                                let _ = switch_result_tx.send((new_symbol, result)).await;
                            });
                            app.set_info("Validating symbol...".to_string());
                        }
                    }
                }
                Event::Resize(width, height) => {
                    // Update app state with new size
                    // (ratatui handles actual resize on next draw)
                    app.set_size(width, height);
                }
                Event::Tick => {
                    // --- Pair Switch: Phase 2 — Poll bootstrap data ---
                    if let Some(ref mut rx) = pending_bootstrap_rx {
                        if let Ok((trades, candles)) = rx.try_recv() {
                            pending_bootstrap_rx = None;
                            let has_trades = !trades.is_empty();
                            let has_candles = !candles.is_empty();
                            if has_trades {
                                last_rest_trade_id = app.load_historical_trades(trades);
                            }
                            if has_candles {
                                // Derive ticker stats from bootstrap candles
                                let high_price = candles.iter().map(|c| c.high).max().unwrap_or(Decimal::ZERO);
                                let low_price = candles.iter().map(|c| c.low).min().unwrap_or(Decimal::ZERO);
                                let volume: Decimal = candles.iter().map(|c| c.volume).sum();
                                let earliest_open = candles.first().map(|c| c.open).unwrap_or(Decimal::ZERO);
                                let latest_close = candles.last().map(|c| c.close).unwrap_or(Decimal::ZERO);
                                let price_change = latest_close - earliest_open;
                                let price_change_percent = if earliest_open != Decimal::ZERO {
                                    (price_change / earliest_open) * Decimal::from(100)
                                } else {
                                    Decimal::ZERO
                                };
                                app.ticker_stats = Some(TickerStats {
                                    price_change,
                                    price_change_percent,
                                    high_price,
                                    low_price,
                                    volume,
                                });
                                app.load_historical_candles(candles);
                            }
                            if !has_trades && !has_candles {
                                app.set_error("Failed to load data for new symbol".to_string());
                            }
                        }
                    }

                    // --- Pair Switch: Phase 3 — Poll validation result + teardown-rebuild ---
                    if let Some(ref mut rx) = pending_switch_rx {
                        if let Ok((new_symbol, result)) = rx.try_recv() {
                            pending_switch_rx = None;
                            match result {
                                Ok(()) => {
                                    // === TEARDOWN ===
                                    // 1. Abort connection manager
                                    connection_manager_handle.abort();

                                    // 2. Abort all background tasks
                                    if let Some(h) = reconciliation_handle.take() { h.abort(); }
                                    if let Some(h) = backfill_handle.take() { h.abort(); }
                                    if let Some(h) = resolution_switch_handle.take() { h.abort(); }
                                    if let Some(h) = tick_switch_handle.take() { h.abort(); }

                                    // === RESET APP STATE ===
                                    app.switch_symbol(new_symbol.clone());

                                    // === REBUILD ===
                                    // 3. Create new trade channel + connection manager
                                    let (new_trade_tx, new_trade_rx) =
                                        ring_channel::ring_channel::<Trade>(NonZeroUsize::new(100).unwrap());
                                    trade_rx = new_trade_rx;

                                    let (mut new_manager, new_status_rx) =
                                        AsterDexTradeConnectionManager::new(new_symbol.clone(), new_trade_tx);
                                    status_rx = new_status_rx;
                                    connection_manager_handle = tokio::spawn(async move {
                                        new_manager.run_with_reconnect().await;
                                    });

                                    // 4. Bootstrap historical data (async via spawned task)
                                    let bootstrap_symbol = new_symbol.clone();
                                    let bootstrap_client = http_client.clone();
                                    let (bootstrap_tx, bootstrap_rx_local) =
                                        mpsc::channel::<(Vec<Trade>, Vec<Candle>)>(1);
                                    let bootstrap_mode = app.mode;
                                    let bootstrap_resolution = app.config.resolution;

                                    tokio::spawn(async move {
                                        match bootstrap_mode {
                                            CandleMode::TickBased => {
                                                match fetch_asterdex_agg_trades(&bootstrap_symbol, 1000, &bootstrap_client).await {
                                                    Ok(agg_trades) => {
                                                        let trades: Vec<Trade> = agg_trades.into_iter().map(Trade::from).collect();
                                                        let _ = bootstrap_tx.send((trades, Vec::new())).await;
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = %e, "Pair switch: bootstrap aggTrades failed");
                                                        let _ = bootstrap_tx.send((Vec::new(), Vec::new())).await;
                                                    }
                                                }
                                            }
                                            CandleMode::TimeBased => {
                                                let interval_str = bootstrap_resolution
                                                    .expect("TimeBased requires resolution")
                                                    .as_interval_str();
                                                match load_asterdex_historical_klines(&bootstrap_symbol, interval_str, 500, &bootstrap_client).await {
                                                    Ok(candles) => {
                                                        let _ = bootstrap_tx.send((Vec::new(), candles)).await;
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = %e, "Pair switch: bootstrap klines failed");
                                                        let _ = bootstrap_tx.send((Vec::new(), Vec::new())).await;
                                                    }
                                                }
                                            }
                                        }
                                    });
                                    pending_bootstrap_rx = Some(bootstrap_rx_local);

                                    // 5. Recreate reconciliation and mode-dependent channels + tasks
                                    let (new_reconcile_tx, new_reconcile_rx) = mpsc::channel::<Vec<Candle>>(1);
                                    reconcile_tx = new_reconcile_tx;
                                    reconcile_rx = new_reconcile_rx;

                                    let (new_backfill_tx, new_backfill_rx) = mpsc::channel::<()>(1);
                                    backfill_tx = new_backfill_tx;

                                    // Reset switch channel option vars
                                    switch_tx_opt = None;
                                    resolution_watch_tx_opt = None;
                                    switch_reconcile_rx_opt = None;
                                    tick_switch_tx_opt = None;
                                    tick_switch_rx_opt = None;

                                    if matches!(app.mode, CandleMode::TimeBased) {
                                        let symbol = new_symbol.clone();
                                        let resolution = app.config.resolution.expect("TimeBased requires resolution");

                                        let (resolution_watch_tx, resolution_watch_rx) =
                                            tokio::sync::watch::channel(resolution);
                                        let (switch_tx, mut switch_rx) = mpsc::channel::<(Resolution, u64)>(1);
                                        let (switch_reconcile_tx, switch_reconcile_rx) =
                                            mpsc::channel::<(Vec<Candle>, u64)>(1);

                                        switch_tx_opt = Some(switch_tx);
                                        resolution_watch_tx_opt = Some(resolution_watch_tx);
                                        switch_reconcile_rx_opt = Some(switch_reconcile_rx);

                                        // Reconciliation task
                                        let recon_tx = reconcile_tx.clone();
                                        let recon_symbol = symbol.clone();
                                        let client_for_recon = http_client.clone();
                                        let recon_watch_rx = resolution_watch_rx.clone();
                                        reconciliation_handle = Some(tokio::spawn(async move {
                                            let mut timer = interval(Duration::from_secs(300));
                                            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
                                            timer.tick().await; // skip first
                                            loop {
                                                timer.tick().await;
                                                let current_resolution = *recon_watch_rx.borrow();
                                                let interval_str = current_resolution.as_interval_str();
                                                match load_asterdex_historical_klines(&recon_symbol, interval_str, 10, &client_for_recon).await {
                                                    Ok(candles) => {
                                                        if recon_tx.send(candles).await.is_err() { break; }
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = %e, "Reconciliation failed");
                                                    }
                                                }
                                            }
                                        }));

                                        // Backfill task
                                        let backfill_reconcile_tx = reconcile_tx.clone();
                                        let client_for_backfill = http_client.clone();
                                        let backfill_watch_rx = resolution_watch_rx;
                                        let backfill_symbol = symbol.clone();
                                        backfill_handle = Some(tokio::spawn(async move {
                                            let mut bf_rx = new_backfill_rx;
                                            while let Some(()) = bf_rx.recv().await {
                                                let current_resolution = *backfill_watch_rx.borrow();
                                                let interval_str = current_resolution.as_interval_str();
                                                match load_asterdex_historical_klines(&backfill_symbol, interval_str, 50, &client_for_backfill).await {
                                                    Ok(candles) => {
                                                        if backfill_reconcile_tx.send(candles).await.is_err() { break; }
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = %e, "Backfill failed");
                                                    }
                                                }
                                            }
                                        }));

                                        // Resolution switch handler
                                        let switch_symbol = symbol;
                                        let client_for_switch = http_client.clone();
                                        resolution_switch_handle = Some(tokio::spawn(async move {
                                            while let Some((new_resolution, generation)) = switch_rx.recv().await {
                                                let interval_str = new_resolution.as_interval_str();
                                                match load_asterdex_historical_klines(&switch_symbol, interval_str, 500, &client_for_switch).await {
                                                    Ok(candles) => {
                                                        if switch_reconcile_tx.send((candles, generation)).await.is_err() { break; }
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = %e, "Resolution switch fetch failed");
                                                        if switch_reconcile_tx.send((Vec::new(), generation)).await.is_err() { break; }
                                                    }
                                                }
                                            }
                                        }));
                                    }

                                    if matches!(app.mode, CandleMode::TickBased) {
                                        let symbol = new_symbol.clone();
                                        let client_for_tick = http_client.clone();
                                        let (tick_tx, mut tick_inner_rx) = mpsc::channel::<(u32, u64)>(1);
                                        let (tick_resp_tx, tick_resp_rx) = mpsc::channel::<(Vec<Trade>, u64)>(1);
                                        tick_switch_tx_opt = Some(tick_tx);
                                        tick_switch_rx_opt = Some(tick_resp_rx);

                                        tick_switch_handle = Some(tokio::spawn(async move {
                                            while let Some((_new_tick_size, generation)) = tick_inner_rx.recv().await {
                                                match fetch_asterdex_agg_trades(&symbol, 1000, &client_for_tick).await {
                                                    Ok(agg_trades) => {
                                                        let trades: Vec<Trade> = agg_trades.into_iter().map(Trade::from).collect();
                                                        if tick_resp_tx.send((trades, generation)).await.is_err() { break; }
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = %e, "Tick-size switch fetch failed");
                                                        if tick_resp_tx.send((Vec::new(), generation)).await.is_err() { break; }
                                                    }
                                                }
                                            }
                                        }));
                                    }

                                    // Reset dedup threshold
                                    last_rest_trade_id = 0;

                                    app.set_info(format!("Switched to {}", new_symbol));
                                    tracing::info!(symbol = %new_symbol, "Pair switch complete");
                                }
                                Err(e) => {
                                    app.set_error(format!("Invalid symbol: {}", e));
                                }
                            }
                        }
                    }

                    // Poll trade channel (non-blocking drain)
                    while let Ok(trade) = trade_rx.try_recv() {
                        // Deduplicate: skip trades already seen from REST
                        if trade.id > last_rest_trade_id {
                            // Route to appropriate handler based on mode
                            match app.mode {
                                CandleMode::TickBased => {
                                    app.add_trade(trade);
                                }
                                CandleMode::TimeBased => {
                                    app.add_trade_time_based(trade);
                                }
                            }
                        }
                    }

                    // Poll reconciliation channel (non-blocking)
                    // Only receives data in time-based mode (tick-based drops the sender)
                    while let Ok(rest_candles) = reconcile_rx.try_recv() {
                        // Skip current bucket (incomplete) - CRITICAL for correctness
                        let current_bucket = app.current_time_bucket();
                        for candle in rest_candles {
                            if Some(candle.open_time) != current_bucket {
                                app.candle_store.update_or_insert(candle);
                            }
                        }
                        tracing::debug!("Applied reconciliation updates");
                    }

                    // Poll resolution switch responses (with generation-based stale rejection)
                    if let Some(ref mut rx) = switch_reconcile_rx_opt {
                        while let Ok((candles, generation)) = rx.try_recv() {
                            if generation == app.resolution_generation {
                                // Current generation -- apply candles
                                if candles.is_empty() {
                                    app.set_error("Failed to fetch candles for new resolution".to_string());
                                } else {
                                    app.load_historical_candles(candles);
                                    let res = app.config.resolution.expect("TimeBased has resolution");
                                    app.set_info(format!("Loaded {} candles", res));
                                }
                            } else {
                                // Stale response from older switch -- discard
                                tracing::debug!(
                                    response_gen = generation,
                                    current_gen = app.resolution_generation,
                                    "Discarding stale resolution switch response"
                                );
                            }
                        }
                    }

                    // Poll tick-size switch responses (with generation-based stale rejection)
                    if let Some(ref mut rx) = tick_switch_rx_opt {
                        while let Ok((trades, generation)) = rx.try_recv() {
                            if generation == app.tick_size_generation {
                                // Current generation -- rebuild candles from trades
                                if trades.is_empty() {
                                    app.set_error("Failed to fetch trades for new tick size".to_string());
                                } else {
                                    app.load_historical_trades(trades);
                                    app.set_info(format!("Loaded {}T candles", app.config.tick_size));
                                }
                            } else {
                                // Stale response from older tick-size switch -- discard
                                tracing::debug!(
                                    response_gen = generation,
                                    current_gen = app.tick_size_generation,
                                    "Discarding stale tick-size switch response"
                                );
                            }
                        }
                    }

                    // Update connection status from watch channel
                    app.connection_status = status_rx.borrow().clone();

                    // Detect reconnection (was disconnected, now connected)
                    // IMPORTANT: Check BEFORE updating was_disconnected to catch the transition
                    let currently_disconnected = app.is_disconnected();
                    let just_reconnected = app.was_disconnected && !currently_disconnected;

                    // Update tracking state FIRST so subsequent ticks don't re-trigger
                    app.was_disconnected = currently_disconnected;

                    // Trigger backfill on reconnection (time-based mode only)
                    if just_reconnected && matches!(app.mode, CandleMode::TimeBased) {
                        tracing::info!("WebSocket reconnected, triggering backfill");
                        let _ = backfill_tx.try_send(());
                    }

                    // Clear stale status messages
                    app.clear_stale_status();
                }
                Event::Error => {
                    // Event handling error - exit
                    break;
                }
            }
        } else {
            // Event channel closed
            break;
        }

        // Check for quit request
        if app.should_quit() {
            break;
        }
    }

    Ok(())
}

/// Render the UI.
fn ui(frame: &mut Frame, app: &App) {
    use ratatui::widgets::{Block, Borders};

    // Basic layout: info widget at top, volume histogram, main content, status bar at bottom
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                                       // [0] Top info widget (prod info)
            Constraint::Length(if app.volume_visible { 2 } else { 0 }), // [1] CVD sparkline (0 when hidden)
            Constraint::Min(0),                                         // [2] Main content area (chart +/- trades)
            Constraint::Length(1),                                       // [3] Bottom status bar
        ])
        .split(frame.area());

    // Top info widget: symbol, price, exchange, status, UTC time
    // Theme is passed to all widgets for consistent styling
    let utc_time = app.utc_time_str();
    let status_bar = StatusBar {
        symbol: &app.config.symbol,
        connection_status: &app.connection_status,
        utc_time: &utc_time,
        last_price: app.last_price,
        status_message: app.current_status(),
        theme: &app.theme,
        ticker_stats: app.ticker_stats.as_ref(),
        stats_visible: app.stats_visible,
    };
    frame.render_widget(status_bar, chunks[0]);

    // CVD sparkline between prod info and chart (when visible)
    if app.volume_visible {
        let cvd_data = app.trade_stats.cvd_as_sparkline_data();
        let net_positive = app.trade_stats.buy_volume() >= app.trade_stats.sell_volume();
        let cvd_sparkline = CvdSparkline::new(&cvd_data, &app.theme, net_positive);
        frame.render_widget(cvd_sparkline, chunks[1]);
    }

    // Generate dynamic chart title showing current mode
    let chart_title = match app.mode {
        CandleMode::TickBased => format!(" Chart - {}T ", app.config.tick_size),
        CandleMode::TimeBased => {
            let res = app.config.resolution.expect("TimeBased requires resolution");
            format!(" Chart - {} ", res.as_interval_str())
        }
    };

    // Split main content based on trades visibility
    if app.trades_visible {
        // Show both chart (85%) and trades (15%)
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(85), // Candlestick chart
                Constraint::Percentage(15), // Trades list
            ])
            .split(chunks[2]);

        // Candlestick chart (volume bars restored inside chart)
        let chart = CandlestickChart::new(&app.candle_store)
            .partial_candle(app.partial_candle())
            .tick_size(app.config.tick_size)
            .mode(app.mode)
            .current_price(app.last_price)
            .theme(&app.theme)
            .connection_lost(app.is_disconnected())
            .volume_visible(true)
            .view_offset(app.view_offset)
            .high_precision(app.high_precision_chart)
            .block(Block::default().borders(Borders::ALL).title(chart_title.as_str()));
        frame.render_widget(chart, main_chunks[0]);

        // Trades list
        let trades_list = TradesList {
            trades: &app.trades_display,
            large_trade_threshold: app.config.large_trade_threshold,
            theme: &app.theme,
            buy_volume: app.trade_stats.buy_volume(),
            sell_volume: app.trade_stats.sell_volume(),
            large_trade_count: app.trade_stats.large_trade_count(),
            size_filter_active: app.size_filter_active,
            size_filter_threshold: app.size_filter_threshold,
        };
        frame.render_widget(trades_list, main_chunks[1]);
    } else {
        // Trades hidden - chart takes full width (volume bars inside chart)
        let chart = CandlestickChart::new(&app.candle_store)
            .partial_candle(app.partial_candle())
            .tick_size(app.config.tick_size)
            .mode(app.mode)
            .current_price(app.last_price)
            .theme(&app.theme)
            .connection_lost(app.is_disconnected())
            .volume_visible(true)
            .view_offset(app.view_offset)
            .high_precision(app.high_precision_chart)
            .block(Block::default().borders(Borders::ALL).title(chart_title.as_str()));
        frame.render_widget(chart, chunks[2]);
    }

    // Bottom status bar: show symbol input buffer during input mode, otherwise hotkey bar
    let bottom_text = if let Some(buffer) = app.symbol_input_buffer() {
        format!(" Symbol: {}█ (Enter to switch, Esc to cancel) ", buffer)
    } else {
        " 'q' quit | '/' switch | +/- tick | 'r'/'R' res | arrows scroll | 'l' live | 'f' filter | 'p' hires | 'P' theme | 't' 'v' 'i' toggles ".to_string()
    };
    let bottom_bar = Paragraph::new(Line::from(bottom_text))
        .style(Style::default().bg(app.theme.background_highlight));
    frame.render_widget(bottom_bar, chunks[3]);
}
