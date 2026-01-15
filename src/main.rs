//! Prediction Market Arbitrage Trading System
//!
//! A high-performance, production-ready arbitrage trading system for cross-platform
//! prediction markets. This system monitors price discrepancies between Kalshi and
//! Polymarket, executing risk-free arbitrage opportunities in real-time.
//!
//! ## Strategy
//!
//! The core arbitrage strategy exploits the fundamental property of prediction markets:
//! YES + NO = $1.00 (guaranteed). Arbitrage opportunities exist when:
//!
//! ```
//! Best YES ask (Platform A) + Best NO ask (Platform B) < $1.00
//! ```
//!
//! ## Architecture
//!
//! - **Real-time price monitoring** via WebSocket connections to both platforms
//! - **Lock-free orderbook cache** using atomic operations for zero-copy updates
//! - **SIMD-accelerated arbitrage detection** for sub-millisecond latency
//! - **Concurrent order execution** with automatic position reconciliation
//! - **Circuit breaker protection** with configurable risk limits
//! - **Market discovery system** with intelligent caching and incremental updates

mod cache;
mod circuit_breaker;
mod config;
mod discovery;
mod execution;
mod kalshi;
mod polymarket;
mod polymarket_clob;
mod position_tracker;
mod types;

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::{RwLock, watch};
use tracing::{error, info, warn};

use cache::TeamCache;
use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use config::{ARB_THRESHOLD, enabled_leagues, WS_RECONNECT_DELAY_SECS};
use discovery::DiscoveryClient;
use execution::{ExecutionEngine, create_execution_channel, run_execution_loop};
use kalshi::{KalshiConfig, KalshiApiClient};
use polymarket_clob::{PolymarketAsyncClient, PreparedCreds, SharedAsyncClient};
use position_tracker::{PositionTracker, create_position_channel, position_writer_loop};
use types::{GlobalState, PriceCents};

/// Polymarket CLOB API host
const POLY_CLOB_HOST: &str = "https://clob.polymarket.com";
/// Polygon chain ID
const POLYGON_CHAIN_ID: u64 = 137;

