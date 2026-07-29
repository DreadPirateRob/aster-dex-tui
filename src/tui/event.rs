// src/tui/event.rs
// Async event handling with tokio::select! multiplexing

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent, KeyEventKind};
use futures::{FutureExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Application events unified from multiple sources.
#[derive(Debug, Clone)]
pub enum Event {
    /// Periodic tick for state updates (e.g., polling network channels)
    Tick,
    /// Render frame request (fixed FPS)
    Render,
    /// Keyboard event (key press only, not release)
    Key(KeyEvent),
    /// Terminal resize (width, height)
    Resize(u16, u16),
    /// Error in event handling
    Error,
}

/// Async event handler that multiplexes keyboard, tick, and render events.
///
/// Spawns a tokio task that uses `select!` to poll:
/// - crossterm::event::EventStream for keyboard/resize events
/// - tick interval for periodic state updates
/// - render interval for fixed-rate rendering
pub struct EventHandler {
    /// Receiver for events from the spawned task
    rx: mpsc::UnboundedReceiver<Event>,
    /// Handle to the spawned task (for cleanup)
    #[allow(dead_code)]
    task: JoinHandle<()>,
}

impl EventHandler {
    /// Create a new EventHandler.
    ///
    /// # Arguments
    /// * `tick_rate` - Interval between Tick events for state updates
    /// * `frame_rate` - Target frames per second for Render events
    pub fn new(tick_rate: Duration, frame_rate: f64) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let render_interval = Duration::from_secs_f64(1.0 / frame_rate);

        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_timer = tokio::time::interval(tick_rate);
            let mut render_timer = tokio::time::interval(render_interval);

            // Set MissedTickBehavior to prevent catching up after delays
            tick_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            render_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                let tick_delay = tick_timer.tick();
                let render_delay = render_timer.tick();
                let crossterm_event = reader.next().fuse();

                tokio::select! {
                    // Tick event for state updates
                    _ = tick_delay => {
                        if tx.send(Event::Tick).is_err() {
                            break; // Receiver dropped, exit
                        }
                    }

                    // Render event for fixed-rate rendering
                    _ = render_delay => {
                        if tx.send(Event::Render).is_err() {
                            break;
                        }
                    }

                    // Keyboard/mouse/resize events from terminal
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(evt)) => {
                                let event = match evt {
                                    CrosstermEvent::Key(key) => {
                                        // Only handle key press, not release
                                        if key.kind == KeyEventKind::Press {
                                            Some(Event::Key(key))
                                        } else {
                                            None
                                        }
                                    }
                                    CrosstermEvent::Resize(w, h) => {
                                        Some(Event::Resize(w, h))
                                    }
                                    _ => None,
                                };

                                if let Some(event) = event {
                                    if tx.send(event).is_err() {
                                        break;
                                    }
                                }
                            }
                            Some(Err(_)) => {
                                let _ = tx.send(Event::Error);
                                break;
                            }
                            None => {
                                // EventStream ended
                                break;
                            }
                        }
                    }
                }
            }
        });

        Self { rx, task }
    }

    /// Wait for the next event.
    ///
    /// Returns None if the event channel is closed.
    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}
