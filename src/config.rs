//! System configuration and league mapping definitions.
//!
//! This module contains all configuration constants, league mappings, and
//! environment variable parsing for the trading system.

/// Kalshi WebSocket URL
pub const KALSHI_WS_URL: &str = "wss://api.elections.kalshi.com/trade-api/ws/v2";

/// Kalshi REST API base URL
pub const KALSHI_API_BASE: &str = "https://api.elections.kalshi.com/trade-api/v2";

/// Polymarket WebSocket URL
pub const POLYMARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// Gamma API base URL (Polymarket market data)
pub const GAMMA_API_BASE: &str = "https://gamma-api.polymarket.com";

/// Arb threshold: alert when total cost < this (e.g., 0.995 = 0.5% profit)
pub const ARB_THRESHOLD: f64 = 0.995;

/// Polymarket ping interval (seconds) - keep connection alive
pub const POLY_PING_INTERVAL_SECS: u64 = 30;

/// Kalshi API rate limit delay (milliseconds between requests)
/// Kalshi limit: 20 req/sec = 50ms minimum. We use 60ms for safety margin.
pub const KALSHI_API_DELAY_MS: u64 = 60;

/// WebSocket reconnect delay (seconds)
pub const WS_RECONNECT_DELAY_SECS: u64 = 5;

/// Which leagues to monitor (empty slice = all)
pub const ENABLED_LEAGUES: &[&str] = &[];

/// Price logging enabled (set PRICE_LOGGING=1 to enable)
#[allow(dead_code)]
pub fn price_logging_enabled() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("PRICE_LOGGING")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false)
    })
}

/// League configuration for market discovery
#[derive(Debug, Clone)]
pub struct LeagueConfig {
    pub league_code: &'static str,
    pub poly_prefix: &'static str,
    pub kalshi_series_game: &'static str,
    pub kalshi_series_spread: Option<&'static str>,
    pub kalshi_series_total: Option<&'static str>,
    pub kalshi_series_btts: Option<&'static str>,
}

/// Get all supported leagues with their configurations
pub fn get_league_configs() -> Vec<LeagueConfig> {
    vec![
        // Major European leagues (full market types)
        LeagueConfig {
            league_code: "epl",
            poly_prefix: "epl",
            kalshi_series_game: "KXEPLGAME",
            kalshi_series_spread: Some("KXEPLSPREAD"),
            kalshi_series_total: Some("KXEPLTOTAL"),
            kalshi_series_btts: Some("KXEPLBTTS"),
        },
        LeagueConfig {
            league_code: "bundesliga",
            poly_prefix: "bun",
            kalshi_series_game: "KXBUNDESLIGAGAME",
            kalshi_series_spread: Some("KXBUNDESLIGASPREAD"),
            kalshi_series_total: Some("KXBUNDESLIGATOTAL"),
            kalshi_series_btts: Some("KXBUNDESLIGABTTS"),
        },
        LeagueConfig {
            league_code: "laliga",
            poly_prefix: "lal",
            kalshi_series_game: "KXLALIGAGAME",
            kalshi_series_spread: Some("KXLALIGASPREAD"),
            kalshi_series_total: Some("KXLALIGATOTAL"),
            kalshi_series_btts: Some("KXLALIGABTTS"),
        },
        LeagueConfig {
            league_code: "seriea",
            poly_prefix: "sea",
            kalshi_series_game: "KXSERIEAGAME",
            kalshi_series_spread: Some("KXSERIEASPREAD"),
            kalshi_series_total: Some("KXSERIEATOTAL"),
            kalshi_series_btts: Some("KXSERIEABTTS"),
        },
        LeagueConfig {
            league_code: "ligue1",
            poly_prefix: "fl1",
            kalshi_series_game: "KXLIGUE1GAME",
            kalshi_series_spread: Some("KXLIGUE1SPREAD"),
            kalshi_series_total: Some("KXLIGUE1TOTAL"),
            kalshi_series_btts: Some("KXLIGUE1BTTS"),
        },
        LeagueConfig {
            league_code: "ucl",
            poly_prefix: "ucl",
            kalshi_series_game: "KXUCLGAME",
            kalshi_series_spread: Some("KXUCLSPREAD"),
            kalshi_series_total: Some("KXUCLTOTAL"),
            kalshi_series_btts: Some("KXUCLBTTS"),
        },
        // Secondary European leagues (moneyline only)
        LeagueConfig {
            league_code: "uel",
            poly_prefix: "uel",
            kalshi_series_game: "KXUELGAME",
            kalshi_series_spread: None,
            kalshi_series_total: None,
            kalshi_series_btts: None,
        },
        LeagueConfig {
            league_code: "eflc",
            poly_prefix: "elc",
            kalshi_series_game: "KXEFLCHAMPIONSHIPGAME",
            kalshi_series_spread: None,
            kalshi_series_total: None,
            kalshi_series_btts: None,
        },
        // US Sports
        LeagueConfig {
            league_code: "nba",
            poly_prefix: "nba",
            kalshi_series_game: "KXNBAGAME",
            kalshi_series_spread: Some("KXNBASPREAD"),
            kalshi_series_total: Some("KXNBATOTAL"),
            kalshi_series_btts: None,
        },
        LeagueConfig {
            league_code: "nfl",
            poly_prefix: "nfl",
            kalshi_series_game: "KXNFLGAME",
            kalshi_series_spread: Some("KXNFLSPREAD"),
            kalshi_series_total: Some("KXNFLTOTAL"),
            kalshi_series_btts: None,
        },
        LeagueConfig {
            league_code: "nhl",
            poly_prefix: "nhl",
            kalshi_series_game: "KXNHLGAME",
            kalshi_series_spread: Some("KXNHLSPREAD"),
            kalshi_series_total: Some("KXNHLTOTAL"),
            kalshi_series_btts: None,
        },
        LeagueConfig {
            league_code: "mlb",
            poly_prefix: "mlb",
            kalshi_series_game: "KXMLBGAME",
            kalshi_series_spread: Some("KXMLBSPREAD"),
            kalshi_series_total: Some("KXMLBTOTAL"),
            kalshi_series_btts: None,
        },
        LeagueConfig {
            league_code: "mls",
            poly_prefix: "mls",
            kalshi_series_game: "KXMLSGAME",
            kalshi_series_spread: None,
            kalshi_series_total: None,
            kalshi_series_btts: None,
        },
        LeagueConfig {
            league_code: "ncaaf",
            poly_prefix: "cfb",
            kalshi_series_game: "KXNCAAFGAME",
            kalshi_series_spread: Some("KXNCAAFSPREAD"),
            kalshi_series_total: Some("KXNCAAFTOTAL"),
            kalshi_series_btts: None,
        },
    ]
}

/// Get config for a specific league
pub fn get_league_config(league: &str) -> Option<LeagueConfig> {
    get_league_configs()
        .into_iter()
        .find(|c| c.league_code == league || c.poly_prefix == league)
}

/// Discovery refresh interval in minutes (default: 15, 0 = disabled)
pub fn discovery_interval_mins() -> u64 {
    static CACHED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("DISCOVERY_INTERVAL_MINS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_interval_returns_reasonable_value() {
        // Note: This tests that discovery_interval_mins() returns a valid value.
        // Since the value is cached with OnceLock, this test works reliably
        // only when DISCOVERY_INTERVAL_MINS is not set in the test environment.
        // The function returns the cached value (default 15 if not set).
        let result = discovery_interval_mins();
        // Verify it returns a reasonable interval (up to 24 hours in minutes)
        assert!(result <= 60 * 24, "Should return a reasonable interval");
    }
}