//! WebSocket protocol (controller <-> trader).
//!
//! This intentionally mirrors `trader::protocol` so the controller can run
//! as a standalone binary on a separate machine without linking to trader code.

use serde::{Deserialize, Serialize};

/// Platform identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Kalshi,
    Polymarket,
}

/// Arbitrage type for order execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbType {
    PolyYesKalshiNo,
    KalshiYesPolyNo,
    PolyOnly,
    KalshiOnly,
}

/// Incoming messages from host (controller) to trader
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    #[serde(rename = "init")]
    Init {
        platforms: Vec<Platform>,
        #[serde(default)]
        dry_run: bool,
    },

    #[serde(rename = "execute")]
    Execute {
        market_id: u16,
        arb_type: ArbType,
        yes_price: u16,
        no_price: u16,
        yes_size: u16,
        no_size: u16,

        // Optional metadata so trader can execute without a shared DB.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pair_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kalshi_market_ticker: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        poly_yes_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        poly_no_token: Option<String>,
    },

    #[serde(rename = "ping")]
    Ping { timestamp: u64 },

    #[serde(rename = "pong")]
    Pong { timestamp: u64 },

    #[serde(rename = "status")]
    Status,
}

/// Outgoing messages from trader to host (controller)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    #[serde(rename = "init_ack")]
    InitAck {
        success: bool,
        platforms: Vec<Platform>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    #[serde(rename = "execution_result")]
    ExecutionResult {
        market_id: u16,
        success: bool,
        profit_cents: i16,
        latency_ns: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    #[serde(rename = "ping")]
    Ping { timestamp: u64 },

    #[serde(rename = "pong")]
    Pong { timestamp: u64 },

    #[serde(rename = "status")]
    Status {
        connected: bool,
        platforms: Vec<Platform>,
        dry_run: bool,
    },

    #[serde(rename = "error")]
    Error { message: String },
}

