// src/data/account.rs
// Account summary data model for AsterDEX account overview
//
// Pure computation functions for aggregating position metrics:
// total unrealized PnL, open position count, total notional exposure,
// long/short exposure breakdown, and daily realized PnL.

use crate::data::position::{Position, PositionSide};
use rust_decimal::Decimal;

/// Exposure metrics for one side (Long or Short)
#[derive(Debug, Clone)]
pub struct SideExposure {
    pub notional: Decimal,
    pub unrealized_pnl: Decimal,
    pub count: usize,
}

/// Aggregated account metrics for the account overview panel
#[derive(Debug, Clone)]
pub struct AccountSummary {
    pub wallet_balance: Decimal,
    pub available_balance: Decimal,
    pub total_unrealized_pnl: Decimal,
    pub open_position_count: usize,
    pub total_notional: Decimal,
    pub long_exposure: SideExposure,
    pub short_exposure: SideExposure,
    pub daily_realized_pnl: Decimal,
}

/// Sum unrealized PnL across all positions using Position::unrealized_pnl()
pub(crate) fn compute_total_unrealized_pnl(positions: &[Position]) -> Decimal {
    positions.iter().map(|p| p.unrealized_pnl()).sum()
}

/// Count of open positions (caller pre-filters zero-size per Phase 41 pattern)
#[allow(dead_code)] // Used by binary crate (src/run/account.rs) via build_account_summary
pub(crate) fn compute_open_position_count(positions: &[Position]) -> usize {
    positions.len()
}

/// Total notional exposure (absolute values) across all positions
#[allow(dead_code)] // Used by binary crate (src/run/account.rs) via build_account_summary
pub(crate) fn compute_total_notional(positions: &[Position]) -> Decimal {
    positions.iter().map(|p| p.notional.abs()).sum()
}

/// Break down exposure by side: returns (long, short) SideExposure
pub(crate) fn compute_exposure_by_side(positions: &[Position]) -> (SideExposure, SideExposure) {
    let mut long = SideExposure {
        notional: Decimal::ZERO,
        unrealized_pnl: Decimal::ZERO,
        count: 0,
    };
    let mut short = SideExposure {
        notional: Decimal::ZERO,
        unrealized_pnl: Decimal::ZERO,
        count: 0,
    };

    for p in positions {
        let target = match p.position_side {
            PositionSide::Long => &mut long,
            PositionSide::Short => &mut short,
        };
        target.notional += p.notional.abs();
        target.unrealized_pnl += p.unrealized_pnl();
        target.count += 1;
    }

    (long, short)
}

/// Sum daily realized PnL from extracted Decimal values (avoids coupling to network types)
#[allow(dead_code)] // Used by binary crate (src/run/account.rs) via build_account_summary
pub(crate) fn compute_daily_realized_pnl(realized_pnls: &[Decimal]) -> Decimal {
    realized_pnls.iter().copied().sum()
}

