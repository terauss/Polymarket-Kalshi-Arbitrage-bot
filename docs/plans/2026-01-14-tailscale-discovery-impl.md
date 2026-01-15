# Tailscale Auto-Discovery Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable automatic peer discovery between controller and trader over Tailscale VPN with zero manual IP configuration.

**Architecture:** New `tailscale` library crate provides verification and UDP beacon protocol. New `bootstrap` CLI handles initial setup. Controller sends beacons to all Tailscale peers; trader listens and auto-connects.

**Tech Stack:** Rust, tokio (async UDP/process), serde_json (beacon payload), toml (config), directories (cross-platform paths)

---

## Task 1: Create tailscale library crate structure

**Files:**
- Create: `tailscale/Cargo.toml`
- Create: `tailscale/src/lib.rs`
- Modify: `Cargo.toml` (workspace)

**Step 1: Create tailscale/Cargo.toml**

```toml
[package]
name = "tailscale"
version = "0.1.0"
edition = "2021"
description = "Tailscale verification and peer discovery"

[dependencies]
anyhow = "1.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "1.0"
tokio = { version = "1.0", features = ["process", "net", "time", "sync"] }
tracing = "0.1"
```

**Step 2: Create tailscale/src/lib.rs**

```rust
//! Tailscale verification and peer discovery for controller/trader communication.

pub mod beacon;
pub mod config;
pub mod verify;

pub use beacon::{BeaconListener, BeaconSender, ControllerInfo};
pub use config::Config;
pub use verify::{TailscaleError, TailscaleStatus};
```

**Step 3: Add tailscale to workspace**

In `Cargo.toml`, add `"tailscale"` to the `members` array:

```toml
[workspace]
resolver = "2"
members = [
  "controller",
  "trader",
  "tailscale",
]
```

**Step 4: Verify crate compiles**

Run: `cargo check -p tailscale`
Expected: Errors about missing modules (beacon, config, verify) - that's expected, we'll add them next.

**Step 5: Commit**

```bash
git add tailscale/ Cargo.toml
git commit -m "feat(tailscale): add crate structure"
```

---

## Task 2: Implement TailscaleStatus and verify module

**Files:**
- Create: `tailscale/src/verify.rs`

**Step 1: Write the verify module**

```rust
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
```

**Step 2: Run test to verify module compiles**

Run: `cargo test -p tailscale`
Expected: Test passes (or shows expected Tailscale error if not installed)

**Step 3: Commit**

```bash
git add tailscale/src/verify.rs
git commit -m "feat(tailscale): add verify module for status checking"
```

---

## Task 3: Implement config module

**Files:**
- Create: `tailscale/src/config.rs`

**Step 1: Write the config module**

```rust
//! Configuration file management for ~/.arb/config.toml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_DIR: &str = ".arb";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Controller,
    Trader,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub role: Role,
    #[serde(default = "default_beacon_port")]
    pub beacon_port: u16,
    #[serde(default = "default_ws_port")]
    pub ws_port: u16,
}

fn default_beacon_port() -> u16 {
    9000
}

fn default_ws_port() -> u16 {
    9001
}

impl Default for Config {
    fn default() -> Self {
        Self {
            role: Role::Trader,
            beacon_port: default_beacon_port(),
            ws_port: default_ws_port(),
        }
    }
}

impl Config {
    /// Get the config file path (~/.arb/config.toml)
    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        Ok(home.join(CONFIG_DIR).join(CONFIG_FILE))
    }

    /// Load config from ~/.arb/config.toml
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Could not read config file: {}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Could not parse config file: {}", path.display()))?;
        Ok(config)
    }

    /// Save config to ~/.arb/config.toml
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Could not create config directory: {}", parent.display()))?;
        }
        let contents = toml::to_string_pretty(self).context("Could not serialize config")?;
        fs::write(&path, contents)
            .with_context(|| format!("Could not write config file: {}", path.display()))?;
        Ok(())
    }

    /// Check if config file exists
    pub fn exists() -> bool {
        Self::path().map(|p| p.exists()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization() {
        let config = Config {
            role: Role::Controller,
            beacon_port: 9000,
            ws_port: 9001,
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("role = \"controller\""));
        assert!(toml_str.contains("beacon_port = 9000"));
        assert!(toml_str.contains("ws_port = 9001"));

        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.role, Role::Controller);
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            role = "trader"
            beacon_port = 8000
            ws_port = 8001
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.role, Role::Trader);
        assert_eq!(config.beacon_port, 8000);
        assert_eq!(config.ws_port, 8001);
    }
}
```

