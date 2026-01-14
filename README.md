# Polymarket-Kalshi Arbitrage Bot

This repo is a **Rust Cargo workspace** containing:

- **`controller/`**: the main arbitrage bot (market discovery + websockets + execution)
- **`trader/`**: an optional **remote trader** client (`remote-trader`) that can receive execution instructions over WebSocket

## Getting Started (Controller)

- **Full beginner docs** live in `controller/doc/` (start here):
  - [`controller/doc/01-getting-started.md`](controller/doc/01-getting-started.md)

### Build

```bash
cargo build -p controller --release
```

### Configure

Create your `.env` in the **repo root** (this folder). The controller will auto-load it.

Key vars:
- **Kalshi**: `KALSHI_API_KEY_ID`, `KALSHI_PRIVATE_KEY_PATH`
- **Polymarket**: `POLY_PRIVATE_KEY`, `POLY_FUNDER`

### Run

```bash
# Dry run (paper trading)
cargo run -p controller --release

# Live execution
DRY_RUN=0 cargo run -p controller --release
```

### Quick Smoke Test (Discovery Only)

```bash
DISCOVERY_ONLY=1 FORCE_DISCOVERY=1 cargo run -p controller --release
```

## Remote Trader (Optional)

If you use the remote trader:

```bash
cargo run -p remote-trader --release
```

See `trader/README.md` for required environment variables.

