# Esports Discovery Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add esports market discovery (CS2, LoL, CoD) using series-based team name matching.

**Architecture:** Extend existing LeagueConfig with `poly_series_id` field. For esports leagues, fetch Polymarket events via `/events?series_id=X`, extract full team names from titles, normalize and match against Kalshi events by (date, team1, team2).

**Tech Stack:** Rust, reqwest, serde, regex (already in dev-dependencies)

---

### Task 1: Add poly_series_id to LeagueConfig

**Files:**
- Modify: `src/config.rs:46-54`

**Step 1: Add the new field to LeagueConfig struct**

In `src/config.rs`, add `poly_series_id` field to the struct:

```rust
/// League configuration for market discovery
#[derive(Debug, Clone)]
pub struct LeagueConfig {
    pub league_code: &'static str,
    pub poly_prefix: &'static str,
    pub kalshi_series_game: &'static str,
    pub kalshi_series_spread: Option<&'static str>,
    pub kalshi_series_total: Option<&'static str>,
    pub kalshi_series_btts: Option<&'static str>,
    pub poly_series_id: Option<&'static str>,  // NEW: Polymarket series ID for esports
}
```

**Step 2: Update all existing LeagueConfig instances**

Add `poly_series_id: None` to all 14 existing sports configs in `get_league_configs()`.

**Step 3: Run tests to verify no breakage**

Run: `cargo test`
Expected: All tests pass

**Step 4: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add poly_series_id field to LeagueConfig

Enables esports discovery via Polymarket's series-based API.
Sports leagues use None (existing team cache approach)."
```

---

### Task 2: Add esports LeagueConfigs

**Files:**
- Modify: `src/config.rs` (in `get_league_configs()`)

**Step 1: Add CS2 config**

After the last sports config (ncaaf), add:

```rust
// Esports - Counter-Strike 2
LeagueConfig {
    league_code: "cs2",
    poly_prefix: "cs2",
    kalshi_series_game: "KXCS2GAME",
    kalshi_series_spread: None,
    kalshi_series_total: None,
    kalshi_series_btts: None,
    poly_series_id: Some("10310"),
},
```

**Step 2: Add LoL config**

```rust
// Esports - League of Legends
LeagueConfig {
    league_code: "lol",
    poly_prefix: "lol",
    kalshi_series_game: "KXLOLGAME",
    kalshi_series_spread: None,
    kalshi_series_total: None,
    kalshi_series_btts: None,
    poly_series_id: Some("10311"),
},
```

**Step 3: Add CoD config**

```rust
// Esports - Call of Duty
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

**Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add CS2, LoL, and CoD esports configs

- CS2: series_id 10310
- LoL: series_id 10311
- CoD: series_id 10427"
```

---

### Task 3: Add PolyEvent type and fetch_events_by_series

**Files:**
- Modify: `src/types.rs` (add PolyEvent struct)
- Modify: `src/polymarket.rs` (add fetch method)

**Step 1: Add PolyEvent struct to types.rs**

After the existing `GammaMarket` struct (around line 1275), add:

```rust
/// Polymarket event from /events endpoint
#[derive(Debug, Deserialize)]
pub struct PolyEvent {
    pub slug: Option<String>,
    pub title: Option<String>,
    pub markets: Option<Vec<PolyEventMarket>>,
}

