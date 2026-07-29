// src/data/liquidation.rs
// Data types for the real-time liquidation feed widget.
//
// Liquidation events come from AsterDEX's forceOrder WebSocket stream.
// Each event represents a forced liquidation of a trader's position.

use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum number of liquidation events kept in memory.
const MAX_EVENTS: usize = 1000;

/// Which side of the market was liquidated.
///
/// Note the inversion from the order side:
/// - A SELL order means a long position was forcibly closed (long liquidated)
/// - A BUY order means a short position was forcibly closed (short liquidated)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidationSide {
    /// Long position was liquidated (triggered by a forced SELL order)
    LongLiquidated,
    /// Short position was liquidated (triggered by a forced BUY order)
    ShortLiquidated,
}

/// A single liquidation event with pre-parsed Decimal precision.
#[derive(Debug, Clone)]
pub struct LiquidationEvent {
    pub symbol: String,
    pub side: LiquidationSide,
    pub quantity: Decimal,
    pub price: Decimal,
    pub usd_value: Decimal,
    /// Milliseconds since epoch
    pub timestamp: u64,
}

/// Accumulator for liquidation events with rolling statistics.
///
/// Holds a bounded deque of recent events plus aggregate volume
/// tracking by side. Flash events track indices of large liquidations
/// that should be visually highlighted.
pub struct LiquidationFeed {
    pub events: VecDeque<LiquidationEvent>,
    pub scroll_offset: usize,
    pub total_volume: Decimal,
    pub long_liq_volume: Decimal,
    pub short_liq_volume: Decimal,
    pub large_threshold_usd: Decimal,
    pub flash_events: Vec<(usize, Instant)>,
    pub session_start: Instant,
}

impl LiquidationFeed {
    /// Create a new empty liquidation feed.
    pub fn new(large_threshold_usd: Decimal) -> Self {
        Self {
            events: VecDeque::new(),
            scroll_offset: 0,
            total_volume: Decimal::ZERO,
            long_liq_volume: Decimal::ZERO,
            short_liq_volume: Decimal::ZERO,
            large_threshold_usd,
            flash_events: Vec::new(),
            session_start: Instant::now(),
        }
    }

    /// Push a new liquidation event into the feed.
    ///
    /// Updates rolling volume totals by side, detects large liquidations
    /// for flash highlighting, and enforces the MAX_EVENTS bound.
    pub fn push_event(&mut self, event: LiquidationEvent) {
        // Update volume totals
        self.total_volume += event.usd_value;
        match event.side {
            LiquidationSide::LongLiquidated => {
                self.long_liq_volume += event.usd_value;
            }
            LiquidationSide::ShortLiquidated => {
                self.short_liq_volume += event.usd_value;
            }
        }

        // Check for large liquidation flash
        if event.usd_value >= self.large_threshold_usd {
            self.flash_events
                .push((self.events.len(), Instant::now()));
        }

        // Push event to back
        self.events.push_back(event);

        // Enforce MAX_EVENTS bound
        if self.events.len() > MAX_EVENTS {
            self.events.pop_front();

            // Adjust flash event indices (shift down by 1)
            for flash in &mut self.flash_events {
                flash.0 = flash.0.saturating_sub(1);
            }

            // Adjust scroll offset
            self.scroll_offset = self.scroll_offset.saturating_sub(1);
        }
    }

    /// Remove expired flash events (older than 1.5 seconds).
    pub fn expire_flashes(&mut self) {
        self.flash_events
            .retain(|(_, instant)| instant.elapsed() < Duration::from_millis(1500));
    }

    /// Returns the number of events in the feed.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}
