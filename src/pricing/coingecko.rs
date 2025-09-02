//! CoinGecko pricing provider.
//!
//! Two endpoints in play:
//!   - `/simple/token_price/{platform}` for contract-address lookups (EVM + SPL)
//!   - `/simple/price?ids=...` for canonical coin IDs (native coins)
//!
//! We batch per-chain to minimize round-trips and respect the free-tier
//! rate limit (~30 req/min). The paid key, if provided, raises this ceiling.

use std::collections::HashMap;

use anyhow::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde_json::Value;

use crate::chains::PriceKey;
use crate::config::{Chain, Config};
use super::{PriceBook, PricePoint};

const BASE_URL: &str = "https://api.coingecko.com/api/v3";

pub async fn fetch(cfg: &Config, client: &reqwest::Client, keys: &[PriceKey]) -> Result<PriceBook> {
    let mut book = PriceBook::default();

    // Partition by chain.
    let mut by_chain: HashMap<Chain, Vec<&PriceKey>> = HashMap::new();
    let mut native_ids: Vec<(&PriceKey, &'static str)> = Vec::new();
    for k in keys {
        if k.identifier.is_empty() {
            // Native coin — use coin id lookup.
            let id = match k.chain {
                Chain::Solana => "solana",
                Chain::Ethereum | Chain::Base | Chain::Arbitrum => "ethereum",
            };
            native_ids.push((k, id));
        } else {
            by_chain.entry(k.chain).or_default().push(k);
        }
    }

    // --- Native coins --------------------------------------------------------
    if !native_ids.is_empty() {
        let mut ids: Vec<&str> = native_ids.iter().map(|(_, id)| *id).collect();
        ids.sort();
        ids.dedup();
        let url = format!(
            "{}/simple/price?ids={}&vs_currencies={}&include_24hr_change=true",
            BASE_URL,
            ids.join(","),
            cfg.general.currency,
        );
        let req = add_key(client.get(&url), &cfg.pricing.coingecko_api_key);
        if let Ok(resp) = req.send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<Value>().await {
                    for (k, id) in &native_ids {
                        if let Some(p) = parse_simple_price(&v, id, &cfg.general.currency) {
                            book.insert((*k).clone(), p);
                        }
                    }
                }
            }
        }
    }

    // --- Contract-address lookups per platform -------------------------------
    for (chain, ks) in by_chain {
        let Some(platform) = coingecko_platform(chain) else { continue };
        // CoinGecko free endpoint caps addresses per call (~100 is safe).
        for chunk in ks.chunks(100) {
            let contracts: Vec<&str> = chunk.iter().map(|k| k.identifier.as_str()).collect();
            let url = format!(
                "{}/simple/token_price/{}?contract_addresses={}&vs_currencies={}&include_24hr_change=true",
                BASE_URL,
                platform,
                contracts.join(","),
                cfg.general.currency,
            );
            let req = add_key(client.get(&url), &cfg.pricing.coingecko_api_key);
            let Ok(resp) = req.send().await else { continue };
            if !resp.status().is_success() { continue; }
            let Ok(v) = resp.json::<Value>().await else { continue };
            for k in chunk {
                if let Some(p) = parse_simple_price(&v, &k.identifier, &cfg.general.currency) {
                    book.insert((*k).clone(), p);
                }
            }
        }
    }

    Ok(book)
}

fn coingecko_platform(chain: Chain) -> Option<&'static str> {
    match chain {
        Chain::Solana => Some("solana"),
        Chain::Ethereum => Some("ethereum"),
        Chain::Base => Some("base"),
        Chain::Arbitrum => Some("arbitrum-one"),
    }
}

fn parse_simple_price(v: &Value, id: &str, vs: &str) -> Option<PricePoint> {
    let entry = v.get(id).or_else(|| v.get(id.to_ascii_lowercase().as_str()))?;
    let price = entry.get(vs)?.as_f64()?;
    let change = entry.get(&format!("{}_24h_change", vs)).and_then(|c| c.as_f64());
    let usd = Decimal::from_f64(price).unwrap_or_default();
    Some(PricePoint { usd, change_24h_pct: change, source: "coingecko" })
}

fn add_key(rb: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    if key.is_empty() { rb } else { rb.header("x-cg-pro-api-key", key) }
}

