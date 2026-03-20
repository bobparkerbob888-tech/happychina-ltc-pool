use chrono::{DateTime, Utc};
use log::{debug, error, info};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use std::time::Duration;
use std::sync::Arc;

/// Database handle wrapping a PostgreSQL connection pool
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

/// Shared share batcher for high-throughput share insertion
pub struct ShareBatcher {
    tx: tokio::sync::mpsc::Sender<ShareInsert>,
}

impl ShareBatcher {
    /// Create a new share batcher that flushes every 1 second or every 100 shares.
    pub fn new(db: Arc<Db>) -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::channel::<ShareInsert>(4096);
        let batcher = Arc::new(Self { tx });
        
        // Spawn the background flush task
        tokio::spawn(Self::flush_loop(db, rx));
        
        batcher
    }
    
    /// Submit a share to be batched
    pub async fn submit(&self, share: ShareInsert) {
        if let Err(e) = self.tx.send(share).await {
            log::warn!("Share batcher channel full/closed, dropping share: {}", e);
        }
    }
    
    /// Background task that drains shares and batch-inserts them
    async fn flush_loop(db: Arc<Db>, mut rx: tokio::sync::mpsc::Receiver<ShareInsert>) {
        let mut buffer: Vec<ShareInsert> = Vec::with_capacity(128);
        let mut flush_interval = tokio::time::interval(Duration::from_secs(1));
        
        loop {
            tokio::select! {
                _ = flush_interval.tick() => {
                    // Timer fired: flush whatever we have
                    if !buffer.is_empty() {
                        Self::do_flush(&db, &mut buffer).await;
                    }
                }
                share = rx.recv() => {
                    match share {
                        Some(s) => {
                            buffer.push(s);
                            // Flush if buffer is full
                            if buffer.len() >= 100 {
                                Self::do_flush(&db, &mut buffer).await;
                            }
                        }
                        None => {
                            // Channel closed: flush remaining and exit
                            if !buffer.is_empty() {
                                Self::do_flush(&db, &mut buffer).await;
                            }
                            info!("Share batcher shutting down");
                            break;
                        }
                    }
                }
            }
        }
    }
    
    /// Flush the buffer to DB using batch insert
    async fn do_flush(db: &Db, buffer: &mut Vec<ShareInsert>) {
        let count = buffer.len();
        match db.insert_shares_batch(buffer).await {
            Ok(()) => {
                debug!("Batch inserted {} shares", count);
            }
            Err(e) => {
                // On batch failure, try individual inserts as fallback
                log::warn!("Batch share insert failed ({}), trying individual: {}", count, e);
                for share in buffer.iter() {
                    if let Err(e2) = db.insert_share(share).await {
                        log::warn!("Individual share insert also failed: {}", e2);
                    }
                }
            }
        }
        buffer.clear();
    }
}

// ── Row types ──────────────────────────────────────────────

/// A miner record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MinerRow {
    pub address: String,
    pub created_at: DateTime<Utc>,
}

/// A worker record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkerRow {
    pub id: i32,
    pub miner: String,
    pub worker_name: String,
    pub last_seen: DateTime<Utc>,
    pub hashrate: f64,
    pub difficulty: f64,
    pub is_online: bool,
    pub user_agent: String,
}

/// A share record (for insertion)
#[derive(Debug, Clone)]
pub struct ShareInsert {
    pub miner: String,
    pub worker: String,
    pub difficulty: f64,
    pub share_difficulty: f64,
    pub ip_address: String,
    pub user_agent: String,
    pub is_valid: bool,
}

/// A block record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BlockRow {
    pub id: i32,
    pub coin: String,
    pub height: i64,
    pub hash: String,
    pub block_hash: String,
    pub miner: String,
    pub worker: String,
    pub reward: f64,
    pub difficulty: f64,
    pub net_difficulty: f64,
    pub confirmations: i32,
    pub status: String,
    pub algo: String,
    pub created_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
}

/// For inserting a new block
#[derive(Debug, Clone)]
pub struct BlockInsert {
    pub coin: String,
    pub height: i64,
    pub hash: String,
    pub block_hash: String,
    pub miner: String,
    pub worker: String,
    pub reward: f64,
    pub difficulty: f64,
    pub net_difficulty: f64,
}

/// A balance record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BalanceRow {
    pub miner: String,
    pub coin: String,
    pub amount: f64,
}

