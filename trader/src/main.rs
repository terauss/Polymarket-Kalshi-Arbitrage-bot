//! Remote trading bot - receives trading instructions via WebSocket

mod api;
mod config;
mod execution;
mod heartbeat;
mod paths;
mod protocol;
mod trader;
mod websocket;

use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{error, info, warn};

use config::Config;
use protocol::IncomingMessage;
use trader::Trader;
use websocket::WebSocketClient;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load environment variables from `.env` (supports workspace-root `.env`)
    paths::load_dotenv();

    info!("[MAIN] Starting remote trader...");
    let one_shot = std::env::var("ONE_SHOT")
        .map(|v| v == "1" || v == "true" || v == "yes")
        .unwrap_or(false);

    // Load configuration
    let config = Arc::new(Config::from_env().context("Failed to load configuration")?);
    info!("[MAIN] Configuration loaded (dry_run={})", config.dry_run);

    // Create trader
    let mut trader = Trader::new(config.clone());

    // Create WebSocket client
    let mut ws_client = WebSocketClient::new(config.websocket_url.clone());

    // Main event loop with reconnection logic
    loop {
        match ws_client.connect().await {
            Ok((outgoing_tx, mut incoming_rx)) => {
                info!("[MAIN] WebSocket connected, starting main loop");

                // Start heartbeat manager
                let heartbeat = Arc::new(heartbeat::HeartbeatManager::new(outgoing_tx.clone(), 5, 15));
                let heartbeat_handle = heartbeat.start();

                // Main message processing loop
                let mut connection_healthy = true;
                while connection_healthy {
                    tokio::select! {
                        // Handle incoming messages
                        msg = incoming_rx.recv() => {
                            match msg {
                                Some(IncomingMessage::Ping { timestamp }) => {
                                    // Host sent ping, respond with pong and update heartbeat
                                    heartbeat.record_pong();
                                    let response = trader.handle_message(IncomingMessage::Ping { timestamp }).await;
                                    if outgoing_tx.send(response).is_err() {
                                        warn!("[MAIN] Failed to send pong - connection may be closed");
                                        connection_healthy = false;
                                    }
                                }
                                Some(IncomingMessage::Pong { timestamp }) => {
                                    // Host responded to our ping
                                    heartbeat.record_pong();
                                }
                                Some(msg) => {
                                    let is_execute = matches!(msg, IncomingMessage::Execute { .. });
                                    let response = trader.handle_message(msg).await;
                                    if outgoing_tx.send(response).is_err() {
                                        warn!("[MAIN] Failed to send response - connection may be closed");
                                        connection_healthy = false;
                                    }
                                    if one_shot && is_execute {
                                        info!("[MAIN] ONE_SHOT enabled; exiting after first execute");
                                        return Ok(());
                                    }
                                }
                                None => {
                                    warn!("[MAIN] Incoming channel closed");
                                    connection_healthy = false;
                                }
                            }
                        }
                        // Check for heartbeat timeout
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                            if heartbeat.is_timed_out() {
                                warn!("[MAIN] Heartbeat timeout detected");
                                connection_healthy = false;
                            }
                        }
                    }
                }

                // Stop heartbeat
                heartbeat_handle.abort();
                info!("[MAIN] Connection lost, will attempt to reconnect...");
            }
            Err(e) => {
                error!("[MAIN] Failed to connect: {}", e);
                warn!("[MAIN] Retrying in 5 seconds...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}
