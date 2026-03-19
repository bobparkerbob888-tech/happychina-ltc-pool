use sqlx::Row;
/// Pool and miner stats collector.
/// Periodically computes hashrates, counts miners/workers, and inserts stats rows.

use std::sync::Arc;
use log::{error, info};
use tokio::time::{interval, Duration};

use crate::db::Db;

/// Hashrate estimation window in seconds (10 minutes).
const HASHRATE_WINDOW_SECS: i64 = 600;

/// Start the stats updater background task.
pub async fn run_stats_updater(
    db: Db,
    update_interval_secs: u64,
    worker_timeout_secs: i64,
) {
    info!("Stats updater started (interval={}s)", update_interval_secs);
    let mut ticker = interval(Duration::from_secs(update_interval_secs));

    loop {
        ticker.tick().await;

        if let Err(e) = update_stats(&db, worker_timeout_secs).await {
            error!("Stats updater error: {}", e);
        }
    }
}

/// Compute and store pool stats.
async fn update_stats(
    db: &Db,
    worker_timeout_secs: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Mark stale workers offline
    let stale_count = db.mark_stale_workers_offline(worker_timeout_secs).await?;
    if stale_count > 0 {
        info!("Marked {} stale workers offline", stale_count);
    }

    // Compute pool hashrate from shares in the window
    // Correct formula: hashrate = SUM(difficulty) * 2^16 / window_seconds
    // where difficulty = the actual stratum difficulty at time of share submission
    // For scrypt: diff 1 = 2^16 hashes (not 2^32, because scrypt maxTarget is 0x0000ffff...)
    let shares_per_sec = db.get_shares_per_sec(HASHRATE_WINDOW_SECS).await?;

    // Calculate hashrate: SUM(difficulty) * 65536 / window
    let sum_diff: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(difficulty), 0) FROM shares WHERE created_at > NOW() - make_interval(secs => $1) AND is_valid = true"
    )
    .bind(HASHRATE_WINDOW_SECS as f64)
    .fetch_one(db.pool())
    .await
    .unwrap_or(0.0);

    let hashrate = sum_diff * 65536.0 / HASHRATE_WINDOW_SECS as f64;

    // Count miners/workers from recent shares instead of workers table
    let miners: i32 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT miner)::int4 FROM shares WHERE created_at > NOW() - make_interval(secs => $1)"
    )
    .bind(HASHRATE_WINDOW_SECS as f64)
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    let workers: i32 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT worker)::int4 FROM shares WHERE created_at > NOW() - make_interval(secs => $1)"
    )
    .bind(HASHRATE_WINDOW_SECS as f64)
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    // Insert pool stats row
    db.insert_pool_stat(hashrate, miners, workers, shares_per_sec).await?;

    log::debug!(
        "Stats: hashrate={:.2} H/s, miners={}, workers={}, shares/s={:.2}",
        hashrate, miners, workers, shares_per_sec
    );

    // Clean up old stats (keep 7 days)
    let (pool_deleted, miner_deleted) = db.delete_old_stats(7).await?;
    if pool_deleted > 0 || miner_deleted > 0 {
        log::debug!("Cleaned up {} pool stats, {} miner stats", pool_deleted, miner_deleted);
    }

    // Clean up old shares (keep 3 days)
    let shares_deleted = db.delete_old_shares(3).await?;
    if shares_deleted > 0 {
        log::debug!("Cleaned up {} old shares", shares_deleted);
    }

    Ok(())
}
