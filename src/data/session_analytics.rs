// src/data/session_analytics.rs
// Session analytics data accumulator for live PnL tracking, win rate, fees, and drawdown.
//
// Processes ORDER_TRADE_UPDATE fill events to track realized PnL, commissions,
// win/loss classification, equity curve with drawdown, and recent fills.

use crate::network::asterdex_stream_types::OrderUpdatePayload;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::VecDeque;
use std::str::FromStr;
use std::time::Instant;

/// Snapshot equity every 10 seconds.
const EQUITY_SNAPSHOT_INTERVAL_SECS: u64 = 10;

/// 120 snapshots * 10s = 20 minutes of equity history.
const EQUITY_CURVE_LENGTH: usize = 120;

/// Maximum number of recent fills kept in memory.
#[allow(dead_code)]
const MAX_RECENT_FILLS: usize = 200;

/// A single fill event recorded for display in the recent fills list.
#[derive(Debug, Clone)]
pub struct SessionFill {
    pub symbol: String,
    pub side: String,
    pub price: Decimal,
    pub quantity: Decimal,
    pub realized_pnl: Decimal,
    #[allow(dead_code)]
    pub commission: Decimal,
    /// Milliseconds since epoch.
    pub time: u64,
}

/// Session analytics accumulator tracking realized PnL, commissions, win/loss,
/// equity curve, drawdown, and recent fills.
///
/// Follows the LiquidationFeed bounded-accumulator pattern with session-scoped
/// statistics.
pub struct SessionAnalytics {
    // Session identity
    pub session_start: Instant,
    pub session_start_utc: chrono::DateTime<chrono::Utc>,

    // Realized PnL tracking (from ORDER_TRADE_UPDATE rp field)
    pub total_realized_pnl: Decimal,
    pub trade_count: u32,
    pub winning_trades: u32,
    pub losing_trades: u32,
    pub total_win_amount: Decimal,
    /// Stored as positive value.
    pub total_loss_amount: Decimal,

    // Commission tracking (from ORDER_TRADE_UPDATE n field)
    pub total_commissions: Decimal,

    // Unrealized PnL (from ACCOUNT_UPDATE + mark price)
    pub current_unrealized_pnl: Decimal,

    // Equity curve for sparkline
    /// Scaled combined PnL values, bounded at EQUITY_CURVE_LENGTH.
    pub equity_curve: VecDeque<i64>,
    pub last_equity_snapshot: Instant,

    // Drawdown tracking
    pub peak_equity: Decimal,
    pub max_drawdown: Decimal,
    pub current_drawdown: Decimal,

    // Starting balance (REST bootstrap)
    pub starting_wallet_balance: Decimal,

    // Recent fills for display
    pub recent_fills: VecDeque<SessionFill>,

    // Bootstrap cutoff to prevent double-counting
    pub bootstrap_cutoff_time: u64,
}

impl SessionAnalytics {
    /// Create a new SessionAnalytics with all counters at zero.
    pub fn new(starting_wallet_balance: Decimal) -> Self {
        Self {
            session_start: Instant::now(),
            session_start_utc: chrono::Utc::now(),
            total_realized_pnl: Decimal::ZERO,
            trade_count: 0,
            winning_trades: 0,
            losing_trades: 0,
            total_win_amount: Decimal::ZERO,
            total_loss_amount: Decimal::ZERO,
            total_commissions: Decimal::ZERO,
            current_unrealized_pnl: Decimal::ZERO,
            equity_curve: VecDeque::new(),
            last_equity_snapshot: Instant::now(),
            peak_equity: Decimal::ZERO,
            max_drawdown: Decimal::ZERO,
            current_drawdown: Decimal::ZERO,
            starting_wallet_balance,
            recent_fills: VecDeque::new(),
            bootstrap_cutoff_time: 0,
        }
    }

