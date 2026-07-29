// src/network/reconnect.rs
// Exponential backoff strategy for WebSocket reconnection

use std::time::Duration;

/// Exponential backoff for reconnection attempts.
///
/// Sequence: 1s -> 2s -> 4s -> 8s -> 16s (max)
/// No jitter - exact intervals for predictable behavior.
pub struct ExponentialBackoff {
    base: Duration,
    max_delay: Duration,
    multiplier: u32,
    current_attempt: u32,
    max_attempts: u32,
}

impl ExponentialBackoff {
    /// Create backoff with default settings from CONTEXT.md:
    /// - Base delay: 1 second
    /// - Max delay: 16 seconds
    /// - Max attempts: 10
    pub fn new() -> Self {
        Self {
            base: Duration::from_secs(1),
            max_delay: Duration::from_secs(16),
            multiplier: 2,
            current_attempt: 0,
            max_attempts: 10,
        }
    }

    /// Get next delay, or None if max attempts exhausted.
    /// Increments attempt counter.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.current_attempt >= self.max_attempts {
            return None;
        }

        // Calculate: base * 2^attempt, capped at max
        let delay = self.base * self.multiplier.pow(self.current_attempt);
        let delay = delay.min(self.max_delay);

        self.current_attempt += 1;
        Some(delay)
    }

    /// Reset attempt counter (call after successful connection)
    pub fn reset(&mut self) {
        self.current_attempt = 0;
    }

    /// Current attempt number (1-indexed for display)
    pub fn attempt(&self) -> u32 {
        self.current_attempt
    }

    /// Max attempts allowed
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_sequence() {
        let mut backoff = ExponentialBackoff::new();

        // Verify exact sequence: 1s, 2s, 4s, 8s, 16s, 16s, 16s...
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(2)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(4)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(8)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(16)));
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(16))); // capped
    }

    #[test]
    fn test_max_attempts() {
        let mut backoff = ExponentialBackoff::new();

        // Exhaust all 10 attempts
        for _ in 0..10 {
            assert!(backoff.next_delay().is_some());
        }

        // 11th attempt returns None
        assert_eq!(backoff.next_delay(), None);
    }

    #[test]
    fn test_reset() {
        let mut backoff = ExponentialBackoff::new();

        backoff.next_delay();
        backoff.next_delay();
        assert_eq!(backoff.attempt(), 2);

        backoff.reset();
        assert_eq!(backoff.attempt(), 0);
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));
    }
}
