pub mod evm;
pub mod solana;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::{Chain, Config, Wallet};

/// A single token holding resolved from a chain RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holding {
    pub wallet_label: String,
    pub chain: Chain,
    pub symbol: String,
    pub name: String,
    /// Chain-local identifier: SPL mint on Solana, contract address on EVM,
    /// empty string for the native coin.
    pub identifier: String,
    pub decimals: u8,
    /// Amount in whole tokens (already scaled by decimals).
    pub amount: Decimal,
    /// Optional logo URL, populated by some RPCs.
    pub logo: Option<String>,
}

impl Holding {
    pub fn price_key(&self) -> PriceKey {
        PriceKey {
            chain: self.chain,
            identifier: self.identifier.clone(),
            symbol: self.symbol.clone(),
        }
    }
}

/// Identifier used to join a holding to a pricing source.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PriceKey {
    pub chain: Chain,
    pub identifier: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxCategory {
    Swap,
    Transfer,
    Stake,
    Unstake,
    LiquidityAdd,
    LiquidityRemove,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub wallet_label: String,
    pub chain: Chain,
    pub signature: String,
    pub timestamp: DateTime<Utc>,
    pub category: TxCategory,
    pub summary: String,
    /// Signed, in whole-token units. Negative = out of wallet, positive = in.
    pub amount: Option<Decimal>,
    pub symbol: Option<String>,
    /// USD value at tx time when known (used for cost basis).
    pub usd_value: Option<Decimal>,
}

pub async fn fetch_holdings(cfg: &Config, client: &reqwest::Client, w: &Wallet) -> Result<Vec<Holding>> {
    match w.chain {
        Chain::Solana => solana::holdings(cfg, client, w).await,
        _ => evm::holdings(cfg, client, w).await,
    }
}

pub async fn fetch_transactions(
    cfg: &Config,
    client: &reqwest::Client,
    w: &Wallet,
    limit: usize,
) -> Result<Vec<Transaction>> {
    match w.chain {
        Chain::Solana => solana::transactions(cfg, client, w, limit).await,
        _ => evm::transactions(cfg, client, w, limit).await,
    }
}
