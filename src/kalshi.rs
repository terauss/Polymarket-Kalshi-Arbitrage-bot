//! Kalshi platform integration client.
//!
//! This module provides REST API and WebSocket clients for interacting with
//! the Kalshi prediction market platform, including order execution and
//! real-time price feed management.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::{SinkExt, StreamExt};
use pkcs1::DecodeRsaPrivateKey;
use rsa::{
    pss::SigningKey,
    sha2::Sha256,
    signature::{RandomizedSigner, SignatureEncoding},
    RsaPrivateKey,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::{http::Request, Message}};
use tracing::{debug, error, info};

use crate::config::{KALSHI_WS_URL, KALSHI_API_BASE, KALSHI_API_DELAY_MS};
use crate::execution::NanoClock;
use crate::types::{
    KalshiEventsResponse, KalshiMarketsResponse, KalshiEvent, KalshiMarket,
    GlobalState, FastExecutionRequest, ArbType, PriceCents, SizeCents, fxhash_str,
};

// === Order Types ===

use std::borrow::Cow;
use std::fmt::Write;
use arrayvec::ArrayString;

#[derive(Debug, Clone, Serialize)]
pub struct KalshiOrderRequest<'a> {
    pub ticker: Cow<'a, str>,
    pub action: &'static str,
    pub side: &'static str,
    #[serde(rename = "type")]
    pub order_type: &'static str,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_price: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_price: Option<i64>,
    pub client_order_id: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_ts: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_in_force: Option<&'static str>,
}

impl<'a> KalshiOrderRequest<'a> {
    /// Create an IOC (immediate-or-cancel) buy order
    pub fn ioc_buy(ticker: Cow<'a, str>, side: &'static str, price_cents: i64, count: i64, client_order_id: Cow<'a, str>) -> Self {
        let (yes_price, no_price) = if side == "yes" {
            (Some(price_cents), None)
        } else {
            (None, Some(price_cents))
        };

        Self {
            ticker,
            action: "buy",
            side,
            order_type: "limit",
            count,
            yes_price,
            no_price,
            client_order_id,
            expiration_ts: None,
            time_in_force: Some("immediate_or_cancel"),
        }
    }

