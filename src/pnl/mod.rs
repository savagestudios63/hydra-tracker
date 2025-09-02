//! Profit / loss tracking.
//!
//! Cost basis is computed with a **weighted-average** method per
//! (wallet, chain, token-identifier) lot. Every BUY (positive amount with
//! usd_value known) increases total-cost and total-qty; every SELL (negative)
//! realizes PnL against the current average cost, leaving average cost per
//! remaining unit unchanged (this is the standard weighted-average rule).
//!
//! A rolling history of portfolio values is persisted to disk so the PnL
//! sparkline can survive process restarts.

use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::chains::{Holding, PriceKey, Transaction};
use crate::config::{Chain, Config};
use crate::pricing::PriceBook;

#[derive(Debug, Clone, Default)]
pub struct Lot {
    pub qty: Decimal,
    pub avg_cost_usd: Decimal,
    pub realized_pnl_usd: Decimal,
}

#[derive(Debug, Clone, Default)]
pub struct PnlReport {
    /// per-lot accounting, keyed by (wallet, chain, identifier).
    pub lots: HashMap<String, Lot>,
    /// Quick lookup: key = same as lots.
    pub total_realized: Decimal,
    pub total_unrealized: Decimal,
}

/// Build cost-basis lots from chronological transactions.
///
/// Transactions are expected newest-first from the RPCs; we reverse them
/// internally so the weighted average is computed in the order trades
/// actually settled.
pub fn compute(
    holdings: &[Holding],
    transactions: &[Transaction],
    prices: &PriceBook,
) -> PnlReport {
    let mut lots: HashMap<String, Lot> = HashMap::new();

    let mut sorted: Vec<&Transaction> = transactions.iter().collect();
    sorted.sort_by_key(|t| t.timestamp);
    for tx in sorted {
        let Some(amount) = tx.amount else { continue };
        let Some(usd) = tx.usd_value else { continue };
        let symbol_or_id = tx.symbol.clone().unwrap_or_default();
        let key = lot_key(&tx.wallet_label, tx.chain, &symbol_or_id);
        let lot = lots.entry(key).or_default();
        if amount > Decimal::ZERO {
            // Buy / receive with cost.
            let new_qty = lot.qty + amount;
            if new_qty > Decimal::ZERO {
                lot.avg_cost_usd = (lot.qty * lot.avg_cost_usd + usd) / new_qty;
            }
            lot.qty = new_qty;
        } else if amount < Decimal::ZERO {
            // Sell. Realized = (proceeds - qty_sold * avg_cost).
            let qty_sold = -amount;
            let cost_of_sold = qty_sold * lot.avg_cost_usd;
            lot.realized_pnl_usd += usd - cost_of_sold;
            lot.qty -= qty_sold;
            if lot.qty < Decimal::ZERO { lot.qty = Decimal::ZERO; }
        }
    }

    // Unrealized: current_value - qty_held * avg_cost, for any holding whose
    // lot we tracked. Holdings without a corresponding buy history have
    // unknown cost basis and contribute zero to unrealized PnL.
    let mut total_realized = Decimal::ZERO;
    let mut total_unrealized = Decimal::ZERO;
    for lot in lots.values() { total_realized += lot.realized_pnl_usd; }

    for h in holdings {
        let key = lot_key(&h.wallet_label, h.chain, &h.symbol);
        let Some(lot) = lots.get(&key) else { continue };
        if lot.qty.is_zero() { continue; }
        let Some(price) = prices.get(&h.price_key()) else { continue };
        let market_value = h.amount * price.usd;
        let basis_value = h.amount.min(lot.qty) * lot.avg_cost_usd;
        total_unrealized += market_value - basis_value;
    }

    PnlReport { lots, total_realized, total_unrealized }
}

fn lot_key(wallet: &str, chain: Chain, sym_or_id: &str) -> String {
    format!("{}|{}|{}", wallet, chain.short(), sym_or_id)
}

// --- history persistence ---------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub ts: DateTime<Utc>,
    pub total_usd: f64,
}

pub fn load_history(cfg: &Config) -> Vec<Snapshot> {
    let path = history_file(cfg);
    let Ok(text) = fs::read_to_string(&path) else { return Vec::new() };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Snapshot>(l).ok())
        .collect()
}

pub fn append_snapshot(cfg: &Config, snap: &Snapshot) -> Result<()> {
    let path = history_file(cfg);
    if let Some(p) = path.parent() { fs::create_dir_all(p).ok(); }
    let mut line = serde_json::to_string(snap)?;
    line.push('\n');
    use std::io::Write;
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn history_file(cfg: &Config) -> PathBuf {
    crate::config::history_dir(cfg).join("snapshots.ndjson")
}

/// Helper used by the UI when it needs a per-holding price lookup keyed by
/// symbol rather than price-key. Returns `None` if the token is unpriced.
pub fn current_value(h: &Holding, prices: &PriceBook) -> Option<Decimal> {
    let p = prices.get(&PriceKey {
        chain: h.chain,
        identifier: h.identifier.clone(),
        symbol: h.symbol.clone(),
    })?;
    Some(h.amount * p.usd)
}
