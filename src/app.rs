//! Application state + refresh orchestration.
//!
//! `AppState` is the source of truth the UI renders from. It owns the
//! holdings / transactions / prices / pnl snapshots and provides a
//! `spawn_refresh` that fires all the RPC + pricing calls concurrently on a
//! background tokio task; completion is signaled through an mpsc channel so
//! the UI thread can swap in the new data on its next tick without blocking.

use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use chrono::Utc;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use tokio::sync::mpsc;

use crate::{
    chains::{self, Holding, PriceKey, Transaction},
    config::{Chain, Config, Wallet},
    pnl::{self, PnlReport, Snapshot},
    pricing::{self, PriceBook},
};

pub struct AppState {
    pub cfg: Config,
    client: reqwest::Client,
    inner: Arc<RwLock<Inner>>,
    refresh_tx: mpsc::UnboundedSender<RefreshResult>,
    refresh_rx: mpsc::UnboundedReceiver<RefreshResult>,
    /// UI state that doesn't need to be shared with the background refresh task.
    pub ui: UiState,
}

#[derive(Default)]
pub struct Inner {
    pub holdings: Vec<Holding>,
    pub transactions: Vec<Transaction>,
    pub prices: PriceBook,
    pub pnl: PnlReport,
    pub history: Vec<Snapshot>,
    pub last_refresh: Option<chrono::DateTime<Utc>>,
    pub refreshing: bool,
    pub last_error: Option<String>,
}

