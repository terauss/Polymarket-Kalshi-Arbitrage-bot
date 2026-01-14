# Runtime Market Discovery Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable periodic scanning for new markets while the bot is running, triggering WebSocket reconnection when new markets are discovered.

**Architecture:** Background tokio task polls Kalshi API using `min_created_ts` filter every N minutes. When new markets are found, it signals the main loop to rebuild state and reconnect WebSockets. Uses a shutdown channel pattern for graceful WebSocket teardown.

**Tech Stack:** Rust, tokio (async runtime), tokio::sync (channels, Notify)

---

## Task 1: Add Configuration

**Files:**
- Modify: `src/config.rs`

**Step 1: Write the test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_interval_default() {
        // Clear any existing env var
        std::env::remove_var("DISCOVERY_INTERVAL_MINS");
        assert_eq!(discovery_interval_mins(), 15);
    }

    #[test]
    fn test_discovery_interval_custom() {
        std::env::set_var("DISCOVERY_INTERVAL_MINS", "30");
        assert_eq!(discovery_interval_mins(), 30);
        std::env::remove_var("DISCOVERY_INTERVAL_MINS");
    }

    #[test]
    fn test_discovery_interval_zero_disables() {
        std::env::set_var("DISCOVERY_INTERVAL_MINS", "0");
        assert_eq!(discovery_interval_mins(), 0);
        std::env::remove_var("DISCOVERY_INTERVAL_MINS");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib config::tests::test_discovery_interval`
Expected: FAIL with "cannot find function `discovery_interval_mins`"

**Step 3: Write minimal implementation**

Add to `src/config.rs`:

```rust
/// Discovery refresh interval in minutes (default: 15, 0 = disabled)
pub fn discovery_interval_mins() -> u64 {
    std::env::var("DISCOVERY_INTERVAL_MINS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib config::tests::test_discovery_interval`
Expected: PASS

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add DISCOVERY_INTERVAL_MINS setting

Default 15 minutes, set to 0 to disable runtime discovery."
```

---

## Task 2: Add `discover_since` Method to DiscoveryClient

**Files:**
- Modify: `src/discovery.rs`
- Modify: `src/kalshi.rs` (add `get_markets_since` method)

**Step 1: Add Kalshi API method for timestamp-filtered markets**

Add to `src/kalshi.rs`:

```rust
/// Get markets created since a given timestamp for a series
pub async fn get_markets_since(&self, series_ticker: &str, since_ts: u64) -> Result<Vec<KalshiMarket>> {
    let path = format!(
        "/markets?series_ticker={}&min_created_ts={}&status=open&limit=100",
        series_ticker, since_ts
    );
    let resp: KalshiMarketsResponse = self.get(&path).await?;
    Ok(resp.markets)
}
```

**Step 2: Run build to verify it compiles**

Run: `cargo build --lib`
Expected: PASS

**Step 3: Commit Kalshi method**

```bash
git add src/kalshi.rs
git commit -m "feat(kalshi): add get_markets_since for timestamp-filtered queries"
```

**Step 4: Add discover_since method to DiscoveryClient**

Add to `src/discovery.rs`:

```rust
/// Discover new markets created since a given timestamp
/// Returns only NEW pairs not already in the known_tickers set
pub async fn discover_since(
    &self,
    since_ts: u64,
    known_tickers: &std::collections::HashSet<String>,
    leagues: &[&str],
) -> DiscoveryResult {
    let configs: Vec<_> = if leagues.is_empty() {
        get_league_configs()
    } else {
        leagues.iter()
            .filter_map(|l| get_league_config(l))
            .collect()
    };

    let mut result = DiscoveryResult::default();

    for config in &configs {
        match self.discover_series_since(config, since_ts, known_tickers).await {
            Ok(pairs) => {
                if !pairs.is_empty() {
                    tracing::info!("  {} {}: {} new pairs",
                        config.league_code, "discovery", pairs.len());
                }
                result.pairs.extend(pairs);
            }
            Err(e) => {
                result.errors.push(format!("{}: {}", config.league_code, e));
            }
        }
    }

    result.kalshi_events_found = result.pairs.len();
    result.poly_matches = result.pairs.len();
    result
}

/// Discover new markets for a single league since timestamp
async fn discover_series_since(
    &self,
    config: &LeagueConfig,
    since_ts: u64,
    known_tickers: &std::collections::HashSet<String>,
) -> Result<Vec<MarketPair>> {
    let mut all_pairs = Vec::new();

    // Check all series for this league
    let series_list: Vec<&str> = [
        Some(config.kalshi_series_game),
        config.kalshi_series_spread,
        config.kalshi_series_total,
        config.kalshi_series_btts,
    ].into_iter().flatten().collect();

    for series in series_list {
        // Rate limit
        {
            let _permit = self.kalshi_semaphore.acquire().await
                .map_err(|e| anyhow::anyhow!("semaphore closed: {}", e))?;
            self.kalshi_limiter.until_ready().await;
        }

        let markets = match self.kalshi.get_markets_since(series, since_ts).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("  Failed to query {}: {}", series, e);
                continue;
            }
        };

        // Filter to only new markets
        let new_markets: Vec<_> = markets.into_iter()
            .filter(|m| !known_tickers.contains(&m.ticker))
            .collect();

        if new_markets.is_empty() {
            continue;
        }

        // Look up on Polymarket (reuse existing logic pattern)
        for market in new_markets {
            // Parse event from market ticker to build poly slug
            if let Some(pair) = self.try_match_market(config, &market).await {
                all_pairs.push(pair);
            }
        }
    }

    Ok(all_pairs)
}

/// Try to match a single Kalshi market to Polymarket
async fn try_match_market(&self, config: &LeagueConfig, market: &KalshiMarket) -> Option<MarketPair> {
    // Extract event ticker from market ticker (format: SERIES-EVENTID-SUFFIX)
    let parts: Vec<&str> = market.ticker.split('-').collect();
    if parts.len() < 2 {
        return None;
    }

    // Reconstruct event ticker (SERIES-EVENTID)
    let event_ticker = format!("{}-{}", parts[0], parts[1]);

    // Determine market type from series
    let market_type = if market.ticker.contains("SPREAD") {
        MarketType::Spread
    } else if market.ticker.contains("TOTAL") {
        MarketType::Total
    } else if market.ticker.contains("BTTS") {
        MarketType::Btts
    } else {
        MarketType::Moneyline
    };

    // Parse event ticker to get teams and date
    let parsed = parse_kalshi_event_ticker(&event_ticker)?;

    // Build poly slug
    let poly_slug = self.build_poly_slug(config.poly_prefix, &parsed, market_type, market);

    // Look up on Polymarket
    let _permit = self.gamma_semaphore.acquire().await.ok()?;
    let (yes_token, no_token) = self.gamma.lookup_market(&poly_slug).await.ok()??;

    let team_suffix = extract_team_suffix(&market.ticker);

    Some(MarketPair {
        pair_id: format!("{}-{}", poly_slug, market.ticker).into(),
        league: config.league_code.into(),
        market_type,
        description: format!("{}", market.title).into(),
        kalshi_event_ticker: event_ticker.into(),
        kalshi_market_ticker: market.ticker.clone().into(),
        poly_slug: poly_slug.into(),
        poly_yes_token: yes_token.into(),
        poly_no_token: no_token.into(),
        line_value: market.floor_strike,
        team_suffix: team_suffix.map(|s| s.into()),
    })
}
```

**Step 5: Run build to verify it compiles**

Run: `cargo build --lib`
Expected: PASS

**Step 6: Commit**

```bash
git add src/discovery.rs
git commit -m "feat(discovery): add discover_since for incremental market discovery

Uses min_created_ts filter to only query new markets."
```

---

## Task 3: Add Shutdown Signal Support to WebSocket Handlers

**Files:**
- Modify: `src/kalshi.rs`
- Modify: `src/polymarket.rs`

**Step 1: Modify Kalshi WebSocket to accept shutdown signal**

Update `run_ws` signature in `src/kalshi.rs`:

```rust
use tokio::sync::watch;

/// WebSocket runner with shutdown support
pub async fn run_ws(
    config: &KalshiConfig,
    state: Arc<GlobalState>,
    exec_tx: mpsc::Sender<FastExecutionRequest>,
    threshold_cents: PriceCents,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    // ... existing setup code ...

    loop {
        tokio::select! {
            biased;

            // Check shutdown signal first
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("[KALSHI] Shutdown signal received, disconnecting...");
                    break;
                }
            }

            msg = read.next() => {
                // ... existing message handling ...
            }
        }
    }

    Ok(())
}
```

**Step 2: Modify Polymarket WebSocket similarly**

Update `run_ws` in `src/polymarket.rs`:

```rust
use tokio::sync::watch;

