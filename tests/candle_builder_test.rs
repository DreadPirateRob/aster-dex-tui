use crypto_tui::data::candle::{CandleStore, TradeCountBarBuilder};
use crypto_tui::data::types::{Candle, Trade, TradeSide};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

fn make_trade(id: u64, price: &str, quantity: &str, timestamp: u64) -> Trade {
    Trade {
        id,
        price: price.parse().unwrap(),
        quantity: quantity.parse().unwrap(),
        timestamp,
        side: TradeSide::Buy,
        is_buyer_maker: false,
    }
}

#[test]
fn builder_returns_none_when_under_threshold() {
    let mut builder = TradeCountBarBuilder::new(3);
    let trade = make_trade(1, "100.0", "1.0", 1000);
    assert!(builder.add_trade(&trade).is_none());
    assert!(builder.add_trade(&trade).is_none());
    // Still under threshold (2 < 3)
}

#[test]
fn builder_returns_candle_at_threshold() {
    let mut builder = TradeCountBarBuilder::new(3);
    builder.add_trade(&make_trade(1, "100.0", "1.0", 1000));
    builder.add_trade(&make_trade(2, "100.0", "1.0", 1001));
    let result = builder.add_trade(&make_trade(3, "100.0", "1.0", 1002));
    assert!(result.is_some());
    let candle = result.unwrap();
    assert_eq!(candle.trade_count, 3);
}

#[test]
fn builder_calculates_correct_ohlc() {
    let mut builder = TradeCountBarBuilder::new(3);
    builder.add_trade(&make_trade(1, "100.0", "1.0", 1000)); // Open
    builder.add_trade(&make_trade(2, "105.0", "1.0", 1001)); // High
    let candle = builder
        .add_trade(&make_trade(3, "95.0", "1.0", 1002))
        .unwrap(); // Low, Close

    assert_eq!(candle.open, dec!(100.0));
    assert_eq!(candle.high, dec!(105.0));
    assert_eq!(candle.low, dec!(95.0));
    assert_eq!(candle.close, dec!(95.0));
}

#[test]
fn builder_calculates_correct_volume() {
    let mut builder = TradeCountBarBuilder::new(3);
    builder.add_trade(&make_trade(1, "100.0", "1.5", 1000));
    builder.add_trade(&make_trade(2, "100.0", "2.5", 1001));
    let candle = builder
        .add_trade(&make_trade(3, "100.0", "3.0", 1002))
        .unwrap();

    assert_eq!(candle.volume, dec!(7.0)); // 1.5 + 2.5 + 3.0
}

#[test]
fn builder_tracks_timestamps() {
    let mut builder = TradeCountBarBuilder::new(3);
    builder.add_trade(&make_trade(1, "100.0", "1.0", 1000));
    builder.add_trade(&make_trade(2, "100.0", "1.0", 1500));
    let candle = builder
        .add_trade(&make_trade(3, "100.0", "1.0", 2000))
        .unwrap();

    assert_eq!(candle.open_time, 1000);
    assert_eq!(candle.close_time, 2000);
}

#[test]
fn builder_resets_after_completion() {
    let mut builder = TradeCountBarBuilder::new(2);
    builder.add_trade(&make_trade(1, "100.0", "1.0", 1000));
    let candle1 = builder
        .add_trade(&make_trade(2, "100.0", "1.0", 1001))
        .unwrap();

    // Next trade starts new candle
    assert!(builder
        .add_trade(&make_trade(3, "200.0", "1.0", 2000))
        .is_none());
    let candle2 = builder
        .add_trade(&make_trade(4, "200.0", "1.0", 2001))
        .unwrap();

    assert_eq!(candle1.open, dec!(100.0));
    assert_eq!(candle2.open, dec!(200.0));
}

#[test]
fn candle_store_respects_capacity() {
    let mut store = CandleStore::new(3);
    // Add 4 candles to a store with capacity 3
    for i in 0..4 {
        store.push(Candle {
            open: Decimal::from(i),
            high: Decimal::from(i),
            low: Decimal::from(i),
            close: Decimal::from(i),
            volume: Decimal::ONE,
            buy_volume: Decimal::ZERO,
            sell_volume: Decimal::ZERO,
            trade_count: 1,
            open_time: i as u64,
            close_time: i as u64,
        });
    }
    assert_eq!(store.len(), 3);
    // Oldest (i=0) should be evicted, newest should be i=1,2,3
    let first = store.iter().next().unwrap();
    assert_eq!(first.open, Decimal::from(1));
}
