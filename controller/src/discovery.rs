//! Intelligent market discovery and matching system.
//!
//! This module handles the discovery of matching markets between Kalshi and Polymarket,
//! with support for caching, incremental updates, and parallel processing.

use anyhow::Result;
use futures_util::{stream, StreamExt};
use governor::{Quota, RateLimiter, state::NotKeyed, clock::DefaultClock, middleware::NoOpMiddleware};
use serde::{Serialize, Deserialize};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::cache::TeamCache;
use crate::config::{LeagueConfig, get_league_configs, get_league_config};
use crate::kalshi::KalshiApiClient;
use crate::polymarket::GammaClient;
use crate::types::{MarketPair, MarketType, DiscoveryResult, KalshiMarket, KalshiEvent};

/// Max concurrent Gamma API requests
const GAMMA_CONCURRENCY: usize = 20;

/// Kalshi rate limit: 2 requests per second (very conservative - they rate limit aggressively)
/// Must be conservative because discovery runs many leagues/series in parallel
const KALSHI_RATE_LIMIT_PER_SEC: u32 = 2;

/// Max concurrent Kalshi API requests GLOBALLY across all leagues/series
/// This is the hard cap - prevents bursting even when rate limiter has tokens
const KALSHI_GLOBAL_CONCURRENCY: usize = 1;

/// Cache file path
const DISCOVERY_CACHE_PATH: &str = ".discovery_cache.json";

/// Cache TTL in seconds (2 hours - new markets appear every ~2 hours)
const CACHE_TTL_SECS: u64 = 2 * 60 * 60;

/// Task for parallel Gamma lookup
struct GammaLookupTask {
    event: Arc<KalshiEvent>,
    market: KalshiMarket,
    poly_slug: String,
    market_type: MarketType,
    league: String,
}

/// Type alias for Kalshi rate limiter
type KalshiRateLimiter = RateLimiter<NotKeyed, governor::state::InMemoryState, DefaultClock, NoOpMiddleware>;

/// Persistent cache for discovered market pairs
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscoveryCache {
    /// Unix timestamp when cache was created
    timestamp_secs: u64,
    /// Cached market pairs
    pairs: Vec<MarketPair>,
    /// Set of known Kalshi market tickers (for incremental updates)
    known_kalshi_tickers: Vec<String>,
}

impl DiscoveryCache {
    fn new(pairs: Vec<MarketPair>) -> Self {
        let known_kalshi_tickers: Vec<String> = pairs.iter()
            .map(|p| p.kalshi_market_ticker.to_string())
            .collect();
        Self {
            timestamp_secs: current_unix_secs(),
            pairs,
            known_kalshi_tickers,
        }
    }

    fn is_expired(&self) -> bool {
        let now = current_unix_secs();
        now.saturating_sub(self.timestamp_secs) > CACHE_TTL_SECS
    }

    fn age_secs(&self) -> u64 {
        current_unix_secs().saturating_sub(self.timestamp_secs)
    }