/// A withdrawal record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WithdrawalRow {
    pub id: i32,
    pub miner: String,
    pub coin: String,
    pub amount: f64,
    pub fee: f64,
    pub tx_hash: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub payout_address: Option<String>,
}

/// A payout address record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PayoutAddressRow {
    pub miner: String,
    pub coin: String,
    pub address: String,
    pub created_at: DateTime<Utc>,
}

/// A pool stats record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PoolStatRow {
    pub id: i32,
    pub hashrate: f64,
    pub miners: i32,
    pub workers: i32,
    pub shares_per_sec: f64,
    pub created_at: DateTime<Utc>,
}

/// A miner stats record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MinerStatRow {
    pub id: i32,
    pub miner: String,
    pub worker: String,
    pub hashrate: f64,
    pub shares_per_sec: f64,
    pub created_at: DateTime<Utc>,
}

/// An earning record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EarningRow {
    pub id: i32,
    pub miner: String,
    pub coin: String,
    pub block_id: Option<i32>,
    pub amount: f64,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// PPLNS share row for reward calculation
#[derive(Debug, Clone)]
pub struct PplnsShare {
    pub miner: String,
    pub difficulty: f64,
}

impl Db {
    /// Connect to PostgreSQL and create the connection pool
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        info!("Connecting to database...");
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .max_lifetime(Duration::from_secs(1800))
            .connect(database_url)
            .await?;

