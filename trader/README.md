## Remote Trader

This binary is designed to run on a separate machine from the controller.

### Configure controller IP + port (where to connect)

Set `WEBSOCKET_URL` to the controller’s reachable address:

- **Same machine**: `ws://127.0.0.1:9001`
- **Two machines (LAN/WAN)**: `ws://<CONTROLLER_IP>:9001`

### Environment variables

| Variable | Required | Example | Description |
|----------|----------|---------|-------------|
| `WEBSOCKET_URL` | Yes | `ws://192.168.1.10:9001` | Where to connect (controller host + port). |
| `DRY_RUN` | No | `1` | If `1`, the trader will **log the trades it would place** but will not execute real orders. |
| `ONE_SHOT` | No | `1` | If `1`, exit after receiving and handling the first `execute` message (useful for smoke tests). |
| `RUST_LOG` | No | `info` | Logging verbosity. |

### Run examples

**Trader (client) → connect to controller:**

```bash
DRY_RUN=1 WEBSOCKET_URL=ws://<CONTROLLER_IP>:9001 RUST_LOG=info cargo run -p remote-trader --release
```

**Smoke test mode (exit after first execute):**

```bash
DRY_RUN=1 ONE_SHOT=1 WEBSOCKET_URL=ws://127.0.0.1:9001 RUST_LOG=info cargo run -p remote-trader
```

