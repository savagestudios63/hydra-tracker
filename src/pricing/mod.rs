//! Pricing aggregation: CoinGecko -> Jupiter -> DexScreener.
//!
//! Each provider is queried in order; the first non-empty result for a given
//! `PriceKey` wins. The result is a `PriceBook` keyed by `PriceKey`, shared
//! across Holdings and PnL computations.

pub mod coingecko;
pub mod dexscreener;
pub mod jupiter;

use std::collections::HashMap;

use anyhow::Result;
use rust_decimal::Decimal;

use crate::chains::PriceKey;
use crate::config::{Chain, Config};

#[derive(Debug, Clone, Default)]
pub struct PricePoint {
    pub usd: Decimal,
    pub change_24h_pct: Option<f64>,
    pub source: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct PriceBook {
    map: HashMap<PriceKey, PricePoint>,
}

impl PriceBook {
    pub fn get(&self, k: &PriceKey) -> Option<&PricePoint> { self.map.get(k) }
    pub fn insert(&mut self, k: PriceKey, p: PricePoint) { self.map.entry(k).or_insert(p); }
    pub fn len(&self) -> usize { self.map.len() }
    pub fn merge(&mut self, other: PriceBook) {
        for (k, v) in other.map { self.insert(k, v); }
    }
}

pub async fn fetch_all(
    cfg: &Config,
    client: &reqwest::Client,
    keys: &[PriceKey],
) -> Result<PriceBook> {
    let mut book = PriceBook::default();

    // 1. CoinGecko — broad coverage of both Solana SPL and EVM ERC-20 via
    //    platform contract lookup, plus canonical coin IDs for natives.
    if let Ok(cg) = coingecko::fetch(cfg, client, keys).await {
        book.merge(cg);
    }

    // 2. Jupiter — authoritative for Solana long-tail (post-LFG, many tokens
    //    aren't on CoinGecko for hours/days).
    let sol_missing: Vec<_> = keys.iter()
        .filter(|k| k.chain == Chain::Solana && book.get(k).is_none() && !k.identifier.is_empty())
        .cloned()
        .collect();
    if !sol_missing.is_empty() {
        if let Ok(jp) = jupiter::fetch(client, &sol_missing).await {
            book.merge(jp);
        }
    }

    // 3. DexScreener fallback — last-resort pool-based pricing for anything
    //    still unpriced. No API key, aggressive rate limits, so we only call
    //    it for tokens neither CoinGecko nor Jupiter returned.
    let still_missing: Vec<_> = keys.iter()
        .filter(|k| book.get(k).is_none() && !k.identifier.is_empty())
        .cloned()
        .collect();
    if !still_missing.is_empty() {
        if let Ok(dx) = dexscreener::fetch(client, &still_missing).await {
            book.merge(dx);
        }
    }
    Ok(book)
}