/// Market within a Polymarket event
#[derive(Debug, Deserialize)]
pub struct PolyEventMarket {
    pub slug: Option<String>,
    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: Option<String>,
}
```

**Step 2: Add fetch_events_by_series to GammaClient**

In `src/polymarket.rs`, add this method to `impl GammaClient`:

```rust
/// Fetch events by Polymarket series ID (for esports)
pub async fn fetch_events_by_series(&self, series_id: &str) -> Result<Vec<PolyEvent>> {
    let url = format!(
        "{}/events?series_id={}&closed=false&limit=100",
        GAMMA_API_BASE, series_id
    );

    let resp = self.http.get(&url).send().await?;

    if !resp.status().is_success() {
        return Ok(vec![]);
    }

    let events: Vec<PolyEvent> = resp.json().await?;
    Ok(events)
}
```

**Step 3: Add import for PolyEvent**

At top of `src/polymarket.rs`, update the import from types:

```rust
use crate::types::{
    GlobalState, FastExecutionRequest, ArbType, PriceCents, SizeCents,
    parse_price, fxhash_str, PolyEvent,
};
```

**Step 4: Run build to verify**

Run: `cargo build`
Expected: Build succeeds

**Step 5: Commit**

```bash
git add src/types.rs src/polymarket.rs
git commit -m "feat(polymarket): add fetch_events_by_series for esports discovery

Fetches events from Gamma API using series_id parameter.
Returns PolyEvent with nested markets containing token IDs."
```

---

### Task 4: Add esports name parsing utilities

**Files:**
- Modify: `src/discovery.rs` (add helper functions)

**Step 1: Add normalize_esports_team function**

After the existing helper functions (around line 800), add:

```rust
/// Normalize esports team name for matching
/// "FURIA Esports" -> "furia", "Cloud9 New York" -> "cloud9-new-york"
fn normalize_esports_team(name: &str) -> String {
    let lower = name.to_lowercase();
    let cleaned = lower
        .replace(" esports", "")
        .replace(" gaming", "")
        .replace(" team", "")
        .replace(" clan", "")
        .replace(".", "")
        .replace("'", "");
    cleaned.split_whitespace().collect::<Vec<_>>().join("-")
}
```

**Step 2: Add parse_poly_event_title function**

```rust
/// Parse Polymarket event title to extract team names
/// "Counter-Strike: Team1 vs Team2 (BO3) - Tournament" -> Some((team1, team2))
fn parse_poly_event_title(title: &str) -> Option<(String, String)> {
    // Pattern: "Game: Team1 vs Team2 (BON)"
    let re = regex::Regex::new(r"(?i):\s*(.+?)\s+vs\.?\s+(.+?)\s*\(").ok()?;
    if let Some(caps) = re.captures(title) {
        return Some((
            caps.get(1)?.as_str().trim().to_string(),
            caps.get(2)?.as_str().trim().to_string(),
        ));
    }

    // Fallback: "Game: Team1 vs Team2 - Tournament"
    let re2 = regex::Regex::new(r"(?i):\s*(.+?)\s+vs\.?\s+(.+?)\s*-").ok()?;
    if let Some(caps) = re2.captures(title) {
        return Some((
            caps.get(1)?.as_str().trim().to_string(),
            caps.get(2)?.as_str().trim().to_string(),
        ));
    }

    None
}
```

**Step 3: Add extract_date_from_poly_slug function**

```rust
/// Extract date from Polymarket slug
/// "cs2-team1-team2-2026-01-16" -> Some("2026-01-16")
fn extract_date_from_poly_slug(slug: &str) -> Option<String> {
    let parts: Vec<&str> = slug.split('-').collect();
    if parts.len() >= 4 {
        let year = parts[parts.len() - 3];
        let month = parts[parts.len() - 2];
        let day = parts[parts.len() - 1];

        if year.len() == 4 && month.len() == 2 && day.len() == 2 {
            return Some(format!("{}-{}-{}", year, month, day));
        }
    }
    None
}
```

**Step 4: Add parse_esports_kalshi_title function**

```rust
/// Parse Kalshi esports event title
/// "Tournament: Team1 vs. Team2" -> Some((team1, team2))
fn parse_esports_kalshi_title(title: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(r"(?i):\s*(.+?)\s+vs\.?\s+(.+)$").ok()?;
    let caps = re.captures(title)?;
    Some((
        caps.get(1)?.as_str().trim().to_string(),
        caps.get(2)?.as_str().trim().to_string(),
    ))
}
```

**Step 5: Add regex to dependencies**

In `Cargo.toml`, add regex to main dependencies (it's already in dev-dependencies):

```toml
regex = "1.10"
```

**Step 6: Run build**

Run: `cargo build`
Expected: Build succeeds

**Step 7: Commit**

```bash
git add src/discovery.rs Cargo.toml
git commit -m "feat(discovery): add esports name parsing utilities

