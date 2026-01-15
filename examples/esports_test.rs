//! Quick test to validate esports discovery strategy
//! Run with: dotenvx run -- cargo run --example esports_test

use anyhow::Result;
use arb_bot::kalshi::KalshiApiClient;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;

// (Kalshi series, Polymarket prefix, Polymarket series_id)
const ESPORTS_SERIES: &[(&str, &str, &str)] = &[
    ("KXCS2GAME", "cs2", "10310"),
    ("KXLOLGAME", "lol", "10311"),
    ("KXCODGAME", "codmw", "10427"),
];

#[derive(Debug, Deserialize)]
struct PolyEvent {
    slug: Option<String>,
    title: Option<String>,
    markets: Option<Vec<PolyMarket>>,
}

#[derive(Debug, Deserialize)]
struct PolyMarket {
    slug: Option<String>,
    #[serde(rename = "clobTokenIds")]
    clob_token_ids: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = arb_bot::kalshi::KalshiConfig::from_env()?;
    let kalshi = KalshiApiClient::new(config);
    let http = reqwest::Client::new();

    println!("=== Esports Discovery Test (Series-Based Lookup) ===\n");

    for (kalshi_series, poly_prefix, poly_series_id) in ESPORTS_SERIES {
        println!("========== {} ==========\n", kalshi_series);

        // PHASE 1: Fetch Polymarket events
        println!("📥 Fetching Polymarket {} events (series_id={})...", poly_prefix, poly_series_id);

        let url = format!(
            "https://gamma-api.polymarket.com/events?series_id={}&closed=false&limit=100",
            poly_series_id
        );
        let poly_events: Vec<PolyEvent> = http.get(&url).send().await?.json().await?;
        println!("   Found {} Polymarket events\n", poly_events.len());

        // Build lookup: (date, norm_team1, norm_team2) -> (slug, yes_token, no_token)
        let mut poly_lookup: HashMap<String, (String, String, String)> = HashMap::new();

        for event in &poly_events {
            let slug = match &event.slug {
                Some(s) => s,
                None => continue,
            };

            let title = match &event.title {
                Some(t) => t,
                None => continue,
            };

            // Parse title: "Counter-Strike: Team1 vs Team2 (BO3) - Tournament"
            if let Some((team1, team2)) = parse_poly_title(title) {
                if let Some(date) = extract_date_from_poly_slug(slug) {
                    let norm1 = normalize_team(&team1);
                    let norm2 = normalize_team(&team2);

                    // Get tokens from the moneyline market
                    // Moneyline = slug has no suffix like -game1, -total, -map-
                    if let Some(markets) = &event.markets {
                        for market in markets {
                            let market_slug = market.slug.as_ref().map(|s| s.as_str()).unwrap_or("");
                            let is_moneyline = !market_slug.contains("-game")
                                && !market_slug.contains("-total")
                                && !market_slug.contains("-map-")
                                && !market_slug.contains("-handicap");

                            if is_moneyline {
                                if let Some((yes, no)) = parse_tokens(&market.clob_token_ids) {
                                    let key1 = format!("{}:{}:{}", date, norm1, norm2);
                                    let key2 = format!("{}:{}:{}", date, norm2, norm1);
                                    poly_lookup.insert(key1, (slug.clone(), yes.clone(), no.clone()));
                                    poly_lookup.insert(key2, (slug.clone(), yes, no));
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        println!("   Built {} lookup entries\n", poly_lookup.len() / 2);

        // PHASE 2: Fetch and match Kalshi events
        println!("🔍 Fetching Kalshi {} events...", kalshi_series);
        let kalshi_events = kalshi.get_events(kalshi_series, 20).await?;
        println!("   Found {} Kalshi events\n", kalshi_events.len());

        let mut matched = 0;
        let mut missed = 0;

        for event in &kalshi_events {
            if let Some((team1, team2, date)) = parse_esports_event(&event.title, &event.event_ticker) {
                let norm1 = normalize_team(&team1);
                let norm2 = normalize_team(&team2);
                let key = format!("{}:{}:{}", date, norm1, norm2);

                if let Some((slug, yes, no)) = poly_lookup.get(&key) {
                    println!("  ✅ {} vs {} @ {}", team1, team2, date);
                    println!("     Kalshi: {}", event.event_ticker);
                    println!("     Poly:   {} (yes={:.15}..., no={:.15}...)", slug, yes, no);
                    matched += 1;
                } else {
                    // Show nearby poly matches for debugging
                    let nearby: Vec<_> = poly_lookup.keys()
                        .filter(|k| k.starts_with(&format!("{}:", date)))
                        .take(3)
                        .collect();

                    println!("  ❌ {} vs {} @ {} (norm: {} vs {})", team1, team2, date, norm1, norm2);
                    if !nearby.is_empty() {
                        println!("     Poly has on {}: {:?}", date, nearby);
                    }
                    missed += 1;
                }
            }
        }

        println!("\n   Summary: {} matched, {} missed\n", matched, missed);
    }

    Ok(())
}

fn parse_tokens(clob_ids: &Option<String>) -> Option<(String, String)> {
    let ids: Vec<String> = clob_ids.as_ref()
        .and_then(|s| serde_json::from_str(s).ok())?;
    if ids.len() >= 2 {
        Some((ids[0].clone(), ids[1].clone()))
    } else {
        None
    }
}

/// Parse Polymarket event title
fn parse_poly_title(title: &str) -> Option<(String, String)> {
    // Pattern: "Counter-Strike: Team1 vs Team2 (BO3) - Tournament"
    let patterns = [
        r"(?i):\s*(.+?)\s+vs\.?\s+(.+?)\s*\(",      // "CS: Team1 vs Team2 (BO3)"
        r"(?i):\s*(.+?)\s+vs\.?\s+(.+?)\s*-",       // "CS: Team1 vs Team2 - Tournament"
        r"(?i)(.+?)\s+vs\.?\s+(.+?)\s*\(",          // "Team1 vs Team2 (BO3)"
    ];

    for pattern in patterns {
        if let Ok(re) = Regex::new(pattern) {
            if let Some(caps) = re.captures(title) {
                return Some((
                    caps.get(1)?.as_str().trim().to_string(),
                    caps.get(2)?.as_str().trim().to_string(),
                ));
            }
        }
    }
    None
}

/// Extract date from Polymarket slug: "cs2-team1-team2-2026-01-16" -> "2026-01-16"
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

/// Parse Kalshi esports event title
fn parse_esports_event(title: &str, ticker: &str) -> Option<(String, String, String)> {
    let re = Regex::new(r"(?i):\s*(.+?)\s+vs\.?\s+(.+)$").ok()?;
    let caps = re.captures(title)?;
    let team1 = caps.get(1)?.as_str().trim().to_string();
    let team2 = caps.get(2)?.as_str().trim().to_string();
    let date = extract_date_from_ticker(ticker)?;
    Some((team1, team2, date))
}

/// Extract date from Kalshi ticker like "KXCS2GAME-26JAN16FURIA9INE" -> "2026-01-16"
fn extract_date_from_ticker(ticker: &str) -> Option<String> {
    let parts: Vec<&str> = ticker.split('-').collect();
    if parts.len() < 2 { return None; }
    let date_part = parts[1];
    if date_part.len() < 7 { return None; }

    let year = format!("20{}", &date_part[..2]);
    let month = match &date_part[2..5].to_uppercase()[..] {
        "JAN" => "01", "FEB" => "02", "MAR" => "03", "APR" => "04",
        "MAY" => "05", "JUN" => "06", "JUL" => "07", "AUG" => "08",
        "SEP" => "09", "OCT" => "10", "NOV" => "11", "DEC" => "12",
        _ => return None,
    };
    let day = &date_part[5..7];
    Some(format!("{}-{}-{}", year, month, day))
}

/// Normalize team name for matching
fn normalize_team(name: &str) -> String {
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
