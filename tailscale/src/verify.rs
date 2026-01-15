//! Tailscale daemon verification and status querying.

use std::net::Ipv4Addr;
use std::process::Command;
use thiserror::Error;
use tracing::{debug, warn};

#[derive(Error, Debug)]
pub enum TailscaleError {
    #[error("Tailscale not installed - run: brew install tailscale")]
    NotInstalled,
    #[error("Tailscale daemon not running - run: sudo tailscaled")]
    DaemonNotRunning,
    #[error("Tailscale not connected - run: tailscale up")]
    NotConnected,
    #[error("No Tailscale peers found - ensure other machine has joined your Tailnet")]
    NoPeers,
    #[error("Tailscale command failed: {0}")]
    CommandFailed(String),
    #[error("Failed to parse Tailscale status: {0}")]
    ParseError(String),
}

#[derive(Debug, Clone)]
pub struct TailscaleStatus {
    pub self_ip: Ipv4Addr,
    pub peers: Vec<Ipv4Addr>,
}

/// Query Tailscale status and return connection info.
pub fn verify() -> Result<TailscaleStatus, TailscaleError> {
    // Run tailscale status --json
    let output = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TailscaleError::NotInstalled
            } else {
                TailscaleError::CommandFailed(e.to_string())
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not running") || stderr.contains("stopped") {
            return Err(TailscaleError::DaemonNotRunning);
        }
        return Err(TailscaleError::CommandFailed(stderr.to_string()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| TailscaleError::ParseError(e.to_string()))?;

    // Check backend state
    let backend_state = json
        .get("BackendState")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    debug!("Tailscale BackendState: {}", backend_state);

    if backend_state != "Running" {
        return Err(TailscaleError::NotConnected);
    }

    // Extract self IP
    let self_ip = json
        .get("Self")
        .and_then(|s| s.get("TailscaleIPs"))
        .and_then(|ips| ips.as_array())
        .and_then(|arr| arr.first())
        .and_then(|ip| ip.as_str())
        .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
        .ok_or_else(|| TailscaleError::ParseError("Could not find self IP".to_string()))?;

    // Extract peer IPs (only online peers)
    let mut peers = Vec::new();
    if let Some(peer_map) = json.get("Peer").and_then(|p| p.as_object()) {
        for (_key, peer) in peer_map {
            let online = peer.get("Online").and_then(|o| o.as_bool()).unwrap_or(false);
            if !online {
                continue;
            }

            if let Some(ip) = peer
                .get("TailscaleIPs")
                .and_then(|ips| ips.as_array())
                .and_then(|arr| arr.first())
                .and_then(|ip| ip.as_str())
                .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
            {
                peers.push(ip);
            }
        }
    }

    if peers.is_empty() {
        warn!("No online Tailscale peers found");
    }

    Ok(TailscaleStatus { self_ip, peers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_returns_result() {
        // This test just ensures the function runs without panic
        // Actual Tailscale availability depends on the machine
        let result = verify();
        // Either succeeds or returns a typed error
        match result {
            Ok(status) => {
                assert!(!status.self_ip.is_unspecified());
            }
            Err(e) => {
                // Any TailscaleError is acceptable in test environment
                println!("Expected error in test env: {}", e);
            }
        }
    }
}
