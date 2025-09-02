//! DexScreener fallback pricing.
//!
//! DexScreener's `/tokens/{chain}/{address}` endpoint returns an array of
//! pools. We pick the one with the highest 24h USD volume (best liquidity
//! signal) and use its price + priceChange.h24. DexScreener is aggressively
//! rate-limited (≈60/min shared), so we only call it for tokens that neither
//! CoinGecko nor Jupiter resolved.

use anyhow::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;

use crate::chains::PriceKey;
use crate::config::Chain;
use super::{PriceBook, PricePoint};

const BASE_URL: &str = "https://api.dexscreener.com/latest/dex/tokens";

pub async fn fetch(client: &reqwest::Client, keys: &[PriceKey]) -> Result<PriceBook> {
    let mut book = PriceBook::default();
    // DexScreener accepts up to 30 comma-joined addresses per request but the
    // response shape is "pairs where either base or quote matches any input",
    // so we post-process by identifier.
    for chunk in keys.chunks(30) {
        let Some(chain_slug) = ds_chain(chunk[0].chain) else { continue };
        // All keys in a chunk must share a chain for the URL to be valid.
        let same_chain: Vec<_> = chunk.iter().filter(|k| k.chain == chunk[0].chain).collect();
        let ids: Vec<&str> = same_chain.iter().map(|k| k.identifier.as_str()).collect();
        if ids.is_empty() { continue; }
        let url = format!("{}/{}", BASE_URL, ids.join(","));
        let Ok(resp) = client.get(&url).send().await else { continue };
        if !resp.status().is_success() { continue; }
        let Ok(payload) = resp.json::<DsResp>().await else { continue };

        for k in &same_chain {
            let id_l = k.identifier.to_ascii_lowercase();
            let best = payload.pairs.iter()
                .filter(|p| p.chain_id.eq_ignore_ascii_case(chain_slug))
                .filter(|p| p.base_token.address.eq_ignore_ascii_case(&id_l)
                    || p.quote_token.address.eq_ignore_ascii_case(&id_l))
                .max_by(|a, b| vol_of(a).partial_cmp(&vol_of(b)).unwrap_or(std::cmp::Ordering::Equal));
            if let Some(p) = best {
                let raw_price = p.price_usd.as_deref().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let usd = Decimal::from_f64(raw_price).unwrap_or_default();
                if !usd.is_zero() {
                    book.insert((*k).clone(), PricePoint {
                        usd,
                        change_24h_pct: p.price_change.as_ref().and_then(|c| c.h24),
                        source: "dexscreener",
                    });
                }
            }
        }
    }
    Ok(book)
}

fn vol_of(p: &Pair) -> f64 { p.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0) }

fn ds_chain(c: Chain) -> Option<&'static str> {
    match c {
        Chain::Solana => Some("solana"),
        Chain::Ethereum => Some("ethereum"),
        Chain::Base => Some("base"),
        Chain::Arbitrum => Some("arbitrum"),
    }
}

#[derive(Deserialize)]
struct DsResp {
    #[serde(default)]
    pairs: Vec<Pair>,
}

#[derive(Deserialize)]
struct Pair {
    #[serde(rename = "chainId")]
    chain_id: String,
    #[serde(rename = "baseToken")]
    base_token: Token,
    #[serde(rename = "quoteToken")]
    quote_token: Token,
    #[serde(rename = "priceUsd")]
    price_usd: Option<String>,
    #[serde(rename = "priceChange")]
    price_change: Option<PriceChange>,
    volume: Option<Volume>,
}

#[derive(Deserialize)]
struct Token { address: String }

#[derive(Deserialize)]
struct PriceChange { h24: Option<f64> }

#[derive(Deserialize)]
struct Volume { h24: Option<f64> }