    /// Create an IOC (immediate-or-cancel) sell order
    pub fn ioc_sell(ticker: Cow<'a, str>, side: &'static str, price_cents: i64, count: i64, client_order_id: Cow<'a, str>) -> Self {
        let (yes_price, no_price) = if side == "yes" {
            (Some(price_cents), None)
        } else {
            (None, Some(price_cents))
        };

        Self {
            ticker,
            action: "sell",
            side,
            order_type: "limit",
            count,
            yes_price,
            no_price,
            client_order_id,
            expiration_ts: None,
            time_in_force: Some("immediate_or_cancel"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KalshiOrderResponse {
    pub order: KalshiOrderDetails,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct KalshiOrderDetails {
    pub order_id: String,
    pub ticker: String,
    pub status: String,        // "resting", "canceled", "executed", "pending"
    #[serde(default)]
    pub remaining_count: Option<i64>,
    #[serde(default)]
    pub queue_position: Option<i64>,
    pub action: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub yes_price: Option<i64>,
    pub no_price: Option<i64>,
    pub created_time: Option<String>,
    #[serde(default)]
    pub taker_fill_count: Option<i64>,
    #[serde(default)]
    pub maker_fill_count: Option<i64>,
    #[serde(default)]
    pub place_count: Option<i64>,
    #[serde(default)]
    pub taker_fill_cost: Option<i64>,
    #[serde(default)]
    pub maker_fill_cost: Option<i64>,
}

#[allow(dead_code)]
impl KalshiOrderDetails {
    /// Total filled contracts
    pub fn filled_count(&self) -> i64 {
        self.taker_fill_count.unwrap_or(0) + self.maker_fill_count.unwrap_or(0)
    }

    /// Check if order was fully filled
    pub fn is_filled(&self) -> bool {
        self.status == "executed" || self.remaining_count == Some(0)
    }

    /// Check if order was partially filled
    pub fn is_partial(&self) -> bool {
        self.filled_count() > 0 && !self.is_filled()
    }
}

// === Kalshi Auth Config ===

pub struct KalshiConfig {
    pub api_key_id: String,
    pub private_key: RsaPrivateKey,
}

impl KalshiConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        let api_key_id = std::env::var("KALSHI_API_KEY_ID").context("KALSHI_API_KEY_ID not set")?;
        // Support both KALSHI_PRIVATE_KEY_PATH and KALSHI_PRIVATE_KEY_FILE for compatibility
        let key_path = std::env::var("KALSHI_PRIVATE_KEY_PATH")
            .or_else(|_| std::env::var("KALSHI_PRIVATE_KEY_FILE"))
            .unwrap_or_else(|_| "kalshi_private_key.txt".to_string());
        let private_key_pem = std::fs::read_to_string(&key_path)
            .with_context(|| format!("Failed to read private key from {}", key_path))?
            .trim()
            .to_owned();
        let private_key = RsaPrivateKey::from_pkcs1_pem(&private_key_pem)
            .context("Failed to parse private key PEM")?;
        Ok(Self { api_key_id, private_key })
    }

    pub fn sign(&self, message: &str) -> Result<String> {
        tracing::debug!("[KALSHI-DEBUG] Signing message: {}", message);
        let signing_key = SigningKey::<Sha256>::new(self.private_key.clone());
        let signature = signing_key.sign_with_rng(&mut rand::thread_rng(), message.as_bytes());
        let sig_b64 = BASE64.encode(signature.to_bytes());
        tracing::debug!("[KALSHI-DEBUG] Signature (first 50 chars): {}...", &sig_b64[..50.min(sig_b64.len())]);
        Ok(sig_b64)
    }
}

// === Kalshi REST API Client ===

/// Timeout for order requests (shorter than general API timeout)
const ORDER_TIMEOUT: Duration = Duration::from_secs(5);

use std::sync::atomic::{AtomicU32, Ordering};

/// Global order counter for unique client_order_id generation
static ORDER_COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct KalshiApiClient {
    http: reqwest::Client,
    pub config: KalshiConfig,
}

impl KalshiApiClient {
    pub fn new(config: KalshiConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to build HTTP client"),
            config,
        }
    }

    #[inline]
    fn next_order_id() -> ArrayString<24> {
        let counter = ORDER_COUNTER.fetch_add(1, Ordering::Relaxed);
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut buf = ArrayString::<24>::new();
        let _ = write!(&mut buf, "a{}{}", ts, counter);
        buf
    }
    
    /// Generic authenticated GET request with retry on rate limit
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let mut retries = 0;
        const MAX_RETRIES: u32 = 5;

        loop {
            let url = format!("{}{}", KALSHI_API_BASE, path);
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            // Kalshi signature uses FULL path including /trade-api/v2 prefix
            let full_path = format!("/trade-api/v2{}", path);
            let signature = self.config.sign(&format!("{}GET{}", timestamp_ms, full_path))?;
            
            let resp = self.http
                .get(&url)
                .header("KALSHI-ACCESS-KEY", &self.config.api_key_id)
                .header("KALSHI-ACCESS-SIGNATURE", &signature)
                .header("KALSHI-ACCESS-TIMESTAMP", timestamp_ms.to_string())
                .send()
                .await?;
            
            let status = resp.status();
            
            // Handle rate limit with exponential backoff
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                retries += 1;
                if retries > MAX_RETRIES {
                    anyhow::bail!("Kalshi API rate limited after {} retries", MAX_RETRIES);
                }
                let backoff_ms = 2000 * (1 << retries); // 4s, 8s, 16s, 32s, 64s
                debug!("[KALSHI] Rate limited, backing off {}ms (retry {}/{})", 
                       backoff_ms, retries, MAX_RETRIES);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                continue;
            }
            
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Kalshi API error {}: {}", status, body);
            }
            
            let data: T = resp.json().await?;
            tokio::time::sleep(Duration::from_millis(KALSHI_API_DELAY_MS)).await;
            return Ok(data);
        }
    }
    
    pub async fn get_events(&self, series_ticker: &str, limit: u32) -> Result<Vec<KalshiEvent>> {
        let path = format!("/events?series_ticker={}&limit={}&status=open", series_ticker, limit);
        let resp: KalshiEventsResponse = self.get(&path).await?;
        Ok(resp.events)
    }
    
    pub async fn get_markets(&self, event_ticker: &str) -> Result<Vec<KalshiMarket>> {
        let path = format!("/markets?event_ticker={}", event_ticker);
        let resp: KalshiMarketsResponse = self.get(&path).await?;
        Ok(resp.markets)
    }

    /// Get markets created since a given timestamp for a series
    pub async fn get_markets_since(&self, series_ticker: &str, since_ts: u64) -> Result<Vec<KalshiMarket>> {
        let path = format!(
            "/markets?series_ticker={}&min_created_ts={}&status=open&limit=100",
            series_ticker, since_ts
        );
        let resp: KalshiMarketsResponse = self.get(&path).await?;
        Ok(resp.markets)
    }
    
    /// Generic authenticated POST request
    async fn post<T: serde::de::DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", KALSHI_API_BASE, path);
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Kalshi signature uses FULL path including /trade-api/v2 prefix
        let full_path = format!("/trade-api/v2{}", path);
        let msg = format!("{}POST{}", timestamp_ms, full_path);
        let signature = self.config.sign(&msg)?;

        let resp = self.http
            .post(&url)
            .header("KALSHI-ACCESS-KEY", &self.config.api_key_id)
            .header("KALSHI-ACCESS-SIGNATURE", &signature)
            .header("KALSHI-ACCESS-TIMESTAMP", timestamp_ms.to_string())
            .header("Content-Type", "application/json")
            .timeout(ORDER_TIMEOUT)
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Kalshi API error {}: {}", status, body);
        }
        
        let data: T = resp.json().await?;
        Ok(data)
    }
    
    /// Create an order on Kalshi
    pub async fn create_order(&self, order: &KalshiOrderRequest<'_>) -> Result<KalshiOrderResponse> {
        let path = "/portfolio/orders";
        self.post(path, order).await
    }
    
    /// Create an IOC buy order (convenience method)
    pub async fn buy_ioc(
        &self,
        ticker: &str,
        side: &str,  // "yes" or "no"
        price_cents: i64,
        count: i64,
    ) -> Result<KalshiOrderResponse> {
        debug_assert!(!ticker.is_empty(), "ticker must not be empty");
        debug_assert!(price_cents >= 1 && price_cents <= 99, "price must be 1-99");
        debug_assert!(count >= 1, "count must be >= 1");

        let side_static: &'static str = if side == "yes" { "yes" } else { "no" };
        let order_id = Self::next_order_id();
        let order = KalshiOrderRequest::ioc_buy(
            Cow::Borrowed(ticker),
            side_static,
            price_cents,
            count,
            Cow::Borrowed(&order_id)
        );
        debug!("[KALSHI] IOC {} {} @{}¢ x{}", side, ticker, price_cents, count);

        let resp = self.create_order(&order).await?;
        debug!("[KALSHI] {} filled={}", resp.order.status, resp.order.filled_count());
        Ok(resp)
    }

    pub async fn sell_ioc(
        &self,
        ticker: &str,
        side: &str,
        price_cents: i64,
        count: i64,
    ) -> Result<KalshiOrderResponse> {
        debug_assert!(!ticker.is_empty(), "ticker must not be empty");
        debug_assert!(price_cents >= 1 && price_cents <= 99, "price must be 1-99");
        debug_assert!(count >= 1, "count must be >= 1");

        let side_static: &'static str = if side == "yes" { "yes" } else { "no" };
        let order_id = Self::next_order_id();
        let order = KalshiOrderRequest::ioc_sell(
            Cow::Borrowed(ticker),
            side_static,
            price_cents,
            count,
            Cow::Borrowed(&order_id)
        );
        debug!("[KALSHI] SELL {} {} @{}¢ x{}", side, ticker, price_cents, count);

        let resp = self.create_order(&order).await?;
        debug!("[KALSHI] {} filled={}", resp.order.status, resp.order.filled_count());
        Ok(resp)
    }
}

