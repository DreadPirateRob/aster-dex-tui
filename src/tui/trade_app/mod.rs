// src/tui/trade_app/mod.rs
// Trade ticket form state machine: TradeApp struct, type definitions (TradeOrderType, BracketState,
// LegResult, Focus, AppMode, OrderSummary), and constructor.

mod input;
mod order_builder;

use crate::data::order::{OrderSide, TimeInForce};
use crate::data::position::Position;
use crate::network::asterdex_exchange_info::SymbolInfo;
use crate::network::asterdex_trading::{OrderParams, OrderResponse, TradingError};
use crate::tui::text_input::TextInput;
use crate::tui::theme::Theme;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// TradeOrderType
// ---------------------------------------------------------------------------

/// Order types supported by the trade ticket form.
///
/// Cycles via hotkey: Market -> Limit -> StopMarket -> StopLimit -> TrailingStop -> Market
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeOrderType {
    Market,
    Limit,
    StopMarket,
    StopLimit,
    TrailingStop,
}

impl TradeOrderType {
    /// Cycle to the next order type.
    pub fn cycle(self) -> Self {
        match self {
            TradeOrderType::Market => TradeOrderType::Limit,
            TradeOrderType::Limit => TradeOrderType::StopMarket,
            TradeOrderType::StopMarket => TradeOrderType::StopLimit,
            TradeOrderType::StopLimit => TradeOrderType::TrailingStop,
            TradeOrderType::TrailingStop => TradeOrderType::Market,
        }
    }

    /// Cycle to the previous order type.
    pub fn cycle_back(self) -> Self {
        match self {
            TradeOrderType::Market => TradeOrderType::TrailingStop,
            TradeOrderType::Limit => TradeOrderType::Market,
            TradeOrderType::StopMarket => TradeOrderType::Limit,
            TradeOrderType::StopLimit => TradeOrderType::StopMarket,
            TradeOrderType::TrailingStop => TradeOrderType::StopLimit,
        }
    }

    /// Human-readable label for display.
    pub fn label(&self) -> &'static str {
        match self {
            TradeOrderType::Market => "MARKET",
            TradeOrderType::Limit => "LIMIT",
            TradeOrderType::StopMarket => "STOP MKT",
            TradeOrderType::StopLimit => "STOP LMT",
            TradeOrderType::TrailingStop => "TRAIL STOP",
        }
    }

    /// Returns true if this order type requires a price field.
    pub fn needs_price(&self) -> bool {
        matches!(self, TradeOrderType::Limit | TradeOrderType::StopLimit)
    }

    /// Returns true if this order type requires a stop price field.
    pub fn needs_stop_price(&self) -> bool {
        matches!(self, TradeOrderType::StopMarket | TradeOrderType::StopLimit)
    }

    /// Returns true if this order type requires a callback rate field.
    pub fn needs_callback_rate(&self) -> bool {
        matches!(self, TradeOrderType::TrailingStop)
    }

    /// Returns true if this order type supports an activation price field.
    pub fn needs_activation_price(&self) -> bool {
        matches!(self, TradeOrderType::TrailingStop)
    }
}

// ---------------------------------------------------------------------------
// BracketState & LegResult
// ---------------------------------------------------------------------------

/// Lifecycle state of a bracket order (entry + SL + TP).
///
/// Tracks the bracket from entry submission through SL/TP placement to completion.
#[derive(Debug, Clone)]
pub enum BracketState {
    /// Entry order submitted, waiting for FILLED status via ORDER_TRADE_UPDATE.
    WaitingForFill { entry_order_id: u64 },
    /// Entry filled, placing SL and TP orders.
    PlacingSLTP {
        _entry_qty: Decimal,
        _entry_avg_price: Decimal,
    },
    /// All legs resolved (success, failure, or skipped).
    Complete {
        entry: LegResult,
        sl: LegResult,
        tp: LegResult,
    },
}

/// Result of a single bracket leg (entry, SL, or TP).
#[derive(Debug, Clone)]
pub enum LegResult {
    /// Order placed successfully.
    Success { order_id: u64 },
    /// Order placement failed.
    Failed { error: String },
    /// Leg was skipped (e.g., no SL price entered).
    Skipped,
    /// Leg is in flight (not yet resolved).
    _Pending,
}

// ---------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------

/// Which form field currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Side,
    OrderType,
    Quantity,
    Price,
    StopPrice,
    CallbackRate,
    ActivationPrice,
    BracketSLPrice,
    BracketTPPrice,
    ReduceOnly,
    TimeInForce,
    Submit,
}