    fn has_ticker(&self, ticker: &str) -> bool {
        self.known_kalshi_tickers.iter().any(|t| t == ticker)
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Market discovery and matching client for cross-platform market identification
pub struct DiscoveryClient {
    kalshi: Arc<KalshiApiClient>,
    gamma: Arc<GammaClient>,
    pub team_cache: Arc<TeamCache>,
    kalshi_limiter: Arc<KalshiRateLimiter>,
    kalshi_semaphore: Arc<Semaphore>,  // Global concurrency limit for Kalshi
    gamma_semaphore: Arc<Semaphore>,
}

impl DiscoveryClient {
    pub fn new(kalshi: KalshiApiClient, team_cache: TeamCache) -> Self {
        // Create token bucket rate limiter for Kalshi
        let quota = Quota::per_second(NonZeroU32::new(KALSHI_RATE_LIMIT_PER_SEC).unwrap());
        let kalshi_limiter = Arc::new(RateLimiter::direct(quota));

        Self {
            kalshi: Arc::new(kalshi),
            gamma: Arc::new(GammaClient::new()),
            team_cache: Arc::new(team_cache),
            kalshi_limiter,
            kalshi_semaphore: Arc::new(Semaphore::new(KALSHI_GLOBAL_CONCURRENCY)),
            gamma_semaphore: Arc::new(Semaphore::new(GAMMA_CONCURRENCY)),
        }
    }

    /// Load cache from disk (async)
    async fn load_cache() -> Option<DiscoveryCache> {
        let path = crate::paths::resolve_workspace_file(DISCOVERY_CACHE_PATH);
        let data = tokio::fs::read_to_string(&path).await.ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Save cache to disk (async)
    async fn save_cache(cache: &DiscoveryCache) -> Result<()> {
        let data = serde_json::to_string_pretty(cache)?;
        let path = crate::paths::resolve_workspace_file(DISCOVERY_CACHE_PATH);
        tokio::fs::write(&path, data).await?;
        Ok(())
    }
    
    /// Discover all market pairs with caching support
    ///
    /// Strategy:
    /// 1. Try to load cache from disk
    /// 2. If cache exists and is fresh (<2 hours), use it directly
    /// 3. If cache exists but is stale, load it + fetch incremental updates
    /// 4. If no cache, do full discovery
    pub async fn discover_all(&self, leagues: &[&str]) -> DiscoveryResult {
        // Try to load existing cache
        let cached = Self::load_cache().await;

        match cached {
            Some(cache) if !cache.is_expired() => {
                // Cache is fresh - use it directly
                info!("📂 Loaded {} pairs from cache (age: {}s)",
                      cache.pairs.len(), cache.age_secs());
                return DiscoveryResult {
                    pairs: cache.pairs,
                    kalshi_events_found: 0,  // From cache
                    poly_matches: 0,
                    poly_misses: 0,
                    errors: vec![],
                };
            }
            Some(cache) => {
                // Cache is stale - do incremental discovery
                info!("📂 Cache expired (age: {}s), doing incremental refresh...", cache.age_secs());
                return self.discover_incremental(leagues, cache).await;
            }
            None => {
                // No cache - do full discovery
                info!("📂 No cache found, doing full discovery...");
            }
        }

        // Full discovery (no cache)
        let result = self.discover_full(leagues).await;

        // Save to cache
        if !result.pairs.is_empty() {
            let cache = DiscoveryCache::new(result.pairs.clone());
            if let Err(e) = Self::save_cache(&cache).await {
                warn!("Failed to save discovery cache: {}", e);
            } else {
                info!("💾 Saved {} pairs to cache", result.pairs.len());
            }
        }

        result
    }

    /// Force full discovery (ignores cache)
    pub async fn discover_all_force(&self, leagues: &[&str]) -> DiscoveryResult {
        info!("🔄 Forced full discovery (ignoring cache)...");
        let result = self.discover_full(leagues).await;

        // Save to cache
        if !result.pairs.is_empty() {
            let cache = DiscoveryCache::new(result.pairs.clone());
            if let Err(e) = Self::save_cache(&cache).await {
                warn!("Failed to save discovery cache: {}", e);
            } else {
                info!("💾 Saved {} pairs to cache", result.pairs.len());
            }
        }

        result
    }

    /// Full discovery without cache
    async fn discover_full(&self, leagues: &[&str]) -> DiscoveryResult {
        let configs: Vec<_> = if leagues.is_empty() {
            get_league_configs()
        } else {
            leagues.iter()
                .filter_map(|l| get_league_config(l))
                .collect()
        };

        // Parallel discovery across all leagues
        let league_futures: Vec<_> = configs.iter()
            .map(|config| self.discover_league(config, None))
            .collect();

        let league_results = futures_util::future::join_all(league_futures).await;

        // Merge results
        let mut result = DiscoveryResult::default();
        for league_result in league_results {
            result.pairs.extend(league_result.pairs);
            result.poly_matches += league_result.poly_matches;
            result.errors.extend(league_result.errors);
        }
        result.kalshi_events_found = result.pairs.len();

        result
    }

    /// Incremental discovery - merge cached pairs with newly discovered ones
    async fn discover_incremental(&self, leagues: &[&str], cache: DiscoveryCache) -> DiscoveryResult {
        let configs: Vec<_> = if leagues.is_empty() {
            get_league_configs()
        } else {
            leagues.iter()
                .filter_map(|l| get_league_config(l))
                .collect()
        };

        // Discover with filter for known tickers
        let league_futures: Vec<_> = configs.iter()
            .map(|config| self.discover_league(config, Some(&cache)))
            .collect();

        let league_results = futures_util::future::join_all(league_futures).await;

        // Merge cached pairs with newly discovered ones
        let mut all_pairs = cache.pairs;
        let mut new_count = 0;

        for league_result in league_results {
            for pair in league_result.pairs {
                if !all_pairs.iter().any(|p| *p.kalshi_market_ticker == *pair.kalshi_market_ticker) {
                    all_pairs.push(pair);
                    new_count += 1;
                }
            }
        }

        if new_count > 0 {
            info!("🆕 Found {} new market pairs", new_count);

            // Update cache
            let new_cache = DiscoveryCache::new(all_pairs.clone());
            if let Err(e) = Self::save_cache(&new_cache).await {
                warn!("Failed to update discovery cache: {}", e);
            } else {
                info!("💾 Updated cache with {} total pairs", all_pairs.len());
            }
        } else {
            info!("✅ No new markets found, using {} cached pairs", all_pairs.len());

            // Just update timestamp to extend TTL
            let refreshed_cache = DiscoveryCache::new(all_pairs.clone());
            let _ = Self::save_cache(&refreshed_cache).await;
        }

        DiscoveryResult {
            pairs: all_pairs,
            kalshi_events_found: new_count,
            poly_matches: new_count,
            poly_misses: 0,
            errors: vec![],
        }
    }
    
    /// Discover all market types for a single league (PARALLEL)
    /// If cache is provided, only discovers markets not already in cache
    async fn discover_league(&self, config: &LeagueConfig, cache: Option<&DiscoveryCache>) -> DiscoveryResult {
        info!("🔍 Discovering {} markets...", config.league_code);

        let market_types = [MarketType::Moneyline, MarketType::Spread, MarketType::Total, MarketType::Btts];

        // Parallel discovery across market types
        let type_futures: Vec<_> = market_types.iter()
            .filter_map(|market_type| {
                let series = self.get_series_for_type(config, *market_type)?;
                Some(self.discover_series(config, series, *market_type, cache))
            })
            .collect();

        let type_results = futures_util::future::join_all(type_futures).await;

        let mut result = DiscoveryResult::default();
        for (pairs_result, market_type) in type_results.into_iter().zip(market_types.iter()) {
            match pairs_result {
                Ok(pairs) => {
                    let count = pairs.len();
                    if count > 0 {
                        info!("  ✅ {} {}: {} pairs", config.league_code, market_type, count);
                    }
                    result.poly_matches += count;
                    result.pairs.extend(pairs);
                }
                Err(e) => {
                    result.errors.push(format!("{} {}: {}", config.league_code, market_type, e));
                }
            }
        }

        result
    }
    
    fn get_series_for_type(&self, config: &LeagueConfig, market_type: MarketType) -> Option<&'static str> {
        match market_type {
            MarketType::Moneyline => Some(config.kalshi_series_game),
            MarketType::Spread => config.kalshi_series_spread,
            MarketType::Total => config.kalshi_series_total,
            MarketType::Btts => config.kalshi_series_btts,
        }
    }
    
    /// Discover markets for a specific series (PARALLEL Kalshi + Gamma lookups)
    /// If cache is provided, skips markets already in cache
    async fn discover_series(
        &self,
        config: &LeagueConfig,
        series: &str,
        market_type: MarketType,
        cache: Option<&DiscoveryCache>,
    ) -> Result<Vec<MarketPair>> {
        // Fetch Kalshi events
        {
            let _permit = self.kalshi_semaphore.acquire().await.map_err(|e| anyhow::anyhow!("semaphore closed: {}", e))?;
            self.kalshi_limiter.until_ready().await;
        }
        let events = self.kalshi.get_events(series, 50).await?;

        if events.is_empty() {
            return Ok(vec![]);
        }
        info!("  📡 {} {}: {} events from Kalshi", config.league_code, market_type, events.len());

        // PHASE 2: Parallel market fetching 
        let kalshi = self.kalshi.clone();
        let limiter = self.kalshi_limiter.clone();
        let semaphore = self.kalshi_semaphore.clone();

        // Parse events first, filtering out unparseable ones
        let parsed_events: Vec<_> = events.into_iter()
            .filter_map(|event| {
                let parsed = match parse_kalshi_event_ticker(&event.event_ticker) {
                    Some(p) => p,
                    None => {
                        warn!("  ⚠️ Could not parse event ticker {}", event.event_ticker);
                        return None;
                    }
                };
                Some((parsed, event))
            })
            .collect();

        // Execute market fetches with GLOBAL concurrency limit
        let market_results: Vec<_> = stream::iter(parsed_events)
            .map(|(parsed, event)| {
                let kalshi = kalshi.clone();
                let limiter = limiter.clone();
                let semaphore = semaphore.clone();
                let event_ticker = event.event_ticker.clone();
                async move {
                    let _permit = semaphore.acquire().await.ok();
                    // rate limit
                    limiter.until_ready().await;
                    let markets_result = kalshi.get_markets(&event_ticker).await;
                    (parsed, Arc::new(event), markets_result)
                }
            })
            .buffer_unordered(KALSHI_GLOBAL_CONCURRENCY * 2)  // Allow some buffering, semaphore is the real limit
            .collect()
            .await;

        // Collect all (event, market) pairs
        let mut event_markets = Vec::with_capacity(market_results.len() * 3);
        let mut cached_count = 0usize;
        for (parsed, event, markets_result) in market_results {
            match markets_result {
                Ok(markets) => {
                    for market in markets {
                        // Skip if already in cache
                        if let Some(c) = cache {
                            if c.has_ticker(&market.ticker) {
                                cached_count += 1;
                                continue;
                            }
                        }
                        event_markets.push((parsed.clone(), event.clone(), market));
                    }
                }
                Err(e) => {
                    warn!("  ⚠️ Failed to get markets for {}: {}", event.event_ticker, e);
                }
            }
        }

        if event_markets.is_empty() {
            if cached_count > 0 {
                info!("  ✅ {} {}: {} markets (all cached)", config.league_code, market_type, cached_count);
            }
            return Ok(vec![]);
        }
        info!("  🔎 {} {}: looking up {} new markets on Polymarket{}",
              config.league_code, market_type, event_markets.len(),
              if cached_count > 0 { format!(" ({} cached)", cached_count) } else { String::new() });

        // Parallel Gamma lookups with semaphore
        let lookup_futures: Vec<_> = event_markets
            .into_iter()
            .map(|(parsed, event, market)| {
                let poly_slug = self.build_poly_slug(config.poly_prefix, &parsed, market_type, &market);
                
                GammaLookupTask {
                    event,
                    market,
                    poly_slug,
                    market_type,
                    league: config.league_code.to_string(),
                }
            })
            .collect();
        
        // Execute lookups in parallel 
        let pairs: Vec<MarketPair> = stream::iter(lookup_futures)
            .map(|task| {
                let gamma = self.gamma.clone();
                let semaphore = self.gamma_semaphore.clone();
                async move {
                    let _permit = semaphore.acquire().await.ok()?;
                    match gamma.lookup_market(&task.poly_slug).await {
                        Ok(Some((yes_token, no_token))) => {
                            let team_suffix = extract_team_suffix(&task.market.ticker);
                            Some(MarketPair {
                                pair_id: format!("{}-{}", task.poly_slug, task.market.ticker).into(),
                                league: task.league.into(),
                                market_type: task.market_type,
                                description: format!("{} - {}", task.event.title, task.market.title).into(),
                                kalshi_event_ticker: task.event.event_ticker.clone().into(),
                                kalshi_market_ticker: task.market.ticker.into(),
                                poly_slug: task.poly_slug.into(),
                                poly_yes_token: yes_token.into(),
                                poly_no_token: no_token.into(),
                                line_value: task.market.floor_strike,
                                team_suffix: team_suffix.map(|s| s.into()),
                            })
                        }
                        Ok(None) => None,
                        Err(e) => {
                            warn!("  ⚠️ Gamma lookup failed for {}: {}", task.poly_slug, e);
                            None
                        }
                    }
                }
            })
            .buffer_unordered(GAMMA_CONCURRENCY)
            .filter_map(|x| async { x })
            .collect()
            .await;

        if !pairs.is_empty() {
            info!("  ✅ {} {}: matched {} pairs", config.league_code, market_type, pairs.len());
        }

        Ok(pairs)
    }
    
    /// Build Polymarket slug from Kalshi event data
    fn build_poly_slug(
        &self,
        poly_prefix: &str,
        parsed: &ParsedKalshiTicker,
        market_type: MarketType,
        market: &KalshiMarket,
    ) -> String {
        // Convert Kalshi team codes to Polymarket codes using cache
        let poly_team1 = self.team_cache
            .kalshi_to_poly(poly_prefix, &parsed.team1)
            .unwrap_or_else(|| parsed.team1.to_lowercase());
        let poly_team2 = self.team_cache
            .kalshi_to_poly(poly_prefix, &parsed.team2)
            .unwrap_or_else(|| parsed.team2.to_lowercase());

        // Convert date from "25DEC27" to "2025-12-27"
        let date_str = kalshi_date_to_iso(&parsed.date);

        // Base slug: league-team1-team2-date
        let base = format!("{}-{}-{}-{}", poly_prefix, poly_team1, poly_team2, date_str);

        match market_type {
            MarketType::Moneyline => {
                if let Some(suffix) = extract_team_suffix(&market.ticker) {
                    if suffix.to_lowercase() == "tie" {
                        format!("{}-draw", base)
                    } else {
                        let poly_suffix = self.team_cache
                            .kalshi_to_poly(poly_prefix, &suffix)
                            .unwrap_or_else(|| suffix.to_lowercase());
                        format!("{}-{}", base, poly_suffix)
                    }
                } else {
                    base
                }
            }
            MarketType::Spread => {
                if let Some(floor) = market.floor_strike {
                    let floor_str = format!("{:.1}", floor).replace(".", "pt");
                    format!("{}-spread-{}", base, floor_str)
                } else {
                    format!("{}-spread", base)
                }
            }
            MarketType::Total => {
                if let Some(floor) = market.floor_strike {
                    let floor_str = format!("{:.1}", floor).replace(".", "pt");
                    format!("{}-total-{}", base, floor_str)
                } else {
                    format!("{}-total", base)
                }
            }
            MarketType::Btts => {
                format!("{}-btts", base)
            }
        }
    }

    /// Discover new markets created since a given timestamp
    /// Returns only NEW pairs not already in the known_tickers set
    pub async fn discover_since(
        &self,
        since_ts: u64,
        known_tickers: &std::collections::HashSet<String>,
        leagues: &[&str],
    ) -> DiscoveryResult {
        let configs: Vec<_> = if leagues.is_empty() {
            get_league_configs()
        } else {
            leagues.iter()
                .filter_map(|l| get_league_config(l))
                .collect()
        };

        let mut result = DiscoveryResult::default();

        for config in &configs {
            match self.discover_series_since(config, since_ts, known_tickers).await {
                Ok(pairs) => {
                    if !pairs.is_empty() {
                        tracing::info!("  {} {}: {} new pairs",
                            config.league_code, "discovery", pairs.len());
                    }
                    result.pairs.extend(pairs);
                }
                Err(e) => {
                    result.errors.push(format!("{}: {}", config.league_code, e));
                }
            }
        }

        result.kalshi_events_found = result.pairs.len();
        result.poly_matches = result.pairs.len();
        result
    }

    /// Discover new markets for a single league since timestamp
    async fn discover_series_since(
        &self,
        config: &LeagueConfig,
        since_ts: u64,
        known_tickers: &std::collections::HashSet<String>,
    ) -> Result<Vec<MarketPair>> {
        let mut all_pairs = Vec::new();

        // Check all series for this league
        let series_list: Vec<&str> = [
            Some(config.kalshi_series_game),
            config.kalshi_series_spread,
            config.kalshi_series_total,
            config.kalshi_series_btts,
        ].into_iter().flatten().collect();

        for series in series_list {
            // Rate limit
            {
                let _permit = self.kalshi_semaphore.acquire().await
                    .map_err(|e| anyhow::anyhow!("semaphore closed: {}", e))?;
                self.kalshi_limiter.until_ready().await;
            }

            let markets = match self.kalshi.get_markets_since(series, since_ts).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("  Failed to query {}: {}", series, e);
                    continue;
                }
            };

            // Filter to only new markets
            let new_markets: Vec<_> = markets.into_iter()
                .filter(|m| !known_tickers.contains(&m.ticker))
                .collect();

            if new_markets.is_empty() {
                continue;
            }

            // Look up on Polymarket in parallel
            let pairs: Vec<MarketPair> = stream::iter(new_markets)
                .map(|market| {
                    async move {
                        self.try_match_market(config, &market).await
                    }
                })
                .buffer_unordered(GAMMA_CONCURRENCY)
                .filter_map(|x| async { x })
                .collect()
                .await;

            all_pairs.extend(pairs);
        }

        Ok(all_pairs)
    }