    /// Process an ORDER_TRADE_UPDATE fill event.
    #[allow(dead_code)]
    ///
    /// Only processes FILLED or PARTIALLY_FILLED events. Skips events with
    /// trade_time <= bootstrap_cutoff_time to prevent double-counting.
    /// If symbol_filter is Some, skips events that don't match.
    pub fn process_order_fill(&mut self, payload: &OrderUpdatePayload, symbol_filter: Option<&str>) {
        // Only process fill events
        if payload.status != "FILLED" && payload.status != "PARTIALLY_FILLED" {
            return;
        }

        // Apply symbol filter
        if let Some(filter) = symbol_filter {
            if !payload.s.eq_ignore_ascii_case(filter) {
                return;
            }
        }

        // Guard against bootstrap double-counting
        if payload.trade_time <= self.bootstrap_cutoff_time {
            return;
        }

        // Parse realized PnL and commission
        let realized_pnl = Decimal::from_str(&payload.rp).unwrap_or(Decimal::ZERO);
        let commission = Decimal::from_str(&payload.n).unwrap_or(Decimal::ZERO);

        // Accumulate commission
        self.total_commissions += commission;

        // Track realized PnL (only count non-zero rp as actual trades with PnL)
        if realized_pnl != Decimal::ZERO {
            self.total_realized_pnl += realized_pnl;
            self.trade_count += 1;

            if realized_pnl > Decimal::ZERO {
                self.winning_trades += 1;
                self.total_win_amount += realized_pnl;
            } else {
                self.losing_trades += 1;
                self.total_loss_amount += realized_pnl.abs();
            }
        }

        // Parse fill details for recent fills display
        let price = Decimal::from_str(&payload.ap).unwrap_or(Decimal::ZERO);
        let quantity = Decimal::from_str(&payload.z).unwrap_or(Decimal::ZERO);

        // Push to recent fills
        self.recent_fills.push_back(SessionFill {
            symbol: payload.s.clone(),
            side: payload.side.clone(),
            price,
            quantity,
            realized_pnl,
            commission,
            time: payload.trade_time,
        });

        // Enforce bounds
        if self.recent_fills.len() > MAX_RECENT_FILLS {
            self.recent_fills.pop_front();
        }
    }

    /// Take a snapshot of the current equity for the sparkline curve.
    ///
    /// Computes combined PnL, scales to i64, and updates drawdown tracking.
    pub fn snapshot_equity(&mut self) {
        let combined_pnl = self.total_realized_pnl + self.current_unrealized_pnl - self.total_commissions;
        let scaled = (combined_pnl * Decimal::from(100))
            .to_i64()
            .unwrap_or(0);

        self.equity_curve.push_back(scaled);
        if self.equity_curve.len() > EQUITY_CURVE_LENGTH {
            self.equity_curve.pop_front();
        }

        // Update drawdown tracking
        if combined_pnl > self.peak_equity {
            self.peak_equity = combined_pnl;
        }
        let drawdown = self.peak_equity - combined_pnl;
        self.current_drawdown = drawdown;
        if drawdown > self.max_drawdown {
            self.max_drawdown = drawdown;
        }

        self.last_equity_snapshot = Instant::now();
    }

    /// Convert equity curve to sparkline-compatible u64 data.
    ///
    /// Shift-to-zero transform matching TradeStats::cvd_as_sparkline_data() pattern:
    /// find minimum, subtract from all values, cast to u64.
    pub fn equity_as_sparkline_data(&self) -> Vec<u64> {
        if self.equity_curve.is_empty() {
            return vec![];
        }

        let min_val = *self.equity_curve.iter().min().unwrap_or(&0);
        self.equity_curve
            .iter()
            .map(|v| (v - min_val) as u64)
            .collect()
    }

    /// Win rate as a percentage (0.0 to 100.0). Returns 0.0 if no trades.
    pub fn win_rate(&self) -> f64 {
        if self.trade_count == 0 {
            return 0.0;
        }
        (self.winning_trades as f64 / self.trade_count as f64) * 100.0
    }

    /// Average win amount. Returns ZERO if no winning trades.
    pub fn avg_win(&self) -> Decimal {
        if self.winning_trades == 0 {
            return Decimal::ZERO;
        }
        self.total_win_amount / Decimal::from(self.winning_trades)
    }

    /// Average loss amount. Returns ZERO if no losing trades.
    pub fn avg_loss(&self) -> Decimal {
        if self.losing_trades == 0 {
            return Decimal::ZERO;
        }
        self.total_loss_amount / Decimal::from(self.losing_trades)
    }

    /// Net PnL: realized + unrealized - commissions.
    pub fn net_pnl(&self) -> Decimal {
        self.total_realized_pnl + self.current_unrealized_pnl - self.total_commissions
    }