        info!("Database connected successfully");
        Ok(Db { pool })
    }

    /// Run migrations from SQL file
    pub async fn run_migration(&self, sql: &str) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(sql).execute(&self.pool).await?;
        info!("Database migration completed");
        Ok(())
    }

    /// Get the underlying pool (for advanced usage)
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // ── Miners ─────────────────────────────────────────────

    /// Ensure a miner exists (upsert)
    pub async fn ensure_miner(&self, address: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO miners (address) VALUES ($1) ON CONFLICT (address) DO NOTHING",
        )
        .bind(address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get a miner by address
    pub async fn get_miner(&self, address: &str) -> Result<Option<MinerRow>, sqlx::Error> {
        sqlx::query_as::<_, MinerRow>("SELECT address, created_at FROM miners WHERE address = $1")
            .bind(address)
            .fetch_optional(&self.pool)
            .await
    }

    /// Count active miners (seen in last 10 minutes)
    pub async fn count_active_miners(&self) -> Result<i64, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COUNT(DISTINCT miner) as cnt FROM workers WHERE is_online = true",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<i64, _>("cnt"))
    }

    // ── Workers ────────────────────────────────────────────

    /// Upsert a worker (update last_seen, hashrate, difficulty)
    pub async fn upsert_worker(
        &self,
        miner: &str,
        worker_name: &str,
        difficulty: f64,
        user_agent: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO workers (miner, worker_name, last_seen, difficulty, is_online, user_agent)
             VALUES ($1, $2, NOW(), $3, true, $4)
             ON CONFLICT (miner, worker_name) DO UPDATE SET
                last_seen = NOW(),
                difficulty = $3,
                is_online = true,
                user_agent = $4",
        )
        .bind(miner)
        .bind(worker_name)
        .bind(difficulty)
        .bind(user_agent)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update worker hashrate
    pub async fn update_worker_hashrate(
        &self,
        miner: &str,
        worker_name: &str,
        hashrate: f64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE workers SET hashrate = $3, last_seen = NOW()
             WHERE miner = $1 AND worker_name = $2",
        )
        .bind(miner)
        .bind(worker_name)
        .bind(hashrate)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark workers offline if not seen in given duration
    pub async fn mark_stale_workers_offline(&self, timeout_secs: i64) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE workers SET is_online = false
             WHERE is_online = true AND last_seen < NOW() - make_interval(secs => $1)",
        )
        .bind(timeout_secs as f64)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Get all workers for a miner
    pub async fn get_workers(&self, miner: &str) -> Result<Vec<WorkerRow>, sqlx::Error> {
        sqlx::query_as::<_, WorkerRow>(
            "SELECT id, miner, worker_name, last_seen, hashrate, difficulty, is_online, user_agent
             FROM workers WHERE miner = $1 ORDER BY worker_name",
        )
        .bind(miner)
        .fetch_all(&self.pool)
        .await
    }

    /// Count online workers
    pub async fn count_online_workers(&self) -> Result<i64, sqlx::Error> {
        let row =
            sqlx::query("SELECT COUNT(*) as cnt FROM workers WHERE is_online = true")
                .fetch_one(&self.pool)
                .await?;
        Ok(row.get::<i64, _>("cnt"))
    }

    // ── Shares ─────────────────────────────────────────────

    /// Insert a share
    pub async fn insert_share(&self, share: &ShareInsert) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO shares (miner, worker, difficulty, share_difficulty, ip_address, user_agent, is_valid)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&share.miner)
        .bind(&share.worker)
        .bind(share.difficulty)
        .bind(share.share_difficulty)
        .bind(&share.ip_address)
        .bind(&share.user_agent)
        .bind(share.is_valid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert multiple shares in a batch
    pub async fn insert_shares_batch(&self, shares: &[ShareInsert]) -> Result<(), sqlx::Error> {
        if shares.is_empty() {
            return Ok(());
        }

        // Build a batch insert query
        let mut query = String::from(
            "INSERT INTO shares (miner, worker, difficulty, share_difficulty, ip_address, user_agent, is_valid) VALUES ",
        );

        let mut params_count = 0;
        for (i, _share) in shares.iter().enumerate() {
            if i > 0 {
                query.push_str(", ");
            }
            let base = params_count;
            query.push_str(&format!(
                "(${}, ${}, ${}, ${}, ${}, ${}, ${})",
                base + 1,
                base + 2,
                base + 3,
                base + 4,
                base + 5,
                base + 6,
                base + 7
            ));
            params_count += 7;
        }

        let mut q = sqlx::query(&query);
        for share in shares {
            q = q
                .bind(&share.miner)
                .bind(&share.worker)
                .bind(share.difficulty)
                .bind(share.share_difficulty)
                .bind(&share.ip_address)
                .bind(&share.user_agent)
                .bind(share.is_valid);
        }

        q.execute(&self.pool).await?;
        Ok(())
    }

    /// Get shares per second in the last N seconds
    pub async fn get_shares_per_sec(&self, window_secs: i64) -> Result<f64, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COUNT(*) as cnt FROM shares
             WHERE created_at > NOW() - make_interval(secs => $1) AND is_valid = true",
        )
        .bind(window_secs as f64)
        .fetch_one(&self.pool)
        .await?;
        let count: i64 = row.get("cnt");
        Ok(count as f64 / window_secs as f64)
    }

    /// Get PPLNS shares for reward calculation
    /// Returns the last `window_size` shares (by total difficulty sum)
    pub async fn get_pplns_shares(
        &self,
        window_size: f64,
    ) -> Result<Vec<PplnsShare>, sqlx::Error> {
        // Get shares in reverse chronological order, accumulating difficulty
        // until we reach the window size
        let rows = sqlx::query(
            "SELECT miner, difficulty FROM shares
             WHERE is_valid = true
             ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        let mut total_diff = 0.0;

        for row in rows {
            if total_diff >= window_size {
                break;
            }
            let miner: String = row.get("miner");
            let difficulty: f64 = row.get("difficulty");
            total_diff += difficulty;
            result.push(PplnsShare { miner, difficulty });
        }

        Ok(result)
    }

    /// Count total shares
    pub async fn count_shares(&self) -> Result<i64, sqlx::Error> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM shares WHERE is_valid = true")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>("cnt"))
    }

    /// Delete old shares (for cleanup, keep last N days)
    pub async fn delete_old_shares(&self, days: i32) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM shares WHERE created_at < NOW() - make_interval(days => $1)",
        )
        .bind(days)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    // ── Blocks ─────────────────────────────────────────────

    /// Insert a found block
    pub async fn insert_block(&self, block: &BlockInsert) -> Result<i32, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO blocks (coin, height, hash, block_hash, miner, worker, reward, difficulty, net_difficulty)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING id",
        )
        .bind(&block.coin)
        .bind(block.height)
        .bind(&block.hash)
        .bind(&block.block_hash)
        .bind(&block.miner)
        .bind(&block.worker)
        .bind(block.reward)
        .bind(block.difficulty)
        .bind(block.net_difficulty)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("id"))
    }

    /// Get pending blocks (for confirmation tracking)
    pub async fn get_pending_blocks(&self) -> Result<Vec<BlockRow>, sqlx::Error> {
        sqlx::query_as::<_, BlockRow>(
            "SELECT id, coin, height, hash, block_hash, miner, worker, reward, difficulty,
                    net_difficulty, confirmations, status, algo, created_at, confirmed_at
             FROM blocks WHERE status = 'pending' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Update block confirmation count
    pub async fn update_block_confirmations(
        &self,
        block_id: i32,
        confirmations: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE blocks SET confirmations = $2 WHERE id = $1")
            .bind(block_id)
            .bind(confirmations)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update block_hash (e.g. when resolved via getblockhash fallback)
    pub async fn update_block_hash(
        &self,
        block_id: i32,
        block_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE blocks SET block_hash = $2 WHERE id = $1")
            .bind(block_id)
            .bind(block_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Mark a block as confirmed
    pub async fn confirm_block(&self, block_id: i32) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE blocks SET status = 'confirmed', confirmed_at = NOW() WHERE id = $1",
        )
        .bind(block_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a block as orphaned
    pub async fn orphan_block(&self, block_id: i32) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE blocks SET status = 'orphaned' WHERE id = $1")
            .bind(block_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get recent blocks (for dashboard)
    pub async fn get_recent_blocks(
        &self,
        coin: Option<&str>,
        limit: i64,
    ) -> Result<Vec<BlockRow>, sqlx::Error> {
        if let Some(coin) = coin {
            sqlx::query_as::<_, BlockRow>(
                "SELECT id, coin, height, hash, block_hash, miner, worker, reward, difficulty,
                        net_difficulty, confirmations, status, algo, created_at, confirmed_at
                 FROM blocks WHERE coin = $1 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(coin)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, BlockRow>(
                "SELECT id, coin, height, hash, block_hash, miner, worker, reward, difficulty,
                        net_difficulty, confirmations, status, algo, created_at, confirmed_at
                 FROM blocks ORDER BY created_at DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        }
    }

    /// Count blocks by coin and status
    pub async fn count_blocks(
        &self,
        coin: Option<&str>,
        status: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let (query_str, result) = match (coin, status) {
            (Some(c), Some(s)) => {
                let r = sqlx::query(
                    "SELECT COUNT(*) as cnt FROM blocks WHERE coin = $1 AND status = $2",
                )
                .bind(c)
                .bind(s)
                .fetch_one(&self.pool)
                .await?;
                ("", r)
            }
            (Some(c), None) => {
                let r = sqlx::query("SELECT COUNT(*) as cnt FROM blocks WHERE coin = $1")
                    .bind(c)
                    .fetch_one(&self.pool)
                    .await?;
                ("", r)
            }
            (None, Some(s)) => {
                let r = sqlx::query("SELECT COUNT(*) as cnt FROM blocks WHERE status = $1")
                    .bind(s)
                    .fetch_one(&self.pool)
                    .await?;
                ("", r)
            }
            (None, None) => {
                let r = sqlx::query("SELECT COUNT(*) as cnt FROM blocks")
                    .fetch_one(&self.pool)
                    .await?;
                ("", r)
            }
        };
        let _ = query_str;
        Ok(result.get::<i64, _>("cnt"))
    }

    // ── Balances ───────────────────────────────────────────

    /// Get all balances for a miner
    pub async fn get_balances(&self, miner: &str) -> Result<Vec<BalanceRow>, sqlx::Error> {
        sqlx::query_as::<_, BalanceRow>(
            "SELECT miner, coin, amount FROM balances WHERE miner = $1 ORDER BY coin",
        )
        .bind(miner)
        .fetch_all(&self.pool)
        .await
    }

    /// Get balance for a specific miner and coin
    pub async fn get_balance(
        &self,
        miner: &str,
        coin: &str,
    ) -> Result<f64, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COALESCE(amount, 0) as amount FROM balances WHERE miner = $1 AND coin = $2",
        )
        .bind(miner)
        .bind(coin)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(r.get::<f64, _>("amount")),
            None => Ok(0.0),
        }
    }

    /// Credit (add to) a miner's balance for a coin
    pub async fn credit_balance(
        &self,
        miner: &str,
        coin: &str,
        amount: f64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO balances (miner, coin, amount)
             VALUES ($1, $2, $3)
             ON CONFLICT (miner, coin) DO UPDATE SET amount = balances.amount + $3",
        )
        .bind(miner)
        .bind(coin)
        .bind(amount)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Debit (subtract from) a miner's balance for a coin
    /// Returns error if insufficient balance
    pub async fn debit_balance(
        &self,
        miner: &str,
        coin: &str,
        amount: f64,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE balances SET amount = amount - $3
             WHERE miner = $1 AND coin = $2 AND amount >= $3",
        )
        .bind(miner)
        .bind(coin)
        .bind(amount)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    // ── Withdrawals ────────────────────────────────────────

    /// Create a new withdrawal request
    pub async fn create_withdrawal(
        &self,
        miner: &str,
        coin: &str,
        amount: f64,
        fee: f64,
    ) -> Result<i32, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO withdrawals (miner, coin, amount, fee)
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(miner)
        .bind(coin)
        .bind(amount)
        .bind(fee)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("id"))
    }

    /// Create a new withdrawal request with a specific payout address
    pub async fn create_withdrawal_with_address(
        &self,
        miner: &str,
        coin: &str,
        amount: f64,
        fee: f64,
        payout_address: &str,
    ) -> Result<i32, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO withdrawals (miner, coin, amount, fee, payout_address)
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(miner)
        .bind(coin)
        .bind(amount)
        .bind(fee)
        .bind(payout_address)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("id"))
    }

    /// Mark a withdrawal as completed with tx hash
    pub async fn complete_withdrawal(
        &self,
        id: i32,
        tx_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE withdrawals SET status = 'completed', tx_hash = $2, completed_at = NOW()
             WHERE id = $1",
        )
        .bind(id)
        .bind(tx_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a withdrawal as failed
    pub async fn fail_withdrawal(
        &self,
        id: i32,
        error_message: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE withdrawals SET status = 'failed', error_message = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(error_message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get pending withdrawals
    pub async fn get_pending_withdrawals(&self) -> Result<Vec<WithdrawalRow>, sqlx::Error> {
        sqlx::query_as::<_, WithdrawalRow>(
            "SELECT id, miner, coin, amount, fee, tx_hash, status, error_message, created_at, completed_at, payout_address
             FROM withdrawals WHERE status = 'pending' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Get withdrawals for a miner
    pub async fn get_miner_withdrawals(
        &self,
        miner: &str,
        limit: i64,
    ) -> Result<Vec<WithdrawalRow>, sqlx::Error> {
        sqlx::query_as::<_, WithdrawalRow>(
            "SELECT id, miner, coin, amount, fee, tx_hash, status, error_message, created_at, completed_at, payout_address
             FROM withdrawals WHERE miner = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(miner)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Check if miner has a recent withdrawal for rate limiting
    pub async fn has_recent_withdrawal(
        &self,
        miner: &str,
        coin: &str,
        hours: i32,
    ) -> Result<bool, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COUNT(*) as cnt FROM withdrawals
             WHERE miner = $1 AND coin = $2
             AND created_at > NOW() - make_interval(hours => $3)
             AND status IN ('pending', 'completed')",
        )
        .bind(miner)
        .bind(coin)
        .bind(hours)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<i64, _>("cnt") > 0)
    }

    // ── Pool Stats ─────────────────────────────────────────

    /// Insert a pool stats snapshot
    pub async fn insert_pool_stat(
        &self,
        hashrate: f64,
        miners: i32,
        workers: i32,
        shares_per_sec: f64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO pool_stats (hashrate, miners, workers, shares_per_sec)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(hashrate)
        .bind(miners)
        .bind(workers)
        .bind(shares_per_sec)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get the latest pool stat
    pub async fn get_latest_pool_stat(&self) -> Result<Option<PoolStatRow>, sqlx::Error> {
        sqlx::query_as::<_, PoolStatRow>(
            "SELECT id, hashrate, miners, workers, shares_per_sec, created_at
             FROM pool_stats ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
    }

    /// Get pool stats history (for charts)
    pub async fn get_pool_stats_history(
        &self,
        hours: i32,
    ) -> Result<Vec<PoolStatRow>, sqlx::Error> {
        sqlx::query_as::<_, PoolStatRow>(
            "SELECT id, hashrate, miners, workers, shares_per_sec, created_at
             FROM pool_stats
             WHERE created_at > NOW() - make_interval(hours => $1)
             ORDER BY created_at ASC",
        )
        .bind(hours)
        .fetch_all(&self.pool)
        .await
    }

    // ── Miner Stats ────────────────────────────────────────

    /// Insert a miner stats snapshot
    pub async fn insert_miner_stat(
        &self,
        miner: &str,
        worker: &str,
        hashrate: f64,
        shares_per_sec: f64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO miner_stats (miner, worker, hashrate, shares_per_sec)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(miner)
        .bind(worker)
        .bind(hashrate)
        .bind(shares_per_sec)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get miner hashrate history (for charts)
    pub async fn get_miner_stats_history(
        &self,
        miner: &str,
        hours: i32,
    ) -> Result<Vec<MinerStatRow>, sqlx::Error> {
        sqlx::query_as::<_, MinerStatRow>(
            "SELECT id, miner, worker, hashrate, shares_per_sec, created_at
             FROM miner_stats
             WHERE miner = $1 AND created_at > NOW() - make_interval(hours => $2)
             ORDER BY created_at ASC",
        )
        .bind(miner)
        .bind(hours)
        .fetch_all(&self.pool)
        .await
    }

    /// Delete old stats (for cleanup)
    pub async fn delete_old_stats(&self, days: i32) -> Result<(u64, u64), sqlx::Error> {
        let pool_deleted = sqlx::query(
            "DELETE FROM pool_stats WHERE created_at < NOW() - make_interval(days => $1)",
        )
        .bind(days)
        .execute(&self.pool)
        .await?
        .rows_affected();

        let miner_deleted = sqlx::query(
            "DELETE FROM miner_stats WHERE created_at < NOW() - make_interval(days => $1)",
        )
        .bind(days)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok((pool_deleted, miner_deleted))
    }

    // ── Earnings ───────────────────────────────────────────

    /// Record an earning for a miner
    pub async fn insert_earning(
        &self,
        miner: &str,
        coin: &str,
        block_id: i32,
        amount: f64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO earnings (miner, coin, block_id, amount)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(miner)
        .bind(coin)
        .bind(block_id)
        .bind(amount)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get earnings for a miner
    pub async fn get_miner_earnings(
        &self,
        miner: &str,
        limit: i64,
    ) -> Result<Vec<EarningRow>, sqlx::Error> {
        sqlx::query_as::<_, EarningRow>(
            "SELECT id, miner, coin, block_id, amount, status, created_at
             FROM earnings WHERE miner = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(miner)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    // ── Aggregate Queries ──────────────────────────────────

    /// Get total pool hashrate from worker data
    pub async fn get_total_hashrate(&self) -> Result<f64, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(hashrate), 0) as total FROM workers WHERE is_online = true",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<f64, _>("total"))
    }

    /// Get miner total hashrate
    pub async fn get_miner_hashrate(&self, miner: &str) -> Result<f64, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(hashrate), 0) as total FROM workers
             WHERE miner = $1 AND is_online = true",
        )
        .bind(miner)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<f64, _>("total"))
    }

    /// Get top miners by hashrate
    pub async fn get_top_miners(&self, limit: i64) -> Result<Vec<(String, f64)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT miner, SUM(hashrate) as total_hashrate FROM workers
             WHERE is_online = true
             GROUP BY miner ORDER BY total_hashrate DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<String, _>("miner"),
                    r.get::<f64, _>("total_hashrate"),
                )
            })
            .collect())
    }

    // ── Payout Addresses ──────────────────────────────────

    /// Set or update a payout address for a miner and coin
    pub async fn set_payout_address(
        &self,
        miner: &str,
        coin: &str,
        address: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO payout_addresses (miner, coin, address)
             VALUES ($1, $2, $3)
             ON CONFLICT (miner, coin) DO UPDATE SET address = $3, created_at = NOW()",
        )
        .bind(miner)
        .bind(coin)
        .bind(address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get payout address for a specific miner and coin
    pub async fn get_payout_address(
        &self,
        miner: &str,
        coin: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT address FROM payout_addresses WHERE miner = $1 AND coin = $2",
        )
        .bind(miner)
        .bind(coin)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.get::<String, _>("address")))
    }

    /// Get all payout addresses for a miner
    pub async fn get_payout_addresses(
        &self,
        miner: &str,
    ) -> Result<Vec<PayoutAddressRow>, sqlx::Error> {
        sqlx::query_as::<_, PayoutAddressRow>(
            "SELECT miner, coin, address, created_at
             FROM payout_addresses WHERE miner = $1 ORDER BY coin",
        )
        .bind(miner)
        .fetch_all(&self.pool)
        .await
    }

    /// Delete a payout address for a miner and coin
    pub async fn delete_payout_address(
        &self,
        miner: &str,
        coin: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM payout_addresses WHERE miner = $1 AND coin = $2",
        )
        .bind(miner)
        .bind(coin)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