    /// Try to match a single Kalshi market to Polymarket
    async fn try_match_market(&self, config: &LeagueConfig, market: &KalshiMarket) -> Option<MarketPair> {
        // Extract event ticker from market ticker (format: SERIES-EVENTID-SUFFIX)
        let parts: Vec<&str> = market.ticker.split('-').collect();
        if parts.len() < 2 {
            return None;
        }

        // Reconstruct event ticker (SERIES-EVENTID)
        let event_ticker = format!("{}-{}", parts[0], parts[1]);

        // Determine market type from series
        let market_type = if market.ticker.contains("SPREAD") {
            MarketType::Spread
        } else if market.ticker.contains("TOTAL") {
            MarketType::Total
        } else if market.ticker.contains("BTTS") {
            MarketType::Btts
        } else {
            MarketType::Moneyline
        };

        // Parse event ticker to get teams and date
        let parsed = parse_kalshi_event_ticker(&event_ticker)?;

        // Build poly slug
        let poly_slug = self.build_poly_slug(config.poly_prefix, &parsed, market_type, market);

        // Look up on Polymarket
        let _permit = self.gamma_semaphore.acquire().await.ok()?;
        let (yes_token, no_token) = match self.gamma.lookup_market(&poly_slug).await {
            Ok(Some((yes, no))) => (yes, no),
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("  ⚠️ Gamma lookup failed for {}: {}", poly_slug, e);
                return None;
            }
        };

