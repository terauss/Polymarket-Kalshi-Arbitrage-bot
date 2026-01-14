//! Kalshi execution engine

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use crate::api::kalshi::{KalshiApiClient, KalshiConfig};
use crate::config::Config;
use crate::execution::{ExecutionEngine, ExecutionResult, OrderRequest};
use crate::protocol::{ArbType, Platform};

/// Kalshi execution engine
pub struct KalshiEngine {
    client: Arc<KalshiApiClient>,
}

impl KalshiEngine {
    /// Create a new Kalshi engine
    pub fn new(config: &Config) -> Result<Self> {
        let api_key = config
            .kalshi_api_key
            .as_ref()
            .context("Kalshi API key required")?
            .clone();
        let private_key = config
            .kalshi_private_key
            .as_ref()
            .context("Kalshi private key required")?
            .clone();

        let kalshi_config = KalshiConfig::new(api_key, &private_key)?;
        let client = Arc::new(KalshiApiClient::new(kalshi_config));

        Ok(Self { client })
    }

    /// Execute a Kalshi order
    async fn execute_kalshi_order(
        &self,
        ticker: &str,
        side: &str,
        action: &str, // "buy" or "sell"
        price: i64,
        size: i64,
        dry_run: bool,
    ) -> Result<(i64, i64)> {
        if dry_run {
            info!("[KALSHI] DRY RUN: Would {} {} {} contracts at {}¢", action, size, side, price);
            return Ok((size, price * size));
        }

        // Execute IOC order on Kalshi
        let response = match action {
            "sell" => self.client.sell_ioc(ticker, side, price, size).await?,
            "buy" => self.client.buy_ioc(ticker, side, price, size).await?,
            _ => anyhow::bail!("Invalid action: {}", action),
        };
        
        let filled = response.order.filled_count();
        let cost = response.order.taker_fill_cost.unwrap_or(0)
            + response.order.maker_fill_cost.unwrap_or(0);

        info!(
            "[KALSHI] Order executed: {} {} {} filled, cost={}¢",
            action, side, filled, cost
        );

        Ok((filled, cost))
    }
}

#[async_trait]
impl ExecutionEngine for KalshiEngine {
    async fn execute_order(
        &self,
        request: &OrderRequest,
        dry_run: bool,
    ) -> Result<ExecutionResult> {
        let start = Instant::now();
        let market_id = request.market_id;

        // Calculate max contracts from size (min of both sides)
        let max_contracts = (request.yes_size.min(request.no_size) / 100) as i64;

        if max_contracts < 1 {
            return Ok(ExecutionResult {
                market_id,
                success: false,
                profit_cents: 0,
                latency_ns: start.elapsed().as_nanos() as u64,
                error: Some("Insufficient liquidity".to_string()),
            });
        }

        let ticker = request
            .kalshi_market_ticker
            .as_deref()
            .unwrap_or_else(|| {
                // Backward-compatible fallback for older controllers.
                // Real execution requires the real ticker, but dry_run can still log.
                "UNKNOWN-KALSHI-TICKER"
            });

        // Determine execution based on arb type
        let (yes_filled, no_filled, yes_cost, no_cost) = match request.arb_type {
            ArbType::KalshiOnly => {
                // Execute both legs on Kalshi (selling both sides for arbitrage)
                let yes_result = self
                    .execute_kalshi_order(&ticker, "yes", "sell", request.yes_price as i64, max_contracts, dry_run)
                    .await?;
                let no_result = self
                    .execute_kalshi_order(&ticker, "no", "sell", request.no_price as i64, max_contracts, dry_run)
                    .await?;

                (yes_result.0, no_result.0, yes_result.1, no_result.1)
            }
            ArbType::PolyYesKalshiNo => {
                // Only execute NO leg on Kalshi (selling NO)
                let no_result = self
                    .execute_kalshi_order(&ticker, "no", "sell", request.no_price as i64, max_contracts, dry_run)
                    .await?;
                (0, no_result.0, 0, no_result.1)
            }
            ArbType::KalshiYesPolyNo => {
                // Only execute YES leg on Kalshi (selling YES)
                let yes_result = self
                    .execute_kalshi_order(&ticker, "yes", "sell", request.yes_price as i64, max_contracts, dry_run)
                    .await?;
                (yes_result.0, 0, yes_result.1, 0)
            }
            ArbType::PolyOnly => {
                // Kalshi engine doesn't handle PolyOnly
                return Ok(ExecutionResult {
                    market_id,
                    success: false,
                    profit_cents: 0,
                    latency_ns: start.elapsed().as_nanos() as u64,
                    error: Some("Invalid arb type for Kalshi engine".to_string()),
                });
            }
        };

        let matched = yes_filled.min(no_filled);
        let success = matched > 0;
        let actual_profit = if success {
            (matched * 100) - ((yes_cost + no_cost) / 100) as i64
        } else {
            0
        };

        let latency_ns = start.elapsed().as_nanos() as u64;

        if success {
            info!(
                "[KALSHI] ✅ Executed {} contracts | profit={}¢ | latency={}µs",
                matched,
                actual_profit,
                latency_ns / 1000
            );
        }

        Ok(ExecutionResult {
            market_id,
            success,
            profit_cents: actual_profit as i16,
            latency_ns,
            error: if success { None } else { Some("Execution failed".to_string()) },
        })
    }

    async fn can_execute(&self, _market_id: u16, _size: i64) -> Result<()> {
        // TODO: Implement circuit breaker logic
        Ok(())
    }

    fn platform(&self) -> Platform {
        Platform::Kalshi
    }
}
