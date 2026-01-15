//! High-performance order execution engine for arbitrage opportunities.
//!
//! This module handles concurrent order execution across both platforms,
//! position reconciliation, and automatic exposure management.

use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{info, warn, error};

use crate::kalshi::KalshiApiClient;
use crate::polymarket_clob::SharedAsyncClient;
use crate::types::{
    ArbType, MarketPair,
    FastExecutionRequest, GlobalState,
    cents_to_price,
};
use crate::circuit_breaker::CircuitBreaker;
use crate::position_tracker::{FillRecord, PositionChannel};

// =============================================================================
// EXECUTION ENGINE
// =============================================================================

/// High-precision monotonic clock for latency measurement and performance tracking
pub struct NanoClock {
    start: Instant,
}

impl NanoClock {
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }

    #[inline(always)]
    pub fn now_ns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }
}

impl Default for NanoClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Core execution engine for processing arbitrage opportunities
pub struct ExecutionEngine {
    kalshi: Arc<KalshiApiClient>,
    poly_async: Arc<SharedAsyncClient>,
    state: Arc<GlobalState>,
    circuit_breaker: Arc<CircuitBreaker>,
    position_channel: PositionChannel,
    in_flight: Arc<[AtomicU64; 8]>,
    clock: Arc<NanoClock>,
    pub dry_run: bool,
    test_mode: bool,
}

impl ExecutionEngine {
    pub fn new(
        kalshi: Arc<KalshiApiClient>,
        poly_async: Arc<SharedAsyncClient>,
        state: Arc<GlobalState>,
        circuit_breaker: Arc<CircuitBreaker>,
        position_channel: PositionChannel,
        dry_run: bool,
        clock: Arc<NanoClock>,
    ) -> Self {
        let test_mode = std::env::var("TEST_ARB")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);