impl Focus {
    /// All focus variants in tab order.
    /// Tab order: Side, OrderType, Quantity, Price, StopPrice, CallbackRate, ActivationPrice,
    ///            BracketSLPrice, BracketTPPrice, ReduceOnly, TimeInForce, Submit
    const ALL: [Focus; 12] = [
        Focus::Side,
        Focus::OrderType,
        Focus::Quantity,
        Focus::Price,
        Focus::StopPrice,
        Focus::CallbackRate,
        Focus::ActivationPrice,
        Focus::BracketSLPrice,
        Focus::BracketTPPrice,
        Focus::ReduceOnly,
        Focus::TimeInForce,
        Focus::Submit,
    ];

    /// Returns true if this focus variant is visible for the given order type and bracket state.
    fn is_visible(&self, order_type: &TradeOrderType, bracket_enabled: bool) -> bool {
        match self {
            Focus::Side | Focus::OrderType | Focus::Quantity | Focus::Submit => true,
            Focus::Price => order_type.needs_price(),
            Focus::StopPrice => order_type.needs_stop_price(),
            Focus::CallbackRate => order_type.needs_callback_rate(),
            Focus::ActivationPrice => order_type.needs_activation_price(),
            Focus::BracketSLPrice => bracket_enabled,
            Focus::BracketTPPrice => bracket_enabled,
            // ReduceOnly is always visible (applies to all order types)
            Focus::ReduceOnly => true,
            // TimeInForce is visible only for order types that need price (Limit, StopLimit)
            Focus::TimeInForce => order_type.needs_price(),
        }
    }

    /// Tab forward, skipping fields not relevant for the current order type or bracket state.
    pub fn next(self, order_type: &TradeOrderType, bracket_enabled: bool) -> Self {
        let current_idx = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        for offset in 1..=Self::ALL.len() {
            let idx = (current_idx + offset) % Self::ALL.len();
            let candidate = Self::ALL[idx];
            if candidate.is_visible(order_type, bracket_enabled) {
                return candidate;
            }
        }
        self // fallback (should not happen)
    }

    /// Shift-Tab backward, skipping fields not relevant for the current order type or bracket state.
    pub fn prev(self, order_type: &TradeOrderType, bracket_enabled: bool) -> Self {
        let current_idx = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        for offset in 1..=Self::ALL.len() {
            let idx = (current_idx + Self::ALL.len() - offset) % Self::ALL.len();
            let candidate = Self::ALL[idx];
            if candidate.is_visible(order_type, bracket_enabled) {
                return candidate;
            }
        }
        self // fallback (should not happen)
    }

    /// Returns true if this focus is a text input field.
    pub fn is_text_field(&self) -> bool {
        matches!(
            self,
            Focus::Quantity
                | Focus::Price
                | Focus::StopPrice
                | Focus::CallbackRate
                | Focus::ActivationPrice
                | Focus::BracketSLPrice
                | Focus::BracketTPPrice
        )
    }
}

// ---------------------------------------------------------------------------
// AppMode
// ---------------------------------------------------------------------------

/// Current mode of the trade ticket form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Normal form editing (navigating, typing)
    Editing,
    /// Showing confirmation dialog before order submission
    Confirming,
    /// Waiting for async order placement to complete
    Submitting,
    /// Showing the result of order placement (success or failure)
    ShowingResult,
    /// Waiting for bracket entry order to fill (watching ORDER_TRADE_UPDATE)
    BracketWaiting,
    /// Showing per-leg bracket result (entry + SL + TP statuses)
    ShowingBracketResult,
}

// ---------------------------------------------------------------------------
// OrderSummary
// ---------------------------------------------------------------------------

/// Snapshot of order details for the confirmation dialog.
///
/// Captured at the moment the user presses Enter on Submit,
/// so the confirmation shows exactly what was validated.
#[derive(Debug, Clone)]
pub struct OrderSummary {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: TradeOrderType,
    pub quantity: Decimal,
    pub price: Option<Decimal>,
    pub stop_price: Option<Decimal>,
    pub callback_rate: Option<Decimal>,
    pub activation_price: Option<Decimal>,
    pub mark_price: Option<Decimal>,
    pub bracket_sl_price: Option<Decimal>,
    pub bracket_tp_price: Option<Decimal>,
}

// ---------------------------------------------------------------------------
// TradeApp
// ---------------------------------------------------------------------------

/// Application state for the trade ticket TUI form.
///
/// Holds complete form state: side, order type, text inputs, focus position,
/// mode (editing/confirming/submitting/result), and async submission channel.
pub struct TradeApp {
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange info for the symbol (precision, filters)
    pub symbol_info: SymbolInfo,
    /// Color theme
    pub theme: Theme,
    /// Flag to signal graceful shutdown
    pub should_quit: bool,