**Step 2: Add dependencies to tailscale/Cargo.toml**

Add `dirs` and `toml` to dependencies:

```toml
[dependencies]
anyhow = "1.0"
dirs = "5.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "1.0"
tokio = { version = "1.0", features = ["process", "net", "time", "sync"] }
toml = "0.8"
tracing = "0.1"
```

**Step 3: Run tests**

Run: `cargo test -p tailscale`
Expected: All tests pass

**Step 4: Commit**

```bash
git add tailscale/
git commit -m "feat(tailscale): add config module for ~/.arb/config.toml"
```

---

## Task 4: Implement beacon module

**Files:**
- Create: `tailscale/src/beacon.rs`

**Step 1: Write the beacon module**

```rust
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
    socket: UdpSocket,
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
```

**Step 2: Add tokio-util dependency**

Add to `tailscale/Cargo.toml`:

```toml
tokio-util = "0.7"
```

**Step 3: Run tests**

Run: `cargo test -p tailscale -- --nocapture`
Expected: All tests pass including beacon sender/receiver test

**Step 4: Commit**

```bash
git add tailscale/
git commit -m "feat(tailscale): add beacon module for UDP discovery"
```

---

## Task 5: Create bootstrap crate structure

**Files:**
- Create: `bootstrap/Cargo.toml`
- Create: `bootstrap/src/main.rs`
- Modify: `Cargo.toml` (workspace)

**Step 1: Create bootstrap/Cargo.toml**

```toml
[package]
name = "bootstrap"
version = "0.1.0"
edition = "2021"
description = "Setup CLI for arbitrage bot Tailscale configuration"

[[bin]]
name = "bootstrap"
path = "src/main.rs"

[dependencies]
anyhow = "1.0"
tailscale = { path = "../tailscale" }
```

**Step 2: Create bootstrap/src/main.rs**

```rust
//! Bootstrap CLI for arbitrage bot Tailscale setup.
//!
//! Guides user through:
//! 1. Tailscale verification
//! 2. Role selection (controller/trader)
//! 3. Config file creation
//! 4. Optionally launching the appropriate binary

use anyhow::{Context, Result};
use std::io::{self, Write};
use std::process::Command;
use tailscale::config::{Config, Role};
use tailscale::verify;

fn main() -> Result<()> {
    println!("=== Arbitrage Bot Setup ===\n");

    // Step 1: Verify Tailscale
    print!("Checking Tailscale... ");
    io::stdout().flush()?;

    let status = match verify::verify() {
        Ok(s) => {
            println!("Connected as {}", s.self_ip);
            s
        }
        Err(e) => {
            println!("FAILED");
            eprintln!("\nError: {}", e);
            eprintln!("\nPlease ensure Tailscale is installed and connected:");
            eprintln!("  1. Install: brew install tailscale");
            eprintln!("  2. Start daemon: sudo tailscaled");
            eprintln!("  3. Connect: tailscale up");
            std::process::exit(1);
        }
    };

    // Step 2: Show peers
    if status.peers.is_empty() {
        println!("\nNo peers found on your Tailnet.");
        println!("Make sure the other machine has joined your Tailnet.");
        println!("You can continue setup, but the beacon won't find anyone yet.\n");
    } else {
        println!("Found {} peer(s): {:?}\n", status.peers.len(), status.peers);
    }

    // Step 3: Prompt for role
    println!("What role is this machine?");
    println!("  1) Controller (runs arbitrage detection, hosts WebSocket server)");
    println!("  2) Trader (executes trades, connects to controller)");
    print!("\nEnter choice [1/2]: ");
    io::stdout().flush()?;

    let role = loop {
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        match input.trim() {
            "1" => break Role::Controller,
            "2" => break Role::Trader,
            _ => {
                print!("Invalid choice. Enter 1 or 2: ");
                io::stdout().flush()?;
            }
        }
    };

    // Step 4: Write config
    let config = Config {
        role: role.clone(),
        beacon_port: 9000,
        ws_port: 9001,
    };

    config.save()?;
    let config_path = Config::path()?;
    println!("\nConfig written to {}", config_path.display());

    // Step 5: Offer to launch
    let binary_name = match role {
        Role::Controller => "controller",
        Role::Trader => "remote-trader",
    };

    println!("\nStart {} now? [Y/n]: ", binary_name);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input.is_empty() || input == "y" || input == "yes" {
        println!("\nLaunching {}...\n", binary_name);

        // Try cargo run first (development), fall back to direct binary
        let status = Command::new("cargo")
            .args(["run", "-p", binary_name, "--release"])
            .status();

        match status {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(_) => {
                // Try direct binary
                let status = Command::new(binary_name).status();
                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                    Err(e) => {
                        eprintln!("Failed to launch {}: {}", binary_name, e);
                        eprintln!("Run manually: cargo run -p {} --release", binary_name);
                        std::process::exit(1);
                    }
                }
            }
        }
    } else {
        println!("\nSetup complete. Run manually:");
        println!("  cargo run -p {} --release", binary_name);
    }

    Ok(())
}
```

