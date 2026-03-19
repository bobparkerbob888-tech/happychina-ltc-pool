# HappyChina LTC Merge Mining Pool

A high-performance Litecoin merge mining pool written in Rust. Mines LTC as the parent chain and simultaneously merge-mines 8 auxiliary scrypt coins.

## Supported Coins

| Coin | Symbol | Role | Default RPC Port |
|------|--------|------|-----------------|
| Litecoin | LTC | Parent | 9332 |
| Dogecoin | DOGE | Aux | 22555 |
| Pepecoin | PEPE | Aux | 33873 |
| Bellscoin | BELLS | Aux | 19918 |
| Luckycoin | LKY | Aux | 9918 |
| Junkcoin | JKC | Aux | 9772 |
| Dingocoin | DINGO | Aux | 34646 |
| Shibacoin | SHIC | Aux | 33863 |
| TrumpPoW | TRMP | Aux | 33883 |

## Features

- **Scrypt PoW** (N=1024, r=1, p=1) — compatible with all Litecoin ASIC miners
- **Merge mining** — mine all 9 coins simultaneously with no extra hashrate cost
- **PPLNS payouts** — fair reward distribution based on recent shares
- **Variable difficulty (vardiff)** — automatic difficulty adjustment for optimal share rates
- **Multiple stratum ports** — fixed and vardiff ports for different miner configurations
- **Real-time web dashboard** — pool stats, block tracking, miner/worker monitoring
- **Automatic withdrawals** — request payouts directly from the web interface
- **PostgreSQL database** — reliable storage for shares, blocks, balances, and stats

## Umbrel Installation

1. Add this app store to your Umbrel:
   - Go to **Settings > App Stores** and add: `https://github.com/bobparkerbob888-tech/happychina-ltc-pool`

2. Install the **HappyChina LTC Merge Mining Pool** app from the store.

3. Before starting, configure your pool:
   - Copy `config.toml.template` to your app data directory as `config.toml`
   - Edit `config.toml` with your wallet addresses and RPC credentials
   - Ensure all 9 coin daemons are running and accessible

4. Start the app from the Umbrel dashboard.

## Manual Docker Setup

```bash
# Clone the repository
git clone https://github.com/bobparkerbob888-tech/happychina-ltc-pool.git
cd happychina-ltc-pool/ltc-merge-pool

# Create your configuration
cp config.toml.template config.toml
# Edit config.toml with your settings

# Set APP_DATA_DIR for docker-compose volumes
export APP_DATA_DIR=$(pwd)

# Build and start
docker compose up -d --build
```

## Prerequisites

All 9 coin daemons must be running with RPC enabled. Each daemon needs:

- RPC server enabled (`server=1`)
- RPC credentials configured
- For aux coins: `auxpow=1` or equivalent merge-mining support enabled
- Network accessible from the Docker container (uses `host.docker.internal` by default)

## Stratum Ports

| Port | Difficulty | Vardiff | Description |
|------|-----------|---------|-------------|
| 3332 | 1,000,000 | No | ASIC Fixed 1M |
| 3333 | 1,000,000 | Yes | ASIC Vardiff (starts at 1M) |
| 3334 | 2,000,000,000 | No | Mega Fixed 2B |
| 3335 | 500,000,000 | Yes | Mega Vardiff (starts at 500M) |

## Connecting Miners

Configure your ASIC miner with:
- **URL**: `stratum+tcp://YOUR_SERVER_IP:3332` (or 3333 for vardiff)
- **Worker**: `YOUR_LTC_ADDRESS.worker_name`
- **Password**: `x` (anything)

## Web Dashboard

Access the pool dashboard at `http://YOUR_SERVER_IP:3080`

The dashboard shows:
- Pool hashrate, active miners, and workers
- Network difficulty and estimated time to find a block
- Odds of finding a block per day/week/year
- All 9 merge-mined coins with current difficulty and height
- Recent blocks found across all coins
- Miner lookup with per-worker stats
- Balance and withdrawal management

## Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  ASIC Miner  │────▶│   Stratum    │────▶│  Job Manager │
│  (scrypt)    │     │   Server     │     │  (templates) │
└──────────────┘     └──────────────┘     └──────┬───────┘
                                                  │
                     ┌──────────────┐     ┌───────▼──────┐
                     │  PostgreSQL  │◀────│   Pool Core   │
                     │  (shares,    │     │  (validation, │
                     │   blocks,    │     │   payments,   │
                     │   balances)  │     │   stats)      │
                     └──────────────┘     └───────┬───────┘
                                                  │
                     ┌──────────────┐     ┌───────▼──────┐
                     │   Web UI     │◀────│  HTTP API    │
                     │  (dashboard) │     │  (actix-web) │
                     └──────────────┘     └──────────────┘
                                                  │
                     ┌──────────────┐              │
                     │  Coin RPCs   │◀─────────────┘
                     │  (9 daemons) │
                     └──────────────┘
```

## License

All rights reserved.

## Admin Panel

Access the admin dashboard at `http://YOUR_SERVER_IP/admin.html`

**Default password:** `happychina`

**Change your password immediately after first login!**

### Admin Features:
- Pool overview (hashrate, miners, blocks, earnings)
- Fee settings (change pool fee %)
- Pool fee address management
- Per-coin reward address management
- Miner list with hashrates and balances
- Block history with status
- Earnings and withdrawal tracking
- Password management

### Security:
- All admin endpoints require authentication via `X-Admin-Key` header
- Password is stored in `config.toml` as `admin_key`
- Change the default password on first login
