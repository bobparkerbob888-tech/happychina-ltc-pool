/// Block tracker: polls pending blocks for confirmations,
/// marks them confirmed (triggering PPLNS) or orphaned.

use std::collections::HashMap;
use std::sync::Arc;
use log::{error, info, warn};
use tokio::time::{interval, Duration};

use crate::config::Config;
use crate::db::Db;
use crate::rpc::RpcClient;
use super::payments::distribute_pplns;

/// Start the block tracker background task.
/// Runs every `check_interval_secs` seconds, checking pending blocks for confirmations.
pub async fn run_block_tracker(
    db: Db,
    rpc_clients: Arc<HashMap<String, RpcClient>>,
    config: Arc<Config>,
    check_interval_secs: u64,
) {
    info!("Block tracker started (interval={}s)", check_interval_secs);
    let mut ticker = interval(Duration::from_secs(check_interval_secs));

    loop {
        ticker.tick().await;

        if let Err(e) = check_pending_blocks(&db, &rpc_clients, &config).await {
            error!("Block tracker error: {}", e);
        }
    }
}

/// Check all pending blocks for confirmations.
async fn check_pending_blocks(
    db: &Db,
    rpc_clients: &HashMap<String, RpcClient>,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pending = db.get_pending_blocks().await?;
    if pending.is_empty() {
        return Ok(());
    }

    log::debug!("Checking {} pending blocks", pending.len());

    for block in &pending {
        let coin_config = match config.coin_by_symbol(&block.coin) {
            Some(c) => c,
            None => {
                warn!("No coin config for symbol {} (block id={})", block.coin, block.id);
                continue;
            }
        };

        let rpc = match rpc_clients.get(&block.coin) {
            Some(r) => r,
            None => {
                warn!("No RPC client for {} (block id={})", block.coin, block.id);
                continue;
            }
        };

        // Determine the block hash for getblock lookup.
        // Prefer block_hash (SHA-256d identity hash), fall back to hash (scrypt/PoW hash).
        let mut block_hash = if !block.block_hash.is_empty() {
            block.block_hash.clone()
        } else {
            block.hash.clone()
        };

        // Try getblock by hash first
        let mut block_info = rpc.get_block(&block_hash).await;

        // If getblock fails and we know the height, try getblockhash(height) as fallback
        if block_info.is_err() && block.height > 0 {
            log::debug!(
                "getblock({}) failed for {} id={}, trying getblockhash({})",
                &block_hash[..std::cmp::min(16, block_hash.len())],
                block.coin, block.id, block.height
            );
            if let Ok(hash_from_height) = rpc.get_block_hash(block.height as u64).await {
                info!(
                    "Height {} => hash {} for {} id={}",
                    block.height,
                    &hash_from_height[..std::cmp::min(16, hash_from_height.len())],
                    block.coin, block.id
                );
                // Update the block_hash in DB so future lookups are fast
                if let Err(e) = db.update_block_hash(block.id, &hash_from_height).await {
                    warn!("Failed to update block_hash in DB: {}", e);
                }
                block_hash = hash_from_height.clone();
                block_info = rpc.get_block(&hash_from_height).await;
            }
        }

        match block_info {
            Ok(info) => {
                if info.confirmations < 0 {
                    // Negative confirmations = orphaned (not in best chain)
                    info!(
                        "Block {} {} at height {} is ORPHANED (confirmations={})",
                        block.coin, block.id, block.height, info.confirmations
                    );
                    db.orphan_block(block.id).await?;
                } else {
                    let confs = info.confirmations as i32;
                    db.update_block_confirmations(block.id, confs).await?;

                    let required = coin_config.confirmation_depth as i32;
                    if confs >= required {
                        info!(
                            "Block {} {} at height {} CONFIRMED ({}/{})",
                            block.coin, block.id, block.height, confs, required
                        );
                        db.confirm_block(block.id).await?;

                        // Run PPLNS reward distribution
                        if let Err(e) = distribute_pplns(
                            db,
                            block,
                            config.pool.pplns_window as f64,
                            config.pool.fee_percent,
                        ).await {
                            error!(
                                "PPLNS distribution failed for block {} {}: {}",
                                block.coin, block.id, e
                            );
                        }
                    } else {
                        log::debug!(
                            "Block {} {} at height {} has {}/{} confirmations",
                            block.coin, block.id, block.height, confs, required
                        );
                    }
                }
            }
            Err(e) => {
                // If we cannot find the block, it might be orphaned
                let err_str = format!("{}", e);
                if err_str.contains("Block not found") || err_str.contains("-5") {
                    info!(
                        "Block {} {} at height {} NOT FOUND - marking orphaned",
                        block.coin, block.id, block.height
                    );
                    db.orphan_block(block.id).await?;
                } else {
                    warn!(
                        "RPC error checking block {} {} at height {}: {}",
                        block.coin, block.id, block.height, e
                    );
                }
            }
        }
    }

    Ok(())
}