- normalize_esports_team: removes common suffixes, lowercases
- parse_poly_event_title: extracts teams from Polymarket titles
- parse_esports_kalshi_title: extracts teams from Kalshi titles
- extract_date_from_poly_slug: gets date from slug suffix"
```

---

### Task 5: Add unit tests for esports parsing

**Files:**
- Modify: `src/discovery.rs` (add tests module)

**Step 1: Add tests for normalize_esports_team**

In the `#[cfg(test)]` module at the bottom of `src/discovery.rs`:

```rust
#[test]
fn test_normalize_esports_team() {
    assert_eq!(normalize_esports_team("FURIA Esports"), "furia");
    assert_eq!(normalize_esports_team("Cloud9 New York"), "cloud9-new-york");
    assert_eq!(normalize_esports_team("Team Liquid"), "liquid");
    assert_eq!(normalize_esports_team("G2"), "g2");
    assert_eq!(normalize_esports_team("BetBoom Team"), "betboom");
    assert_eq!(normalize_esports_team("Gen.G"), "geng");
}

#[test]
fn test_parse_poly_event_title() {
    let (t1, t2) = parse_poly_event_title("Counter-Strike: FURIA vs 9INE (BO3)").unwrap();
    assert_eq!(t1, "FURIA");
    assert_eq!(t2, "9INE");

    let (t1, t2) = parse_poly_event_title("LoL: T1 vs DRX (BO5) - LCK Finals").unwrap();
    assert_eq!(t1, "T1");
    assert_eq!(t2, "DRX");
}

#[test]
fn test_extract_date_from_poly_slug() {
    assert_eq!(
        extract_date_from_poly_slug("cs2-furia-9ine-2026-01-16"),
        Some("2026-01-16".to_string())
    );
    assert_eq!(
        extract_date_from_poly_slug("lol-t1-drx-2026-01-18"),
        Some("2026-01-18".to_string())
    );
    assert_eq!(extract_date_from_poly_slug("invalid"), None);
}

#[test]
fn test_parse_esports_kalshi_title() {
    let (t1, t2) = parse_esports_kalshi_title("BLAST Bounty 2026: FURIA vs. 9INE").unwrap();
    assert_eq!(t1, "FURIA");
    assert_eq!(t2, "9INE");
}
```

**Step 2: Run tests**

Run: `cargo test normalize_esports -- --nocapture`
Run: `cargo test parse_poly_event -- --nocapture`
Run: `cargo test extract_date_from_poly -- --nocapture`
Run: `cargo test parse_esports_kalshi -- --nocapture`
Expected: All pass

**Step 3: Commit**

```bash
git add src/discovery.rs
git commit -m "test(discovery): add unit tests for esports parsing functions"
```

---

### Task 6: Implement discover_esports_league

**Files:**
- Modify: `src/discovery.rs`

**Step 1: Add discover_esports_league method**

Add this method to `impl DiscoveryClient`:

