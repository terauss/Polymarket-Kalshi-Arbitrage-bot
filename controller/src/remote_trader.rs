//! WebSocket server hosted by the controller.
//!
//! The remote trader connects as a *client* (so this works across NAT / "controller is reachable"),
//! then the controller sends `init` and `execute` messages.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{info, warn};

use crate::remote_protocol::{IncomingMessage, OutgoingMessage, Platform};

#[derive(Clone)]
pub struct RemoteTraderHandle {
    outgoing: Arc<RwLock<Option<mpsc::UnboundedSender<IncomingMessage>>>>,
}

impl RemoteTraderHandle {
    pub async fn is_connected(&self) -> bool {
        self.outgoing.read().await.is_some()
    }

    /// Send a message to the currently connected trader (if any).
    pub fn try_send(&self, msg: IncomingMessage) -> bool {
        if let Some(tx) = self.outgoing.blocking_read().as_ref() {
            tx.send(msg).is_ok()
        } else {
            false
        }
    }

    pub async fn send(&self, msg: IncomingMessage) -> Result<()> {
        let tx = self
            .outgoing
            .read()
            .await
            .as_ref()
            .cloned()
            .context("no trader connected")?;
        tx.send(msg).map_err(|_| anyhow::anyhow!("trader channel closed"))?;
        Ok(())
    }
}

pub struct RemoteTraderServer {
    bind: SocketAddr,
    platforms: Vec<Platform>,
    dry_run: bool,
    handle: RemoteTraderHandle,
}

impl RemoteTraderServer {
    pub fn new(bind: SocketAddr, platforms: Vec<Platform>, dry_run: bool) -> Self {
        Self {
            bind,
            platforms,
            dry_run,
            handle: RemoteTraderHandle {
                outgoing: Arc::new(RwLock::new(None)),
            },
        }
    }

    pub fn handle(&self) -> RemoteTraderHandle {
        self.handle.clone()
    }

    pub async fn run(self) -> Result<()> {
        let listener = TcpListener::bind(self.bind)
            .await
            .with_context(|| format!("bind remote trader ws server on {}", self.bind))?;
        info!(
            "[REMOTE] WebSocket server listening on {} (dry_run={})",
            self.bind, self.dry_run
        );

        loop {
            let (stream, peer) = listener.accept().await.context("accept trader tcp")?;
            info!("[REMOTE] Trader connected from {}", peer);

            let ws = accept_async(stream).await.context("accept trader websocket")?;
            let (mut write, mut read) = ws.split();

            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<IncomingMessage>();
            {
                let mut guard = self.handle.outgoing.write().await;
                *guard = Some(out_tx.clone());
            }

            // Immediately init the trader.
            let init = IncomingMessage::Init {
                platforms: self.platforms.clone(),
                dry_run: self.dry_run,
            };
            let _ = out_tx.send(init);

            // Writer task
            let writer = tokio::spawn(async move {
                while let Some(msg) = out_rx.recv().await {
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if let Err(e) = write.send(Message::Text(json)).await {
                                return Err(anyhow::anyhow!("ws send failed: {}", e));
                            }
                        }
                        Err(e) => warn!("[REMOTE] serialize error: {}", e),
                    }
                }
                Ok::<(), anyhow::Error>(())
            });

            // Reader task
            let out_tx_for_pong = out_tx.clone();
            let reader = tokio::spawn(async move {
                while let Some(frame) = read.next().await {
                    match frame {
                        Ok(Message::Text(text)) => match serde_json::from_str::<OutgoingMessage>(&text)
                        {
                            Ok(OutgoingMessage::Ping { timestamp }) => {
                                // Trader heartbeat ping → respond with pong so trader stays healthy.
                                let _ = out_tx_for_pong.send(IncomingMessage::Pong { timestamp });
                            }
                            Ok(OutgoingMessage::InitAck { success, error, .. }) => {
                                if success {
                                    info!("[REMOTE] Trader init_ack: success");
                                } else {
                                    warn!(
                                        "[REMOTE] Trader init_ack: failure: {}",
                                        error.unwrap_or_else(|| "unknown".to_string())
                                    );
                                }
                            }
                            Ok(OutgoingMessage::ExecutionResult {
                                market_id,
                                success,
                                profit_cents,
                                latency_ns,
                                error,
                            }) => {
                                if success {
                                    info!(
                                        "[REMOTE] ✅ trader execution_result market_id={} profit={}¢ latency={}µs",
                                        market_id,
                                        profit_cents,
                                        latency_ns / 1000
                                    );
                                } else {
                                    warn!(
                                        "[REMOTE] ❌ trader execution_result market_id={} err={}",
                                        market_id,
                                        error.unwrap_or_else(|| "unknown".to_string())
                                    );
                                }
                            }
                            Ok(other) => {
                                info!("[REMOTE] trader msg: {:?}", other);
                            }
                            Err(e) => warn!("[REMOTE] parse error: {} (text={})", e, text),
                        },
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(e) => {
                            return Err(anyhow::anyhow!("ws read failed: {}", e));
                        }
                    }
                }
                Ok::<(), anyhow::Error>(())
            });

            // Wait until either side ends, then clear connection state.
            let _ = tokio::select! {
                r = reader => r,
                w = writer => w,
            };

            {
                let mut guard = self.handle.outgoing.write().await;
                *guard = None;
            }
            warn!("[REMOTE] Trader disconnected; waiting for reconnect...");
        }
    }
}

