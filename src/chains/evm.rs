//! EVM integration via the Alchemy Portfolio / Token API.
//!
//! Alchemy exposes `alchemy_getTokenBalances` (fast bulk balances) +
//! `alchemy_getTokenMetadata` (decimals + symbol) + `alchemy_getAssetTransfers`
//! (tx history with categorization). The native coin (ETH) is fetched via
//! `eth_getBalance`. Network routing is via separate RPC URLs per chain.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;
use serde_json::json;

use crate::config::{Chain, Config, Wallet};
use super::{Holding, Transaction, TxCategory};

const NATIVE_DECIMALS: u8 = 18;

pub async fn holdings(cfg: &Config, client: &reqwest::Client, w: &Wallet) -> Result<Vec<Holding>> {
    let url = endpoint(cfg, w.chain)?;
    let (native, tokens) = tokio::join!(
        native_balance(client, &url, w),
        token_balances(client, &url, w),
    );
    let mut out = vec![native.with_context(|| format!("native balance for {}", w.label))?];
    out.extend(tokens?);
    Ok(out)
}

pub async fn transactions(
    cfg: &Config,
    client: &reqwest::Client,
    w: &Wallet,
    limit: usize,
) -> Result<Vec<Transaction>> {
    let url = endpoint(cfg, w.chain)?;
    let categories = ["external", "erc20", "erc721", "erc1155"];
    let body = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "alchemy_getAssetTransfers",
        "params": [{
            "fromAddress": w.address,
            "category": categories,
            "withMetadata": true,
            "excludeZeroValue": true,
            "maxCount": format!("0x{:x}", limit.min(1000)),
            "order": "desc"
        }]
    });
    let resp: RpcResponse<TransfersResult> = client.post(&url).json(&body).send().await?.json().await?;
    let transfers = resp.into_result()?.transfers;

    let mut out = Vec::with_capacity(transfers.len());
    for t in transfers {
        let ts = t.metadata.as_ref()
            .and_then(|m| DateTime::parse_from_rfc3339(&m.block_timestamp).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let amount = t.value.and_then(|v| Decimal::from_f64(v));
        let from_me = t.from.eq_ignore_ascii_case(&w.address);
        let signed = amount.map(|a| if from_me { -a } else { a });
        let symbol = t.asset.clone();
        let summary = match (&symbol, &amount) {
            (Some(s), Some(v)) => format!("{} {:.4} {}", if from_me { "sent" } else { "recv" }, v, s),
            _ => t.category.clone().unwrap_or_else(|| "transfer".into()),
        };
        out.push(Transaction {
            wallet_label: w.label.clone(),
            chain: w.chain,
            signature: t.hash.clone(),
            timestamp: ts,
            category: categorize_evm(&t),
            summary,
            amount: signed,
            symbol,
            usd_value: None,
        });
    }
    Ok(out)
}

fn categorize_evm(t: &Transfer) -> TxCategory {
    match t.category.as_deref() {
        Some("erc20") | Some("external") => {
            // Heuristic: if `to` is a known DEX router, classify as swap.
            if let Some(to) = &t.to {
                let to_l = to.to_ascii_lowercase();
                if DEX_ROUTERS.iter().any(|r| r.eq_ignore_ascii_case(&to_l)) {
                    return TxCategory::Swap;
                }
            }
            TxCategory::Transfer
        }
        Some("erc721") | Some("erc1155") => TxCategory::Transfer,
        _ => TxCategory::Unknown,
    }
}

// A tiny, non-exhaustive set of known routers used purely as a heuristic.
// Address-based DEX detection should be swapped for a fuller label DB later.
const DEX_ROUTERS: &[&str] = &[
    "0x7a250d5630b4cf539739df2c5dacb4c659f2488d", // Uniswap V2
    "0xe592427a0aece92de3edee1f18e0157c05861564", // Uniswap V3
    "0x68b3465833fb72a70ecdf485e0e4c7bd8665fc45", // Uniswap universal router
    "0x1111111254eeb25477b68fb85ed929f73a960582", // 1inch
];

async fn native_balance(client: &reqwest::Client, url: &str, w: &Wallet) -> Result<Holding> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "eth_getBalance",
        "params": [w.address, "latest"]
    });
    let resp: RpcResponse<String> = client.post(url).json(&body).send().await?.json().await?;
    let hex_val = resp.into_result()?;
    let raw = parse_hex_u256_to_decimal(&hex_val, NATIVE_DECIMALS)?;
    Ok(Holding {
        wallet_label: w.label.clone(),
        chain: w.chain,
        symbol: w.chain.native_symbol().to_string(),
        name: w.chain.native_symbol().to_string(),
        identifier: String::new(),
        decimals: NATIVE_DECIMALS,
        amount: raw,
        logo: None,
    })
}