/// Background task that periodically discovers new markets
async fn discovery_refresh_task(
    discovery: Arc<DiscoveryClient>,
    state: Arc<GlobalState>,
    shutdown_tx: watch::Sender<bool>,
    interval_mins: u64,
) {
    // Get leagues from env (cached)
    let enabled = enabled_leagues();
    let leagues_vec: Vec<&str> = enabled.iter().map(|s| s.as_str()).collect();
    let leagues: &[&str] = &leagues_vec;
    use std::time::{SystemTime, UNIX_EPOCH};

    if interval_mins == 0 {
        info!("[DISCOVERY] Runtime discovery disabled (DISCOVERY_INTERVAL_MINS=0)");
        return;
    }

    info!("[DISCOVERY] Runtime discovery enabled (interval: {}m)", interval_mins);

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_mins * 60));
    interval.tick().await; // Skip immediate first tick

    // Track last discovery timestamp
    let mut last_discovery_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    loop {
        interval.tick().await;

        info!("[DISCOVERY] Running scheduled discovery...");

        // Build set of known tickers
        let known_tickers: HashSet<String> = state.markets.iter()
            .take(state.market_count())
            .filter_map(|m| m.pair())
            .map(|p| p.kalshi_market_ticker.to_string())
            .collect();

        // Discover new markets since last check
        let result = discovery.discover_since(last_discovery_ts, &known_tickers, leagues).await;

        // Update timestamp for next iteration
        last_discovery_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
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

        // Add new pairs to global state (thread-safe via interior mutability)
        let mut added_count = 0;
        for pair in result.pairs {
            if let Some(market_id) = state.add_pair(pair) {
                added_count += 1;
                info!("[DISCOVERY] Added market_id {} to state", market_id);
            } else {
                warn!("[DISCOVERY] Failed to add pair - state full (MAX_MARKETS reached)");
            }
        }
        info!("[DISCOVERY] Added {} new markets to state (total: {})", added_count, state.market_count());

        // Signal WebSockets to reconnect with updated subscriptions
        info!("[DISCOVERY] Signaling WebSocket reconnect for {} new markets...", added_count);
        if shutdown_tx.send(true).is_err() {
            warn!("[DISCOVERY] Failed to signal WebSocket reconnect - receivers dropped");
        }

        // Give WebSockets time to shut down gracefully
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Reset shutdown signal for next cycle
        if shutdown_tx.send(false).is_err() {
            warn!("[DISCOVERY] Failed to reset shutdown signal - receivers dropped");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("arb_bot=info".parse().unwrap()),
        )
        .init();

    info!("🚀 Prediction Market Arbitrage System v2.0");
    info!("   Profit threshold: <{:.1}¢ ({:.1}% minimum profit)",
          ARB_THRESHOLD * 100.0, (1.0 - ARB_THRESHOLD) * 100.0);

    // Get enabled leagues (convert &[String] to Vec<&str> for API compatibility)
    let enabled = enabled_leagues();
    let leagues: Vec<&str> = enabled.iter().map(|s| s.as_str()).collect();
    let leagues: &[&str] = if leagues.is_empty() { &[] } else { &leagues };
    info!("   Monitored leagues: {:?}", if leagues.is_empty() { "all" } else { "filtered" });
    if !leagues.is_empty() {
        info!("   Enabled: {:?}", leagues);
    }

    // Check for dry run mode
    let dry_run = std::env::var("DRY_RUN").map(|v| v == "1" || v == "true").unwrap_or(true);
    if dry_run {
        info!("   Mode: DRY RUN (set DRY_RUN=0 to execute)");
    } else {
        warn!("   Mode: LIVE EXECUTION");
    }

    // Load Kalshi credentials
    let kalshi_config = KalshiConfig::from_env()?;
    info!("[KALSHI] API key loaded");

    // Load Polymarket credentials
    dotenvy::dotenv().ok();
    let poly_private_key = std::env::var("POLY_PRIVATE_KEY")
        .context("POLY_PRIVATE_KEY not set")?;
    let poly_funder = std::env::var("POLY_FUNDER")
        .context("POLY_FUNDER not set (your wallet address)")?;

    // Create async Polymarket client and derive API credentials
    info!("[POLYMARKET] Creating async client and deriving API credentials...");
    let poly_async_client = PolymarketAsyncClient::new(
        POLY_CLOB_HOST,
        POLYGON_CHAIN_ID,
        &poly_private_key,
        &poly_funder,
    )?;
    let api_creds = poly_async_client.derive_api_key(0).await?;
    let prepared_creds = PreparedCreds::from_api_creds(&api_creds)?;
    let poly_async = Arc::new(SharedAsyncClient::new(poly_async_client, prepared_creds, POLYGON_CHAIN_ID));

    // Load neg_risk cache from Python script output
    match poly_async.load_cache(".clob_market_cache.json") {
        Ok(count) => info!("[POLYMARKET] Loaded {} neg_risk entries from cache", count),
        Err(e) => warn!("[POLYMARKET] Could not load neg_risk cache: {}", e),
    }

    info!("[POLYMARKET] Client ready for {}", &poly_funder[..10]);

    // Load team code mapping cache
    let team_cache = TeamCache::load();
    info!("📂 Loaded {} team code mappings", team_cache.len());

    // Create Kalshi API client
    let kalshi_api = Arc::new(KalshiApiClient::new(kalshi_config));

    // Run discovery (with caching support)
    let force_discovery = std::env::var("FORCE_DISCOVERY")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    info!("🔍 Market discovery{}...",
          if force_discovery { " (forced refresh)" } else { "" });

    let discovery = DiscoveryClient::new(
        KalshiApiClient::new(KalshiConfig::from_env()?),
        team_cache
    );

    let result = if force_discovery {
        discovery.discover_all_force(leagues).await
    } else {
        discovery.discover_all(leagues).await
    };

    info!("📊 Market discovery complete:");
    info!("   - Matched market pairs: {}", result.pairs.len());

    if !result.errors.is_empty() {
        for err in &result.errors {
            warn!("   ⚠️ {}", err);
        }
    }

    if result.pairs.is_empty() {
        error!("No market pairs found!");
        return Ok(());
    }

    // Display discovered market pairs
    info!("📋 Discovered market pairs:");
    for pair in &result.pairs {
        info!("   ✅ {} | {} | Kalshi: {}",
              pair.description,
              pair.market_type,
              pair.kalshi_market_ticker);
    }

    // Build global state
    let state = Arc::new({
        let s = GlobalState::new();
        for pair in result.pairs {
            s.add_pair(pair);
        }
        info!("📡 Global state initialized: tracking {} markets", s.market_count());
        s
    });

    // Initialize execution infrastructure
    let (exec_tx, exec_rx) = create_execution_channel();
    let circuit_breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::from_env()));

    // Create shutdown channel for WebSocket reconnection
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let position_tracker = Arc::new(RwLock::new(PositionTracker::new()));
    let (position_channel, position_rx) = create_position_channel();

    tokio::spawn(position_writer_loop(position_rx, position_tracker));

    let threshold_cents: PriceCents = ((ARB_THRESHOLD * 100.0).round() as u16).max(1);
    info!("   Execution threshold: {} cents", threshold_cents);

    let engine = Arc::new(ExecutionEngine::new(
        kalshi_api.clone(),
        poly_async,
        state.clone(),
        circuit_breaker.clone(),
        position_channel,
        dry_run,
    ));

    let exec_handle = tokio::spawn(run_execution_loop(exec_rx, engine));

    // === TEST MODE: Synthetic arbitrage injection ===
    // TEST_ARB=1 to enable, TEST_ARB_TYPE=poly_yes_kalshi_no|kalshi_yes_poly_no|poly_only|kalshi_only
    let test_arb = std::env::var("TEST_ARB").map(|v| v == "1" || v == "true").unwrap_or(false);
    if test_arb {
        let test_state = state.clone();
        let test_exec_tx = exec_tx.clone();
        let test_dry_run = dry_run;

        // Parse arb type from environment (default: poly_yes_kalshi_no)
        let arb_type_str = std::env::var("TEST_ARB_TYPE").unwrap_or_else(|_| "poly_yes_kalshi_no".to_string());

        tokio::spawn(async move {
            use types::{FastExecutionRequest, ArbType};

            // Wait for WebSocket connections to establish and populate orderbooks
            info!("[TEST] Injecting synthetic arbitrage opportunity in 10 seconds...");
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

            // Parse arb type
            let arb_type = match arb_type_str.to_lowercase().as_str() {
                "poly_yes_kalshi_no" | "pykn" | "0" => ArbType::PolyYesKalshiNo,
                "kalshi_yes_poly_no" | "kypn" | "1" => ArbType::KalshiYesPolyNo,
                "poly_only" | "poly" | "2" => ArbType::PolyOnly,
                "kalshi_only" | "kalshi" | "3" => ArbType::KalshiOnly,
                _ => {
                    warn!("[TEST] Unknown TEST_ARB_TYPE='{}', defaulting to PolyYesKalshiNo", arb_type_str);
                    warn!("[TEST] Valid values: poly_yes_kalshi_no, kalshi_yes_poly_no, poly_only, kalshi_only");
                    ArbType::PolyYesKalshiNo
                }
            };

            // Set prices based on arb type for realistic test scenarios
            let (yes_price, no_price, description) = match arb_type {
                ArbType::PolyYesKalshiNo => (40, 50, "P_yes=40¢ + K_no=50¢ + fee≈2¢ = 92¢ → 8¢ profit"),
                ArbType::KalshiYesPolyNo => (40, 50, "K_yes=40¢ + P_no=50¢ + fee≈2¢ = 92¢ → 8¢ profit"),
                ArbType::PolyOnly => (48, 50, "P_yes=48¢ + P_no=50¢ + fee=0¢ = 98¢ → 2¢ profit (NO FEES!)"),
                ArbType::KalshiOnly => (44, 44, "K_yes=44¢ + K_no=44¢ + fee≈4¢ = 92¢ → 8¢ profit (DOUBLE FEES)"),
            };

            // Find first market with valid state
            let market_count = test_state.market_count();
            for market_id in 0..market_count {
                if let Some(market) = test_state.get_by_id(market_id as u16) {
                    if let Some(pair) = market.pair() {
                        // SIZE: 1000 cents = 10 contracts (Poly $1 min requires ~3 contracts at 40¢)
                        let fake_req = FastExecutionRequest {
                            market_id: market_id as u16,
                            yes_price,
                            no_price,
                            yes_size: 1000,  // 1000¢ = 10 contracts
                            no_size: 1000,   // 1000¢ = 10 contracts
                            arb_type,
                            detected_ns: 0,
                        };

                        warn!("[TEST] 🧪 Injecting synthetic {:?} arbitrage for: {}", arb_type, pair.description);
                        warn!("[TEST]    Scenario: {}", description);
                        warn!("[TEST]    Position size capped to 10 contracts for safety");
                        warn!("[TEST]    Execution mode: DRY_RUN={}", test_dry_run);

                        if let Err(e) = test_exec_tx.send(fake_req).await {
                            error!("[TEST] Failed to send fake arb: {}", e);
                        }
                        break;
                    }
                }
            }
        });
    }

    // Initialize Kalshi WebSocket connection (config reused on reconnects)
    let kalshi_state = state.clone();
    let kalshi_exec_tx = exec_tx.clone();
    let kalshi_threshold = threshold_cents;
    let kalshi_ws_config = KalshiConfig::from_env()?;
    let kalshi_shutdown_rx = shutdown_rx.clone();
    let kalshi_handle = tokio::spawn(async move {
        loop {
            let shutdown_rx = kalshi_shutdown_rx.clone();
            if let Err(e) = kalshi::run_ws(&kalshi_ws_config, kalshi_state.clone(), kalshi_exec_tx.clone(), kalshi_threshold, shutdown_rx).await {
                error!("[KALSHI] WebSocket disconnected: {} - reconnecting...", e);
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(WS_RECONNECT_DELAY_SECS)).await;
        }
    });

    // Initialize Polymarket WebSocket connection
    let poly_state = state.clone();
    let poly_exec_tx = exec_tx.clone();
    let poly_threshold = threshold_cents;
    let poly_shutdown_rx = shutdown_rx.clone();
    let poly_handle = tokio::spawn(async move {
        loop {
            let shutdown_rx = poly_shutdown_rx.clone();
            if let Err(e) = polymarket::run_ws(poly_state.clone(), poly_exec_tx.clone(), poly_threshold, shutdown_rx).await {
                error!("[POLYMARKET] WebSocket disconnected: {} - reconnecting...", e);
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(WS_RECONNECT_DELAY_SECS)).await;
        }
    });

    // System health monitoring and arbitrage diagnostics
    let heartbeat_state = state.clone();
    let heartbeat_threshold = threshold_cents;
    let heartbeat_handle = tokio::spawn(async move {
        use crate::types::kalshi_fee_cents;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let market_count = heartbeat_state.market_count();
            let mut with_kalshi = 0;
            let mut with_poly = 0;
            let mut with_both = 0;
            // Track best arbitrage opportunity: (total_cost, market_id, p_yes, k_no, k_yes, p_no, fee, is_poly_yes_kalshi_no)
            let mut best_arb: Option<(u16, u16, u16, u16, u16, u16, u16, bool)> = None;

            for market in heartbeat_state.markets.iter().take(market_count) {
                let (k_yes, k_no, _, _) = market.kalshi.load();
                let (p_yes, p_no, _, _) = market.poly.load();
                let has_k = k_yes > 0 && k_no > 0;
                let has_p = p_yes > 0 && p_no > 0;
                if k_yes > 0 || k_no > 0 { with_kalshi += 1; }
                if p_yes > 0 || p_no > 0 { with_poly += 1; }
                if has_k && has_p {
                    with_both += 1;

                    let fee1 = kalshi_fee_cents(k_no);
                    let cost1 = p_yes + k_no + fee1;

                    let fee2 = kalshi_fee_cents(k_yes);
                    let cost2 = k_yes + fee2 + p_no;

                    let (best_cost, best_fee, is_poly_yes) = if cost1 <= cost2 {
                        (cost1, fee1, true)
                    } else {
                        (cost2, fee2, false)
                    };

                    if best_arb.is_none() || best_cost < best_arb.as_ref().unwrap().0 {
                        best_arb = Some((best_cost, market.market_id, p_yes, k_no, k_yes, p_no, best_fee, is_poly_yes));
                    }
                }
            }

            let now = chrono::Local::now().format("%H:%M:%S");
            print!("\r[{}] 💓 {} markets | K:{} P:{} Both:{} | threshold={}¢    ",
                   now, market_count, with_kalshi, with_poly, with_both, heartbeat_threshold);
            let _ = std::io::stdout().flush();

            if let Some((cost, market_id, p_yes, k_no, k_yes, p_no, fee, is_poly_yes)) = best_arb {
                let gap = cost as i16 - heartbeat_threshold as i16;
                let pair = heartbeat_state.get_by_id(market_id)
                    .and_then(|m| m.pair());
                let desc = pair
                    .as_ref()
                    .map(|p| {
                        if let Some(line) = p.line_value {
                            format!("{} ({})", p.description, line)
                        } else {
                            p.description.to_string()
                        }
                    })
                    .unwrap_or_else(|| "Unknown".to_string());
                let leg_breakdown = if is_poly_yes {
                    format!("P_yes({}¢) + K_no({}¢) + K_fee({}¢) = {}¢", p_yes, k_no, fee, cost)
                } else {
                    format!("K_yes({}¢) + P_no({}¢) + K_fee({}¢) = {}¢", k_yes, p_no, fee, cost)
                };
                if gap < 0 {
                    println!();  // Move to new line before logging opportunity
                    info!("📊 Best opportunity: {} | {} | gap={:+}¢ | [Poly_yes={}¢ Kalshi_no={}¢ Kalshi_yes={}¢ Poly_no={}¢]",
                          desc, leg_breakdown, gap, p_yes, k_no, k_yes, p_no);
                }
            } else if with_both == 0 {
                println!();  // Move to new line before warning
                warn!("⚠️  No markets with both Kalshi and Polymarket prices - verify WebSocket connections");
            }
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
        ).await;
    });

    // Main event loop - run until termination
    info!("✅ All systems operational - entering main event loop");
    let _ = tokio::join!(kalshi_handle, poly_handle, heartbeat_handle, exec_handle, discovery_handle);

    Ok(())
}