        let team_suffix = extract_team_suffix(&market.ticker);

        Some(MarketPair {
            pair_id: format!("{}-{}", poly_slug, market.ticker).into(),
            league: config.league_code.into(),
            market_type,
            description: format!("{}", market.title).into(),
            kalshi_event_ticker: event_ticker.into(),
            kalshi_market_ticker: market.ticker.clone().into(),
            poly_slug: poly_slug.into(),
            poly_yes_token: yes_token.into(),
            poly_no_token: no_token.into(),
            line_value: market.floor_strike,
            team_suffix: team_suffix.map(|s| s.into()),
        })
    }
}

// === Helpers ===

#[derive(Debug, Clone)]
struct ParsedKalshiTicker {
    date: String,  // "25DEC27"
    team1: String, // "CFC"
    team2: String, // "AVL"
}

/// Parse Kalshi event ticker like "KXEPLGAME-25DEC27CFCAVL" or "KXNCAAFGAME-25DEC27M-OHFRES"
fn parse_kalshi_event_ticker(ticker: &str) -> Option<ParsedKalshiTicker> {
    let parts: Vec<&str> = ticker.split('-').collect();
    if parts.len() < 2 {
        return None;
    }

    // Handle two formats:
    // 1. "KXEPLGAME-25DEC27CFCAVL" - date+teams in parts[1]
    // 2. "KXNCAAFGAME-25DEC27M-OHFRES" - date in parts[1], teams in parts[2]
    let (date, teams_part) = if parts.len() >= 3 && parts[2].len() >= 4 {
        // Format 2: 3-part ticker with separate teams section
        // parts[1] is like "25DEC27M" (date + optional suffix)
        let date_part = parts[1];
        let date = if date_part.len() >= 7 {
            date_part[..7].to_uppercase()
        } else {
            return None;
        };
        (date, parts[2])
    } else {
        // Format 1: 2-part ticker with combined date+teams
        let date_teams = parts[1];
        // Minimum: 7 (date) + 2 + 2 (min team codes) = 11
        if date_teams.len() < 11 {
            return None;
        }
        let date = date_teams[..7].to_uppercase();
        let teams = &date_teams[7..];
        (date, teams)
    };

    // Split team codes - try to find the best split point
    // Team codes range from 2-4 chars (e.g., OM, CFC, FRES)
    let (team1, team2) = split_team_codes(teams_part);

    Some(ParsedKalshiTicker { date, team1, team2 })
}