```rust
/// Discover esports market pairs using series-based name matching
async fn discover_esports_league(&self, config: &LeagueConfig) -> DiscoveryResult {
    let series_id = match config.poly_series_id {
        Some(id) => id,
        None => return DiscoveryResult::default(),
    };

    info!("🎮 Discovering {} esports markets (series_id={})...", config.league_code, series_id);

    // Phase 1: Build Polymarket lookup from events
    let poly_events = match self.gamma.fetch_events_by_series(series_id).await {
        Ok(events) => events,
        Err(e) => {
            warn!("Failed to fetch Polymarket events for {}: {}", config.league_code, e);
            return DiscoveryResult {
                errors: vec![format!("{}: {}", config.league_code, e)],
                ..Default::default()
            };
        }
    };

    // Build lookup: (date:norm_team1:norm_team2) -> (slug, yes_token, no_token)
    let mut poly_lookup: std::collections::HashMap<String, (String, String, String)> =
        std::collections::HashMap::new();

    for event in &poly_events {
        let slug = match &event.slug {
            Some(s) => s,
            None => continue,
        };

        let title = match &event.title {
            Some(t) => t,
            None => continue,
        };

        if let Some((team1, team2)) = parse_poly_event_title(title) {
            if let Some(date) = extract_date_from_poly_slug(slug) {
                let norm1 = normalize_esports_team(&team1);
                let norm2 = normalize_esports_team(&team2);

                // Find moneyline market (no -game, -total, -map suffix)
                if let Some(markets) = &event.markets {
                    for market in markets {
                        let market_slug = market.slug.as_deref().unwrap_or("");
                        let is_moneyline = !market_slug.contains("-game")
                            && !market_slug.contains("-total")
                            && !market_slug.contains("-map-")
                            && !market_slug.contains("-handicap");

                        if is_moneyline {
                            if let Some(tokens) = &market.clob_token_ids {
                                if let Ok(ids) = serde_json::from_str::<Vec<String>>(tokens) {
                                    if ids.len() >= 2 {
                                        let key1 = format!("{}:{}:{}", date, norm1, norm2);
                                        let key2 = format!("{}:{}:{}", date, norm2, norm1);
                                        poly_lookup.insert(key1, (slug.clone(), ids[0].clone(), ids[1].clone()));
                                        poly_lookup.insert(key2, (slug.clone(), ids[0].clone(), ids[1].clone()));
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    info!("  📊 Built {} Polymarket lookup entries", poly_lookup.len() / 2);

    // Phase 2: Fetch and match Kalshi events
    let kalshi_events = {
        let _permit = self.kalshi_semaphore.acquire().await.ok();
        self.kalshi_limiter.until_ready().await;
        match self.kalshi.get_events(config.kalshi_series_game, 50).await {
            Ok(events) => events,
            Err(e) => {
                warn!("Failed to fetch Kalshi events for {}: {}", config.league_code, e);
                return DiscoveryResult {
                    errors: vec![format!("{}: {}", config.league_code, e)],
                    ..Default::default()
                };
            }
        }
    };

    let mut pairs = Vec::new();

    for event in &kalshi_events {
        if let Some((team1, team2)) = parse_esports_kalshi_title(&event.title) {
            if let Some(date) = parse_kalshi_event_ticker(&event.event_ticker)
                .map(|p| kalshi_date_to_iso(&p.date))
            {
                let norm1 = normalize_esports_team(&team1);
                let norm2 = normalize_esports_team(&team2);
                let key = format!("{}:{}:{}", date, norm1, norm2);

                if let Some((slug, yes_token, no_token)) = poly_lookup.get(&key) {
                    // Get Kalshi markets for this event
                    let markets = {
                        let _permit = self.kalshi_semaphore.acquire().await.ok();
                        self.kalshi_limiter.until_ready().await;
                        self.kalshi.get_markets(&event.event_ticker).await.unwrap_or_default()
                    };

                    for market in markets {
                        let team_suffix = extract_team_suffix(&market.ticker);

                        pairs.push(MarketPair {
                            pair_id: format!("{}-{}", slug, market.ticker).into(),
                            league: config.league_code.into(),
                            market_type: MarketType::Moneyline,
                            description: format!("{} - {}", event.title, market.title).into(),
                            kalshi_event_ticker: event.event_ticker.clone().into(),
                            kalshi_market_ticker: market.ticker.into(),
                            poly_slug: slug.clone().into(),
                            poly_yes_token: yes_token.clone().into(),
                            poly_no_token: no_token.clone().into(),
                            line_value: market.floor_strike,
                            team_suffix: team_suffix.map(|s| s.into()),
                        });
                    }
                }
            }
        }
    }

    if !pairs.is_empty() {
        info!("  ✅ {} {}: matched {} pairs", config.league_code, "esports", pairs.len());
    }

    DiscoveryResult {
        pairs,
        kalshi_events_found: kalshi_events.len(),
        poly_matches: poly_lookup.len() / 2,
        poly_misses: 0,
        errors: vec![],
    }
}
```