    /// Session duration since start.
    pub fn session_duration(&self) -> std::time::Duration {
        self.session_start.elapsed()
    }

    /// Whether enough time has elapsed for a new equity snapshot.
    pub fn should_snapshot_equity(&self) -> bool {
        self.last_equity_snapshot.elapsed().as_secs() >= EQUITY_SNAPSHOT_INTERVAL_SECS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_fill_payload(symbol: &str, side: &str, status: &str, rp: &str, n: &str, trade_time: u64) -> OrderUpdatePayload {
        OrderUpdatePayload {
            s: symbol.to_string(),
            side: side.to_string(),
            order_type: "MARKET".to_string(),
            i: 1,
            status: status.to_string(),
            p: "50000".to_string(),
            q: "0.1".to_string(),
            z: "0.1".to_string(),
            ap: "50000".to_string(),
            trade_time,
            n: n.to_string(),
            rp: rp.to_string(),
            commission_asset: "USDT".to_string(),
        }
    }

    #[test]
    fn test_new_session_analytics() {
        let analytics = SessionAnalytics::new(dec!(10000));
        assert_eq!(analytics.total_realized_pnl, Decimal::ZERO);
        assert_eq!(analytics.trade_count, 0);
        assert_eq!(analytics.winning_trades, 0);
        assert_eq!(analytics.losing_trades, 0);
        assert_eq!(analytics.total_commissions, Decimal::ZERO);
        assert_eq!(analytics.starting_wallet_balance, dec!(10000));
        assert!(analytics.recent_fills.is_empty());
        assert!(analytics.equity_curve.is_empty());
    }

    #[test]
    fn test_process_winning_fill() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        let payload = make_fill_payload("BTCUSDT", "SELL", "FILLED", "50.5", "2.0", 1000);

        analytics.process_order_fill(&payload, None);

        assert_eq!(analytics.total_realized_pnl, dec!(50.5));
        assert_eq!(analytics.trade_count, 1);
        assert_eq!(analytics.winning_trades, 1);
        assert_eq!(analytics.losing_trades, 0);
        assert_eq!(analytics.total_win_amount, dec!(50.5));
        assert_eq!(analytics.total_commissions, dec!(2.0));
        assert_eq!(analytics.recent_fills.len(), 1);
    }

    #[test]
    fn test_process_losing_fill() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        let payload = make_fill_payload("BTCUSDT", "SELL", "FILLED", "-30.0", "1.5", 1000);

        analytics.process_order_fill(&payload, None);