/// WebSocket runner with shutdown support
pub async fn run_ws(
    state: Arc<GlobalState>,
    exec_tx: mpsc::Sender<FastExecutionRequest>,
    threshold_cents: PriceCents,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    // ... existing setup code ...

    loop {
        tokio::select! {
            biased;

            // Check shutdown signal first
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("[POLY] Shutdown signal received, disconnecting...");
                    break;
                }
            }

            _ = ping_interval.tick() => {
                // ... existing ping code ...
            }

            msg = read.next() => {
                // ... existing message handling ...
            }
        }
    }

    Ok(())
}
```

**Step 3: Run build to verify it compiles**

Run: `cargo build --lib`
Expected: PASS

**Step 4: Commit**

```bash
git add src/kalshi.rs src/polymarket.rs
git commit -m "feat(websocket): add shutdown signal support for graceful reconnect

WebSocket handlers now accept a watch channel to trigger graceful shutdown."
```

---

## Task 4: Implement Discovery Refresh Task in Main

**Files:**
- Modify: `src/main.rs`

**Step 1: Add imports and state structures**

Add at top of `src/main.rs`:

```rust
use std::collections::HashSet;
use tokio::sync::watch;
```

**Step 2: Create discovery refresh task function**

Add new function in `src/main.rs`:

```rust
/// Background task that periodically discovers new markets
async fn discovery_refresh_task(
    discovery: Arc<DiscoveryClient>,
    state: Arc<GlobalState>,
    shutdown_tx: watch::Sender<bool>,
    interval_mins: u64,
    leagues: &'static [&'static str],
) {
    use std::time::{SystemTime, UNIX_EPOCH};

    if interval_mins == 0 {
        info!("[DISCOVERY] Runtime discovery disabled (DISCOVERY_INTERVAL_MINS=0)");
        return;
    }

    let mut interval = tokio::time::interval(Duration::from_secs(interval_mins * 60));
    interval.tick().await; // Skip immediate first tick

    // Track last discovery timestamp
    let mut last_discovery_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    loop {
        interval.tick().await;

        info!("[DISCOVERY] Running scheduled discovery (interval: {}m)...", interval_mins);

        // Build set of known tickers
        let known_tickers: HashSet<String> = state.markets.iter()
            .take(state.market_count())
            .filter_map(|m| m.pair.as_ref())
            .map(|p| p.kalshi_market_ticker.to_string())
            .collect();

        // Discover new markets since last check
        let result = discovery.discover_since(last_discovery_ts, &known_tickers, leagues).await;

        // Update timestamp for next iteration
        last_discovery_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Log errors but continue
        for err in &result.errors {
            warn!("[DISCOVERY] {}", err);
        }

        if result.pairs.is_empty() {
            info!("[DISCOVERY] No new markets found");
            continue;
        }

        // Log new markets with highlighted formatting
        info!("[DISCOVERY] NEW MARKETS DISCOVERED: {}", result.pairs.len());
        for pair in &result.pairs {
            info!("[DISCOVERY]   -> {} | {} | {}",
                pair.league, pair.description, pair.kalshi_market_ticker);
        }

        // Signal WebSockets to reconnect
        // NOTE: In a full implementation, we'd need to:
        // 1. Store new pairs temporarily
        // 2. Signal shutdown
        // 3. Rebuild state with new pairs
        // 4. Restart WebSocket handlers
        //
        // For now, log that reconnect is needed
        info!("[DISCOVERY] Signaling WebSocket reconnect for {} new markets...", result.pairs.len());
        let _ = shutdown_tx.send(true);

        // Give WebSockets time to shut down gracefully
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Reset shutdown signal for next cycle
        let _ = shutdown_tx.send(false);
    }
}
```

**Step 3: Wire up in main function**

Modify `main()` to:

1. Create shutdown channel
2. Pass to WebSocket handlers
3. Spawn discovery refresh task

Key changes to `main()`:

```rust
// Create shutdown channel for WebSocket reconnection
let (shutdown_tx, shutdown_rx) = watch::channel(false);

