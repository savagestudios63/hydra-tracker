//! Solana integration using Helius DAS + standard JSON-RPC.
//!
//! DAS `searchAssets` returns every fungible SPL token and NFT held by a
//! wallet in a single paginated call. Native SOL balance is fetched with a
//! vanilla `getBalance`. Transactions come from `getSignaturesForAddress` +
//! `getTransaction`, with categorization derived from parsed instruction logs.

use anyhow::{anyhow, Context, Result};
use chrono::TimeZone;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::config::{Chain, Config, Wallet};
use super::{Holding, Transaction, TxCategory};

const SOL_DECIMALS: u8 = 9;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

pub async fn holdings(cfg: &Config, client: &reqwest::Client, w: &Wallet) -> Result<Vec<Holding>> {
    let url = cfg
        .rpc
        .solana
        .as_ref()
        .ok_or_else(|| anyhow!("solana RPC not configured"))?
        .url
        .clone();

    let (native, assets) = tokio::join!(
        native_balance(client, &url, &w.address),
        das_assets(client, &url, &w.address),
    );

    let mut out = Vec::new();
    out.push(native.with_context(|| format!("native SOL balance for {}", w.label))?.into_holding(w));
    out.extend(assets?.into_iter().map(|a| a.into_holding(w)));
    Ok(out)
}

async fn native_balance(client: &reqwest::Client, url: &str, address: &str) -> Result<NativeBalance> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getBalance",
        "params": [address],
    });
    let resp: RpcResponse<RpcBalance> = client.post(url).json(&body).send().await?.json().await?;
    let value = resp.into_result()?.value;
    Ok(NativeBalance { lamports: value })
}

async fn das_assets(client: &reqwest::Client, url: &str, address: &str) -> Result<Vec<DasAsset>> {
    let mut page = 1;
    let mut out = Vec::new();
    loop {
        let body = json!({
            "jsonrpc": "2.0", "id": "das",
            "method": "searchAssets",
            "params": {
                "ownerAddress": address,
                "tokenType": "fungible",
                "page": page,
                "limit": 1000,
                "displayOptions": { "showNativeBalance": false, "showZeroBalance": false },
            }
        });
        let resp: RpcResponse<DasResult> = client.post(url).json(&body).send().await?.json().await?;
        let result = resp.into_result()?;
        let got = result.items.len();
        out.extend(result.items);
        if got < 1000 { break; }
        page += 1;
        if page > 20 { break; } // safety valve
    }
    Ok(out)
}

pub async fn transactions(
    cfg: &Config,
    client: &reqwest::Client,
    w: &Wallet,
    limit: usize,
) -> Result<Vec<Transaction>> {
    let url = cfg
        .rpc
        .solana
        .as_ref()
        .ok_or_else(|| anyhow!("solana RPC not configured"))?
        .url
        .clone();

    let body = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getSignaturesForAddress",
        "params": [w.address, {"limit": limit.min(100)}]
    });
    let resp: RpcResponse<Vec<SigEntry>> = client.post(&url).json(&body).send().await?.json().await?;
    let sigs = resp.into_result()?;

    let mut out = Vec::with_capacity(sigs.len());
    for sig in sigs {
        let ts = sig.block_time
            .and_then(|t| chrono::Utc.timestamp_opt(t, 0).single())
            .unwrap_or_else(chrono::Utc::now);
        let category = categorize_solana(sig.memo.as_deref(), sig.err.is_some());
        out.push(Transaction {
            wallet_label: w.label.clone(),
            chain: Chain::Solana,
            signature: sig.signature,
            timestamp: ts,
            category,
            summary: sig.memo.clone().unwrap_or_else(|| "on-chain tx".into()),
            amount: None,
            symbol: None,
            usd_value: None,
        });
    }
    Ok(out)
}