// === WebSocket Message Types ===

#[derive(Deserialize, Debug)]
pub struct KalshiWsMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub msg: Option<KalshiWsMsgBody>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct KalshiWsMsgBody {
    pub market_ticker: Option<String>,
    // Snapshot fields - arrays of [price_cents, quantity]
    pub yes: Option<Vec<Vec<i64>>>,
    pub no: Option<Vec<Vec<i64>>>,
    // Delta fields
    pub price: Option<i64>,
    pub delta: Option<i64>,
    pub side: Option<String>,
}

#[derive(Serialize)]
struct SubscribeCmd {
    id: i32,
    cmd: &'static str,
    params: SubscribeParams,
}

#[derive(Serialize)]
struct SubscribeParams {
    channels: Vec<&'static str>,
    market_tickers: Vec<String>,
}

// =============================================================================
// WebSocket Runner
// =============================================================================

/// WebSocket runner
pub async fn run_ws(
    config: &KalshiConfig,
    state: Arc<GlobalState>,
    exec_tx: mpsc::Sender<FastExecutionRequest>,
    threshold_cents: PriceCents,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let tickers: Vec<String> = state.markets.iter()
        .take(state.market_count())
        .filter_map(|m| m.pair().map(|p| p.kalshi_market_ticker.to_string()))
        .collect();

    if tickers.is_empty() {
        info!("[KALSHI] No markets to monitor");
        tokio::time::sleep(Duration::from_secs(u64::MAX)).await;
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .to_string();

    let signature = config.sign(&format!("{}GET/trade-api/ws/v2", timestamp))?;

    let request = Request::builder()
        .uri(KALSHI_WS_URL)
        .header("KALSHI-ACCESS-KEY", &config.api_key_id)
        .header("KALSHI-ACCESS-SIGNATURE", &signature)
        .header("KALSHI-ACCESS-TIMESTAMP", &timestamp)
        .header("Host", "api.elections.kalshi.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .body(())?;

    let (ws_stream, _) = connect_async(request).await.context("Failed to connect to Kalshi")?;
    info!("[KALSHI] Connected");

    let (mut write, mut read) = ws_stream.split();

    // Subscribe to all tickers
    let subscribe_msg = SubscribeCmd {
        id: 1,
        cmd: "subscribe",
        params: SubscribeParams {
            channels: vec!["orderbook_delta"],
            market_tickers: tickers.clone(),
        },
    };

    write.send(Message::Text(serde_json::to_string(&subscribe_msg)?)).await?;
    info!("[KALSHI] Subscribed to {} markets", tickers.len());

    let clock = NanoClock::new();

    loop {
        tokio::select! {
            biased;

            // Check shutdown signal first
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("[KALSHI] Shutdown signal received, disconnecting...");
                    break;
                }
            }

            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<KalshiWsMessage>(&text) {
                            Ok(kalshi_msg) => {
                                let ticker = kalshi_msg.msg.as_ref()
                                    .and_then(|m| m.market_ticker.as_ref());

                                let Some(ticker) = ticker else { continue };
                                let ticker_hash = fxhash_str(ticker);

                                let Some(market_id) = state.kalshi_to_id.read().get(&ticker_hash).copied() else { continue };
                                let market = &state.markets[market_id as usize];

                                match kalshi_msg.msg_type.as_str() {
                                    "orderbook_snapshot" => {
                                        if let Some(body) = &kalshi_msg.msg {
                                            process_kalshi_snapshot(market, body);

                                            // Check for arbs
                                            let arb_mask = market.check_arbs(threshold_cents);
                                            if arb_mask != 0 {
                                                send_kalshi_arb_request(market_id, market, arb_mask, &exec_tx, &clock).await;
                                            }
                                        }
                                    }
                                    "orderbook_delta" => {
                                        if let Some(body) = &kalshi_msg.msg {
                                            process_kalshi_delta(market, body);

                                            let arb_mask = market.check_arbs(threshold_cents);
                                            if arb_mask != 0 {
                                                send_kalshi_arb_request(market_id, market, arb_mask, &exec_tx, &clock).await;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Err(e) => {
                                // Log at trace level - unknown message types are normal
                                tracing::trace!("[KALSHI] WS parse error: {} (msg: {}...)", e, &text[..text.len().min(100)]);
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Some(Err(e)) => {
                        error!("[KALSHI] WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

/// Process Kalshi orderbook snapshot
/// Note: Kalshi sends BIDS - to buy YES you pay (100 - best_NO_bid), to buy NO you pay (100 - best_YES_bid)
#[inline]
fn process_kalshi_snapshot(market: &crate::types::AtomicMarketState, body: &KalshiWsMsgBody) {
    // Find best YES bid (highest price) - this determines NO ask
    let (no_ask, no_size) = body.yes.as_ref()
        .and_then(|levels| {
            levels.iter()
                .filter_map(|l| {
                    if l.len() >= 2 && l[1] > 0 {  // Has quantity
                        Some((l[0], l[1]))  // (price, qty)
                    } else {
                        None
                    }
                })
                .max_by_key(|(p, _)| *p)  // Highest bid
                .map(|(price, qty)| {
                    let ask = (100 - price) as PriceCents;  // To buy NO, pay 100 - YES_bid
                    let size = (qty * price / 100) as SizeCents;
                    (ask, size)
                })
        })
        .unwrap_or((0, 0));

    // Find best NO bid (highest price) - this determines YES ask
    let (yes_ask, yes_size) = body.no.as_ref()
        .and_then(|levels| {
            levels.iter()
                .filter_map(|l| {
                    if l.len() >= 2 && l[1] > 0 {
                        Some((l[0], l[1]))
                    } else {
                        None
                    }
                })
                .max_by_key(|(p, _)| *p)
                .map(|(price, qty)| {
                    let ask = (100 - price) as PriceCents;  // To buy YES, pay 100 - NO_bid
                    let size = (qty * price / 100) as SizeCents;
                    (ask, size)
                })
        })
        .unwrap_or((0, 0));

    // Store
    market.kalshi.store(yes_ask, no_ask, yes_size, no_size);
}

/// Process Kalshi orderbook delta
/// Note: Deltas update bid levels; we recompute asks from best bids
#[inline]
fn process_kalshi_delta(market: &crate::types::AtomicMarketState, body: &KalshiWsMsgBody) {
    // For deltas, recompute from snapshot-like format
    // Kalshi deltas have yes/no as arrays of [price, new_qty]
    let (current_yes, current_no, current_yes_size, current_no_size) = market.kalshi.load();

    // Process YES bid updates (affects NO ask)
    let (no_ask, no_size) = if let Some(levels) = &body.yes {
        // Find best (highest) YES bid with non-zero quantity
        levels.iter()
            .filter_map(|l| {
                if l.len() >= 2 && l[1] > 0 {
                    Some((l[0], l[1]))
                } else {
                    None
                }
            })
            .max_by_key(|(p, _)| *p)
            .map(|(price, qty)| {
                let ask = (100 - price) as PriceCents;
                let size = (qty * price / 100) as SizeCents;
                (ask, size)
            })
            .unwrap_or((current_no, current_no_size))
    } else {
        (current_no, current_no_size)
    };

    // Process NO bid updates (affects YES ask)
    let (yes_ask, yes_size) = if let Some(levels) = &body.no {
        levels.iter()
            .filter_map(|l| {
                if l.len() >= 2 && l[1] > 0 {
                    Some((l[0], l[1]))
                } else {
                    None
                }
            })
            .max_by_key(|(p, _)| *p)
            .map(|(price, qty)| {
                let ask = (100 - price) as PriceCents;
                let size = (qty * price / 100) as SizeCents;
                (ask, size)
            })
            .unwrap_or((current_yes, current_yes_size))
    } else {
        (current_yes, current_yes_size)
    };

    market.kalshi.store(yes_ask, no_ask, yes_size, no_size);
}

/// Send arb request from Kalshi handler
#[inline]
async fn send_kalshi_arb_request(
    market_id: u16,
    market: &crate::types::AtomicMarketState,
    arb_mask: u8,
    exec_tx: &mpsc::Sender<FastExecutionRequest>,
    clock: &NanoClock,
) {
    let (k_yes, k_no, k_yes_size, k_no_size) = market.kalshi.load();
    let (p_yes, p_no, p_yes_size, p_no_size) = market.poly.load();

    let (yes_price, no_price, yes_size, no_size, arb_type) = if arb_mask & 1 != 0 {
        (p_yes, k_no, p_yes_size, k_no_size, ArbType::PolyYesKalshiNo)
    } else if arb_mask & 2 != 0 {
        (k_yes, p_no, k_yes_size, p_no_size, ArbType::KalshiYesPolyNo)
    } else if arb_mask & 4 != 0 {
        (p_yes, p_no, p_yes_size, p_no_size, ArbType::PolyOnly)
    } else if arb_mask & 8 != 0 {
        (k_yes, k_no, k_yes_size, k_no_size, ArbType::KalshiOnly)
    } else {
        return;
    };

    let req = FastExecutionRequest {
        market_id,
        yes_price,
        no_price,
        yes_size,
        no_size,
        arb_type,
        detected_ns: clock.now_ns(),
    };

    let _ = exec_tx.try_send(req);
}