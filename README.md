# crypto-tui

A terminal-based crypto trading terminal built in Rust. Provides real-time candlestick charts (trade-count and time-based), order management, trade execution, position tracking, and account overview for AsterDEX perpetual futures. Designed to replace TradingView for pattern recognition and active trading without subscription fees.

## Features

**Charting**
- Trade-count candles (`--tick-size N`) for precise pattern recognition
- Time-based candles (`--resolution 1m/5m/15m/1h/...`) with REST bootstrap and live trade aggregation
- Volume histogram with toggle
- Cumulative Volume Delta (CVD) indicator
- Color themes (dark, high-contrast, light) with runtime switching
- Chart navigation via arrow keys

**Trading**
- Market, Limit, Stop-Market, Stop-Limit, and Trailing-Stop orders
- Leverage display and editing
- Quick-size hotkeys (F1-F4: 25%/50%/75%/100%)
- Confirmation overlay before submission
- Client-side exchange filter validation (lot size, price filter, min notional)
- Reduce-only support

**Positions**
- Real-time mark price and unrealized PnL
- Funding rates display
- Close, reverse, set stop-loss, and set take-profit via hotkeys

**Orders**
- Order history with real-time WebSocket updates
- Status and side filters
- Order cancellation with safety confirmation
- All-pairs watching mode

**Account**
- Margin ratio and available balance
- Daily PnL
- Long/short exposure breakdown
- Auto-refresh with WebSocket mark price

**Multi-Exchange**
- Binance US and Coinbase (charting only)
- AsterDEX (trading, orders, positions, account)

## Prerequisites

