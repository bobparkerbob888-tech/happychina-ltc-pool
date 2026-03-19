-- HappyChain Pool - Initial Schema
-- Run as: PGPASSWORD=pool psql -U pool -h 127.0.0.1 -d happychain -f migrations/001_initial.sql

-- Miners table: one row per unique mining address
CREATE TABLE IF NOT EXISTS miners (
    address     TEXT PRIMARY KEY,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Workers table: tracks individual mining rigs
CREATE TABLE IF NOT EXISTS workers (
    id          SERIAL PRIMARY KEY,
    miner       TEXT NOT NULL REFERENCES miners(address) ON DELETE CASCADE,
    worker_name TEXT NOT NULL DEFAULT 'default',
    last_seen   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    hashrate    DOUBLE PRECISION NOT NULL DEFAULT 0,
    difficulty  DOUBLE PRECISION NOT NULL DEFAULT 0,
    is_online   BOOLEAN NOT NULL DEFAULT FALSE,
    user_agent  TEXT NOT NULL DEFAULT '',
    UNIQUE(miner, worker_name)
);

-- Shares table: records every valid share submitted
-- Uses BIGSERIAL for high-volume inserts
CREATE TABLE IF NOT EXISTS shares (
    id                  BIGSERIAL PRIMARY KEY,
    miner               TEXT NOT NULL,
    worker              TEXT NOT NULL DEFAULT 'default',
    difficulty          DOUBLE PRECISION NOT NULL,
    share_difficulty    DOUBLE PRECISION NOT NULL,
    ip_address          TEXT NOT NULL DEFAULT '',
    user_agent          TEXT NOT NULL DEFAULT '',
    is_valid            BOOLEAN NOT NULL DEFAULT TRUE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Blocks table: records found blocks across all coins
CREATE TABLE IF NOT EXISTS blocks (
    id              SERIAL PRIMARY KEY,
    coin            TEXT NOT NULL,
    height          BIGINT NOT NULL,
    hash            TEXT NOT NULL DEFAULT '',
    block_hash      TEXT NOT NULL DEFAULT '',
    miner           TEXT NOT NULL DEFAULT '',
    worker          TEXT NOT NULL DEFAULT '',
    reward          DOUBLE PRECISION NOT NULL DEFAULT 0,
    difficulty      DOUBLE PRECISION NOT NULL DEFAULT 0,
    net_difficulty  DOUBLE PRECISION NOT NULL DEFAULT 0,
    confirmations   INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL DEFAULT 'pending',
    algo            TEXT NOT NULL DEFAULT 'scrypt',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    confirmed_at    TIMESTAMPTZ
);

-- Balances table: tracks each miner's balance per coin
CREATE TABLE IF NOT EXISTS balances (
    miner   TEXT NOT NULL,
    coin    TEXT NOT NULL,
    amount  DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (miner, coin)
);

-- Withdrawals table: tracks payout requests and their status
CREATE TABLE IF NOT EXISTS withdrawals (
    id              SERIAL PRIMARY KEY,
    miner           TEXT NOT NULL,
    coin            TEXT NOT NULL,
    amount          DOUBLE PRECISION NOT NULL,
    fee             DOUBLE PRECISION NOT NULL DEFAULT 0,
    tx_hash         TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    error_message   TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ
);

-- Pool stats table: periodic snapshots of pool-wide metrics
CREATE TABLE IF NOT EXISTS pool_stats (
    id              SERIAL PRIMARY KEY,
    hashrate        DOUBLE PRECISION NOT NULL DEFAULT 0,
    miners          INTEGER NOT NULL DEFAULT 0,
    workers         INTEGER NOT NULL DEFAULT 0,
    shares_per_sec  DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Miner stats table: periodic snapshots of per-miner metrics
CREATE TABLE IF NOT EXISTS miner_stats (
    id              SERIAL PRIMARY KEY,
    miner           TEXT NOT NULL,
    worker          TEXT NOT NULL DEFAULT '',
    hashrate        DOUBLE PRECISION NOT NULL DEFAULT 0,
    shares_per_sec  DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Earnings table: tracks credited earnings per block per miner
CREATE TABLE IF NOT EXISTS earnings (
    id          SERIAL PRIMARY KEY,
    miner       TEXT NOT NULL,
    coin        TEXT NOT NULL,
    block_id    INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    amount      DOUBLE PRECISION NOT NULL,
    status      TEXT NOT NULL DEFAULT 'credited',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ============================================================
-- Indexes for query performance
-- ============================================================

-- Shares: queried by time range (for PPLNS) and by miner
CREATE INDEX IF NOT EXISTS idx_shares_created_at ON shares(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_shares_miner ON shares(miner);
CREATE INDEX IF NOT EXISTS idx_shares_miner_created ON shares(miner, created_at DESC);

-- Blocks: queried by coin, status, and time
CREATE INDEX IF NOT EXISTS idx_blocks_coin_status ON blocks(coin, status);
CREATE INDEX IF NOT EXISTS idx_blocks_created_at ON blocks(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_blocks_status ON blocks(status);
CREATE INDEX IF NOT EXISTS idx_blocks_miner ON blocks(miner);

-- Balances: queried by miner
CREATE INDEX IF NOT EXISTS idx_balances_miner ON balances(miner);

-- Withdrawals: queried by miner, status, and time
CREATE INDEX IF NOT EXISTS idx_withdrawals_miner ON withdrawals(miner);
CREATE INDEX IF NOT EXISTS idx_withdrawals_status ON withdrawals(status);
CREATE INDEX IF NOT EXISTS idx_withdrawals_created_at ON withdrawals(created_at DESC);

-- Pool stats: queried by time
CREATE INDEX IF NOT EXISTS idx_pool_stats_created_at ON pool_stats(created_at DESC);

-- Miner stats: queried by miner and time
CREATE INDEX IF NOT EXISTS idx_miner_stats_miner ON miner_stats(miner);
CREATE INDEX IF NOT EXISTS idx_miner_stats_created_at ON miner_stats(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_miner_stats_miner_created ON miner_stats(miner, created_at DESC);

-- Earnings: queried by miner, coin, and time
CREATE INDEX IF NOT EXISTS idx_earnings_miner ON earnings(miner);
CREATE INDEX IF NOT EXISTS idx_earnings_miner_coin ON earnings(miner, coin);
CREATE INDEX IF NOT EXISTS idx_earnings_created_at ON earnings(created_at DESC);

-- Workers: queried by miner
CREATE INDEX IF NOT EXISTS idx_workers_miner ON workers(miner);