// ... existing setup ...

// Modify Kalshi WebSocket spawn to include shutdown_rx
let kalshi_shutdown_rx = shutdown_rx.clone();
let kalshi_handle = tokio::spawn(async move {
    loop {
        let shutdown_rx = kalshi_shutdown_rx.clone();
        if let Err(e) = kalshi::run_ws(&kalshi_ws_config, kalshi_state.clone(), kalshi_exec_tx.clone(), kalshi_threshold, shutdown_rx).await {
            error!("[KALSHI] WebSocket disconnected: {} - reconnecting...", e);
        }
        tokio::time::sleep(Duration::from_secs(WS_RECONNECT_DELAY_SECS)).await;
    }
});

// Modify Polymarket WebSocket spawn similarly
let poly_shutdown_rx = shutdown_rx.clone();
let poly_handle = tokio::spawn(async move {
    loop {
        let shutdown_rx = poly_shutdown_rx.clone();
        if let Err(e) = polymarket::run_ws(poly_state.clone(), poly_exec_tx.clone(), poly_threshold, shutdown_rx).await {
            error!("[POLYMARKET] WebSocket disconnected: {} - reconnecting...", e);
        }
        tokio::time::sleep(Duration::from_secs(WS_RECONNECT_DELAY_SECS)).await;
    }
});