    // Form fields
    /// Buy or Sell
    pub side: OrderSide,
    /// Order type (Market, Limit, StopMarket, StopLimit)
    pub order_type: TradeOrderType,
    /// Quantity text input
    pub quantity_input: TextInput,
    /// Price text input (for Limit, StopLimit)
    pub price_input: TextInput,
    /// Stop price text input (for StopMarket, StopLimit)
    pub stop_price_input: TextInput,
    /// Callback rate text input (for TrailingStop, 0.1-5.0%)
    pub callback_rate_input: TextInput,
    /// Activation price text input (for TrailingStop, optional)
    pub activation_price_input: TextInput,
    /// Reduce-only mode toggle (default false)
    pub reduce_only: bool,
    /// Time-in-force for limit orders (default GTC)
    pub time_in_force: TimeInForce,

    // Navigation / mode
    /// Current focused field
    pub focus: Focus,
    /// Current form mode
    pub mode: AppMode,

    // Market data
    /// Current mark price from watch channel (updated externally)
    pub mark_price: Option<Decimal>,
    /// Available account balance (populated async from main.rs)
    pub available_balance: Option<Decimal>,
    /// Current leverage setting (populated from leverage response)
    pub leverage: Option<u32>,

    // Order lifecycle
    /// Result of the last order placement attempt
    pub order_result: Option<Result<OrderResponse, TradingError>>,
    /// Snapshot for confirmation dialog
    pub confirm_snapshot: Option<OrderSummary>,
    /// Channel to send validated OrderParams to the event loop for async placement
    pub submit_tx: Option<mpsc::Sender<OrderParams>>,
    /// Validation error message shown inline
    pub error_message: Option<String>,
    /// Recent orders list (populated from order history)
    pub recent_orders: Vec<crate::data::order::Order>,
    /// Maximum leverage for the symbol (from leverage brackets, first bracket's initial_leverage)
    pub max_leverage: Option<u32>,
    /// Full leverage brackets for notional cap validation
    pub leverage_brackets: Vec<crate::network::asterdex_trading::LeverageBracket>,
    /// Whether to show the recent orders table (Zone 3), default true
    pub show_orders: bool,
    /// Channel to send leverage change requests to the event loop
    pub leverage_change_tx: Option<mpsc::Sender<u32>>,
    /// Quantity precision for the current symbol (from SymbolInfo)
    pub quantity_precision: u32,
    /// Whether leverage editing mode is active (typing a leverage value)
    pub leverage_editing: bool,
    /// Text input buffer for leverage editing
    pub leverage_input: TextInput,
    /// Current position for this symbol (from REST + ACCOUNT_UPDATE)
    pub current_position: Option<Position>,

    // Bracket order fields
    /// Whether bracket mode is enabled (SL/TP placed after entry fills)
    pub bracket_enabled: bool,
    /// Stop-loss price input for bracket orders
    pub bracket_sl_price: TextInput,
    /// Take-profit price input for bracket orders
    pub bracket_tp_price: TextInput,
    /// Bracket lifecycle state (None when no bracket in progress)
    pub bracket_state: Option<BracketState>,
}

impl TradeApp {
    /// Create a new TradeApp with default form state.
    pub fn new(symbol: String, symbol_info: SymbolInfo, theme: Theme) -> Self {
        let quantity_precision = symbol_info.quantity_precision as u32;
        Self {
            symbol,
            symbol_info,
            theme,
            should_quit: false,
            side: OrderSide::Buy,
            order_type: TradeOrderType::Market,
            quantity_input: TextInput::new(),
            price_input: TextInput::new(),
            stop_price_input: TextInput::new(),
            callback_rate_input: TextInput::new(),
            activation_price_input: TextInput::new(),
            reduce_only: false,
            time_in_force: TimeInForce::Gtc,
            focus: Focus::Side,
            mode: AppMode::Editing,
            mark_price: None,
            available_balance: None,
            leverage: None,
            order_result: None,
            confirm_snapshot: None,
            submit_tx: None,
            error_message: None,
            recent_orders: Vec::new(),
            max_leverage: None,
            leverage_brackets: Vec::new(),
            show_orders: true,
            leverage_change_tx: None,
            quantity_precision,
            leverage_editing: false,
            leverage_input: TextInput::new(),
            current_position: None,
            bracket_enabled: false,
            bracket_sl_price: TextInput::new(),
            bracket_tp_price: TextInput::new(),
            bracket_state: None,
        }
    }
}

#[cfg(test)]
mod tests;
