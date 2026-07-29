// src/network/types.rs
// Connection state enum and status struct

use std::time::Instant;

/// Connection lifecycle states
#[derive(Debug, Clone)]
pub enum ConnectionState {
    /// Not connected, no active connection attempt
    Disconnected,
    /// Attempting to establish connection
    Connecting,
    /// Successfully connected and receiving data
    Connected,
    /// Connection lost, attempting to reconnect
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
        _next_retry_at: Option<Instant>,
    },
}

impl PartialEq for ConnectionState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Disconnected, Self::Disconnected) => true,
            (Self::Connecting, Self::Connecting) => true,
            (Self::Connected, Self::Connected) => true,
            (
                Self::Reconnecting {
                    attempt: a1,
                    max_attempts: m1,
                    ..
                },
                Self::Reconnecting {
                    attempt: a2,
                    max_attempts: m2,
                    ..
                },
            ) => a1 == a2 && m1 == m2,
            _ => false,
        }
    }
}

/// Connection status with metadata for UI display
#[derive(Debug, Clone)]
pub struct ConnectionStatus {
    /// Current connection state
    pub state: ConnectionState,
    /// Last error message (if any)
    pub _last_error: Option<String>,
    /// Total retry attempts since last successful connection
    pub retry_count: u32,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            _last_error: None,
            retry_count: 0,
        }
    }
}
