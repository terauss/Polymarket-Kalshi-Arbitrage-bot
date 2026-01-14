//! Execution engine for order placement on trading platforms

use anyhow::Result;
use async_trait::async_trait;

use crate::protocol::{ArbType, Platform};

/// Result of an order execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub market_id: u16,
    pub success: bool,
    pub profit_cents: i16,
    pub latency_ns: u64,
    pub error: Option<String>,
}

/// Order execution request
#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub market_id: u16,
    pub arb_type: ArbType,
    pub yes_price: u16,
    pub no_price: u16,
    pub yes_size: u16,
    pub no_size: u16,

    // Optional metadata provided by controller.
    pub pair_id: Option<String>,
    pub description: Option<String>,
    pub kalshi_market_ticker: Option<String>,
    pub poly_yes_token: Option<String>,
    pub poly_no_token: Option<String>,
}

impl OrderRequest {
    /// Calculate profit in cents
    pub fn profit_cents(&self) -> i16 {
        // These requests are expressed primarily in "cents per contract" (price) and
        // "cents notional available" (size). For safety + logging we compute a
        // per-contract approximate profit that cannot overflow.
        let profit = 100i64 - (self.yes_price as i64 + self.no_price as i64);
        profit.clamp(i16::MIN as i64, i16::MAX as i64) as i16
    }
}

/// Trait for platform-specific execution engines
#[async_trait]
pub trait ExecutionEngine: Send + Sync {
    /// Execute an order based on the request
    async fn execute_order(
        &self,
        request: &OrderRequest,
        dry_run: bool,
    ) -> Result<ExecutionResult>;

    /// Check if the engine can execute (circuit breaker, etc.)
    async fn can_execute(&self, market_id: u16, size: i64) -> Result<()>;

    /// Get the platform this engine handles
    fn platform(&self) -> Platform;
}

/// Create execution engines for the specified platforms
pub async fn create_engines(
    platforms: &[Platform],
    config: &crate::config::Config,
) -> Result<Vec<Box<dyn ExecutionEngine>>> {
    let mut engines: Vec<Box<dyn ExecutionEngine>> = Vec::new();

    for platform in platforms {
        match platform {
            Platform::Kalshi => {
                if config.has_kalshi_creds() {
                    engines.push(Box::new(
                        crate::execution::kalshi::KalshiEngine::new(config)?,
                    ));
                } else {
                    anyhow::bail!("Kalshi credentials not available");
                }
            }
            Platform::Polymarket => {
                if config.has_polymarket_creds() {
                    engines.push(Box::new(
                        crate::execution::polymarket::PolymarketEngine::new(config).await?,
                    ));
                } else {
                    anyhow::bail!("Polymarket credentials not available");
                }
            }
        }
    }

    Ok(engines)
}

pub mod dry_run;
pub mod kalshi;
pub mod polymarket;
