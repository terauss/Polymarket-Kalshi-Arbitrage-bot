# Remote Trader (`remote-trader`)

This is an **optional** companion binary that connects to a host over WebSocket and executes trades on demand.

## Build

```bash
cargo build -p remote-trader --release
```

## Configure

The trader auto-loads `.env` from the **repo root** (one folder above `trader/`) or from the current directory.

### Required

- **`WEBSOCKET_URL`**: WebSocket URL for the host/controller to connect to (example: `ws://127.0.0.1:9000/ws`)

### Optional credentials (depending on which platforms you enable)

- **Kalshi**
  - `KALSHI_API_KEY`
  - `KALSHI_PRIVATE_KEY` (PEM contents, not a path)
- **Polymarket**
  - `POLYMARKET_PRIVATE_KEY`
  - `POLYMARKET_API_KEY`
  - `POLYMARKET_API_SECRET`
  - `POLYMARKET_FUNDER` (or `POLY_FUNDER`)

### Other

- `DRY_RUN` (`0`/`1`)

## Run

```bash
cargo run -p remote-trader --release
```

