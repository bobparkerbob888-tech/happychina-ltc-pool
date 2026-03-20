-- Payout addresses: allows miners to set per-coin withdrawal addresses
CREATE TABLE IF NOT EXISTS payout_addresses (
    miner       VARCHAR(128) NOT NULL,
    coin        VARCHAR(10) NOT NULL,
    address     VARCHAR(128) NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (miner, coin)
);

CREATE INDEX IF NOT EXISTS idx_payout_addresses_miner ON payout_addresses(miner);

-- Add payout_address column to withdrawals table to record where coins were actually sent
ALTER TABLE withdrawals ADD COLUMN IF NOT EXISTS payout_address TEXT;
