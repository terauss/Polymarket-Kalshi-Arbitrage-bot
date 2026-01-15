# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

```bash
# Build release
cargo build --release

# Run with environment variables (dry run by default)
dotenvx run -- cargo run --release

# Run live trading
DRY_RUN=0 dotenvx run -- cargo run --release

# Run tests
cargo test

# Run benchmarks
cargo bench

# Force market re-discovery (clears cache)
FORCE_DISCOVERY=1 dotenvx run -- cargo run --release

# Test mode with synthetic arbitrage
TEST_ARB=1 TEST_ARB_TYPE=poly_yes_kalshi_no dotenvx run -- cargo run --release
```

## Architecture Overview

This is a Rust arbitrage bot that monitors price discrepancies between Kalshi and Polymarket prediction markets. The core principle: in prediction markets, YES + NO = $1.00. Arbitrage exists when buying YES on one platform and NO on another costs less than $1.00.

### Data Flow

```
WebSocket Feeds (kalshi.rs, polymarket.rs)
    ↓
Global State with Lock-Free Orderbook Cache (types.rs)
    ↓
Heartbeat Arbitrage Detection (main.rs, every 60s)
    ↓
Execution Loop (execution.rs)
    ↓
Platform Orders (kalshi.rs, polymarket_clob.rs)
    ↓
Position Tracking (position_tracker.rs)
```

### Key Modules

- **`main.rs`** - Entry point, WebSocket orchestration, heartbeat-based arb detection
- **`types.rs`** - Core data structures including `AtomicOrderbook` (lock-free using packed u64 with CAS loops)
- **`execution.rs`** - Concurrent order execution with in-flight deduplication (8-slot bitmask for 512 markets)
- **`kalshi.rs`** - Kalshi REST/WebSocket client with RSA signature authentication
- **`polymarket.rs`** - Polymarket WebSocket client and Gamma API integration
- **`polymarket_clob.rs`** - Polymarket CLOB execution with EIP-712 and HMAC-SHA256 signing
- **`discovery.rs`** - Cross-platform market matching with persistent caching (2-hour TTL)
- **`circuit_breaker.rs`** - Risk management: position limits, daily loss limits, error tracking, cooldown
- **`position_tracker.rs`** - Fill recording, P&L calculation, state persistence to `positions.json`
- **`cache.rs`** - Team code bidirectional mapping between platforms
- **`config.rs`** - League definitions, API endpoints, thresholds (ARB_THRESHOLD = 0.995)

### Arbitrage Types

| Type | Description |
|------|-------------|
| `poly_yes_kalshi_no` | Buy Polymarket YES + Kalshi NO |
| `kalshi_yes_poly_no` | Buy Kalshi YES + Polymarket NO |
| `poly_only` | Both sides on Polymarket (rare) |
| `kalshi_only` | Both sides on Kalshi (rare) |

### Lock-Free Orderbook Design

`AtomicOrderbook` uses a packed u64 format for cache-line efficiency:
- Bit layout: `[yes_ask:16][no_ask:16][yes_size:16][no_size:16]`
- Updates via compare-and-swap loops
- 64-byte aligned for SIMD compatibility

### Fee Calculation

- **Kalshi**: `ceil(0.07 × contracts × price × (1-price))` - factored into arb detection
- **Polymarket**: Zero trading fees

## Environment Variables

**Required credentials:**
- `KALSHI_API_KEY_ID`, `KALSHI_PRIVATE_KEY_PATH` (PEM file)
- `POLY_PRIVATE_KEY` (0x-prefixed), `POLY_FUNDER` (wallet address)

**Execution:** `DRY_RUN` (default: 1), `RUST_LOG` (default: info)

**Circuit breaker:** `CB_ENABLED`, `CB_MAX_POSITION_PER_MARKET`, `CB_MAX_TOTAL_POSITION`, `CB_MAX_DAILY_LOSS`, `CB_MAX_CONSECUTIVE_ERRORS`, `CB_COOLDOWN_SECS`

## Tailscale Setup (Remote Trading)

For running controller and trader on separate machines:

```bash
# First-time setup (run on each machine)
cargo run -p bootstrap

# This will:
# 1. Verify Tailscale is installed and connected
# 2. Prompt for role (controller/trader)
# 3. Write config to ~/.arb/config.toml
# 4. Optionally launch the appropriate binary
```

**Manual setup alternative:**

```bash
# Install Tailscale
brew install tailscale

# Start daemon and connect
sudo tailscaled
tailscale up

# Verify connection
tailscale status
```

**Configuration file:** `~/.arb/config.toml`

```toml
role = "controller"  # or "trader"
beacon_port = 9000   # UDP port for discovery
ws_port = 9001       # WebSocket port
```

**How it works:**
- Controller sends UDP beacon to all Tailscale peers every 2 seconds
- Trader listens for beacon and auto-discovers controller IP/port
- No manual IP configuration required
- Falls back to `WEBSOCKET_URL` env var if set

## Rate Limits

- Kalshi: 2 requests/second (60ms delay between requests)
- Polymarket Gamma API: 20 concurrent requests via semaphore
- WebSocket ping intervals: 30 seconds (Polymarket), heartbeat every 60 seconds

## Supported Markets

Soccer (EPL, Bundesliga, La Liga, Serie A, Ligue 1, UCL, UEL, EFL Championship), NBA, NFL, NHL, MLB, MLS, NCAAF