        Self {
            kalshi,
            poly_async,
            state,
            circuit_breaker,
            position_channel,
            in_flight: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            clock,
            dry_run,
            test_mode,
        }
    }

    /// Process an execution request
    #[inline]
    pub async fn process(&self, req: FastExecutionRequest) -> Result<ExecutionResult> {
        let market_id = req.market_id;

        // Deduplication check (512 markets via 8x u64 bitmask)
        if market_id < 512 {
            let slot = (market_id / 64) as usize;
            let bit = market_id % 64;
            let mask = 1u64 << bit;
            let prev = self.in_flight[slot].fetch_or(mask, Ordering::AcqRel);
            if prev & mask != 0 {
                return Ok(ExecutionResult {
                    market_id,
                    success: false,
                    profit_cents: 0,
                    latency_ns: self.clock.now_ns() - req.detected_ns,
                    error: Some("Already in-flight"),
                });
            }
        }

        // Get market pair
        let market = self.state.get_by_id(market_id)
            .ok_or_else(|| anyhow!("Unknown market_id {}", market_id))?;

        let pair = market.pair()
            .ok_or_else(|| anyhow!("No pair for market_id {}", market_id))?;

        // Calculate profit
        let profit_cents = req.profit_cents();
        if profit_cents < 1 {
            self.release_in_flight(market_id);
            return Ok(ExecutionResult {
                market_id,
                success: false,
                profit_cents: 0,
                latency_ns: self.clock.now_ns() - req.detected_ns,
                error: Some("Profit below threshold"),
            });
        }

        // Calculate max contracts from size (min of both sides)
        let mut max_contracts = (req.yes_size.min(req.no_size) / 100) as i64;

        // Safety: In test mode, cap position size at 10 contracts
        // Note: Polymarket enforces a $1 minimum order value. At 40¢ per contract,
        // a single contract ($0.40) would be rejected. Using 10 contracts ensures
        // we meet the minimum requirement at any reasonable price level.
        if self.test_mode && max_contracts > 10 {
            warn!("[EXEC] ⚠️ TEST_MODE: Position size capped from {} to 10 contracts", max_contracts);
            max_contracts = 10;
        }

        if max_contracts < 1 {
            warn!(
                "[EXEC] Liquidity fail: {:?} | yes_size={}¢ no_size={}¢",
                req.arb_type, req.yes_size, req.no_size
            );
            self.release_in_flight(market_id);
            return Ok(ExecutionResult {
                market_id,
                success: false,
                profit_cents: 0,
                latency_ns: self.clock.now_ns() - req.detected_ns,
                error: Some("Insufficient liquidity"),
            });
        }

        // Circuit breaker check
        if let Err(_reason) = self.circuit_breaker.can_execute(&pair.pair_id, max_contracts).await {
            self.release_in_flight(market_id);
            return Ok(ExecutionResult {
                market_id,
                success: false,
                profit_cents: 0,
                latency_ns: self.clock.now_ns() - req.detected_ns,
                error: Some("Circuit breaker"),
            });
        }

        let latency_to_exec = self.clock.now_ns() - req.detected_ns;
        info!(
            "[EXEC] 🎯 {} | {:?} y={}¢ n={}¢ | profit={}¢ | {}x | {}µs",
            pair.description,
            req.arb_type,
            req.yes_price,
            req.no_price,
            profit_cents,
            max_contracts,
            latency_to_exec / 1000
        );

        if self.dry_run {
            info!("[EXEC] 🏃 DRY RUN - would execute {} contracts", max_contracts);
            self.release_in_flight_delayed(market_id);
            return Ok(ExecutionResult {
                market_id,
                success: true,
                profit_cents,
                latency_ns: latency_to_exec,
                error: Some("DRY_RUN"),
            });
        }

        // Execute both legs concurrently
        let result = self.execute_both_legs_async(&req, &pair, max_contracts).await;

        // Release in-flight after delay
        self.release_in_flight_delayed(market_id);

        match result {
            // Note: For same-platform arbs (PolyOnly/KalshiOnly), these are YES/NO fills, not platform fills
            Ok((yes_filled, no_filled, yes_cost, no_cost, yes_order_id, no_order_id)) => {
                let matched = yes_filled.min(no_filled);
                let success = matched > 0;
                let actual_profit = matched as i16 * 100 - (yes_cost + no_cost) as i16;

                // === Automatic exposure management for mismatched fills ===
                // If one leg fills more than the other, automatically close the excess
                // to maintain market-neutral exposure (non-blocking background task)
                if yes_filled != no_filled && (yes_filled > 0 || no_filled > 0) {
                    let excess = (yes_filled - no_filled).abs();
                    let (leg1_name, leg2_name) = match req.arb_type {
                        ArbType::PolyYesKalshiNo => ("P_yes", "K_no"),
                        ArbType::KalshiYesPolyNo => ("K_yes", "P_no"),
                        ArbType::PolyOnly => ("P_yes", "P_no"),
                        ArbType::KalshiOnly => ("K_yes", "K_no"),
                    };
                    warn!("[EXEC] ⚠️ Fill mismatch: {}={} {}={} (excess={})",
                        leg1_name, yes_filled, leg2_name, no_filled, excess);

                    // Spawn auto-close in background (don't block hot path with 2s sleep)
                    let kalshi = self.kalshi.clone();
                    let poly_async = self.poly_async.clone();
                    let arb_type = req.arb_type;
                    let yes_price = req.yes_price;
                    let no_price = req.no_price;
                    let poly_yes_token = pair.poly_yes_token.clone();
                    let poly_no_token = pair.poly_no_token.clone();
                    let kalshi_ticker = pair.kalshi_market_ticker.clone();
                    let original_cost_per_contract = if yes_filled > no_filled {
                        if yes_filled > 0 { yes_cost / yes_filled } else { 0 }
                    } else {
                        if no_filled > 0 { no_cost / no_filled } else { 0 }
                    };

                    tokio::spawn(async move {
                        Self::auto_close_background(
                            kalshi, poly_async, arb_type, yes_filled, no_filled,
                            yes_price, no_price, poly_yes_token, poly_no_token,
                            kalshi_ticker, original_cost_per_contract
                        ).await;
                    });
                }

                if success {
                    self.circuit_breaker.record_success(&pair.pair_id, matched, matched, actual_profit as f64 / 100.0).await;
                }

                if matched > 0 {
                    let (platform1, side1, platform2, side2) = match req.arb_type {
                        ArbType::PolyYesKalshiNo => ("polymarket", "yes", "kalshi", "no"),
                        ArbType::KalshiYesPolyNo => ("kalshi", "yes", "polymarket", "no"),
                        ArbType::PolyOnly => ("polymarket", "yes", "polymarket", "no"),
                        ArbType::KalshiOnly => ("kalshi", "yes", "kalshi", "no"),
                    };

                    self.position_channel.record_fill(FillRecord::new(
                        &pair.pair_id, &pair.description, platform1, side1,
                        matched as f64, yes_cost as f64 / 100.0 / yes_filled.max(1) as f64,
                        0.0, &yes_order_id,
                    ));
                    self.position_channel.record_fill(FillRecord::new(
                        &pair.pair_id, &pair.description, platform2, side2,
                        matched as f64, no_cost as f64 / 100.0 / no_filled.max(1) as f64,
                        0.0, &no_order_id,
                    ));
                }

                Ok(ExecutionResult {
                    market_id,
                    success,
                    profit_cents: actual_profit,
                    latency_ns: self.clock.now_ns() - req.detected_ns,
                    error: if success { None } else { Some("Partial/no fill") },
                })
            }
            Err(_e) => {
                self.circuit_breaker.record_error().await;
                Ok(ExecutionResult {
                    market_id,
                    success: false,
                    profit_cents: 0,
                    latency_ns: self.clock.now_ns() - req.detected_ns,
                    error: Some("Execution failed"),
                })
            }
        }
    }

    async fn execute_both_legs_async(
        &self,
        req: &FastExecutionRequest,
        pair: &MarketPair,
        contracts: i64,
    ) -> Result<(i64, i64, i64, i64, String, String)> {
        match req.arb_type {
            // === CROSS-PLATFORM: Poly YES + Kalshi NO ===
            ArbType::PolyYesKalshiNo => {
                let kalshi_fut = self.kalshi.buy_ioc(
                    &pair.kalshi_market_ticker,
                    "no",
                    req.no_price as i64,
                    contracts,
                );
                let poly_fut = self.poly_async.buy_fak(
                    &pair.poly_yes_token,
                    cents_to_price(req.yes_price),
                    contracts as f64,
                );
                let (kalshi_res, poly_res) = tokio::join!(kalshi_fut, poly_fut);
                self.extract_cross_results(kalshi_res, poly_res)
            }

            // === CROSS-PLATFORM: Kalshi YES + Poly NO ===
            ArbType::KalshiYesPolyNo => {
                let kalshi_fut = self.kalshi.buy_ioc(
                    &pair.kalshi_market_ticker,
                    "yes",
                    req.yes_price as i64,
                    contracts,
                );
                let poly_fut = self.poly_async.buy_fak(
                    &pair.poly_no_token,
                    cents_to_price(req.no_price),
                    contracts as f64,
                );
                let (kalshi_res, poly_res) = tokio::join!(kalshi_fut, poly_fut);
                self.extract_cross_results(kalshi_res, poly_res)
            }

            // === SAME-PLATFORM: Poly YES + Poly NO ===
            ArbType::PolyOnly => {
                let yes_fut = self.poly_async.buy_fak(
                    &pair.poly_yes_token,
                    cents_to_price(req.yes_price),
                    contracts as f64,
                );
                let no_fut = self.poly_async.buy_fak(
                    &pair.poly_no_token,
                    cents_to_price(req.no_price),
                    contracts as f64,
                );
                let (yes_res, no_res) = tokio::join!(yes_fut, no_fut);
                self.extract_poly_only_results(yes_res, no_res)
            }

            // === SAME-PLATFORM: Kalshi YES + Kalshi NO ===
            ArbType::KalshiOnly => {
                let yes_fut = self.kalshi.buy_ioc(
                    &pair.kalshi_market_ticker,
                    "yes",
                    req.yes_price as i64,
                    contracts,
                );
                let no_fut = self.kalshi.buy_ioc(
                    &pair.kalshi_market_ticker,
                    "no",
                    req.no_price as i64,
                    contracts,
                );
                let (yes_res, no_res) = tokio::join!(yes_fut, no_fut);
                self.extract_kalshi_only_results(yes_res, no_res)
            }
        }
    }

    /// Extract results from cross-platform execution
    fn extract_cross_results(
        &self,
        kalshi_res: Result<crate::kalshi::KalshiOrderResponse>,
        poly_res: Result<crate::polymarket_clob::PolyFillAsync>,
    ) -> Result<(i64, i64, i64, i64, String, String)> {
        let (kalshi_filled, kalshi_cost, kalshi_order_id) = match kalshi_res {
            Ok(resp) => {
                let filled = resp.order.filled_count();
                let cost = resp.order.taker_fill_cost.unwrap_or(0) + resp.order.maker_fill_cost.unwrap_or(0);
                (filled, cost, resp.order.order_id)
            }
            Err(e) => {
                warn!("[EXEC] Kalshi failed: {}", e);
                (0, 0, String::new())
            }
        };

        let (poly_filled, poly_cost, poly_order_id) = match poly_res {
            Ok(fill) => {
                ((fill.filled_size as i64), (fill.fill_cost * 100.0) as i64, fill.order_id)
            }
            Err(e) => {
                warn!("[EXEC] Poly failed: {}", e);
                (0, 0, String::new())
            }
        };

        Ok((kalshi_filled, poly_filled, kalshi_cost, poly_cost, kalshi_order_id, poly_order_id))
    }

    /// Extract results from Poly-only execution (same-platform)
    fn extract_poly_only_results(
        &self,
        yes_res: Result<crate::polymarket_clob::PolyFillAsync>,
        no_res: Result<crate::polymarket_clob::PolyFillAsync>,
    ) -> Result<(i64, i64, i64, i64, String, String)> {
        let (yes_filled, yes_cost, yes_order_id) = match yes_res {
            Ok(fill) => {
                ((fill.filled_size as i64), (fill.fill_cost * 100.0) as i64, fill.order_id)
            }
            Err(e) => {
                warn!("[EXEC] Poly YES failed: {}", e);
                (0, 0, String::new())
            }
        };

        let (no_filled, no_cost, no_order_id) = match no_res {
            Ok(fill) => {
                ((fill.filled_size as i64), (fill.fill_cost * 100.0) as i64, fill.order_id)
            }
            Err(e) => {
                warn!("[EXEC] Poly NO failed: {}", e);
                (0, 0, String::new())
            }
        };

        // For same-platform, return YES as "kalshi" slot and NO as "poly" slot
        // This keeps the existing result handling logic working
        Ok((yes_filled, no_filled, yes_cost, no_cost, yes_order_id, no_order_id))
    }

    /// Extract results from Kalshi-only execution (same-platform)
    fn extract_kalshi_only_results(
        &self,
        yes_res: Result<crate::kalshi::KalshiOrderResponse>,
        no_res: Result<crate::kalshi::KalshiOrderResponse>,
    ) -> Result<(i64, i64, i64, i64, String, String)> {
        let (yes_filled, yes_cost, yes_order_id) = match yes_res {
            Ok(resp) => {
                let filled = resp.order.filled_count();
                let cost = resp.order.taker_fill_cost.unwrap_or(0) + resp.order.maker_fill_cost.unwrap_or(0);
                (filled, cost, resp.order.order_id)
            }
            Err(e) => {
                warn!("[EXEC] Kalshi YES failed: {}", e);
                (0, 0, String::new())
            }
        };

        let (no_filled, no_cost, no_order_id) = match no_res {
            Ok(resp) => {
                let filled = resp.order.filled_count();
                let cost = resp.order.taker_fill_cost.unwrap_or(0) + resp.order.maker_fill_cost.unwrap_or(0);
                (filled, cost, resp.order.order_id)
            }
            Err(e) => {
                warn!("[EXEC] Kalshi NO failed: {}", e);
                (0, 0, String::new())
            }
        };

        // For same-platform, return YES as "kalshi" slot and NO as "poly" slot
        Ok((yes_filled, no_filled, yes_cost, no_cost, yes_order_id, no_order_id))
    }

    /// Background task to automatically close excess exposure from mismatched fills
    async fn auto_close_background(
        kalshi: Arc<KalshiApiClient>,
        poly_async: Arc<SharedAsyncClient>,
        arb_type: ArbType,
        yes_filled: i64,
        no_filled: i64,
        yes_price: u16,
        no_price: u16,
        poly_yes_token: Arc<str>,
        poly_no_token: Arc<str>,
        kalshi_ticker: Arc<str>,
        original_cost_per_contract: i64,
    ) {
        let excess = (yes_filled - no_filled).abs();
        if excess == 0 {
            return;
        }

        // Helper to log P&L after close
        let log_close_pnl = |platform: &str, closed: i64, proceeds: i64| {
            if closed > 0 {
                let close_pnl = proceeds - (original_cost_per_contract * excess);
                info!("[EXEC] ✅ Closed {} {} contracts for {}¢ (P&L: {}¢)",
                    closed, platform, proceeds, close_pnl);
            } else {
                warn!("[EXEC] ⚠️ Failed to close {} excess - 0 filled", platform);
            }
        };

        match arb_type {
            ArbType::PolyOnly => {
                let (token, side, price) = if yes_filled > no_filled {
                    (&poly_yes_token, "yes", yes_price)
                } else {
                    (&poly_no_token, "no", no_price)
                };
                let close_price = cents_to_price((price as i16).saturating_sub(10).max(1) as u16);

                info!("[EXEC] 🔄 Waiting 2s for Poly settlement before auto-close ({} {} contracts)", excess, side);
                tokio::time::sleep(Duration::from_secs(2)).await;

                match poly_async.sell_fak(token, close_price, excess as f64).await {
                    Ok(fill) => log_close_pnl("Poly", fill.filled_size as i64, (fill.fill_cost * 100.0) as i64),
                    Err(e) => warn!("[EXEC] ⚠️ Failed to close Poly excess: {}", e),
                }
            }

            ArbType::KalshiOnly => {
                let (side, price) = if yes_filled > no_filled {
                    ("yes", yes_price as i64)
                } else {
                    ("no", no_price as i64)
                };
                let close_price = price.saturating_sub(10).max(1);

                match kalshi.sell_ioc(&kalshi_ticker, side, close_price, excess).await {
                    Ok(resp) => {
                        let proceeds = resp.order.taker_fill_cost.unwrap_or(0) + resp.order.maker_fill_cost.unwrap_or(0);
                        log_close_pnl("Kalshi", resp.order.filled_count(), proceeds);
                    }
                    Err(e) => warn!("[EXEC] ⚠️ Failed to close Kalshi excess: {}", e),
                }
            }

            ArbType::PolyYesKalshiNo => {
                if yes_filled > no_filled {
                    // Poly YES excess
                    let close_price = cents_to_price((yes_price as i16).saturating_sub(10).max(1) as u16);
                    info!("[EXEC] 🔄 Waiting 2s for Poly settlement before auto-close ({} yes contracts)", excess);
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    match poly_async.sell_fak(&poly_yes_token, close_price, excess as f64).await {
                        Ok(fill) => log_close_pnl("Poly", fill.filled_size as i64, (fill.fill_cost * 100.0) as i64),
                        Err(e) => warn!("[EXEC] ⚠️ Failed to close Poly excess: {}", e),
                    }
                } else {
                    // Kalshi NO excess
                    let close_price = (no_price as i64).saturating_sub(10).max(1);
                    match kalshi.sell_ioc(&kalshi_ticker, "no", close_price, excess).await {
                        Ok(resp) => {
                            let proceeds = resp.order.taker_fill_cost.unwrap_or(0) + resp.order.maker_fill_cost.unwrap_or(0);
                            log_close_pnl("Kalshi", resp.order.filled_count(), proceeds);
                        }
                        Err(e) => warn!("[EXEC] ⚠️ Failed to close Kalshi excess: {}", e),
                    }
                }
            }

            ArbType::KalshiYesPolyNo => {
                if yes_filled > no_filled {
                    // Kalshi YES excess
                    let close_price = (yes_price as i64).saturating_sub(10).max(1);
                    match kalshi.sell_ioc(&kalshi_ticker, "yes", close_price, excess).await {
                        Ok(resp) => {
                            let proceeds = resp.order.taker_fill_cost.unwrap_or(0) + resp.order.maker_fill_cost.unwrap_or(0);
                            log_close_pnl("Kalshi", resp.order.filled_count(), proceeds);
                        }
                        Err(e) => warn!("[EXEC] ⚠️ Failed to close Kalshi excess: {}", e),
                    }
                } else {
                    // Poly NO excess
                    let close_price = cents_to_price((no_price as i16).saturating_sub(10).max(1) as u16);
                    info!("[EXEC] 🔄 Waiting 2s for Poly settlement before auto-close ({} no contracts)", excess);
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    match poly_async.sell_fak(&poly_no_token, close_price, excess as f64).await {
                        Ok(fill) => log_close_pnl("Poly", fill.filled_size as i64, (fill.fill_cost * 100.0) as i64),
                        Err(e) => warn!("[EXEC] ⚠️ Failed to close Poly excess: {}", e),
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn release_in_flight(&self, market_id: u16) {
        if market_id < 512 {
            let slot = (market_id / 64) as usize;
            let bit = market_id % 64;
            let mask = !(1u64 << bit);
            self.in_flight[slot].fetch_and(mask, Ordering::Release);
        }
    }

    fn release_in_flight_delayed(&self, market_id: u16) {
        if market_id < 512 {
            let in_flight = self.in_flight.clone();
            let slot = (market_id / 64) as usize;
            let bit = market_id % 64;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let mask = !(1u64 << bit);
                in_flight[slot].fetch_and(mask, Ordering::Release);
            });
        }
    }
}

/// Result of an execution attempt
#[derive(Debug, Clone, Copy)]
pub struct ExecutionResult {
    /// Market identifier
    pub market_id: u16,
    /// Whether execution was successful
    pub success: bool,
    /// Realized profit in cents
    pub profit_cents: i16,
    /// Total latency from detection to completion in nanoseconds
    pub latency_ns: u64,
    /// Error message if execution failed
    pub error: Option<&'static str>,
}

/// Create a new execution request channel with bounded capacity
pub fn create_execution_channel() -> (mpsc::Sender<FastExecutionRequest>, mpsc::Receiver<FastExecutionRequest>) {
    mpsc::channel(256)
}

/// Main execution event loop - processes arbitrage opportunities as they arrive
pub async fn run_execution_loop(
    mut rx: mpsc::Receiver<FastExecutionRequest>,
    engine: Arc<ExecutionEngine>,
) {
    info!("[EXEC] Execution engine started (dry_run={})", engine.dry_run);

    while let Some(req) = rx.recv().await {
        let engine = engine.clone();

        // Process immediately in spawned task
        tokio::spawn(async move {
            match engine.process(req).await {
                Ok(result) if result.success => {
                    info!(
                        "[EXEC] ✅ market_id={} profit={}¢ latency={}µs",
                        result.market_id, result.profit_cents, result.latency_ns / 1000
                    );
                }
                Ok(result) => {
                    if result.error != Some("Already in-flight") {
                        warn!(
                            "[EXEC] ⚠️ market_id={}: {:?}",
                            result.market_id, result.error
                        );
                    }
                }
                Err(e) => {
                    error!("[EXEC] ❌ Error: {}", e);
                }
            }
        });
    }

    info!("[EXEC] Execution engine stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_clock_latency_accuracy() {
        // Create a shared clock (simulating what main.rs does)
        let clock = Arc::new(NanoClock::new());

        // Simulate detection time from WebSocket handler
        let detected_ns = clock.now_ns();

        // Simulate some processing delay
        std::thread::sleep(std::time::Duration::from_millis(5));

        // Simulate execution engine calculating latency
        let execution_ns = clock.now_ns();
        let latency_ns = execution_ns - detected_ns;

        // Latency should be approximately 5ms (5_000_000 ns), with some tolerance
        // At minimum it should be > 4ms and < 50ms
        assert!(
            latency_ns > 4_000_000,
            "Latency too low: {}ns (expected > 4ms)",
            latency_ns
        );
        assert!(
            latency_ns < 50_000_000,
            "Latency too high: {}ns (expected < 50ms)",
            latency_ns
        );
    }

    #[test]
    fn test_shared_clock_across_threads() {
        use std::sync::mpsc;

        let clock = Arc::new(NanoClock::new());

        // Channel to send detected_ns from "WebSocket thread" to "execution thread"
        let (tx, rx) = mpsc::channel();

        // Simulate WebSocket handler in another thread
        let ws_clock = clock.clone();
        let handle = std::thread::spawn(move || {
            let detected_ns = ws_clock.now_ns();
            tx.send(detected_ns).unwrap();
        });

        handle.join().unwrap();
        let detected_ns = rx.recv().unwrap();

        // Small delay
        std::thread::sleep(std::time::Duration::from_micros(100));

        // Execution thread calculates latency
        let latency_ns = clock.now_ns() - detected_ns;

        // Latency should be positive and reasonable (not billions of ns from clock mismatch)
        assert!(latency_ns > 0, "Latency should be positive");
        assert!(
            latency_ns < 100_000_000, // < 100ms
            "Latency unreasonably high: {}ns - clock sync issue?",
            latency_ns
        );
    }

    #[test]
    fn test_independent_clocks_would_fail() {
        // This test demonstrates why separate clocks are problematic
        // Two clocks created at different times have different baselines

        let clock1 = NanoClock::new();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let clock2 = NanoClock::new();

        // Get "detection time" from clock1
        let detected_ns = clock1.now_ns();

        // Get "execution time" from clock2 (wrong clock!)
        let execution_ns = clock2.now_ns();

        // The "latency" is nonsensical - execution_ns < detected_ns because
        // clock2 started later and has a smaller elapsed time
        // This would cause underflow or negative latency in real code
        assert!(
            execution_ns < detected_ns,
            "This test shows the bug: clock2 ({}) < clock1 ({}) due to different start times",
            execution_ns,
            detected_ns
        );
    }
}