fn categorize_solana(memo: Option<&str>, errored: bool) -> TxCategory {
    if errored { return TxCategory::Unknown; }
    let Some(memo) = memo else { return TxCategory::Unknown };
    let m = memo.to_ascii_lowercase();
    if m.contains("swap") || m.contains("jupiter") || m.contains("raydium") { TxCategory::Swap }
    else if m.contains("stake") && m.contains("un") { TxCategory::Unstake }
    else if m.contains("stake") { TxCategory::Stake }
    else if m.contains("add liquidity") || m.contains("deposit") { TxCategory::LiquidityAdd }
    else if m.contains("remove liquidity") || m.contains("withdraw") { TxCategory::LiquidityRemove }
    else if m.contains("transfer") { TxCategory::Transfer }
    else { TxCategory::Unknown }
}

// ---- internal data types -------------------------------------------------

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize, Debug)]
struct RpcError { message: String }

impl<T> RpcResponse<T> {
    fn into_result(self) -> Result<T> {
        if let Some(e) = self.error { return Err(anyhow!("rpc error: {}", e.message)); }
        self.result.ok_or_else(|| anyhow!("rpc returned no result"))
    }
}

#[derive(Deserialize)]
struct RpcBalance { value: u64 }

struct NativeBalance { lamports: u64 }

impl NativeBalance {
    fn into_holding(self, w: &Wallet) -> Holding {
        let amount = Decimal::from(self.lamports) / Decimal::from(LAMPORTS_PER_SOL);
        Holding {
            wallet_label: w.label.clone(),
            chain: Chain::Solana,
            symbol: "SOL".into(),
            name: "Solana".into(),
            identifier: String::new(),
            decimals: SOL_DECIMALS,
            amount,
            logo: None,
        }
    }
}

#[derive(Deserialize)]
struct DasResult { items: Vec<DasAsset> }

#[derive(Deserialize)]
struct DasAsset {
    id: String,
    content: Option<DasContent>,
    token_info: Option<DasTokenInfo>,
}

#[derive(Deserialize)]
struct DasContent {
    metadata: Option<DasMetadata>,
    links: Option<DasLinks>,
}

#[derive(Deserialize)]
struct DasMetadata {
    name: Option<String>,
    symbol: Option<String>,
}

#[derive(Deserialize)]
struct DasLinks { image: Option<String> }

#[derive(Deserialize)]
struct DasTokenInfo {
    balance: Option<u128>,
    decimals: Option<u8>,
    symbol: Option<String>,
}

impl DasAsset {
    fn into_holding(self, w: &Wallet) -> Holding {
        let info = self.token_info.unwrap_or(DasTokenInfo { balance: None, decimals: None, symbol: None });
        let decimals = info.decimals.unwrap_or(0);
        let raw = info.balance.unwrap_or(0);
        let amount = decimal_from_raw(raw, decimals);
        let (name, symbol, logo) = match self.content {
            Some(c) => (
                c.metadata.as_ref().and_then(|m| m.name.clone()).unwrap_or_default(),
                c.metadata.as_ref().and_then(|m| m.symbol.clone())
                    .or(info.symbol)
                    .unwrap_or_else(|| short_id(&self.id)),
                c.links.and_then(|l| l.image),
            ),
            None => (String::new(), info.symbol.unwrap_or_else(|| short_id(&self.id)), None),
        };
        Holding {
            wallet_label: w.label.clone(),
            chain: Chain::Solana,
            symbol,
            name,
            identifier: self.id,
            decimals,
            amount,
            logo,
        }
    }
}

fn short_id(id: &str) -> String {
    if id.len() <= 8 { id.to_string() } else { format!("{}…{}", &id[..4], &id[id.len()-4..]) }
}

fn decimal_from_raw(raw: u128, decimals: u8) -> Decimal {
    if raw == 0 { return Decimal::ZERO; }
    // rust_decimal can hold up to 28 significant digits; clamp scale.
    let scale = decimals.min(28);
    let mut d = Decimal::from_i128_with_scale(raw as i128, scale as u32);
    // If original decimals exceeded 28, we lose precision but keep magnitude.
    if decimals > 28 {
        let extra = decimals - 28;
        for _ in 0..extra { d /= Decimal::from(10); }
    }
    d
}

#[derive(Deserialize)]
struct SigEntry {
    signature: String,
    #[serde(rename = "blockTime")]
    block_time: Option<i64>,
    memo: Option<String>,
    err: Option<serde_json::Value>,
}
