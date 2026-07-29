// src/network/heartbeat.rs
// Ping/pong heartbeat monitoring for WebSocket connection health

use std::time::{Duration, Instant};
use tokio::time::Interval;

/// Heartbeat monitor for WebSocket connection health.
///
/// Sends ping every 30 seconds (per CONTEXT.md).
/// Tracks pong responses to detect stale connections.
pub struct HeartbeatMonitor {
    ping_interval: Duration,
    pong_timeout: Duration,
    last_pong: Instant,
    awaiting_pong: bool,
}

impl HeartbeatMonitor {
    /// Create monitor with default settings:
    /// - Ping every 30 seconds
    /// - Pong timeout: 60 seconds (2x ping interval, conservative)
    pub fn new() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(60),
            last_pong: Instant::now(),
            awaiting_pong: false,
        }
    }

    /// Create tokio interval for ping scheduling
    pub fn create_interval(&self) -> Interval {
        tokio::time::interval(self.ping_interval)
    }

    /// Mark that we sent a ping and are awaiting pong
    pub fn ping_sent(&mut self) {
        self.awaiting_pong = true;
        tracing::debug!("Ping sent, awaiting pong");
    }

    /// Mark that we received a pong
    pub fn pong_received(&mut self) {
        self.last_pong = Instant::now();
        self.awaiting_pong = false;
        tracing::trace!("Pong received");
    }

    /// Check if connection appears stale (pong timeout exceeded)
    pub fn is_stale(&self) -> bool {
        self.awaiting_pong && self.last_pong.elapsed() > self.pong_timeout
    }

    /// Time since last pong (for logging/debugging)
    pub fn time_since_pong(&self) -> Duration {
        self.last_pong.elapsed()
    }
}

impl Default for HeartbeatMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let monitor = HeartbeatMonitor::new();
        assert!(!monitor.awaiting_pong);
        assert!(!monitor.is_stale());
    }

    #[test]
    fn test_ping_pong_cycle() {
        let mut monitor = HeartbeatMonitor::new();

        monitor.ping_sent();
        assert!(monitor.awaiting_pong);

        monitor.pong_received();
        assert!(!monitor.awaiting_pong);
    }

}