        assert_eq!(analytics.total_realized_pnl, dec!(-30.0));
        assert_eq!(analytics.trade_count, 1);
        assert_eq!(analytics.winning_trades, 0);
        assert_eq!(analytics.losing_trades, 1);
        assert_eq!(analytics.total_loss_amount, dec!(30.0));
        assert_eq!(analytics.total_commissions, dec!(1.5));
    }

    #[test]
    fn test_zero_rp_fill_not_counted_as_trade() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        // Entry fill has rp=0 (position opened, no realized PnL)
        let payload = make_fill_payload("BTCUSDT", "BUY", "FILLED", "0", "2.0", 1000);

        analytics.process_order_fill(&payload, None);

        assert_eq!(analytics.trade_count, 0);
        assert_eq!(analytics.winning_trades, 0);
        assert_eq!(analytics.losing_trades, 0);
        // Commission still accumulated
        assert_eq!(analytics.total_commissions, dec!(2.0));
        // Fill still recorded
        assert_eq!(analytics.recent_fills.len(), 1);
    }

    #[test]
    fn test_symbol_filter() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        let btc_fill = make_fill_payload("BTCUSDT", "SELL", "FILLED", "10.0", "1.0", 1000);
        let eth_fill = make_fill_payload("ETHUSDT", "SELL", "FILLED", "5.0", "0.5", 1001);

        analytics.process_order_fill(&btc_fill, Some("BTCUSDT"));
        analytics.process_order_fill(&eth_fill, Some("BTCUSDT"));

        assert_eq!(analytics.trade_count, 1);
        assert_eq!(analytics.total_realized_pnl, dec!(10.0));
        assert_eq!(analytics.recent_fills.len(), 1);
    }

    #[test]
    fn test_bootstrap_cutoff() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        analytics.bootstrap_cutoff_time = 500;

        let old_fill = make_fill_payload("BTCUSDT", "SELL", "FILLED", "10.0", "1.0", 500);
        let new_fill = make_fill_payload("BTCUSDT", "SELL", "FILLED", "20.0", "1.0", 501);

        analytics.process_order_fill(&old_fill, None);
        analytics.process_order_fill(&new_fill, None);

        // Only the new fill should be processed
        assert_eq!(analytics.trade_count, 1);
        assert_eq!(analytics.total_realized_pnl, dec!(20.0));
    }

    #[test]
    fn test_non_fill_status_ignored() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        let new_order = make_fill_payload("BTCUSDT", "BUY", "NEW", "0", "0", 1000);
        let canceled = make_fill_payload("BTCUSDT", "BUY", "CANCELED", "0", "0", 1001);

        analytics.process_order_fill(&new_order, None);
        analytics.process_order_fill(&canceled, None);

        assert_eq!(analytics.trade_count, 0);
        assert_eq!(analytics.recent_fills.len(), 0);
    }

    #[test]
    fn test_win_rate() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        analytics.winning_trades = 3;
        analytics.losing_trades = 2;
        analytics.trade_count = 5;

        assert!((analytics.win_rate() - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_win_rate_no_trades() {
        let analytics = SessionAnalytics::new(dec!(10000));
        assert!((analytics.win_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_avg_win_loss() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        analytics.winning_trades = 2;
        analytics.total_win_amount = dec!(100);
        analytics.losing_trades = 3;
        analytics.total_loss_amount = dec!(60);

        assert_eq!(analytics.avg_win(), dec!(50));
        assert_eq!(analytics.avg_loss(), dec!(20));
    }

    #[test]
    fn test_net_pnl() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        analytics.total_realized_pnl = dec!(100);
        analytics.current_unrealized_pnl = dec!(50);
        analytics.total_commissions = dec!(10);

        assert_eq!(analytics.net_pnl(), dec!(140));
    }

    #[test]
    fn test_equity_snapshot_and_sparkline() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        analytics.total_realized_pnl = dec!(10);
        analytics.current_unrealized_pnl = dec!(5);
        analytics.total_commissions = dec!(2);

        analytics.snapshot_equity();

        assert_eq!(analytics.equity_curve.len(), 1);
        // combined = 10 + 5 - 2 = 13, scaled = 1300
        assert_eq!(analytics.equity_curve[0], 1300);

        let sparkline = analytics.equity_as_sparkline_data();
        assert_eq!(sparkline.len(), 1);
        assert_eq!(sparkline[0], 0); // single value, min shift makes it 0
    }

    #[test]
    fn test_drawdown_tracking() {
        let mut analytics = SessionAnalytics::new(dec!(10000));

        // Peak at +100
        analytics.total_realized_pnl = dec!(100);
        analytics.snapshot_equity();
        assert_eq!(analytics.peak_equity, dec!(100));
        assert_eq!(analytics.max_drawdown, Decimal::ZERO);

        // Drop to +60
        analytics.total_realized_pnl = dec!(60);
        analytics.snapshot_equity();
        assert_eq!(analytics.peak_equity, dec!(100));
        assert_eq!(analytics.current_drawdown, dec!(40));
        assert_eq!(analytics.max_drawdown, dec!(40));

        // Recovery to +80 (drawdown = 20, but max stays 40)
        analytics.total_realized_pnl = dec!(80);
        analytics.snapshot_equity();
        assert_eq!(analytics.current_drawdown, dec!(20));
        assert_eq!(analytics.max_drawdown, dec!(40));
    }

    #[test]
    fn test_equity_curve_bounded() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        for i in 0..150 {
            analytics.total_realized_pnl = Decimal::from(i);
            analytics.snapshot_equity();
        }
        assert_eq!(analytics.equity_curve.len(), EQUITY_CURVE_LENGTH);
    }

    #[test]
    fn test_partially_filled() {
        let mut analytics = SessionAnalytics::new(dec!(10000));
        let payload = make_fill_payload("BTCUSDT", "SELL", "PARTIALLY_FILLED", "5.0", "0.5", 1000);

        analytics.process_order_fill(&payload, None);

        assert_eq!(analytics.trade_count, 1);
        assert_eq!(analytics.total_realized_pnl, dec!(5.0));
        assert_eq!(analytics.total_commissions, dec!(0.5));
    }
}