- Rust toolchain ([rustup](https://rustup.rs/) recommended)
- API credentials (see Environment Variables)

## Installation

```bash
git clone <repo-url>
cd crypto-tui
cargo build --release
```

The binary will be at `target/release/crypto-tui`.

## Environment Variables

| Variable | Required For | Description |
|----------|-------------|-------------|
| `ASTERDEX_API_KEY` | Trading, orders, positions, account | AsterDEX API key |
| `ASTERDEX_SECRET_KEY` | Trading, orders, positions, account | AsterDEX secret key |
| `COINBASE_API_KEY` | Coinbase charting | Coinbase CDP API key |
| `COINBASE_API_SECRET` | Coinbase charting | Coinbase CDP API secret (PEM format) |

Use a `.env` file in the project root (loaded via dotenvy) or export directly:

```bash
export ASTERDEX_API_KEY="your-key"
export ASTERDEX_SECRET_KEY="your-secret"
```

## Usage

### Chart (default command)

```bash
# Tick-based candles (100 trades per candle)
crypto-tui BTCUSDT --tick-size 100

# Time-based candles (5-minute)
crypto-tui BTCUSDT --resolution 5m

# Coinbase exchange
crypto-tui BTC-USD -e coinbase --resolution 1m

# Custom large trade threshold
crypto-tui BTCUSDT --tick-size 200 -l 50000
```

Either `--tick-size` or `--resolution` must be specified (mutually exclusive).

### Trade

```bash
crypto-tui trade BTCUSDT
crypto-tui trade ETHUSDT --no-orders
```

### Orders

```bash
crypto-tui orders BTCUSDT
crypto-tui orders --all
```

### Positions

```bash
crypto-tui positions BTCUSDT
crypto-tui positions --all
```

### Account

```bash
crypto-tui account
```

## Keyboard Shortcuts

### Chart View

| Key | Action |
|-----|--------|
| `q` | Quit |
| `+`/`-` | Increase/decrease tick size |
| `v` | Toggle volume histogram |
| `t` | Toggle trades widget |
| `p` | Cycle color theme |
| Arrow keys | Navigate chart |

### Trade View

| Key | Action |
|-----|--------|
| `Tab` | Cycle order type |
| `h`/`l` | Toggle Buy/Sell side |
| `F1`-`F4` | Quick size (25%/50%/75%/100%) |
| `Enter` | Submit order |
| `Esc` | Cancel/back |
| `o` | Toggle orders table |
| `L` (Shift+L) | Edit leverage |

### Orders View

| Key | Action |
|-----|--------|
| `j`/`k` or arrows | Navigate orders |
| `d` | Cancel selected order |
| `s` | Cycle status filter |
| `b` | Cycle side filter |

### Positions View

| Key | Action |
|-----|--------|
| `j`/`k` or arrows | Navigate positions |
| `c` | Close position |
| `r` | Reverse position |
| `s` | Set stop-loss |
| `t` | Set take-profit |

## Architecture

```
src/
  main.rs              -- Entry point, CLI dispatch, event loops per subcommand
  config.rs            -- CLI argument parsing (clap), configuration validation
  helpers.rs           -- Shared utility functions
  logging.rs           -- Structured logging setup (tracing + daily file rotation)
  lib.rs               -- Crate-level exports

  data/                -- Domain models and data structures
    candle.rs          -- Trade-count candle builder
    time_candle.rs     -- Time-based candle builder with REST reconciliation
    trade.rs           -- Trade type definitions
    order.rs           -- Order types, status, side enums
    position.rs        -- Position model with reconciliation logic
    account.rs         -- Account summary computation
    velocity.rs        -- Trade velocity indicator
    types.rs           -- Shared types (Trade, etc.)

  network/             -- API clients and WebSocket connections
    rest.rs            -- Binance US REST client (trades, klines, ticker)
    websocket.rs       -- Binance US WebSocket trade stream
    kline.rs           -- Kline/candlestick data fetching and parsing
    coinbase_auth.rs   -- Coinbase JWT authentication (ES256)
    coinbase_candles.rs -- Coinbase REST candle fetching
    coinbase_types.rs  -- Coinbase API type definitions
    coinbase_websocket.rs -- Coinbase WebSocket client
    asterdex_auth.rs   -- AsterDEX HMAC-SHA256 authentication
    asterdex_trading.rs -- Order placement and validation
    asterdex_orders.rs -- Order history fetching
    asterdex_positions.rs -- Position data fetching
    asterdex_account.rs -- Account info and balance
    asterdex_mark_price.rs -- Mark price stream
    asterdex_exchange_info.rs -- Exchange filters (lot size, price, notional)
    asterdex_user_stream.rs -- User data stream (listenKey management)
    asterdex_stream_types.rs -- WebSocket stream message types
    asterdex_trades.rs -- AsterDEX trade history
    cache.rs           -- Disk caching for kline data (1-hour TTL)
    heartbeat.rs       -- WebSocket heartbeat monitoring
    reconnect.rs       -- Auto-reconnection with exponential backoff
    types.rs           -- Shared network types

  tui/                 -- Terminal UI layer
    app.rs             -- Chart application state and rendering
    trade_app.rs       -- Trade ticket application (order form, confirmation)
    orders_app.rs      -- Order history application (table, filters, cancel)
    positions_app.rs   -- Position tracker (table, SL/TP/close/reverse)
    account_app.rs     -- Account dashboard application
    event.rs           -- Async event handler (keyboard + tick)
    terminal.rs        -- Terminal setup/teardown (crossterm + ratatui)
    text_input.rs      -- Text input widget for price/quantity fields
    theme.rs           -- Color theme definitions (dark, high-contrast, light)

    widgets/           -- Reusable UI components
      candlestick_chart.rs -- Candlestick + volume rendering
      status_bar.rs       -- Connection status, ticker, mode display
      trades_list.rs      -- Recent trades table
      trade_ticket.rs     -- Order form rendering
      order_table.rs      -- Order history table
      position_table.rs   -- Position table + management overlays
      account_view.rs     -- Account dashboard rendering

tests/
  candle_builder_test.rs -- Candle builder unit tests
```

## Tech Stack

- **UI:** ratatui 0.30 + crossterm 0.28
- **Async:** tokio (multi-threaded runtime)
- **HTTP:** reqwest (rustls TLS)
- **WebSocket:** tokio-tungstenite (rustls TLS)
- **Precision:** rust_decimal (no floating point for financial data)
- **Auth:** ring (HMAC-SHA256 for AsterDEX), jsonwebtoken (ES256 JWT for Coinbase)
- **Serialization:** serde + serde_json
- **CLI:** clap 4.5 (derive mode)
- **Logging:** tracing + tracing-appender (JSON, daily rotation)

## Development

```bash
# Run in development
cargo run -- BTCUSDT --tick-size 100

# Run tests
cargo test

# Run credential-dependent tests (single-threaded for env var isolation)
cargo test -- --test-threads=1

# Build release
cargo build --release
```
