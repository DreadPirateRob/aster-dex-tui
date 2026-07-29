// src/tui/mod.rs
// TUI infrastructure module

pub mod account_app;
pub mod analytics_app;
pub mod app;
pub mod calendar_app;
pub mod dom_app;
pub mod event;
pub mod funding_app;
pub mod liquidations_app;
pub mod news_app;
pub mod orders_app;
pub mod overview_app;
pub mod positions_app;
pub mod terminal;
pub mod text_input;
pub mod theme;
pub mod trade_app;
pub mod widgets;

pub use account_app::AccountApp;
pub use analytics_app::AnalyticsApp;
pub use app::App;
pub use calendar_app::CalendarApp;
pub use dom_app::DomApp;
pub use event::{Event, EventHandler};
pub use funding_app::FundingApp;
pub use liquidations_app::LiquidationsApp;
pub use news_app::NewsApp;
pub use orders_app::OrdersApp;
pub use overview_app::OverviewApp;
pub use positions_app::PositionsApp;
pub use terminal::Tui;
pub use theme::{Theme, ThemeName};
pub use trade_app::TradeApp;
pub use widgets::{ui_orders, ui_positions, ui_trade, CandlestickChart, CvdSparkline, DomLadder, StatusBar, TradesList};
