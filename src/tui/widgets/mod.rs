// src/tui/widgets/mod.rs
// Widget components for the TUI

pub mod account_view;
pub mod analytics_dashboard;
pub mod calendar_widget;
pub mod candlestick_chart;
pub mod cvd_sparkline;
pub mod dom_ladder;
pub mod flow_imbalance;
pub mod funding_dashboard;
pub mod liquidation_feed;
pub mod news_feed_widget;
pub mod order_table;
pub mod overview_dashboard;
pub mod position_table;
pub mod status_bar;
pub mod trade_ticket;
pub mod trades_list;

pub use account_view::ui_account;
pub use analytics_dashboard::AnalyticsDashboardWidget;
pub use calendar_widget::CalendarWidget;
pub use candlestick_chart::CandlestickChart;
pub use cvd_sparkline::CvdSparkline;
pub use dom_ladder::DomLadder;
pub use flow_imbalance::FlowImbalance;
pub use funding_dashboard::FundingDashboardWidget;
pub use liquidation_feed::LiquidationFeedWidget;
pub use news_feed_widget::NewsFeedWidget;
pub use order_table::ui_orders;
pub use overview_dashboard::OverviewDashboardWidget;
pub use position_table::ui_positions;
pub use status_bar::StatusBar;
pub use trade_ticket::ui_trade;
pub use trades_list::{TradesDisplay, TradesList};