**Step 3: Add bootstrap to workspace**

In `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
  "controller",
  "trader",
  "tailscale",
  "bootstrap",
]
```

**Step 4: Verify bootstrap compiles**

Run: `cargo build -p bootstrap`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add bootstrap/ Cargo.toml
git commit -m "feat(bootstrap): add setup CLI for Tailscale configuration"
```

---

## Task 6: Integrate Tailscale into trader startup

**Files:**
- Modify: `trader/Cargo.toml`
- Modify: `trader/src/config.rs`
- Modify: `trader/src/main.rs`

**Step 1: Add tailscale dependency to trader**

In `trader/Cargo.toml`, add to dependencies:

```toml
tailscale = { path = "../tailscale" }
tokio-util = "0.7"
```

**Step 2: Modify trader config to support discovery mode**

Replace `trader/src/config.rs` with:

```rust
//! Configuration and credential management

use anyhow::{Context, Result};
use std::env;

/// Application configuration loaded from environment variables
#[derive(Debug, Clone)]
pub struct Config {
    pub kalshi_api_key: Option<String>,
    pub kalshi_private_key: Option<String>,
    pub polymarket_private_key: Option<String>,
    pub polymarket_api_key: Option<String>,
    pub polymarket_api_secret: Option<String>,
    pub polymarket_funder: Option<String>,
    pub dry_run: bool,
    /// WebSocket URL - None means use beacon discovery
    pub websocket_url: Option<String>,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        crate::paths::load_dotenv();
        let kalshi_api_key = env::var("KALSHI_API_KEY").ok();
        let kalshi_private_key = env::var("KALSHI_PRIVATE_KEY").ok();
        let polymarket_private_key = env::var("POLYMARKET_PRIVATE_KEY").ok();
        let polymarket_api_key = env::var("POLYMARKET_API_KEY").ok();
        let polymarket_api_secret = env::var("POLYMARKET_API_SECRET").ok();
        let polymarket_funder = env::var("POLYMARKET_FUNDER")
            .or_else(|_| env::var("POLY_FUNDER"))
            .ok();

        let dry_run = env::var("DRY_RUN")
            .map(|v| v == "1" || v == "true" || v == "yes")
            .unwrap_or(false);

        // WEBSOCKET_URL is now optional - if not set, use beacon discovery
        let websocket_url = env::var("WEBSOCKET_URL").ok();

        Ok(Self {
            kalshi_api_key,
            kalshi_private_key,
            polymarket_private_key,
            polymarket_api_key,
            polymarket_api_secret,
            polymarket_funder,
            dry_run,
            websocket_url,
        })
    }

    /// Check if Kalshi credentials are available
    pub fn has_kalshi_creds(&self) -> bool {
        self.kalshi_api_key.is_some() && self.kalshi_private_key.is_some()
    }

    /// Check if Polymarket credentials are available
    pub fn has_polymarket_creds(&self) -> bool {
        self.polymarket_private_key.is_some()
            && (self.polymarket_api_key.is_some() || self.polymarket_funder.is_some())
    }
}
```

**Step 3: Modify trader main.rs for beacon discovery**

Replace `trader/src/main.rs` with:

```rust
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
use std::time::Duration;
use tracing::{error, info, warn};