pub struct UiState {
    pub active_tab: Tab,
    pub wallet_cursor: usize,
    pub holdings_cursor: usize,
    pub transactions_cursor: usize,
    pub pnl_range: PnlRange,
    pub sort_mode: SortMode,
    pub filter: String,
    pub input_mode: InputMode,
    pub command_buffer: String,
    pub status_message: Option<String>,
    pub last_keys: String, // for gg detection
    pub selected_wallet_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab { Holdings, Transactions, Pnl }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PnlRange { Week, Month, All }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode { Value, Change24h, Balance, Symbol, Pnl }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode { Normal, Filter, Command }

impl Default for UiState {
    fn default() -> Self {
        Self {
            active_tab: Tab::Holdings,
            wallet_cursor: 0,
            holdings_cursor: 0,
            transactions_cursor: 0,
            pnl_range: PnlRange::Week,
            sort_mode: SortMode::Value,
            filter: String::new(),
            input_mode: InputMode::Normal,
            command_buffer: String::new(),
            status_message: None,
            last_keys: String::new(),
            selected_wallet_only: false,
        }
    }
}

pub struct RefreshResult {
    pub holdings: Vec<Holding>,
    pub transactions: Vec<Transaction>,
    pub prices: PriceBook,
    pub error: Option<String>,
}

impl AppState {
    pub async fn new(cfg: Config) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(format!("hydra-tracker/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(RwLock::new(Inner::default()));
        inner.write().history = pnl::load_history(&cfg);
        Ok(Self {
            cfg,
            client,
            inner,
            refresh_tx: tx,
            refresh_rx: rx,
            ui: UiState::default(),
        })
    }

    pub fn inner(&self) -> Arc<RwLock<Inner>> { self.inner.clone() }

    /// Spawn an async refresh. Safe to call repeatedly — overlapping calls
    /// just stack; the latest result replaces the previous snapshot.
    pub fn spawn_refresh(&self) {
        let client = self.client.clone();
        let cfg = self.cfg.clone();
        let wallets = self.cfg.wallets.clone();
        let tx = self.refresh_tx.clone();
        let inner = self.inner.clone();
        inner.write().refreshing = true;
        tokio::spawn(async move {
            let result = do_refresh(&cfg, &client, &wallets).await;
            let _ = tx.send(result);
            inner.write().refreshing = false;
        });
    }

    /// Blocking-ish refresh used by `--once` and `--export-csv`.
    pub async fn refresh_now(&mut self) -> Result<()> {
        let result = do_refresh(&self.cfg, &self.client, &self.cfg.wallets).await;
        self.apply(result);
        Ok(())
    }

    /// Drain any completed background refreshes. Returns true if state changed.
    pub fn drain_refreshes(&mut self) -> bool {
        let mut changed = false;
        while let Ok(r) = self.refresh_rx.try_recv() {
            self.apply(r);
            changed = true;
        }
        changed
    }

    fn apply(&mut self, r: RefreshResult) {
        let total = total_usd(&r.holdings, &r.prices);
        let pnl = pnl::compute(&r.holdings, &r.transactions, &r.prices);
        let snap = Snapshot { ts: Utc::now(), total_usd: total.to_f64().unwrap_or(0.0) };
        if let Err(e) = pnl::append_snapshot(&self.cfg, &snap) {
            tracing::warn!(?e, "failed to persist snapshot");
        }

        let mut g = self.inner.write();
        g.holdings = r.holdings;
        g.transactions = r.transactions;
        g.prices = r.prices;
        g.pnl = pnl;
        g.history.push(snap);
        g.last_refresh = Some(Utc::now());
        g.last_error = r.error;
    }

    pub fn print_summary(&self) {
        let g = self.inner.read();
        let total = total_usd(&g.holdings, &g.prices);
        println!("Total: ${:.2}   ({} holdings, {} wallets)",
            total.to_f64().unwrap_or(0.0),
            g.holdings.len(),
            self.cfg.wallets.len(),
        );
        for w in &self.cfg.wallets {
            let sum: Decimal = g.holdings.iter()
                .filter(|h| h.wallet_label == w.label)
                .filter_map(|h| pnl::current_value(h, &g.prices))
                .sum();
            println!("  {:<14} {:>5}  ${:>12.2}",
                w.label, w.chain.short(), sum.to_f64().unwrap_or(0.0));
        }
    }

    pub fn export_csv(&self, path: &Path) -> Result<()> {
        let g = self.inner.read();
        let mut w = csv::Writer::from_path(path)?;
        w.write_record(["wallet","chain","symbol","name","balance","price_usd","value_usd","change_24h"])?;
        for h in &g.holdings {
            let price = g.prices.get(&h.price_key());
            let px = price.map(|p| p.usd).unwrap_or(Decimal::ZERO);
            let val = h.amount * px;
            let ch = price.and_then(|p| p.change_24h_pct).map(|f| format!("{:.2}", f)).unwrap_or_default();
            w.write_record([
                &h.wallet_label,
                h.chain.short(),
                &h.symbol,
                &h.name,
                &h.amount.to_string(),
                &px.to_string(),
                &val.to_string(),
                &ch,
            ])?;
        }
        w.flush()?;
        Ok(())
    }
}

async fn do_refresh(cfg: &Config, client: &reqwest::Client, wallets: &[Wallet]) -> RefreshResult {
    let mut holdings: Vec<Holding> = Vec::new();
    let mut txs: Vec<Transaction> = Vec::new();
    let mut error: Option<String> = None;

    // Fan out per-wallet requests in parallel.
    let holdings_futures = wallets.iter().map(|w| {
        let c = client.clone();
        let cfg = cfg.clone();
        let w = w.clone();
        async move { (w.label.clone(), chains::fetch_holdings(&cfg, &c, &w).await) }
    });
    let tx_futures = wallets.iter().map(|w| {
        let c = client.clone();
        let cfg = cfg.clone();
        let w = w.clone();
        async move { (w.label.clone(), chains::fetch_transactions(&cfg, &c, &w, 50).await) }
    });

    let (holdings_results, tx_results) = tokio::join!(
        futures::future::join_all(holdings_futures),
        futures::future::join_all(tx_futures),
    );
    for (label, r) in holdings_results {
        match r {
            Ok(hs) => holdings.extend(hs),
            Err(e) => {
                let msg = format!("{}: {}", label, e);
                tracing::warn!("{}", msg);
                error = Some(msg);
            }
        }
    }
    for (label, r) in tx_results {
        match r {
            Ok(ts) => txs.extend(ts),
            Err(e) => tracing::debug!("tx fetch failed for {}: {}", label, e),
        }
    }

    // Build unique price keys and fetch.
    let mut seen: HashSet<(Chain, String)> = HashSet::new();
    let mut keys: Vec<PriceKey> = Vec::new();
    for h in &holdings {
        let k = (h.chain, h.identifier.clone());
        if seen.insert(k) {
            keys.push(h.price_key());
        }
    }
    let prices = match pricing::fetch_all(cfg, client, &keys).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("pricing failed: {}", e);
            PriceBook::default()
        }
    };

    RefreshResult { holdings, transactions: txs, prices, error }
}

pub fn total_usd(holdings: &[Holding], prices: &PriceBook) -> Decimal {
    holdings.iter().filter_map(|h| pnl::current_value(h, prices)).sum()
}
