// src/data/velocity.rs
// Trade velocity tracking using sliding window

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Window duration for velocity calculation (60 seconds).
const VELOCITY_WINDOW: Duration = Duration::from_secs(60);

/// Minimum samples required before reporting velocity.
/// Returns None during warmup to avoid misleading extrapolation.
#[allow(dead_code)] // Used by binary crate (velocity method called from tui/app.rs)
const MIN_SAMPLES: usize = 10;

/// Maximum timestamps to store, preventing unbounded memory growth.
/// SYS-05: All containers have fixed capacity bounds.
const MAX_TIMESTAMPS: usize = 10000;

/// Tracks trade velocity using a sliding window.
///
/// Records trade timestamps and calculates trades per minute
/// over a configurable window. Returns None during warmup period
/// (< MIN_SAMPLES) to avoid misleading velocity calculations.
///
/// # Memory Bounds (SYS-05)
/// - MAX_TIMESTAMPS = 10000 entries
/// - Evicts entries older than VELOCITY_WINDOW
/// - Evicts oldest entries when at capacity
pub struct TradeVelocityTracker {
    timestamps: VecDeque<Instant>,
    window: Duration,
}

impl TradeVelocityTracker {
    /// Create a new trade velocity tracker.
    pub fn new() -> Self {
        Self {
            timestamps: VecDeque::with_capacity(1000),
            window: VELOCITY_WINDOW,
        }
    }

    /// Record a trade occurrence at the current instant.
    ///
    /// Automatically evicts entries older than the window duration
    /// and caps total entries at MAX_TIMESTAMPS.
    pub fn record_trade(&mut self) {
        let now = Instant::now();
        self.timestamps.push_back(now);

        // Evict entries older than the window
        while let Some(front) = self.timestamps.front() {
            if now.duration_since(*front) > self.window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }

        // Cap at MAX_TIMESTAMPS to bound memory (SYS-05)
        while self.timestamps.len() > MAX_TIMESTAMPS {
            self.timestamps.pop_front();
        }
    }

    /// Calculate trades per minute over the sliding window.
    ///
    /// Returns None during warmup (< MIN_SAMPLES trades recorded)
    /// to avoid misleading velocity extrapolation.
    #[allow(dead_code)] // Used by binary crate (tui/app.rs)
    pub fn velocity(&self) -> Option<f64> {
        let count = self.timestamps.len();

        // Warmup: require minimum samples
        if count < MIN_SAMPLES {
            return None;
        }

        // Calculate elapsed time between first and last trade
        let first = self.timestamps.front()?;
        let last = self.timestamps.back()?;
        let elapsed = last.duration_since(*first);

        // Avoid division by zero or very small elapsed times
        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs < 0.001 {
            return None;
        }

        // Trades per minute: (count / elapsed_seconds) * 60
        Some((count as f64 / elapsed_secs) * 60.0)
    }
}

impl Default for TradeVelocityTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_new_tracker_returns_none_velocity() {
        let tracker = TradeVelocityTracker::new();
        assert!(tracker.velocity().is_none());
    }

    #[test]
    fn test_warmup_returns_none() {
        let mut tracker = TradeVelocityTracker::new();

        // Add fewer than MIN_SAMPLES trades
        for _ in 0..MIN_SAMPLES - 1 {
            tracker.record_trade();
        }

        // Should still return None during warmup
        assert!(tracker.velocity().is_none());
    }

    #[test]
    fn test_velocity_calculates_after_warmup() {
        let mut tracker = TradeVelocityTracker::new();

        // Add exactly MIN_SAMPLES trades with small delays
        for _ in 0..MIN_SAMPLES {
            tracker.record_trade();
            thread::sleep(Duration::from_millis(1));
        }

        // Should return Some after warmup
        let velocity = tracker.velocity();
        assert!(velocity.is_some());
        assert!(velocity.unwrap() > 0.0);
    }

    #[test]
    fn test_default_impl() {
        let tracker = TradeVelocityTracker::default();
        assert!(tracker.velocity().is_none());
        assert_eq!(tracker.window, VELOCITY_WINDOW);
    }
}
