//! crypto-tui - Real-time crypto market data TUI
//!
//! This crate provides data types and utilities for processing
//! cryptocurrency market data and building trade-count candlesticks.

pub mod config;
pub mod data;
pub mod helpers;
pub mod logging;
pub mod network;
pub mod tui;

/// Tick rate for state updates (ms)
pub const TICK_RATE_MS: u64 = 250;

/// Target frame rate (FPS)
pub const FRAME_RATE: f64 = 30.0;

// Re-export commonly used types at crate root
pub use config::CandleMode;