/// Split a combined team string into two team codes
/// Tries multiple split strategies based on string length
fn split_team_codes(teams: &str) -> (String, String) {
    let len = teams.len();

    // For 6 chars, could be 3+3, 2+4, or 4+2
    // For 5 chars, could be 2+3 or 3+2
    // For 4 chars, must be 2+2
    // For 7 chars, could be 3+4 or 4+3
    // For 8 chars, could be 4+4, 3+5, 5+3

    match len {
        4 => (teams[..2].to_uppercase(), teams[2..].to_uppercase()),
        5 => {
            // Prefer 2+3 (common for OM+ASM, OL+PSG)
            (teams[..2].to_uppercase(), teams[2..].to_uppercase())
        }
        6 => {
            // Check if it looks like 2+4 pattern (e.g., OHFRES = OH+FRES)
            // Common 2-letter codes: OM, OL, OH, SF, LA, NY, KC, TB, etc.
            let first_two = &teams[..2].to_uppercase();
            if is_likely_two_letter_code(first_two) {
                (first_two.clone(), teams[2..].to_uppercase())
            } else {
                // Default to 3+3
                (teams[..3].to_uppercase(), teams[3..].to_uppercase())
            }
        }
        7 => {
            // Could be 3+4 or 4+3 - prefer 3+4
            (teams[..3].to_uppercase(), teams[3..].to_uppercase())
        }
        _ if len >= 8 => {
            // 4+4 or longer
            (teams[..4].to_uppercase(), teams[4..].to_uppercase())
        }
        _ => {
            let mid = len / 2;
            (teams[..mid].to_uppercase(), teams[mid..].to_uppercase())
        }
    }
}

