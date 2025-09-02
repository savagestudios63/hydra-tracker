# hydra-tracker

A cross-chain crypto portfolio tracker that lives in your terminal.

Tracks Solana and EVM (Ethereum / Base / Arbitrum) wallets side-by-side,
fetches token holdings and transaction history straight from RPC, prices
everything via CoinGecko → Jupiter → DexScreener, and renders a full
[ratatui](https://github.com/ratatui/ratatui) dashboard with holdings, tx
feed, and a PnL sparkline.

![screenshot](docs/screenshot.png)

> _Screenshots + recording are in `docs/` — see [Recording your own demo](#recording-your-own-demo) below._

## Install

```bash
cargo install hydra-tracker
```

Or from source:

```bash
git clone https://github.com/yourname/hydra-tracker
cd hydra-tracker
cargo build --release
./target/release/hydra
```

The binary is named `hydra`.

## Quick start

1. Copy the sample config into place:

   ```bash
   # Linux / macOS
   mkdir -p ~/.config/hydra-tracker
   cp config.example.toml ~/.config/hydra-tracker/config.toml

   # Windows (PowerShell)
   New-Item -ItemType Directory -Force "$env:APPDATA\hydra-tracker" | Out-Null
   Copy-Item config.example.toml "$env:APPDATA\hydra-tracker\config.toml"
   ```

2. Paste your RPC keys (Helius for Solana, Alchemy for EVM) and list your
   wallets. See [`config.example.toml`](config.example.toml).

3. Run it:

   ```bash
   hydra               # interactive TUI
   hydra --once        # print summary to stdout and exit
   hydra --export-csv holdings.csv
   ```

## Keybindings

| Key            | Action                                                          |
| -------------- | --------------------------------------------------------------- |
| `j` / `k`      | Move selection down / up                                        |
| `h` / `l`      | Select previous / next wallet                                   |
| `gg` / `G`     | Jump to top / bottom                                            |
| `Tab` / `S-Tab`| Cycle tabs forward / backward                                   |
| `1` / `2` / `3`| Jump directly to Holdings / Transactions / PnL                  |
| `7` / `0` / `a`| PnL range: 7 days / 30 days / all time                          |
| `s`            | Cycle sort order (value → 24h% → pnl → balance → symbol)        |
| `w`            | Toggle "selected wallet only" filter                            |
| `/`            | Filter rows (substring match over symbol / name / wallet)       |
| `:`            | Command mode (`:export csv`, `:add wallet …`, `:refresh`, `:q`) |
| `r` / `R`      | Refresh / refresh + reload history                              |
| `Esc`          | Dismiss filter or status message                                |
| `q` / `Ctrl-C` | Quit                                                            |

## How it works

```
┌──────────────┐   fan-out    ┌──────────────┐
│ wallets.toml │─────────────▶│ chain module │ Solana → Helius DAS
└──────────────┘              │              │ EVM    → Alchemy Portfolio
                              └──────┬───────┘
                                     ▼
                              ┌──────────────┐   1. CoinGecko
                              │   pricing    │   2. Jupiter (SPL)
                              └──────┬───────┘   3. DexScreener
                                     ▼
┌──────────────┐              ┌──────────────┐
│   history    │◀─ snapshot ──│ pnl (lots +  │
│  ndjson log  │              │ unrealized)  │
└──────┬───────┘              └──────┬───────┘
       │                             ▼
       │                      ┌──────────────┐
       └─── sparkline data ──▶│     ui       │ ratatui
                              └──────────────┘
```

- **Refreshes** fire on every `refresh_interval` second tick (or `r`), on a
  background tokio task so the UI stays responsive. The latest result swaps
  in atomically via a `parking_lot::RwLock`.
- **Cost basis** uses the weighted-average method on transactions whose
  `usd_value` is known. Transactions without USD context are kept for the
  feed but skipped from the PnL accumulator.
- **History** is an append-only NDJSON of snapshots (`<data>/snapshots.ndjson`),
  which the sparkline reads on startup.

## Features / roadmap

| Status | Feature                                                          |
| :----: | ---------------------------------------------------------------- |
|   ✅   | Multi-wallet, multi-chain tracking (Solana + Ethereum/Base/Arb)  |
|   ✅   | Helius DAS bulk token balances                                   |
|   ✅   | Alchemy `getTokenBalances` + `getAssetTransfers`                 |
|   ✅   | Layered pricing: CoinGecko → Jupiter → DexScreener fallback      |
|   ✅   | Unrealized PnL from weighted-average cost basis                  |
|   ✅   | Holdings table with configurable sort + dust filter              |
|   ✅   | Transaction feed with swap / transfer / stake / LP categorization|
|   ✅   | PnL sparkline (7d / 30d / all) persisted across restarts         |
|   ✅   | Vim-style navigation (`j/k/h/l/gg/G`)                            |
|   ✅   | `/` live filter and `:` command mode                             |
|   ✅   | `--once` and `--export-csv` non-interactive modes                |
|   🛠   | Enriched tx USD pricing at execution time (better realized PnL)  |
|   🛠   | Staking rewards as a distinct income category                    |
|   🛠   | WebSocket push updates for near-real-time balance changes        |
|   🛠   | Per-position historical charts                                   |
|   🛠   | NFT floor pricing                                                |
|   🛠   | Cosmos / Sui / Aptos chain adapters                              |

## Configuration reference

See [`config.example.toml`](config.example.toml) — every field is
commented. The resolved config path is printed in the error message when
the file is missing, so just run `hydra` once and it'll tell you where to
put it.

`RUST_LOG=debug hydra` enables verbose logging (logs go to the platform
data dir, e.g. `~/.local/share/hydra-tracker/hydra.log.*`).

## Recording your own demo

The repo ships without a pre-baked recording so it stays light. To make
one:

```bash
# asciinema (plain cast file)
asciinema rec docs/demo.cast -c "hydra"

# terminalizer (GIF)
terminalizer record docs/demo -c "hydra"
terminalizer render docs/demo -o docs/demo.gif
```

Then link `docs/demo.gif` (or embed the asciinema player) near the top of
this README.

## Project layout

```
src/
├── main.rs         # CLI + TUI bootstrap
├── app.rs          # AppState, refresh orchestration
├── config.rs       # config parsing + validation
├── chains/         # chain-specific RPC adapters
│   ├── mod.rs
│   ├── solana.rs   # Helius DAS + JSON-RPC
│   └── evm.rs      # Alchemy Portfolio + Transfers
├── pricing/        # layered price sources
│   ├── mod.rs
│   ├── coingecko.rs
│   ├── jupiter.rs
│   └── dexscreener.rs
├── pnl/            # cost basis + history
│   └── mod.rs
└── ui/             # ratatui widgets + event loop
    ├── mod.rs
    ├── layout.rs
    ├── holdings.rs
    ├── transactions.rs
    └── pnl_tab.rs
```

## License

Dual-licensed under MIT or Apache 2.0, at your option.
