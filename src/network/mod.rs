// src/network/mod.rs
// Network layer for AsterDEX exchange connectivity (WebSocket and REST)

pub mod asterdex_account;
pub mod asterdex_auth;
pub mod asterdex_candles;
pub mod asterdex_depth;
pub mod asterdex_exchange_info;
pub mod asterdex_funding_stream;
pub mod asterdex_liquidation_stream;
pub mod asterdex_ticker_stream;
pub mod asterdex_mark_price;
pub mod asterdex_orders;
pub mod asterdex_positions;
pub mod asterdex_stream_types;
pub mod asterdex_trade_stream;
pub mod asterdex_trades;
pub mod asterdex_trading;
pub mod asterdex_user_stream;
pub mod cache;
pub mod crypto_news_stream;
pub mod finnhub_calendar;
pub mod heartbeat;
pub mod kline;
pub mod reconnect;
pub mod types;

// Re-export key types for convenient access
pub use asterdex_account::{fetch_account_info, compute_margin_ratio, AccountInfo};
pub use asterdex_depth::{DepthUpdate, fetch_depth, run_depth_stream};
pub use asterdex_mark_price::{MarkPriceData, run_multi_mark_price_stream};
pub use asterdex_exchange_info::validate_symbol;
pub use asterdex_orders::{fetch_open_orders, fetch_orders};
pub use asterdex_positions::{fetch_positions, AsterDexPosition};
pub use asterdex_trades::{fetch_user_trades, start_of_today_utc_ms, AsterDexUserTrade};
pub use asterdex_trading::{cancel_all_open_orders, cancel_order, change_leverage, fetch_account_balance, fetch_leverage_brackets, fetch_position_mode, place_order, user_friendly_cancel_error, LeverageBracket, OrderParams, OrderResponse, PositionMode, TradingError};
pub use asterdex_candles::{fetch_asterdex_agg_trades, load_asterdex_historical_klines};
pub use asterdex_funding_stream::{fetch_premium_index, run_all_mark_price_stream};
pub use asterdex_liquidation_stream::run_liquidation_stream;
pub use asterdex_ticker_stream::{fetch_ticker_24hr, run_ticker_stream};
pub use crypto_news_stream::{fetch_news_bootstrap, run_news_stream};
pub use finnhub_calendar::{fetch_economic_calendar, poll_economic_calendar};
pub use asterdex_trade_stream::{AsterDexTradeConnectionManager, run_dom_trade_stream};
pub use types::{ConnectionState, ConnectionStatus};
pub use asterdex_stream_types::AccountUpdateData;
pub use asterdex_user_stream::AsterDexUserStreamManager;