async fn token_balances(client: &reqwest::Client, url: &str, w: &Wallet) -> Result<Vec<Holding>> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "alchemy_getTokenBalances",
        "params": [w.address, "erc20"]
    });
    let resp: RpcResponse<TokenBalancesResult> = client.post(url).json(&body).send().await?.json().await?;
    let balances = resp.into_result()?.token_balances;
    let nonzero: Vec<_> = balances.into_iter()
        .filter(|b| b.token_balance.as_deref().map(|s| s != "0x0" && !s.is_empty()).unwrap_or(false))
        .collect();

    let mut out = Vec::with_capacity(nonzero.len());
    for tb in nonzero {
        let Some(balance_hex) = tb.token_balance.as_deref() else { continue };
        let meta = token_metadata(client, url, &tb.contract_address).await.ok();
        let (symbol, name, decimals, logo) = match meta {
            Some(m) => (
                m.symbol.unwrap_or_else(|| short_addr(&tb.contract_address)),
                m.name.unwrap_or_default(),
                m.decimals.unwrap_or(18),
                m.logo,
            ),
            None => (short_addr(&tb.contract_address), String::new(), 18, None),
        };
        let amount = parse_hex_u256_to_decimal(balance_hex, decimals)?;
        if amount.is_zero() { continue; }
        out.push(Holding {
            wallet_label: w.label.clone(),
            chain: w.chain,
            symbol,
            name,
            identifier: tb.contract_address.to_ascii_lowercase(),
            decimals,
            amount,
            logo,
        });
    }
    Ok(out)
}

async fn token_metadata(client: &reqwest::Client, url: &str, contract: &str) -> Result<TokenMetadata> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "alchemy_getTokenMetadata",
        "params": [contract]
    });
    let resp: RpcResponse<TokenMetadata> = client.post(url).json(&body).send().await?.json().await?;
    resp.into_result()
}

fn endpoint(cfg: &Config, chain: Chain) -> Result<String> {
    let ep = match chain {
        Chain::Ethereum => cfg.rpc.ethereum.as_ref(),
        Chain::Base => cfg.rpc.base.as_ref(),
        Chain::Arbitrum => cfg.rpc.arbitrum.as_ref(),
        Chain::Solana => return Err(anyhow!("EVM helper called for Solana")),
    };
    Ok(ep.ok_or_else(|| anyhow!("RPC for {} not configured", chain.short()))?.url.clone())
}

fn short_addr(a: &str) -> String {
    if a.len() < 10 { a.to_string() } else { format!("{}…{}", &a[..6], &a[a.len()-4..]) }
}

fn parse_hex_u256_to_decimal(hex_str: &str, decimals: u8) -> Result<Decimal> {
    let s = hex_str.trim_start_matches("0x");
    if s.is_empty() { return Ok(Decimal::ZERO); }
    // u128 can hold balances well beyond typical ERC-20 supplies scaled to 18 decimals
    // for any realistic retail wallet. For truly huge raw values we fall back to
    // integer-division-by-10^decimals performed on the hex string.
    if s.len() <= 32 {
        let raw = u128::from_str_radix(s, 16)
            .with_context(|| format!("parsing hex {}", hex_str))?;
        return Ok(scaled_decimal(raw, decimals));
    }
    // Fallback: walk digit by digit using u128 pieces. Loses precision beyond
    // 28 significant digits but keeps the magnitude correct for display.
    let mut value = Decimal::ZERO;
    let sixteen = Decimal::from(16);
    for ch in s.chars() {
        let d = ch.to_digit(16).ok_or_else(|| anyhow!("bad hex digit {}", ch))?;
        value = value * sixteen + Decimal::from(d);
    }
    for _ in 0..decimals { value /= Decimal::from(10); }
    Ok(value)
}

fn scaled_decimal(raw: u128, decimals: u8) -> Decimal {
    if raw == 0 { return Decimal::ZERO; }
    let scale = decimals.min(28);
    let mut d = Decimal::from_i128_with_scale(raw as i128, scale as u32);
    if decimals > 28 {
        for _ in 0..(decimals - 28) { d /= Decimal::from(10); }
    }
    d
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
struct TokenBalancesResult {
    #[serde(rename = "tokenBalances")]
    token_balances: Vec<TokenBalance>,
}

#[derive(Deserialize)]
struct TokenBalance {
    #[serde(rename = "contractAddress")]
    contract_address: String,
    #[serde(rename = "tokenBalance")]
    token_balance: Option<String>,
}

#[derive(Deserialize)]
struct TokenMetadata {
    decimals: Option<u8>,
    symbol: Option<String>,
    name: Option<String>,
    logo: Option<String>,
}

#[derive(Deserialize)]
struct TransfersResult { transfers: Vec<Transfer> }

#[derive(Deserialize)]
struct Transfer {
    hash: String,
    from: String,
    to: Option<String>,
    value: Option<f64>,
    asset: Option<String>,
    category: Option<String>,
    metadata: Option<TransferMeta>,
}

#[derive(Deserialize)]
struct TransferMeta {
    #[serde(rename = "blockTimestamp")]
    block_timestamp: String,
}