/// Convenience function: build complete AccountSummary from raw inputs
#[allow(dead_code)] // Used by binary crate (src/run/account.rs)
pub(crate) fn build_account_summary(
    wallet_balance: Decimal,
    available_balance: Decimal,
    positions: &[Position],
    realized_pnls: &[Decimal],
) -> AccountSummary {
    let total_unrealized_pnl = compute_total_unrealized_pnl(positions);
    let open_position_count = compute_open_position_count(positions);
    let total_notional = compute_total_notional(positions);
    let (long_exposure, short_exposure) = compute_exposure_by_side(positions);
    let daily_realized_pnl = compute_daily_realized_pnl(realized_pnls);

    AccountSummary {
        wallet_balance,
        available_balance,
        total_unrealized_pnl,
        open_position_count,
        total_notional,
        long_exposure,
        short_exposure,
        daily_realized_pnl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::str::FromStr;

    /// Helper: create a test position with specified fields, defaults for the rest
    fn make_test_position(
        symbol: &str,
        side: PositionSide,
        amt: &str,
        entry: &str,
        mark: &str,
        notional: &str,
        leverage: u32,
    ) -> Position {
        Position {
            symbol: symbol.to_string(),
            position_side: side,
            position_amt: Decimal::from_str(amt).unwrap(),
            entry_price: Decimal::from_str(entry).unwrap(),
            mark_price: Decimal::from_str(mark).unwrap(),
            unrealized_profit: Decimal::ZERO,
            liquidation_price: Decimal::ZERO,
            leverage,
            margin_type: String::new(),
            isolated_margin: Decimal::ZERO,
            notional: Decimal::from_str(notional).unwrap(),
            update_time: 0,
            funding_rate: None,
            next_funding_time: None,
        }
    }

    // ---- compute_total_unrealized_pnl tests ----

    #[test]
    fn test_total_unrealized_pnl_two_positions() {
        // Long 0.5 BTC: entry 50000, mark 51000 => pnl = 0.5 * (51000 - 50000) = 500
        let long = make_test_position("BTCUSDT", PositionSide::Long, "0.5", "50000", "51000", "25500", 10);
        // Short -1.0 ETH: entry 3000, mark 3100 => pnl = -1.0 * (3100 - 3000) = -100
        let short = make_test_position("ETHUSDT", PositionSide::Short, "-1.0", "3000", "3100", "-3100", 5);
        let positions = vec![long, short];
        let result = compute_total_unrealized_pnl(&positions);
        // 500 + (-100) = 400
        assert_eq!(result, dec!(400));
    }

    #[test]
    fn test_total_unrealized_pnl_empty() {
        let result = compute_total_unrealized_pnl(&[]);
        assert_eq!(result, Decimal::ZERO);
    }

    // ---- compute_open_position_count tests ----

    #[test]
    fn test_open_position_count() {
        let positions = vec![
            make_test_position("BTCUSDT", PositionSide::Long, "0.5", "50000", "51000", "25500", 10),
            make_test_position("ETHUSDT", PositionSide::Short, "-1.0", "3000", "3100", "-3100", 5),
            make_test_position("SOLUSDT", PositionSide::Long, "10.0", "100", "105", "1050", 20),
        ];
        assert_eq!(compute_open_position_count(&positions), 3);
    }

    // ---- compute_total_notional tests ----

    #[test]
    fn test_total_notional_uses_absolute_values() {
        // Notional can be negative for short positions; total should use abs
        let long = make_test_position("BTCUSDT", PositionSide::Long, "0.5", "50000", "51000", "25500", 10);
        let short = make_test_position("ETHUSDT", PositionSide::Short, "-1.0", "3000", "3100", "-3100", 5);
        let positions = vec![long, short];
        let result = compute_total_notional(&positions);
        // abs(25500) + abs(-3100) = 28600
        assert_eq!(result, dec!(28600));
    }

    // ---- compute_exposure_by_side tests ----

    #[test]
    fn test_exposure_by_side_long_and_short() {
        // 2 long positions + 1 short position
        let long1 = make_test_position("BTCUSDT", PositionSide::Long, "0.5", "50000", "51000", "25500", 10);
        let long2 = make_test_position("SOLUSDT", PositionSide::Long, "10.0", "100", "105", "1050", 20);
        let short1 = make_test_position("ETHUSDT", PositionSide::Short, "-1.0", "3000", "3100", "-3100", 5);
        let positions = vec![long1, long2, short1];

        let (long_exp, short_exp) = compute_exposure_by_side(&positions);

        // Long: notional = abs(25500) + abs(1050) = 26550
        assert_eq!(long_exp.notional, dec!(26550));
        // Long: pnl = 0.5*(51000-50000) + 10*(105-100) = 500 + 50 = 550
        assert_eq!(long_exp.unrealized_pnl, dec!(550));
        assert_eq!(long_exp.count, 2);

        // Short: notional = abs(-3100) = 3100
        assert_eq!(short_exp.notional, dec!(3100));
        // Short: pnl = -1.0*(3100-3000) = -100
        assert_eq!(short_exp.unrealized_pnl, dec!(-100));
        assert_eq!(short_exp.count, 1);
    }

    #[test]
    fn test_exposure_by_side_all_long() {
        let long1 = make_test_position("BTCUSDT", PositionSide::Long, "0.5", "50000", "51000", "25500", 10);
        let long2 = make_test_position("ETHUSDT", PositionSide::Long, "2.0", "3000", "3050", "6100", 5);
        let positions = vec![long1, long2];

        let (long_exp, short_exp) = compute_exposure_by_side(&positions);

        assert_eq!(long_exp.count, 2);
        assert!(long_exp.notional > Decimal::ZERO);

        // Short side should be zero
        assert_eq!(short_exp.notional, Decimal::ZERO);
        assert_eq!(short_exp.unrealized_pnl, Decimal::ZERO);
        assert_eq!(short_exp.count, 0);
    }

    // ---- compute_daily_realized_pnl tests ----

    #[test]
    fn test_daily_realized_pnl_sums_values() {
        let pnls = vec![dec!(150.50), dec!(-75.25), dec!(30.00)];
        let result = compute_daily_realized_pnl(&pnls);
        // 150.50 - 75.25 + 30.00 = 105.25
        assert_eq!(result, dec!(105.25));
    }

    #[test]
    fn test_daily_realized_pnl_empty() {
        let result = compute_daily_realized_pnl(&[]);
        assert_eq!(result, Decimal::ZERO);
    }

    // ---- build_account_summary integration test ----

    #[test]
    fn test_build_account_summary_integrates_all() {
        let positions = vec![
            make_test_position("BTCUSDT", PositionSide::Long, "0.5", "50000", "51000", "25500", 10),
            make_test_position("ETHUSDT", PositionSide::Short, "-1.0", "3000", "3100", "-3100", 5),
        ];
        let realized_pnls = vec![dec!(200), dec!(-50)];

        let summary = build_account_summary(
            dec!(10000),
            dec!(5000),
            &positions,
            &realized_pnls,
        );

        assert_eq!(summary.wallet_balance, dec!(10000));
        assert_eq!(summary.available_balance, dec!(5000));
        // total unrealized: 500 + (-100) = 400
        assert_eq!(summary.total_unrealized_pnl, dec!(400));
        assert_eq!(summary.open_position_count, 2);
        // total notional: abs(25500) + abs(-3100) = 28600
        assert_eq!(summary.total_notional, dec!(28600));
        // long exposure
        assert_eq!(summary.long_exposure.notional, dec!(25500));
        assert_eq!(summary.long_exposure.unrealized_pnl, dec!(500));
        assert_eq!(summary.long_exposure.count, 1);
        // short exposure
        assert_eq!(summary.short_exposure.notional, dec!(3100));
        assert_eq!(summary.short_exposure.unrealized_pnl, dec!(-100));
        assert_eq!(summary.short_exposure.count, 1);
        // daily realized: 200 + (-50) = 150
        assert_eq!(summary.daily_realized_pnl, dec!(150));
    }
}
