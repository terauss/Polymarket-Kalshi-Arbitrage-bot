//! Polymarket execution engine

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use crate::api::polymarket::{PolymarketAsyncClient, PreparedCreds, SharedAsyncClient};
use crate::config::Config;
use crate::execution::{ExecutionEngine, ExecutionResult, OrderRequest};
use crate::protocol::{ArbType, Platform};

const POLY_CLOB_HOST: &str = "https://clob.polymarket.com";
const POLYGON_CHAIN_ID: u64 = 137;

/// Polymarket execution engine
pub struct PolymarketEngine {
    client: Arc<SharedAsyncClient>,
}

impl PolymarketEngine {
    /// Create a new Polymarket engine
    pub async fn new(config: &Config) -> Result<Self> {
        let private_key = config
            .polymarket_private_key
            .as_ref()
            .context("Polymarket private key required")?
            .clone();
        let funder = config
            .polymarket_funder
            .as_ref()
            .context("Polymarket funder (wallet address) required")?
            .clone();

        // Create async client
        let poly_async_client = PolymarketAsyncClient::new(
            POLY_CLOB_HOST,
            POLYGON_CHAIN_ID,
            &private_key,
            &funder,
        )?;

        // Get API credentials
        let (api_key, api_secret) = if let (Some(key), Some(secret)) = (
            config.polymarket_api_key.as_ref(),
            config.polymarket_api_secret.as_ref(),
        ) {
            (key.clone(), secret.clone())
        } else {
            // Try to derive API key (may fail if not implemented)
            match poly_async_client.derive_api_key(0).await {
                Ok((key, secret)) => (key, secret),
                Err(_) => {
                    anyhow::bail!(
                        "Polymarket API key derivation not implemented. Please provide POLYMARKET_API_KEY and POLYMARKET_API_SECRET"
                    );
                }
            }
        };

        let prepared_creds = PreparedCreds::new(api_key, api_secret);
        let client = Arc::new(SharedAsyncClient::new(
            poly_async_client,
            prepared_creds,
            POLYGON_CHAIN_ID,
        ));

        Ok(Self { client })
    }

    /// Convert cents to price (0-1 range)
    fn cents_to_price(cents: u16) -> f64 {
        (cents as f64) / 10000.0
    }

    /// Execute a Polymarket order
    async fn execute_polymarket_order(
        &self,
        token: &str,
        side: &str,
        price: f64,
        size: f64,
        dry_run: bool,
    ) -> Result<(f64, f64)> {
        if dry_run {
            info!("[POLY] DRY RUN: Would execute {} {} contracts at {:.4}", size, side, price);
            return Ok((size, price * size));
        }

        // Execute FAK order on Polymarket
        let fill = match side {
            "yes" | "buy" => self.client.buy_fak(token, price, size).await?,
            "no" | "sell" => self.client.sell_fak(token, price, size).await?,
            _ => anyhow::bail!("Invalid side: {}", side),
        };

        info!(
            "[POLY] Order executed: {} filled, cost={:.4}",
            fill.filled_size, fill.fill_cost
        );

        Ok((fill.filled_size, fill.fill_cost))
    }
}

#[async_trait]
impl ExecutionEngine for PolymarketEngine {
    async fn execute_order(
        &self,
        request: &OrderRequest,
        dry_run: bool,
    ) -> Result<ExecutionResult> {
        let start = Instant::now();
        let market_id = request.market_id;

        // Calculate max contracts from size (min of both sides)
        let max_contracts = (request.yes_size.min(request.no_size) / 100) as f64;

        if max_contracts < 1.0 {
            return Ok(ExecutionResult {
                market_id,
                success: false,
                profit_cents: 0,
                latency_ns: start.elapsed().as_nanos() as u64,
                error: Some("Insufficient liquidity".to_string()),
            });
        }

        let yes_token = request
            .poly_yes_token
            .as_deref()
            .unwrap_or("UNKNOWN-POLY-YES-TOKEN");
        let no_token = request
            .poly_no_token
            .as_deref()
            .unwrap_or("UNKNOWN-POLY-NO-TOKEN");

        // Determine execution based on arb type
        let (yes_filled, no_filled, yes_cost, no_cost) = match request.arb_type {
            ArbType::PolyOnly => {
                // Execute both legs on Polymarket
                let yes_price = Self::cents_to_price(request.yes_price);
                let no_price = Self::cents_to_price(request.no_price);

                let yes_result = self
                    .execute_polymarket_order(&yes_token, "yes", yes_price, max_contracts, dry_run)
                    .await?;
                let no_result = self
                    .execute_polymarket_order(&no_token, "no", no_price, max_contracts, dry_run)
                    .await?;

                (yes_result.0 as i64, no_result.0 as i64, (yes_result.1 * 100.0) as i64, (no_result.1 * 100.0) as i64)
            }
            ArbType::PolyYesKalshiNo => {
                // Only execute YES leg on Polymarket
                let yes_price = Self::cents_to_price(request.yes_price);
                let yes_result = self
                    .execute_polymarket_order(&yes_token, "yes", yes_price, max_contracts, dry_run)
                    .await?;
                (yes_result.0 as i64, 0, (yes_result.1 * 100.0) as i64, 0)
            }
            ArbType::KalshiYesPolyNo => {
                // Only execute NO leg on Polymarket
                let no_price = Self::cents_to_price(request.no_price);
                let no_result = self
                    .execute_polymarket_order(&no_token, "no", no_price, max_contracts, dry_run)
                    .await?;
                (0, no_result.0 as i64, 0, (no_result.1 * 100.0) as i64)
            }
            ArbType::KalshiOnly => {
                return Ok(ExecutionResult {
                    market_id,
                    success: false,
                    profit_cents: 0,
                    latency_ns: start.elapsed().as_nanos() as u64,
                    error: Some("Invalid arb type for Polymarket engine".to_string()),
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
                "[POLY] ✅ Executed {} contracts | profit={}¢ | latency={}µs",
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
        Platform::Polymarket
    }
}
