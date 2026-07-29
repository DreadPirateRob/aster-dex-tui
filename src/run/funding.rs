// src/run/funding.rs
// Funding rate dashboard subcommand event loop
//
// Bootstraps from REST /fapi/v1/premiumIndex, connects to
// !markPrice@arr@1s WebSocket stream, drains updates into
// FundingRateTable via FundingApp, and renders the full TUI
// via FundingApp and FundingDashboardWidget.

use crate::data::funding_rates::FundingRateTable;
use crate::helpers;
use crate::network::{fetch_premium_index, run_all_mark_price_stream};
use crate::tui::widgets::FundingDashboardWidget;
use crate::tui::{Event, FundingApp, Theme};
use std::io;
use tokio::sync::mpsc;

pub async fn run(
    symbol_filter: Option<String>,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // REST bootstrap: fetch initial funding rates for all pairs
    let bootstrap = fetch_premium_index(&http_client).await;
    let mut table = FundingRateTable::new();
    for entry in bootstrap {
        table.upsert(entry);
    }
    tracing::info!(count = table.len(), "Funding rate table initialized");

    // Create app state
    let mut app = FundingApp::new(theme, table, symbol_filter);

    // Create mpsc channel for WS updates
    let (tx, mut rx) = mpsc::channel(512);

    // Spawn all-symbols mark price stream
    tokio::spawn(run_all_mark_price_stream(tx));

    // Init TUI
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        let widget = FundingDashboardWidget::new(&app);
                        frame.render_widget(widget, frame.area());
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Tick => {
                    // Drain funding rate updates from WS stream
                    // Data accumulation stays unfiltered (display-level filtering
                    // in visible_entries(), matching news/liquidations pattern)
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

    Ok(())
}
