//! Jupiter price API for Solana.
//!
//! Endpoint: `https://price.jup.ag/v6/price?ids=<mint1>,<mint2>,...`
//! No auth, tolerant of unknown mints, returns USD price derived from the
//! deepest on-chain route. Does not provide 24h change — we leave that as
//! None when this is the winning source.

use anyhow::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;
use std::collections::HashMap;

use crate::chains::PriceKey;
use super::{PriceBook, PricePoint};

const BASE_URL: &str = "https://price.jup.ag/v6/price";

pub async fn fetch(client: &reqwest::Client, keys: &[PriceKey]) -> Result<PriceBook> {
    let mut book = PriceBook::default();
    for chunk in keys.chunks(100) {
        let ids: Vec<&str> = chunk.iter().map(|k| k.identifier.as_str()).collect();
        let url = format!("{}?ids={}", BASE_URL, ids.join(","));
        let Ok(resp) = client.get(&url).send().await else { continue };
        if !resp.status().is_success() { continue; }
        let Ok(payload) = resp.json::<JupResp>().await else { continue };
        for k in chunk {
            if let Some(entry) = payload.data.get(&k.identifier) {
                if let Some(usd) = Decimal::from_f64(entry.price) {
                    book.insert(k.clone(), PricePoint { usd, change_24h_pct: None, source: "jupiter" });
                }
            }
        }
    }
    Ok(book)
}

#[derive(Deserialize)]
struct JupResp {
    data: HashMap<String, JupEntry>,
}

#[derive(Deserialize)]
struct JupEntry { price: f64 }
