//! Configuration and credential management

use anyhow::Result;
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
