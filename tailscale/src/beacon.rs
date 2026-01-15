//! UDP beacon for controller/trader discovery.
//!
//! Controller broadcasts its WebSocket port to all Tailscale peers.
//! Trader listens and discovers the controller automatically.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Beacon payload sent by controller
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeaconPayload {
    pub role: String,
    pub ws_port: u16,
    pub version: String,
}

impl BeaconPayload {
    pub fn controller(ws_port: u16) -> Self {
        Self {
            role: "controller".to_string(),
            ws_port,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Information about discovered controller
#[derive(Debug, Clone)]
pub struct ControllerInfo {
    pub ip: Ipv4Addr,
    pub ws_port: u16,
    pub version: String,
}

/// Sends beacons to all Tailscale peers (controller side)
pub struct BeaconSender {
    socket: UdpSocket,
    peers: Vec<Ipv4Addr>,
    beacon_port: u16,
    payload: Vec<u8>,
    interval: Duration,
}

impl BeaconSender {
    /// Create a new beacon sender
    pub async fn new(
        peers: Vec<Ipv4Addr>,
        beacon_port: u16,
        ws_port: u16,
    ) -> Result<Self> {
        // Bind to any available port for sending
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket for beacon sender")?;

        let payload = BeaconPayload::controller(ws_port);
        let payload_bytes = serde_json::to_vec(&payload)
            .context("Failed to serialize beacon payload")?;

        Ok(Self {
            socket,
            peers,
            beacon_port,
            payload: payload_bytes,
            interval: Duration::from_secs(2),
        })
    }

    /// Run the beacon sender until cancelled
    pub async fn run(&self, cancel: CancellationToken) {
        info!(
            "[BEACON] Starting beacon sender to {} peers on port {}",
            self.peers.len(),
            self.beacon_port
        );

        let mut interval = tokio::time::interval(self.interval);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("[BEACON] Beacon sender stopped");
                    return;
                }
                _ = interval.tick() => {
                    for peer in &self.peers {
                        let addr = SocketAddr::V4(SocketAddrV4::new(*peer, self.beacon_port));
                        match self.socket.send_to(&self.payload, addr).await {
                            Ok(_) => debug!("[BEACON] Sent beacon to {}", addr),
                            Err(e) => warn!("[BEACON] Failed to send to {}: {}", addr, e),
                        }
                    }
                }
            }
        }
    }
}

/// Listens for controller beacons (trader side)
pub struct BeaconListener {
    pub(crate) socket: UdpSocket,
}

impl BeaconListener {
    /// Create a new beacon listener
    pub async fn new(port: u16) -> Result<Self> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        let socket = UdpSocket::bind(addr)
            .await
            .with_context(|| format!("Failed to bind UDP socket on port {}", port))?;

        info!("[BEACON] Listening for controller beacon on port {}", port);

        Ok(Self { socket })
    }

    /// Get the local address the listener is bound to
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Wait for a controller beacon and return its info
    pub async fn wait_for_controller(&self) -> Result<ControllerInfo> {
        let mut buf = [0u8; 1024];

        loop {
            let (len, addr) = self
                .socket
                .recv_from(&mut buf)
                .await
                .context("Failed to receive beacon")?;

            let payload: BeaconPayload = match serde_json::from_slice(&buf[..len]) {
                Ok(p) => p,
                Err(e) => {
                    warn!("[BEACON] Invalid beacon from {}: {}", addr, e);
                    continue;
                }
            };

            if payload.role != "controller" {
                debug!("[BEACON] Ignoring non-controller beacon from {}", addr);
                continue;
            }

            let ip = match addr {
                SocketAddr::V4(v4) => *v4.ip(),
                SocketAddr::V6(_) => {
                    warn!("[BEACON] Ignoring IPv6 beacon from {}", addr);
                    continue;
                }
            };

            info!(
                "[BEACON] Discovered controller at {}:{} (version {})",
                ip, payload.ws_port, payload.version
            );

            return Ok(ControllerInfo {
                ip,
                ws_port: payload.ws_port,
                version: payload.version,
            });
        }
    }

    /// Wait for controller with timeout
    pub async fn wait_for_controller_timeout(
        &self,
        timeout: Duration,
    ) -> Result<ControllerInfo> {
        tokio::time::timeout(timeout, self.wait_for_controller())
            .await
            .context("Timeout waiting for controller beacon")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_beacon_payload_serialization() {
        let payload = BeaconPayload::controller(9001);
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"role\":\"controller\""));
        assert!(json.contains("\"ws_port\":9001"));
    }

    #[tokio::test]
    async fn test_beacon_sender_receiver() {
        // Create listener on ephemeral port
        let listener = BeaconListener::new(0).await.unwrap();
        let port = listener.socket.local_addr().unwrap().port();

        // Create sender targeting localhost
        let sender = BeaconSender::new(
            vec![Ipv4Addr::LOCALHOST],
            port,
            9001,
        )
        .await
        .unwrap();

        // Run sender in background
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let sender_handle = tokio::spawn(async move {
            sender.run(cancel_clone).await;
        });

        // Wait for beacon
        let info = tokio::time::timeout(
            Duration::from_secs(5),
            listener.wait_for_controller(),
        )
        .await
        .expect("Timeout waiting for beacon")
        .expect("Failed to receive beacon");

        assert_eq!(info.ip, Ipv4Addr::LOCALHOST);
        assert_eq!(info.ws_port, 9001);

        // Cleanup
        cancel.cancel();
        let _ = sender_handle.await;
    }
}
