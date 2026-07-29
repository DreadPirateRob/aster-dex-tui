// src/run/liquidations.rs
// Liquidation feed subcommand event loop
//
// Connects to the forceOrder WebSocket stream, drains events into a
// LiquidationFeed with symbol filtering, and renders the full TUI
// via LiquidationsApp and LiquidationFeedWidget.

use crate::data::liquidation::LiquidationFeed;
use crate::helpers;
use crate::network::run_liquidation_stream;
use crate::tui::widgets::LiquidationFeedWidget;
use crate::tui::{Event, LiquidationsApp, Theme};
use rust_decimal::Decimal;
use std::io;
use tokio::sync::mpsc;

pub async fn run(
    symbol_filter: Option<String>,
    large_threshold: Decimal,
    theme: Theme,
) -> io::Result<()> {
    // Create feed and app state
    let feed = LiquidationFeed::new(large_threshold);
    let mut app = LiquidationsApp::new(theme, feed, symbol_filter);

    // Create mpsc channel for liquidation events
    let (liq_tx, mut liq_rx) = mpsc::channel(256);

    // Spawn WebSocket stream
    tokio::spawn(async move {
        run_liquidation_stream(liq_tx).await;
    });

    // Init TUI
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        let widget = LiquidationFeedWidget::new(&app);
                        frame.render_widget(widget, frame.area());
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Tick => {
                    // Drain liquidation events from channel
                    while let Ok(event) = liq_rx.try_recv() {
                        // Apply symbol filter
                        if let Some(ref filter) = app.symbol_filter {
                            if !event.symbol.eq_ignore_ascii_case(filter) {
                                continue;
                            }
                        }
                        app.feed.push_event(event);
                    }
                    app.feed.expire_flashes();
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

    // Restore terminal
    tui_instance.restore();

    Ok(())
}
