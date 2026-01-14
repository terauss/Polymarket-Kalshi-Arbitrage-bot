//! Dry-run execution engine (no real API calls).

use anyhow::Result;
use async_trait::async_trait;
use std::time::Instant;
use tracing::info;

use crate::execution::{ExecutionEngine, ExecutionResult, OrderRequest};
use crate::protocol::Platform;

pub struct DryRunEngine {
    platform: Platform,
}

impl DryRunEngine {
    pub fn new(platform: Platform) -> Self {
        Self { platform }
    }
}

#[async_trait]
impl ExecutionEngine for DryRunEngine {
    async fn execute_order(&self, request: &OrderRequest, _dry_run: bool) -> Result<ExecutionResult> {
        let start = Instant::now();

        let max_contracts = (request.yes_size.min(request.no_size) / 100) as i64;
        let profit_cents = request.profit_cents();

        info!(
            "[DRY:{}] Would execute market_id={} {:?} y={}¢ n={}¢ size=({},{}) max_contracts={} profit≈{}¢ | meta: pair_id={:?} ticker={:?}",
            match self.platform {
                Platform::Kalshi => "KALSHI",
                Platform::Polymarket => "POLY",
            },
            request.market_id,
            request.arb_type,
            request.yes_price,
            request.no_price,
            request.yes_size,
            request.no_size,
            max_contracts,
            profit_cents,
            request.pair_id,
            request.kalshi_market_ticker
        );

        Ok(ExecutionResult {
            market_id: request.market_id,
            success: true,
            profit_cents,
            latency_ns: start.elapsed().as_nanos() as u64,
            error: Some("DRY_RUN".to_string()),
        })
    }

    async fn can_execute(&self, _market_id: u16, _size: i64) -> Result<()> {
        Ok(())
    }

    fn platform(&self) -> Platform {
        self.platform
    }
}

