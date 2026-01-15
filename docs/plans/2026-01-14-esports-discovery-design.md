# Esports Market Discovery Design

**Date:** 2026-01-14
**Status:** Validated via testing

## Overview

Extend the arbitrage bot to support esports markets (CS2, LoL, CoD) alongside existing sports markets. Esports matches are binary outcomes (Team A wins vs Team B wins), structurally identical to sports moneyline markets.

## Supported Games & Market Types

| Game | Kalshi Series | Polymarket Series ID | Market Types |
|------|---------------|---------------------|--------------|
| CS2 | `KXCS2GAME` | `10310` | Moneyline, Map Winner, Total Maps |
| LoL | `KXLOLGAME` | `10311` | Moneyline, Map Winner, Total Maps |
| CoD | `KXCODGAME` | `10427` | Moneyline |

## Discovery Strategy

### The Challenge

Unlike sports where team codes are predictable (CFC, AVL), esports team names vary significantly between platforms:
- Kalshi: "FURIA vs. 9INE" in title, `KXCS2GAME-26JAN16FURIA9INE` in ticker
- Polymarket: `cs2-furia-9ine-2026-01-16` slug, "Counter-Strike: FURIA vs 9INE (BO3)" title

### Validated Solution: Series-Based Name Matching

Tested with 97%+ match rate across all three esports titles.

**Algorithm:**

```
1. Fetch Polymarket events via GET /events?series_id={id}&closed=false
2. For each event:
   a. Parse title to extract full team names
   b. Extract date from slug (last 3 hyphen-separated parts)
   c. Normalize team names (lowercase, remove suffixes)
   d. Store: (date, norm_team1, norm_team2) → (slug, yes_token, no_token)

3. Fetch Kalshi events via get_events(series)
4. For each event:
   a. Parse title to extract team names
   b. Extract date from ticker (26JAN16 → 2026-01-16)
   c. Normalize team names
   d. Lookup in Polymarket map
   e. Create MarketPair if match found
```

**Normalization rules:**
- Lowercase
- Remove common suffixes: "esports", "gaming", "team", "clan"
- Remove punctuation: `.` `'`
- Join words with hyphens

**Example:**
- "FURIA Esports" → `furia`
- "BetBoom Team" → `betboom`
- "Cloud9 New York" → `cloud9-new-york`

## Test Results

From `examples/esports_test.rs`:

| Game | Matched | Missed | Rate |
|------|---------|--------|------|
| CS2 | 8 | 0 | 100% |
| LoL | 19 | 1 | 95% |
| CoD | 8 | 0 | 100% |

The single LoL miss was due to unusual Korean sponsor naming ("OKSavingsBank BRION").

## Configuration Changes

### New LeagueConfig Field

```rust
pub struct LeagueConfig {
    pub league_code: &'static str,
    pub poly_prefix: &'static str,
    pub kalshi_series_game: &'static str,
    pub kalshi_series_spread: Option<&'static str>,
    pub kalshi_series_total: Option<&'static str>,
    pub kalshi_series_btts: Option<&'static str>,
    pub poly_series_id: Option<&'static str>,  // NEW
}
```

### Esports Entries

```rust
LeagueConfig {
    league_code: "cs2",
    poly_prefix: "cs2",
    kalshi_series_game: "KXCS2GAME",
    kalshi_series_spread: None,
    kalshi_series_total: None,
    kalshi_series_btts: None,
    poly_series_id: Some("10310"),
},
LeagueConfig {
    league_code: "lol",
    poly_prefix: "lol",
    kalshi_series_game: "KXLOLGAME",
    kalshi_series_spread: None,
    kalshi_series_total: None,
    kalshi_series_btts: None,
    poly_series_id: Some("10311"),
},
LeagueConfig {
    league_code: "cod",
    poly_prefix: "codmw",
    kalshi_series_game: "KXCODGAME",
    kalshi_series_spread: None,
    kalshi_series_total: None,
    kalshi_series_btts: None,
    poly_series_id: Some("10427"),
},
```

## Implementation Changes

### Files to Modify

| File | Changes |
|------|---------|
| `types.rs` | Add `poly_series_id: Option<&'static str>` to LeagueConfig |
| `config.rs` | Add 3 esports LeagueConfigs |
| `polymarket.rs` | Add `GammaClient::fetch_events_by_series()` method |
| `discovery.rs` | Add esports discovery path with name-based matching |

### New GammaClient Method

```rust
impl GammaClient {
    pub async fn fetch_events_by_series(&self, series_id: &str) -> Result<Vec<PolyEvent>> {
        let url = format!(
            "{}/events?series_id={}&closed=false&limit=100",
            GAMMA_API_BASE, series_id
        );
        let events: Vec<PolyEvent> = self.http.get(&url).send().await?.json().await?;
        Ok(events)
    }
}
```

### Discovery Flow for Esports

```rust
async fn discover_esports_league(&self, config: &LeagueConfig) -> DiscoveryResult {
    let series_id = config.poly_series_id.expect("esports needs series_id");

    // Build Polymarket lookup
    let poly_events = self.gamma.fetch_events_by_series(series_id).await?;
    let poly_lookup = build_esports_lookup(&poly_events);

    // Fetch and match Kalshi events
    let kalshi_events = self.kalshi.get_events(config.kalshi_series_game, 50).await?;

    let mut pairs = Vec::new();
    for event in kalshi_events {
        if let Some((team1, team2, date)) = parse_esports_event(&event) {
            let key = format!("{}:{}:{}", date, normalize(&team1), normalize(&team2));
            if let Some((slug, yes, no)) = poly_lookup.get(&key) {
                pairs.push(MarketPair { /* ... */ });
            }
        }
    }

    DiscoveryResult { pairs, /* ... */ }
}
```

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Team name variations | ~5% miss rate | Log misses, build exception mapping over time |
| Series ID changes | Discovery fails | Query /sports endpoint at startup to refresh IDs |
| New esports titles | Not discovered | Add to config when identified |
| Unusual team names | Miss specific matches | Add normalization exceptions as discovered |

## What Stays Unchanged

- WebSocket price feeds (kalshi.rs, polymarket.rs)
- Lock-free orderbook handling (types.rs)
- Arbitrage detection logic (main.rs)
- Order execution (execution.rs, polymarket_clob.rs)
- Position tracking (position_tracker.rs)
- Circuit breaker (circuit_breaker.rs)
- Team code cache for sports (cache.rs)

## Future Enhancements

1. **Map winner markets**: Kalshi may add KXCS2MAP, KXLOLMAP series
2. **Total maps markets**: Over/under on number of maps played
3. **Dynamic series ID refresh**: Query /sports at startup instead of hardcoding
4. **Fuzzy matching fallback**: For edge cases where normalization fails