**Step 2: Run build**

Run: `cargo build`
Expected: Build succeeds

**Step 3: Commit**

```bash
git add src/discovery.rs
git commit -m "feat(discovery): implement discover_esports_league

Two-phase matching:
1. Fetch Polymarket events by series_id, build name-based lookup
2. Fetch Kalshi events, match by (date, normalized_team1, normalized_team2)

Returns MarketPairs for matched esports events."
```

---

### Task 7: Integrate esports discovery into discover_league

**Files:**
- Modify: `src/discovery.rs`

**Step 1: Update discover_league to check for esports**

At the start of the `discover_league` method, add a check for esports:

```rust
async fn discover_league(&self, config: &LeagueConfig, cache: Option<&DiscoveryCache>) -> DiscoveryResult {
    // Use esports discovery for leagues with poly_series_id
    if config.poly_series_id.is_some() {
        return self.discover_esports_league(config).await;
    }

    // Existing sports discovery logic continues below...
    info!("🔍 Discovering {} markets...", config.league_code);
    // ... rest of existing code
```

**Step 2: Run the esports test**

Run: `dotenvx run -- cargo run --example esports_test`
Expected: Shows matches for CS2, LoL, CoD

**Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 4: Commit**

```bash
git add src/discovery.rs
git commit -m "feat(discovery): integrate esports into main discovery flow

Leagues with poly_series_id now use esports discovery path.
Sports leagues continue using team cache approach."
```

---

### Task 8: Update CLAUDE.md with esports info

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Update Supported Markets section**

Change the "Supported Markets" line to:

```markdown
## Supported Markets

**Sports:** Soccer (EPL, Bundesliga, La Liga, Serie A, Ligue 1, UCL, UEL, EFL Championship), NBA, NFL, NHL, MLB, MLS, NCAAF

**Esports:** CS2, League of Legends, Call of Duty
```

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add esports to supported markets in CLAUDE.md"
```

---

### Task 9: Final integration test

**Step 1: Run full discovery**

Run: `dotenvx run -- cargo run --release 2>&1 | head -100`
Expected: Shows esports leagues being discovered alongside sports

**Step 2: Run all tests**

Run: `cargo test`
Expected: All tests pass

**Step 3: Final commit if any fixes needed**

---

### Task 10: Create PR

**Step 1: Push branch**

```bash
git push -u origin feature/esports-discovery
```

**Step 2: Create PR**

```bash
gh pr create --title "feat: add esports market discovery (CS2, LoL, CoD)" --body "$(cat <<'EOF'
## Summary

- Add esports market discovery for Counter-Strike 2, League of Legends, and Call of Duty
- Uses Polymarket's series_id API for efficient event fetching
- Matches events by normalized team names + date
- Validated with 97%+ match rate in testing

## Changes

- Add `poly_series_id` field to LeagueConfig
- Add 3 esports LeagueConfigs (cs2, lol, cod)
- Add `fetch_events_by_series()` to GammaClient
- Add esports name parsing/normalization utilities
- Integrate esports discovery into main flow

## Test plan

- [x] Unit tests for parsing functions pass
- [x] Integration test (`examples/esports_test.rs`) shows high match rate
- [x] Full test suite passes
- [ ] Manual verification of esports discovery in dry run mode

---
Generated with [Claude Code](https://claude.ai/code)
EOF
)"
```

**Step 3: Report PR URL**