use config::Config;
use protocol::IncomingMessage;
use trader::Trader;
use websocket::WebSocketClient;

/// Discover controller via Tailscale beacon or use configured URL
async fn resolve_websocket_url(config: &Config) -> Result<String> {
    // If WEBSOCKET_URL is set, use it directly
    if let Some(url) = &config.websocket_url {
        info!("[MAIN] Using configured WEBSOCKET_URL: {}", url);
        return Ok(url.clone());
    }

    // Otherwise, use Tailscale beacon discovery
    info!("[MAIN] No WEBSOCKET_URL set, using Tailscale beacon discovery...");

    // Verify Tailscale is connected
    let status = tailscale::verify::verify()
        .context("Tailscale verification failed - run bootstrap first or set WEBSOCKET_URL")?;
    info!("[MAIN] Tailscale connected as {}", status.self_ip);

    // Load beacon config
    let ts_config = tailscale::Config::load()
        .context("Could not load ~/.arb/config.toml - run bootstrap first")?;

    // Listen for controller beacon
    info!("[MAIN] Waiting for controller beacon on port {}...", ts_config.beacon_port);
    let listener = tailscale::BeaconListener::new(ts_config.beacon_port).await?;

    let controller = listener
        .wait_for_controller_timeout(Duration::from_secs(300))
        .await
        .context("Timeout waiting for controller - ensure controller is running")?;

    let url = format!("ws://{}:{}", controller.ip, controller.ws_port);
    info!("[MAIN] Discovered controller at {}", url);

    Ok(url)
}

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

    // Main reconnection loop
    loop {
        // Resolve WebSocket URL (may involve beacon discovery)
        let ws_url = match resolve_websocket_url(&config).await {
            Ok(url) => url,
            Err(e) => {
                error!("[MAIN] Failed to resolve WebSocket URL: {}", e);
                warn!("[MAIN] Retrying in 5 seconds...");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        // Create WebSocket client
        let mut ws_client = WebSocketClient::new(ws_url);

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
                                Some(IncomingMessage::Pong { timestamp: _ }) => {
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
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
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
            }
        }

        warn!("[MAIN] Retrying in 5 seconds...");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
```

**Step 4: Run cargo check**

Run: `cargo check -p remote-trader`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add trader/
git commit -m "feat(trader): integrate Tailscale beacon discovery"
```

---

## Task 7: Integrate Tailscale beacon sender into controller

**Files:**
- Modify: `controller/Cargo.toml`
- Modify: `controller/src/main.rs`

**Step 1: Add tailscale dependency to controller**

In `controller/Cargo.toml`, add to dependencies:

```toml
tailscale = { path = "../tailscale" }
tokio-util = "0.7"
```

**Step 2: Modify controller main.rs to send beacons**

Add the following imports near the top of `controller/src/main.rs` (after line 45):

```rust
use tailscale::beacon::BeaconSender;
use tokio_util::sync::CancellationToken;
```

Then, after the `RemoteTraderServer` is created and spawned (around line 395), add beacon sender logic. Find this block:

```rust
let remote_server = RemoteTraderServer::new(bind, platforms, dry_run);
let trader_handle = remote_server.handle();
tokio::spawn(async move {
    if let Err(e) = remote_server.run().await {
        error!("[REMOTE] server error: {}", e);
    }
});
```

And add beacon sender after it:

```rust
let remote_server = RemoteTraderServer::new(bind, platforms, dry_run);
let trader_handle = remote_server.handle();
tokio::spawn(async move {
    if let Err(e) = remote_server.run().await {
        error!("[REMOTE] server error: {}", e);
    }
});

// Start Tailscale beacon sender if Tailscale is available
let beacon_cancel = CancellationToken::new();
if let Ok(ts_status) = tailscale::verify::verify() {
    if !ts_status.peers.is_empty() {
        let ts_config = tailscale::Config::load().unwrap_or_default();
        info!(
            "[BEACON] Starting beacon to {} peers (port {}, ws_port {})",
            ts_status.peers.len(),
            ts_config.beacon_port,
            ts_config.ws_port
        );
        let beacon_cancel_clone = beacon_cancel.clone();
        tokio::spawn(async move {
            match BeaconSender::new(ts_status.peers, ts_config.beacon_port, ts_config.ws_port).await {
                Ok(sender) => sender.run(beacon_cancel_clone).await,
                Err(e) => error!("[BEACON] Failed to create beacon sender: {}", e),
            }
        });
    } else {
        info!("[BEACON] No Tailscale peers - beacon disabled");
    }
} else {
    info!("[BEACON] Tailscale not available - beacon disabled");
}
```

**Step 3: Run cargo check**

Run: `cargo check -p controller`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add controller/
git commit -m "feat(controller): add Tailscale beacon sender for trader discovery"
```

---

## Task 8: Add integration test for beacon discovery

**Files:**
- Create: `tailscale/tests/beacon_integration.rs`

**Step 1: Write the integration test**

```rust
//! Integration test for beacon sender and listener.

use std::net::Ipv4Addr;
use std::time::Duration;
use tailscale::beacon::{BeaconListener, BeaconSender};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_beacon_discovery_flow() {
    // Simulate controller and trader on localhost

    // 1. Create listener (trader side) on ephemeral port
    let listener = BeaconListener::new(0).await.expect("Failed to create listener");
    let beacon_port = listener.socket.local_addr().unwrap().port();

    // 2. Create sender (controller side) targeting localhost
    let sender = BeaconSender::new(vec![Ipv4Addr::LOCALHOST], beacon_port, 9001)
        .await
        .expect("Failed to create sender");

    // 3. Run sender in background
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let sender_task = tokio::spawn(async move {
        sender.run(cancel_clone).await;
    });

    // 4. Trader discovers controller
    let info = tokio::time::timeout(Duration::from_secs(10), listener.wait_for_controller())
        .await
        .expect("Timeout waiting for controller")
        .expect("Failed to receive beacon");

    // 5. Verify discovered info
    assert_eq!(info.ip, Ipv4Addr::LOCALHOST);
    assert_eq!(info.ws_port, 9001);
    assert!(!info.version.is_empty());

    // 6. Cleanup
    cancel.cancel();
    let _ = sender_task.await;
}

#[tokio::test]
async fn test_beacon_listener_timeout() {
    // Test that listener properly times out when no beacon is sent
    let listener = BeaconListener::new(0).await.expect("Failed to create listener");

    let result = listener
        .wait_for_controller_timeout(Duration::from_millis(100))
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Timeout"));
}
```

**Step 2: Expose socket for testing**

In `tailscale/src/beacon.rs`, make the socket field pub(crate):

```rust
pub struct BeaconListener {
    pub(crate) socket: UdpSocket,
}
```

**Step 3: Run tests**

Run: `cargo test -p tailscale -- --nocapture`
Expected: All tests pass

**Step 4: Commit**

```bash
git add tailscale/
git commit -m "test(tailscale): add beacon integration tests"
```

---

## Task 9: Update documentation

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add Tailscale section to CLAUDE.md**

Add the following section after the "Environment Variables" section:

```markdown
## Tailscale Setup (Remote Trading)

For running controller and trader on separate machines:

```bash
# First-time setup (run on each machine)
cargo run -p bootstrap

# This will:
# 1. Verify Tailscale is installed and connected
# 2. Prompt for role (controller/trader)
# 3. Write config to ~/.arb/config.toml
# 4. Optionally launch the appropriate binary
```

**Manual setup alternative:**

```bash
# Install Tailscale
brew install tailscale

# Start daemon and connect
sudo tailscaled
tailscale up

# Verify connection
tailscale status
```

**Configuration file:** `~/.arb/config.toml`

```toml
role = "controller"  # or "trader"
beacon_port = 9000   # UDP port for discovery
ws_port = 9001       # WebSocket port
```

**How it works:**
- Controller sends UDP beacon to all Tailscale peers every 2 seconds
- Trader listens for beacon and auto-discovers controller IP/port
- No manual IP configuration required
- Falls back to `WEBSOCKET_URL` env var if set
```

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add Tailscale setup instructions"
```

---

## Task 10: Final verification

**Step 1: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

**Step 2: Build release**

Run: `cargo build --release --workspace`
Expected: Builds successfully

**Step 3: Verify bootstrap runs**

Run: `cargo run -p bootstrap -- --help` (will fail gracefully since no --help, but shows it compiles and runs)

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat: complete Tailscale auto-discovery implementation"
```