// Spawn discovery refresh task
let discovery_interval = config::discovery_interval_mins();
let discovery_client = Arc::new(discovery);
let discovery_state = state.clone();
let discovery_handle = tokio::spawn(async move {
    discovery_refresh_task(
        discovery_client,
        discovery_state,
        shutdown_tx,
        discovery_interval,
        ENABLED_LEAGUES,
    ).await;
});
```

**Step 4: Run build and basic test**

Run: `cargo build --release`
Expected: PASS

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): add discovery refresh background task

Periodically scans for new markets and signals WebSocket reconnection.
Configurable via DISCOVERY_INTERVAL_MINS (default 15, 0 to disable)."
```

---

## Task 5: Full Integration - State Rebuild on New Markets

**Files:**
- Modify: `src/main.rs`

This task implements the actual state rebuild when new markets are discovered. The approach:

1. Discovery task stores new pairs in a shared `Arc<RwLock<Vec<MarketPair>>>`
2. WebSocket reconnect loop checks for pending pairs before reconnecting
3. If pending pairs exist, rebuild GlobalState with old + new pairs

**Step 1: Add pending pairs storage**

Add to `main.rs`:

```rust
use tokio::sync::RwLock;

/// Shared storage for newly discovered market pairs pending integration
type PendingPairs = Arc<RwLock<Vec<MarketPair>>>;
```

**Step 2: Modify discovery task to store pending pairs**

Update `discovery_refresh_task` to accept and populate pending pairs:

```rust
async fn discovery_refresh_task(
    discovery: Arc<DiscoveryClient>,
    current_state: Arc<GlobalState>,
    pending_pairs: PendingPairs,
    shutdown_tx: watch::Sender<bool>,
    interval_mins: u64,
    leagues: &'static [&'static str],
) {
    // ... existing setup ...

    // When new markets found:
    if !result.pairs.is_empty() {
        // Store pending pairs
        {
            let mut pending = pending_pairs.write().await;
            pending.extend(result.pairs);
        }

        // Signal reconnect
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = shutdown_tx.send(false);
    }
}
```

**Step 3: Create state rebuild helper**

Add helper function:

```rust
/// Rebuild GlobalState with existing pairs plus new pending pairs
fn rebuild_state_with_pairs(
    existing_state: &GlobalState,
    new_pairs: Vec<MarketPair>,
) -> GlobalState {
    let mut new_state = GlobalState::new();

    // Copy existing pairs
    for market in existing_state.markets.iter().take(existing_state.market_count()) {
        if let Some(pair) = &market.pair {
            new_state.add_pair((**pair).clone());
        }
    }

    // Add new pairs
    for pair in new_pairs {
        new_state.add_pair(pair);
    }

    new_state
}
```

**Step 4: Modify WebSocket loops to rebuild state**

This requires restructuring the main loop to allow state replacement. The WebSocket handlers need access to the latest state on each reconnect iteration.

**Step 5: Run full test**

Run: `DISCOVERY_INTERVAL_MINS=1 DRY_RUN=1 cargo run --release`
Expected: See discovery task running every minute

**Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): implement state rebuild on new market discovery

New markets discovered during runtime are integrated by rebuilding
GlobalState and reconnecting WebSockets with updated subscription list."
```

---

## Task 6: Update README

**Files:**
- Modify: `README.md`

**Step 1: Add new environment variable documentation**

Add to the "Bot Configuration" table:

```markdown
| `DISCOVERY_INTERVAL_MINS` | `15` | Minutes between discovery scans (0 = disabled) |
```

**Step 2: Update "Future Enhancements" section**

Change the runtime discovery item from `[ ]` to `[x]`:

```markdown
- [x] **Runtime market discovery** - Periodic scanning for new markets without restart
```

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document runtime market discovery feature

Add DISCOVERY_INTERVAL_MINS to environment variables table."
```

---

## Task 7: Manual Testing Checklist

**No code changes - verification only**

**Step 1: Test with discovery disabled**

```bash
DISCOVERY_INTERVAL_MINS=0 DRY_RUN=1 dotenvx run -- cargo run --release
```

Expected: No "[DISCOVERY] Running scheduled discovery" messages

**Step 2: Test with short interval**

```bash
DISCOVERY_INTERVAL_MINS=1 DRY_RUN=1 dotenvx run -- cargo run --release
```

Expected: See discovery messages every minute, WebSocket reconnection when new markets found

**Step 3: Test error handling**

Temporarily break network or Kalshi credentials to verify partial failure handling.

Expected: Errors logged, existing markets continue trading

---

## Summary

| Task | Description | Estimated Complexity |
|------|-------------|---------------------|
| 1 | Add configuration | Low |
| 2 | Add discover_since method | Medium |
| 3 | Add shutdown signal to WebSockets | Medium |
| 4 | Implement discovery refresh task | Medium |
| 5 | Full integration - state rebuild | High |
| 6 | Update README | Low |
| 7 | Manual testing | Low |

**Critical path:** Tasks 1-5 must be done in order. Task 6 can be done anytime. Task 7 after all code changes.
