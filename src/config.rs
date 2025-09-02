use std::{fs, path::{Path, PathBuf}};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub rpc: RpcConfig,
    #[serde(default)]
    pub pricing: Pricing,
    #[serde(default)]
    pub wallets: Vec<Wallet>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct General {
    #[serde(default = "default_refresh")]
    pub refresh_interval: u64,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default = "default_dust")]
    pub dust_threshold: f64,
    #[serde(default)]
    pub history_path: String,
}

fn default_refresh() -> u64 { 60 }
fn default_currency() -> String { "usd".into() }
fn default_dust() -> f64 { 0.0 }

impl Default for General {
    fn default() -> Self {
        Self {
            refresh_interval: default_refresh(),
            currency: default_currency(),
            dust_threshold: default_dust(),
            history_path: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RpcConfig {
    pub solana: Option<RpcEndpoint>,
    pub ethereum: Option<RpcEndpoint>,
    pub base: Option<RpcEndpoint>,
    pub arbitrum: Option<RpcEndpoint>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RpcEndpoint {
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Pricing {
    #[serde(default)]
    pub coingecko_api_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Wallet {
    pub label: String,
    pub chain: Chain,
    pub address: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Solana,
    Ethereum,
    Base,
    Arbitrum,
}

impl Chain {
    pub fn short(self) -> &'static str {
        match self {
            Chain::Solana => "SOL",
            Chain::Ethereum => "ETH",
            Chain::Base => "BASE",
            Chain::Arbitrum => "ARB",
        }
    }
    pub fn is_evm(self) -> bool {
        !matches!(self, Chain::Solana)
    }
    pub fn native_symbol(self) -> &'static str {
        match self {
            Chain::Solana => "SOL",
            Chain::Ethereum | Chain::Base | Chain::Arbitrum => "ETH",
        }
    }
}

pub fn load(override_path: Option<&Path>) -> Result<Config> {
    let path = resolve_path(override_path)?;
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parsing config at {}", path.display()))?;
    validate(&cfg)?;
    Ok(cfg)
}

fn resolve_path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    let dirs = directories::ProjectDirs::from("dev", "hydra", "hydra-tracker")
        .ok_or_else(|| anyhow!("cannot determine config directory"))?;
    let p = dirs.config_dir().join("config.toml");
    if !p.exists() {
        return Err(anyhow!(
            "no config found at {} — copy config.example.toml there to get started",
            p.display()
        ));
    }
    Ok(p)
}

fn validate(cfg: &Config) -> Result<()> {
    if cfg.wallets.is_empty() {
        return Err(anyhow!("config has no wallets"));
    }
    for w in &cfg.wallets {
        match w.chain {
            Chain::Solana => {
                bs58::decode(&w.address).into_vec()
                    .map_err(|_| anyhow!("wallet {} has invalid Solana address", w.label))?;
            }
            _ => {
                if !w.address.starts_with("0x") || w.address.len() != 42 {
                    return Err(anyhow!("wallet {} has invalid EVM address", w.label));
                }
            }
        }
    }
    Ok(())
}

pub fn history_dir(cfg: &Config) -> PathBuf {
    if !cfg.general.history_path.is_empty() {
        return PathBuf::from(&cfg.general.history_path);
    }
    directories::ProjectDirs::from("dev", "hydra", "hydra-tracker")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}
