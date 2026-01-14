# Runtime Market Discovery Design

**Date:** 2026-01-13
**Status:** Approved

## Overview

Enable periodic scanning for new markets while the bot is running, without requiring a restart.

### Current Behavior
- Discovery runs once at startup via `discover_all()` in `discovery.rs`
- Has a 2-hour cache TTL, but only checks on restart

### New Behavior
- A background task runs every N minutes (configurable, default 15)
- Uses `min_created_ts` filter from Kalshi API to fetch only new markets
- On new markets: reconnects WebSockets with updated subscription list
- On failure: logs details, keeps trading existing markets, preserves partial results

## Configuration

New environment variable:

```
DISCOVERY_INTERVAL_MINS=15   # Default: 15 minutes, set to 0 to disable
```

## Component Changes

| File | Change |
|------|--------|
| `discovery.rs` | Add `discover_since(timestamp)` method using `min_created_ts` filter |
| `main.rs` | Add discovery refresh task, reconnect logic |
| `config.rs` | Add `DISCOVERY_INTERVAL_MINS` env var |
| `kalshi.rs` | Add `reconnect_with_markets()` method |
| `polymarket.rs` | Add `reconnect_with_markets()` method |

### State Shared Between Tasks

- `Arc<RwLock<Vec<MarketPair>>>` - mutable market pairs list
- `Arc<AtomicU64>` - last discovery timestamp
- Channel or flag to signal "reconnect needed" to WebSocket handlers

## Discovery Refresh Logic

### New Method in `discovery.rs`

```rust
/// Discover markets created since a given timestamp
pub async fn discover_since(&self, since_ts: u64, leagues: &[&str]) -> DiscoveryResult
```

### Flow

1. Query Kalshi API with `min_created_ts={since_ts}` for each series
2. Filter out markets already in known tickers set
3. For new Kalshi markets, lookup corresponding Polymarket tokens via Gamma API
4. Return new `MarketPair` entries (may be empty if no new markets)

### API Call Optimization

- **Current:** Fetches all events, then all markets per event
- **New:** Single `GET /markets?series_ticker={series}&min_created_ts={ts}&status=open` per series
- Reduces from ~2 calls per event to 1 call per series

### Example API Call

```
GET /trade-api/v2/markets?series_ticker=KXEPLGAME&min_created_ts=1736780400&status=open&limit=100
```

### Rate Limiting

- Reuses existing `kalshi_limiter` (2 req/sec)
- ~43 series x 1 call = ~43 calls per refresh cycle
- At 2 req/sec = ~22 seconds for full scan

## Reconnect Flow

After discovery completes with new markets:

1. Discovery task finds N new markets
2. Log: "NEW MARKETS DISCOVERED"
3. For each new market, log details
4. Acquire write lock on market pairs, append new pairs
5. Update last_discovery_timestamp
6. Signal WebSocket handlers to reconnect
7. WebSocket handlers:
   - Close existing connections gracefully
   - Rebuild subscription list from updated market pairs
   - Reconnect with new subscriptions
   - Log reconnection status
8. Resume normal operation

### Logging Examples

**Success:**
```
[INFO] Running scheduled discovery (interval: 15m)...
[INFO] DISCOVERED 3 NEW MARKETS
[INFO]   -> epl | Arsenal vs Chelsea - Moneyline | KXEPLGAME-26JAN15ARSCHE-ARS
[INFO]   -> epl | Arsenal vs Chelsea - Moneyline | KXEPLGAME-26JAN15ARSCHE-CFC
[INFO]   -> epl | Arsenal vs Chelsea - Draw       | KXEPLGAME-26JAN15ARSCHE-TIE
[INFO] Reconnecting WebSockets with 156 total markets...
[INFO] Kalshi WebSocket reconnected
[INFO] Polymarket WebSocket reconnected
```

**Failure:**
```
[WARN] Discovery refresh failed for series KXNBAGAME: Connection timeout
[WARN]   Cause: reqwest::Error { kind: Timeout }
[INFO] Partial result: 2 new markets from other series added
[INFO] Continuing with 154 existing markets
```

## Error Handling & Edge Cases

| Scenario | Behavior |
|----------|----------|
| Discovery finds 0 new markets | Log "No new markets found", skip reconnect |
| Partial failure (some series fail) | Add successful discoveries, log failures, skip reconnect if 0 new |
| WebSocket reconnect fails | Retry with exponential backoff (5s, 10s, 20s), max 3 attempts |
| Discovery runs during active arb execution | Let execution complete, discovery doesn't block trading |
| Bot started with `DISCOVERY_INTERVAL_MINS=0` | Disable runtime discovery entirely, startup-only behavior |
| Kalshi rate limit hit | Existing `kalshi_limiter` handles this, discovery slows but continues |

## Thread Safety

- Market pairs list: `Arc<RwLock<Vec<MarketPair>>>`
- Heartbeat reads with read lock (non-blocking for other readers)
- Discovery writes with write lock (brief, only when adding new markets)
- WebSocket reconnect triggered via `tokio::sync::Notify` or channel

## Design Decisions

### Why Reconnect Instead of Dynamic Subscriptions?

Dynamic WebSocket subscription management would require:
- Tracking subscription state per connection
- Handling mid-stream subscription additions
- More complex error recovery

The reconnect approach is simpler with acceptable trade-off:
- Brief monitoring gap (~2-5 seconds) during reconnect
- Arb detection continues on stale prices until reconnect completes
- Single reconnect batched after full discovery completes

### Why 15 Minutes Default?

- New sports markets typically appear every 1-2 hours
- 15 minutes catches new markets quickly without excessive API calls
- ~43 API calls per cycle = manageable at 2 req/sec rate limit
- Configurable for users who want different behavior
