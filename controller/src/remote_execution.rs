//! Execution path that sends trades to a remote trader over WebSocket.

use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::circuit_breaker::CircuitBreaker;
use crate::remote_protocol::{ArbType as WsArbType, IncomingMessage, Platform as WsPlatform};
use crate::remote_trader::RemoteTraderHandle;
use crate::types::{ArbType, FastExecutionRequest, GlobalState};

pub struct RemoteExecutor {
    state: Arc<GlobalState>,
    circuit_breaker: Arc<CircuitBreaker>,
    trader: RemoteTraderHandle,
    in_flight: Arc<[AtomicU64; 8]>,
    pub dry_run: bool,
}

impl RemoteExecutor {
    pub fn new(
        state: Arc<GlobalState>,
        circuit_breaker: Arc<CircuitBreaker>,
        trader: RemoteTraderHandle,
        dry_run: bool,
    ) -> Self {
        Self {
            state,
            circuit_breaker,
            trader,
            in_flight: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            dry_run,
        }
    }

    fn release_in_flight_delayed(&self, market_id: u16) {
        if market_id < 512 {
            let in_flight = self.in_flight.clone();
            let slot = (market_id / 64) as usize;
            let bit = market_id % 64;
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                let mask = !(1u64 << bit);
                in_flight[slot].fetch_and(mask, Ordering::Release);
            });
        }
    }

    pub async fn process(&self, req: FastExecutionRequest) -> Result<()> {
        let market_id = req.market_id;

        // Deduplication check (512 markets via 8x u64 bitmask)
        if market_id < 512 {
            let slot = (market_id / 64) as usize;
            let bit = market_id % 64;
            let mask = 1u64 << bit;
            let prev = self.in_flight[slot].fetch_or(mask, Ordering::AcqRel);
            if prev & mask != 0 {
                return Ok(());
            }
        }

        let market = self
            .state
            .get_by_id(market_id)
            .ok_or_else(|| anyhow!("Unknown market_id {}", market_id))?;
        let pair = market
            .pair()
            .ok_or_else(|| anyhow!("No pair for market_id {}", market_id))?;

        let profit_cents = req.profit_cents();
        if profit_cents < 1 {
            self.release_in_flight_delayed(market_id);
            return Ok(());
        }

        let max_contracts = (req.yes_size.min(req.no_size) / 100) as i64;
        if max_contracts < 1 {
            self.release_in_flight_delayed(market_id);
            return Ok(());
        }

        // Circuit breaker check
        if let Err(_reason) = self
            .circuit_breaker
            .can_execute(&pair.pair_id, max_contracts)
            .await
        {
            self.release_in_flight_delayed(market_id);
            return Ok(());
        }

        info!(
            "[REMOTE_EXEC] 🎯 {} | {:?} y={}¢ n={}¢ | profit={}¢ | {}x",
            pair.description, req.arb_type, req.yes_price, req.no_price, profit_cents, max_contracts
        );

        if self.dry_run {
            info!(
                "[REMOTE_EXEC] 🏃 DRY RUN - sending execute to trader (no real orders)"
            );
        }

        let msg = IncomingMessage::Execute {
            market_id,
            arb_type: to_ws_arb(req.arb_type),
            yes_price: req.yes_price,
            no_price: req.no_price,
            yes_size: req.yes_size,
            no_size: req.no_size,
            pair_id: Some(pair.pair_id.to_string()),
            description: Some(pair.description.to_string()),
            kalshi_market_ticker: Some(pair.kalshi_market_ticker.to_string()),
            poly_yes_token: Some(pair.poly_yes_token.to_string()),
            poly_no_token: Some(pair.poly_no_token.to_string()),
        };

        if !self.trader.try_send(msg) {
            warn!("[REMOTE_EXEC] No trader connected; dropping execute");
        }

        self.release_in_flight_delayed(market_id);
        Ok(())
    }
}

fn to_ws_arb(a: ArbType) -> WsArbType {
    match a {
        ArbType::PolyYesKalshiNo => WsArbType::PolyYesKalshiNo,
        ArbType::KalshiYesPolyNo => WsArbType::KalshiYesPolyNo,
        ArbType::PolyOnly => WsArbType::PolyOnly,
        ArbType::KalshiOnly => WsArbType::KalshiOnly,
    }
}

#[allow(dead_code)]
fn to_ws_platforms(platforms: &[crate::types::Platform]) -> Vec<WsPlatform> {
    platforms
        .iter()
        .map(|p| match p {
            crate::types::Platform::Kalshi => WsPlatform::Kalshi,
            crate::types::Platform::Polymarket => WsPlatform::Polymarket,
        })
        .collect()
}

/// Remote execution event loop - forwards arbitrage opportunities to the remote trader.
pub async fn run_remote_execution_loop(
    mut rx: mpsc::Receiver<FastExecutionRequest>,
    executor: Arc<RemoteExecutor>,
) {
    info!(
        "[REMOTE_EXEC] Remote execution loop started (dry_run={})",
        executor.dry_run
    );

    while let Some(req) = rx.recv().await {
        let exec = executor.clone();
        tokio::spawn(async move {
            if let Err(e) = exec.process(req).await {
                error!("[REMOTE_EXEC] error: {}", e);
            }
        });
    }
}

