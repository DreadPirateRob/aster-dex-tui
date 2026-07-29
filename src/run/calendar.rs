// src/run/calendar.rs
// Economic calendar subcommand event loop
//
// Bootstraps from Finnhub REST API, spawns a 30-minute polling task,
// renders via CalendarApp + CalendarWidget, and re-fetches on date range change.

use crate::data::economic_calendar::{DateRange, EconomicCalendar};
use crate::helpers;
use crate::network::{fetch_economic_calendar, poll_economic_calendar};
use crate::tui::widgets::CalendarWidget;
use crate::tui::{CalendarApp, Event, Theme};
use std::io;
use tokio::sync::mpsc;

pub async fn run(
    high_only: bool,
    country_filter: Option<String>,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // Validate FINNHUB_API_KEY exists before entering event loop
    let api_key = std::env::var("FINNHUB_API_KEY").unwrap_or_else(|_| {
        eprintln!("Error: FINNHUB_API_KEY environment variable not set.");
        eprintln!("Get a free API key at https://finnhub.io/register");
        std::process::exit(1);
    });

    // REST bootstrap with ThisWeek date range
    let (from, to) = DateRange::ThisWeek.to_dates();
    let bootstrap_events = fetch_economic_calendar(&http_client, &api_key, &from, &to).await;
    let mut calendar = EconomicCalendar::new();
    calendar.replace(bootstrap_events);
    tracing::info!(count = calendar.len(), "Economic calendar initialized");

    // Create app state
    let mut app = CalendarApp::new(theme, calendar, high_only, country_filter);

    // mpsc channel for poll updates
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(poll_economic_calendar(
        tx,
        http_client.clone(),
        api_key.clone(),
    ));

    // Init TUI
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        let widget = CalendarWidget::new(&app);
                        frame.render_widget(widget, frame.area());
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);

                    // Check if date range changed — trigger immediate re-fetch
                    if app.date_range_changed {
                        app.date_range_changed = false;
                        let (from, to) = app.date_range.to_dates();
                        let new_events =
                            fetch_economic_calendar(&http_client, &api_key, &from, &to).await;
                        app.calendar.replace(new_events);
                        tracing::info!(
                            range = app.date_range.label(),
                            count = app.calendar.len(),
                            "Date range changed, re-fetched calendar"
                        );
                    }
                }
                Event::Tick => {
                    // Drain poll updates
                    while let Ok(new_events) = rx.try_recv() {
                        app.calendar.replace(new_events);
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

    tui_instance.restore();
    Ok(())
}
