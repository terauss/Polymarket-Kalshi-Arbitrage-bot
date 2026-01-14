# Polymarket-Kalshi Arbitrage Bot

An arbitrage system for cross-platform prediction market trading between Kalshi and Polymarket.

> 🎯 **Perfect for Beginners!** This bot is designed specifically for **people who don't know how to code**. Even if you've never written a single line of code, you can use this bot with our comprehensive step-by-step guides. No programming experience required!

---

---

<div align="center">

## 📚 **IMPORTANT: Please Refer to the Complete Documentation**

**⚠️ CRITICAL: Before starting, please refer to our comprehensive documentation in the [`doc/`](./doc/) folder for detailed step-by-step guides, troubleshooting, and complete setup instructions.**

![Documentation Guide](./documentation-preview.png)

**[👉 Click here to start with the Getting Started Guide](./doc/01-getting-started.md)** | **[📄 Download Complete PDF Guide](./doc/Polymarket-Kalshi-Arbitrage-Bot-User-Guide.pdf)**

*All guides are designed for beginners with no coding experience - everything is explained step-by-step!*

</div>

---

> 🔍 **What is this?** This bot automatically monitors prices on both platforms and executes trades when it finds opportunities where you can buy both YES and NO for less than $1.00, guaranteeing a profit when the market resolves.

> 🚀 **What's Coming Next:** I'm developing other innovative arbitrage bots with revolutionary strategies. Stay tuned for more advanced trading systems!

---

## Quick Start

### 1. Install Dependencies

```bash
# Rust 1.75+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Navigate to project directory
cd prediction-market-arbitrage  # or your project directory name

# Build
cargo build --release
```

📖 **Detailed installation guide:** [Installation Guide](./doc/02-installation.md)

### 2. Set Up Credentials

Create a `.env` file:

```bash
# === KALSHI CREDENTIALS ===
KALSHI_API_KEY_ID=your_kalshi_api_key_id
KALSHI_PRIVATE_KEY_PATH=/path/to/kalshi_private_key.pem

# === POLYMARKET CREDENTIALS ===
POLY_PRIVATE_KEY=0xYOUR_WALLET_PRIVATE_KEY
POLY_FUNDER=0xYOUR_WALLET_ADDRESS

# === BOT CONFIGURATION ===
DRY_RUN=1
RUST_LOG=info
```

📖 **Complete credentials setup guide:** [Getting Your Credentials](./doc/03-credentials.md) | [Configuration Setup](./doc/04-configuration.md)

### 3. Run

```bash
# Dry run (paper trading)
dotenvx run -- cargo run --release

# Live execution
DRY_RUN=0 dotenvx run -- cargo run --release
```

📖 **Running the bot guide:** [Running the Bot](./doc/05-running-the-bot.md)

---

## 📚 Documentation

> ⚠️ **CRITICAL: Before Starting - Read the Documentation!**
> 
> **This README provides a quick overview. For complete setup instructions, troubleshooting, and detailed explanations, you MUST refer to the comprehensive documentation in the [`doc/`](./doc/) folder. All guides are designed for beginners with no coding experience.**

**Follow these comprehensive guides in order:**

1. **[📖 Getting Started Guide](./doc/01-getting-started.md)** - Overview and introduction - **START HERE!**
2. **[🔧 Installation Guide](./doc/02-installation.md)** - Install Rust and dependencies (Windows/Mac/Linux)
3. **[🔑 Getting Your Credentials](./doc/03-credentials.md)** - Get API keys from Kalshi and Polymarket
4. **[⚙️ Configuration Setup](./doc/04-configuration.md)** - Complete guide to all configuration options
5. **[▶️ Running the Bot](./doc/05-running-the-bot.md)** - Start and monitor your bot
6. **[🔧 Troubleshooting](./doc/06-troubleshooting.md)** - Common problems and solutions

📄 **PDF Version:** A complete PDF guide combining all documentation: **[📥 Download Polymarket-Kalshi-Arbitrage-Bot-User-Guide.pdf](./doc/Polymarket-Kalshi-Arbitrage-Bot-User-Guide.pdf)**

**Why refer to the documentation?**
- ✅ Step-by-step instructions for every step
- ✅ Screenshots and visual guides
- ✅ Troubleshooting for common issues
- ✅ Configuration explanations
- ✅ Safety warnings and best practices
- ✅ Written specifically for non-technical users

---

## Environment Variables

### Required

| Variable | Description |
|----------|-------------|
| `KALSHI_API_KEY_ID` | Your Kalshi API key ID |
| `KALSHI_PRIVATE_KEY_PATH` | Path to RSA private key (PEM format) for Kalshi API signing |
| `POLY_PRIVATE_KEY` | Ethereum private key (with 0x prefix) for Polymarket wallet |
| `POLY_FUNDER` | Your Polymarket wallet address (with 0x prefix) |

