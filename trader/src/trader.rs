//! Main trader logic and state machine

use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::execution::{create_engines, ExecutionEngine, OrderRequest};
use crate::protocol::{ArbType, IncomingMessage, OutgoingMessage, Platform};

/// Trader state
pub enum TraderState {
    Uninitialized,
    Initialized {
        platforms: Vec<Platform>,
        engines: Vec<Box<dyn ExecutionEngine>>,
        dry_run: bool,
    },
    Error {
        message: String,
    },
}

/// Main trader that processes commands and manages execution
pub struct Trader {
    state: TraderState,
    config: Arc<Config>,
    start_time: Instant,
}

impl Trader {
    /// Create a new trader
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            state: TraderState::Uninitialized,
            config,
            start_time: Instant::now(),
        }
    }

    /// Handle an incoming message and return response
    pub async fn handle_message(&mut self, msg: IncomingMessage) -> OutgoingMessage {
        match msg {
            IncomingMessage::Init { platforms, dry_run } => {
                self.handle_init(platforms, dry_run).await
            }
            IncomingMessage::Execute {
                market_id,
                arb_type,
                yes_price,
                no_price,
                yes_size,
                no_size,
                pair_id,
                description,
                kalshi_market_ticker,
                poly_yes_token,
                poly_no_token,
            } => {
                self.handle_execute(
                    market_id,
                    arb_type,
                    yes_price,
                    no_price,
                    yes_size,
                    no_size,
                    pair_id,
                    description,
                    kalshi_market_ticker,
                    poly_yes_token,
                    poly_no_token,
                )
                .await
            }
            IncomingMessage::Ping { timestamp } => self.handle_ping(timestamp),
            IncomingMessage::Pong { timestamp } => OutgoingMessage::Pong { timestamp },
            IncomingMessage::Status => self.handle_status(),
        }
    }

    /// Handle initialization request
    async fn handle_init(
        &mut self,
        platforms: Vec<Platform>,
        dry_run: bool,
    ) -> OutgoingMessage {
        info!("[TRADER] Received init request: platforms={:?}, dry_run={}", platforms, dry_run);

        // In dry-run mode, do not require credentials. Use local dry-run engines.
        let effective_dry_run = dry_run || self.config.dry_run;

        let engines_result = if effective_dry_run {
            Ok(platforms
                .iter()
                .map(|p| Box::new(crate::execution::dry_run::DryRunEngine::new(*p)) as Box<dyn ExecutionEngine>)
                .collect())
        } else {
            create_engines(&platforms, &self.config).await
        };

        match engines_result {
            Ok(engines) => {
                let platform_list: Vec<Platform> = engines.iter().map(|e| e.platform()).collect();
                self.state = TraderState::Initialized {
                    platforms: platform_list.clone(),
                    engines,
                    dry_run: effective_dry_run,
                };
                info!("[TRADER] Initialized successfully with platforms: {:?}", platform_list);
                OutgoingMessage::InitAck {
                    success: true,
                    platforms: platform_list,
                    error: None,
                }
            }
            Err(e) => {
                error!("[TRADER] Initialization failed: {}", e);
                self.state = TraderState::Error {
                    message: e.to_string(),
                };
                OutgoingMessage::InitAck {
                    success: false,
                    platforms: vec![],
                    error: Some(e.to_string()),
                }
            }
        }
    }

    /// Handle order execution request
    async fn handle_execute(
        &mut self,
        market_id: u16,
        arb_type: ArbType,
        yes_price: u16,
        no_price: u16,
        yes_size: u16,
        no_size: u16,
        pair_id: Option<String>,
        description: Option<String>,
        kalshi_market_ticker: Option<String>,
        poly_yes_token: Option<String>,
        poly_no_token: Option<String>,
    ) -> OutgoingMessage {
        let detected_ns = self.start_time.elapsed().as_nanos() as u64;

        match &self.state {
            TraderState::Uninitialized => {
                warn!("[TRADER] Execute request received but trader not initialized");
                OutgoingMessage::ExecutionResult {
                    market_id,
                    success: false,
                    profit_cents: 0,
                    latency_ns: detected_ns,
                    error: Some("Trader not initialized".to_string()),
                }
            }
            TraderState::Error { message } => {
                warn!("[TRADER] Execute request received but trader in error state: {}", message);
                OutgoingMessage::ExecutionResult {
                    market_id,
                    success: false,
                    profit_cents: 0,
                    latency_ns: detected_ns,
                    error: Some(format!("Trader error: {}", message)),
                }
            }
            TraderState::Initialized {
                engines,
                dry_run,
                ..
            } => {
                let request = OrderRequest {
                    market_id,
                    arb_type,
                    yes_price,
                    no_price,
                    yes_size,
                    no_size,
                    pair_id,
                    description,
                    kalshi_market_ticker,
                    poly_yes_token,
                    poly_no_token,
                };

                // Determine which engine(s) to use based on arb_type
                let relevant_engines: Vec<&Box<dyn ExecutionEngine>> = engines
                    .iter()
                    .filter(|e| match arb_type {
                        ArbType::PolyYesKalshiNo => {
                            e.platform() == Platform::Polymarket || e.platform() == Platform::Kalshi
                        }
                        ArbType::KalshiYesPolyNo => {
                            e.platform() == Platform::Kalshi || e.platform() == Platform::Polymarket
                        }
                        ArbType::PolyOnly => e.platform() == Platform::Polymarket,
                        ArbType::KalshiOnly => e.platform() == Platform::Kalshi,
                    })
                    .collect();

                if relevant_engines.is_empty() {
                    warn!(
                        "[TRADER] No suitable engine for arb_type {:?} with available platforms",
                        arb_type
                    );
                    return OutgoingMessage::ExecutionResult {
                        market_id,
                        success: false,
                        profit_cents: 0,
                        latency_ns: detected_ns,
                        error: Some("No suitable execution engine".to_string()),
                    };
                }

                // Execute on all relevant engines
                // For cross-platform arbs, we need to coordinate execution
                let mut results = Vec::new();
                for engine in relevant_engines {
                    match engine.execute_order(&request, *dry_run).await {
                        Ok(result) => results.push(result),
                        Err(e) => {
                            error!("[TRADER] Execution error: {}", e);
                            return OutgoingMessage::ExecutionResult {
                                market_id,
                                success: false,
                                profit_cents: 0,
                                latency_ns: detected_ns,
                                error: Some(e.to_string()),
                            };
                        }
                    }
                }

                // Combine results (for now, use the first result)
                // In a full implementation, we'd combine cross-platform results
                if let Some(result) = results.first() {
                    OutgoingMessage::ExecutionResult {
                        market_id: result.market_id,
                        success: result.success,
                        profit_cents: result.profit_cents,
                        latency_ns: result.latency_ns,
                        error: result.error.clone(),
                    }
                } else {
                    OutgoingMessage::ExecutionResult {
                        market_id,
                        success: false,
                        profit_cents: 0,
                        latency_ns: detected_ns,
                        error: Some("No execution results".to_string()),
                    }
                }
            }
        }
    }

    /// Handle ping message
    fn handle_ping(&self, timestamp: u64) -> OutgoingMessage {
        OutgoingMessage::Pong { timestamp }
    }

    /// Handle status request
    fn handle_status(&self) -> OutgoingMessage {
        match &self.state {
            TraderState::Uninitialized => OutgoingMessage::Status {
                connected: true,
                platforms: vec![],
                dry_run: self.config.dry_run,
            },
            TraderState::Initialized {
                platforms,
                dry_run,
                ..
            } => OutgoingMessage::Status {
                connected: true,
                platforms: platforms.clone(),
                dry_run: *dry_run,
            },
            TraderState::Error { .. } => OutgoingMessage::Status {
                connected: true,
                platforms: vec![],
                dry_run: self.config.dry_run,
            },
        }
    }

    /// Get current state
    pub fn state(&self) -> &TraderState {
        &self.state
    }
}
