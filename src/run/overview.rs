// src/run/overview.rs
// Market overview subcommand event loop
//
// Bootstraps from REST /fapi/v1/ticker/24hr, connects to
// !ticker@arr WebSocket stream, drains updates into
// TickerTable via OverviewApp, and renders the full TUI
// via OverviewApp and OverviewDashboardWidget.

use crate::data::ticker::TickerTable;
use crate::helpers;
use crate::network::{fetch_ticker_24hr, run_ticker_stream};
use crate::tui::widgets::OverviewDashboardWidget;
use crate::tui::{Event, OverviewApp, Theme};
use std::io;
use tokio::sync::mpsc;

pub async fn run(
    symbol_filter: Option<String>,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // REST bootstrap: fetch initial 24hr ticker data for all pairs
    let bootstrap = fetch_ticker_24hr(&http_client).await;
    let mut table = TickerTable::new();
    for entry in bootstrap {
        table.upsert(entry);
    }
    tracing::info!(count = table.len(), "Ticker table initialized from REST bootstrap");

    // Create app state
    let mut app = OverviewApp::new(theme, table, symbol_filter);

    // Create mpsc channel for WS updates (1024 buffer for ~100+ symbols/sec)
    let (tx, mut rx) = mpsc::channel(1024);

    // Spawn all-symbols ticker stream
    tokio::spawn(run_ticker_stream(tx));

    // Init TUI
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        let widget = OverviewDashboardWidget::new(&app);
                        frame.render_widget(widget, frame.area());
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Tick => {
                    // Drain ticker updates from WS stream
                    // Data accumulation stays unfiltered (display-level filtering
                    // in visible_entries(), matching funding/news/liquidations pattern)
                    while let Ok(entry) = rx.try_recv() {
                        app.table.upsert(entry);
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

    // Restore terminal
    tui_instance.restore();

    // Print selected symbol to stdout if user pressed Enter on a row (Unix composability)
    if let Some(symbol) = &app.selected_symbol {
        println!("{}", symbol);
    }

    Ok(())
}
