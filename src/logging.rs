//! Logging initialization module.
//!
//! Provides structured file logging with tracing-subscriber and tracing-appender.
//! Logs are written in JSON format with daily rotation.

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize the logging system with file output.
///
/// Creates a rolling log file in the specified directory with daily rotation.
/// Log format is JSON for structured parsing.
///
/// # Arguments
/// * `log_dir` - Directory path for log files (e.g., "./logs")
///
/// # Returns
/// A `WorkerGuard` that MUST be kept alive for the duration of the program.
/// Dropping the guard will stop log flushing to the file.
///
/// # Example
/// ```ignore
/// let _guard = init_logging("./logs");
/// // ... application code ...
/// // guard is dropped at end of main, flushing logs
/// ```
pub fn init_logging(log_dir: &str) -> WorkerGuard {
    // Create rolling file appender with daily rotation
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("crypto-tui")
        .filename_suffix("log")
        .build(log_dir)
        .expect("failed to create file appender");

    // Wrap in non-blocking writer for async compatibility
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Configure file layer: JSON format, no ANSI codes, include target and thread
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .json();

    // Environment filter with sensible defaults
    // RUST_LOG env var can override, otherwise default to crypto_tui=debug,info
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("crypto_tui=debug,info"));

    // Build and install the subscriber
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .init();

    guard
}
