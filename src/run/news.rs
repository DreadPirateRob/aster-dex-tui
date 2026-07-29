// src/run/news.rs
// News feed subcommand event loop
//
// Bootstraps from REST /api/news, connects to SSE /api/sse stream,
// drains articles into NewsFeed with symbol filtering, and renders
// the full TUI via NewsApp and NewsFeedWidget.

use crate::data::news_feed::NewsFeed;
use crate::helpers;
use crate::network::{fetch_news_bootstrap, run_news_stream};
use crate::tui::widgets::NewsFeedWidget;
use crate::tui::{Event, NewsApp, Theme};
use std::io;
use tokio::sync::mpsc;

pub async fn run(
    symbol_filter: Option<String>,
    category: Option<String>,
    theme: Theme,
    http_client: reqwest::Client,
) -> io::Result<()> {
    // REST bootstrap: fetch initial headlines
    let bootstrap_articles = fetch_news_bootstrap(&http_client).await;
    let mut feed = NewsFeed::new();
    feed.push_articles(bootstrap_articles);
    tracing::info!(count = feed.articles().len(), "News feed initialized");

    // Create app state
    let mut app = NewsApp::new(theme, feed, symbol_filter, category.clone());

    // Create mpsc channel for SSE articles
    let (news_tx, mut news_rx) = mpsc::channel(256);

    // Spawn SSE stream
    tokio::spawn(run_news_stream(news_tx, http_client.clone(), category));

    // Init TUI
    let (mut tui_instance, mut events) = helpers::init_tui()?;

    // Event loop
    loop {
        if let Some(event) = events.next().await {
            match event {
                Event::Render => {
                    tui_instance.terminal().draw(|frame| {
                        let widget = NewsFeedWidget::new(&app);
                        frame.render_widget(widget, frame.area());
                    })?;
                }
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Tick => {
                    // Drain news articles from channel
                    // Data accumulation stays unfiltered (display-level filtering
                    // in visible_articles(), matching analytics widget pattern)
                    while let Ok(article) = news_rx.try_recv() {
                        app.feed.push_article(article);
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