/// Check if a 2-letter code is a known/likely team abbreviation
fn is_likely_two_letter_code(code: &str) -> bool {
    matches!(
        code,
        // European football (Ligue 1, etc.)
        "OM" | "OL" | "FC" |
        // US sports common abbreviations
        "OH" | "SF" | "LA" | "NY" | "KC" | "TB" | "GB" | "NE" | "NO" | "LV" |
        // Generic short codes
        "BC" | "SC" | "AC" | "AS" | "US"
    )
}

/// Convert Kalshi date "25DEC27" to ISO "2025-12-27"
fn kalshi_date_to_iso(kalshi_date: &str) -> String {
    if kalshi_date.len() != 7 {
        return kalshi_date.to_string();
    }
    
    let year = format!("20{}", &kalshi_date[..2]);
    let month = match &kalshi_date[2..5].to_uppercase()[..] {
        "JAN" => "01", "FEB" => "02", "MAR" => "03", "APR" => "04",
        "MAY" => "05", "JUN" => "06", "JUL" => "07", "AUG" => "08",
        "SEP" => "09", "OCT" => "10", "NOV" => "11", "DEC" => "12",
        _ => "01",
    };
    let day = &kalshi_date[5..7];
    
    format!("{}-{}-{}", year, month, day)
}

/// Extract team suffix from market ticker (e.g., "KXEPLGAME-25DEC27CFCAVL-CFC" -> "CFC")
fn extract_team_suffix(ticker: &str) -> Option<String> {
    let mut splits = ticker.splitn(3, '-');
    splits.next()?; // series
    splits.next()?; // event
    splits.next().map(|s| s.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_kalshi_ticker() {
        let parsed = parse_kalshi_event_ticker("KXEPLGAME-25DEC27CFCAVL").unwrap();
        assert_eq!(parsed.date, "25DEC27");
        assert_eq!(parsed.team1, "CFC");
        assert_eq!(parsed.team2, "AVL");
    }
    
    #[test]
    fn test_kalshi_date_to_iso() {
        assert_eq!(kalshi_date_to_iso("25DEC27"), "2025-12-27");
        assert_eq!(kalshi_date_to_iso("25JAN01"), "2025-01-01");
    }
}