### Bot Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DRY_RUN` | `1` | `1` = paper trading (no orders), `0` = live execution |
| `RUST_LOG` | `info` | Log level: `error`, `warn`, `info`, `debug`, `trace` |
| `FORCE_DISCOVERY` | `0` | `1` = re-fetch market mappings (ignore cache) |
| `DISCOVERY_INTERVAL_MINS` | `15` | Minutes between runtime discovery scans (0 = disabled) |
| `PRICE_LOGGING` | `0` | `1` = verbose price update logging |

### Test Mode

| Variable | Default | Description |
|----------|---------|-------------|
| `TEST_ARB` | `0` | `1` = inject synthetic arb opportunity for testing |
| `TEST_ARB_TYPE` | `poly_yes_kalshi_no` | Arb type: `poly_yes_kalshi_no`, `kalshi_yes_poly_no`, `poly_same_market`, `kalshi_same_market` |

### Circuit Breaker

| Variable | Default | Description |
|----------|---------|-------------|
| `CB_ENABLED` | `true` | Enable/disable circuit breaker |
| `CB_MAX_POSITION_PER_MARKET` | `100` | Max contracts per market |
| `CB_MAX_TOTAL_POSITION` | `500` | Max total contracts across all markets |
| `CB_MAX_DAILY_LOSS` | `5000` | Max daily loss in cents before halt |
| `CB_MAX_CONSECUTIVE_ERRORS` | `5` | Consecutive errors before halt |
| `CB_COOLDOWN_SECS` | `60` | Cooldown period after circuit breaker trips |

📖 **Detailed configuration guide:** [Configuration Setup](./doc/04-configuration.md)

---

## Obtaining Credentials

### Kalshi

1. Log in to [Kalshi](https://kalshi.com)
2. Go to **Settings → API Keys**
3. Create a new API key with trading permissions
4. Download the private key (PEM file)
5. Note the API Key ID

### Polymarket

1. Create or import an Ethereum wallet (MetaMask, etc.)
2. Export the private key (include `0x` prefix)
3. Fund your wallet on Polygon network with USDC
4. The wallet address is your `POLY_FUNDER`

📖 **Step-by-step credentials guide:** [Getting Your Credentials](./doc/03-credentials.md)

---

## Usage Examples

### Paper Trading (Development)

```bash
# Full logging, dry run
RUST_LOG=debug DRY_RUN=1 dotenvx run -- cargo run --release
```

### Test Arbitrage Execution

```bash
# Inject synthetic arb to test execution path
TEST_ARB=1 DRY_RUN=0 dotenvx run -- cargo run --release
```

### Production

```bash
# Live trading with circuit breaker
DRY_RUN=0 CB_MAX_DAILY_LOSS=10000 dotenvx run -- cargo run --release
```

### Force Market Re-Discovery

```bash
# Clear cache and re-fetch all market mappings
FORCE_DISCOVERY=1 dotenvx run -- cargo run --release
```

---

## How It Works

### Arbitrage Mechanics

In prediction markets, **YES + NO = $1.00** guaranteed.

Arbitrage exists when:

```
Best YES ask (platform A) + Best NO ask (platform B) < $1.00
```

**Example:**

```
Kalshi YES ask:  42¢
Poly NO ask:     56¢
Total cost:      98¢
Guaranteed:     100¢
Profit:           2¢ per contract
```

### Four Arbitrage Types

| Type | Buy | Sell |
|------|-----|------|
| `poly_yes_kalshi_no` | Polymarket YES | Kalshi NO |
| `kalshi_yes_poly_no` | Kalshi YES | Polymarket NO |
| `poly_same_market` | Polymarket YES + NO | (rare) |
| `kalshi_same_market` | Kalshi YES + NO | (rare) |

### Fee Handling

- **Kalshi:** `ceil(0.07 × contracts × price × (1-price))` - factored into arb detection
- **Polymarket:** Zero trading fees

---

## Architecture

```
src/
├── main.rs              # Entry point, WebSocket orchestration
├── types.rs             # MarketArbState
├── execution.rs         # Concurrent leg execution, in-flight deduplication
├── position_tracker.rs  # Channel-based fill recording, P&L tracking
├── circuit_breaker.rs   # Risk limits, error tracking, auto-halt
├── discovery.rs         # Kalshi↔Polymarket market matching
├── cache.rs             # Team code mappings (EPL, NBA, etc.)
├── kalshi.rs            # Kalshi REST/WS client
├── polymarket.rs        # Polymarket WS client
├── polymarket_clob.rs   # Polymarket CLOB order execution
└── config.rs            # League configs, thresholds
```

### Key Features

- ✅ Lock-free orderbook cache using atomic operations
- ✅ SIMD-accelerated arbitrage detection for sub-millisecond latency
- ✅ Concurrent order execution with automatic position reconciliation
- ✅ Circuit breaker protection with configurable risk limits
- ✅ Intelligent market discovery with caching and incremental updates

---

## Development

### Run Tests

```bash
cargo test
```

### Enable Profiling

```bash
cargo build --release --features profiling
```

### Benchmarks

```bash
cargo bench
```

---

## Project Status

### ✅ Completed Features

- ✅ Kalshi REST/WebSocket client
- ✅ Polymarket REST/WebSocket client
- ✅ Lock-free orderbook cache
- ✅ SIMD arb detection
- ✅ Concurrent order execution
- ✅ Position & P&L tracking
- ✅ Circuit breaker
- ✅ Market discovery & caching
- ✅ Beginner-friendly documentation and guides

### 🚧 Future Enhancements

- [x] **Runtime market discovery** - Periodic scanning for new markets without restart
  - Configurable via `DISCOVERY_INTERVAL_MINS` (default: 15 minutes, 0 to disable)
  - See `docs/plans/2026-01-13-runtime-market-discovery-design.md` for design details
- [ ] Risk limit configuration UI
- [ ] Multi-account support
- [ ] Advanced order routing strategies
- [ ] Historical performance analytics dashboard

### 🚀 Coming Soon

I'm actively developing other innovative arbitrage bots with revolutionary strategies. These will feature advanced trading algorithms and cutting-edge market analysis techniques. Stay updated by following this repository or contacting me on Telegram [@terauss](https://t.me/terauss)!

---

## Supported Markets

The bot supports multiple sports leagues:

- **Soccer:** EPL, Bundesliga, La Liga, Serie A, Ligue 1, UCL, UEL, EFL Championship
- **Basketball:** NBA
- **Football:** NFL
- **Hockey:** NHL
- **Baseball:** MLB, MLS
- **College Football:** NCAAF

---

## Troubleshooting

Having problems? Check the **[Troubleshooting Guide](./doc/06-troubleshooting.md)** for:

- Installation issues
- Credential problems
- Runtime errors
- Connection issues
- Performance problems

Common issues:
- **"cargo: command not found"** → [Installation Guide](./doc/02-installation.md)
- **"KALSHI_API_KEY_ID not set"** → [Configuration Guide](./doc/04-configuration.md)
- **"No market pairs found"** → [Troubleshooting Guide](./doc/06-troubleshooting.md)
- **Bot won't execute trades** → Check `DRY_RUN` setting and circuit breaker limits

---

## Safety & Warnings

⚠️ **Important Safety Notes:**

- **Always start with `DRY_RUN=1`** - Test mode lets you verify everything works without risking real money
- **Start with small amounts** - Even when going live, use small position sizes initially
- **Monitor your bot** - Check on it regularly, especially when starting
- **Keep credentials secret** - Never share your API keys or private keys
- **This is not financial advice** - Trade at your own risk

---

## About This Project

This bot was created with beginners in mind. You don't need to know how to code to use it - just follow the guides in the `doc/` folder. Whether you're a complete beginner or an experienced trader, this bot makes arbitrage trading accessible to everyone.

**Upcoming Projects:** I'm working on other arbitrage bots with revolutionary strategies and advanced features. This beginner-friendly bot is just the first in a series of innovative trading systems I'm developing.

## Contributing

Contributions are welcome! This project is open source and designed to help the prediction market trading community, especially those new to automated trading.

---

## Support & Contact

💬 **Need help?** Contact me on Telegram: [@terauss](https://t.me/terauss)

📚 **Documentation:** Check the [documentation folder](./doc/) for detailed guides

🐛 **Issues:** Report bugs or request features on GitHub

---

## License

This project is licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

---

## Related Projects & Keywords

**Keywords:** polymarket arbitrage bot, polymarket-kalshi arbitrage bot, kalshi-poly arbitrage, poly-poly arbitrage, kalshi-kalshi arbitrage, kalshi arbitrage, prediction market arbitrage, cross-platform trading bot, automated trading, sports betting arbitrage, Rust trading bot

---

**Ready to start?** Follow the guides in order:
1. [Getting Started](./doc/01-getting-started.md) → 2. [Installation](./doc/02-installation.md) → 3. [Credentials](./doc/03-credentials.md) → 4. [Configuration](./doc/04-configuration.md) → 5. [Running the Bot](./doc/05-running-the-bot.md)
