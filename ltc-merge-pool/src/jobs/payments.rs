/// PPLNS reward distribution and withdrawal processor.

use std::collections::HashMap;
use std::sync::Arc;
use log::{error, info, warn};
use tokio::time::{interval, Duration};

use crate::config::Config;
use crate::db::{BlockRow, Db};
use crate::rpc::RpcClient;

/// Distribute rewards for a confirmed block using PPLNS.
///
/// Gets the last `window_size` difficulty worth of shares,
/// computes each miner's proportion, and credits their balance.
pub async fn distribute_pplns(
    db: &Db,
    block: &BlockRow,
    window_size: f64,
    pool_fee_percent: f64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let reward = block.reward;
    if reward <= 0.0 {
        warn!("Block {} {} has zero reward, skipping PPLNS", block.coin, block.id);
        return Ok(());
    }

    // Deduct pool fee
    let fee = reward * (pool_fee_percent / 100.0);
    let distributable = reward - fee;

    if distributable <= 0.0 {
        warn!("Block {} {} has no distributable reward after fee", block.coin, block.id);
        return Ok(());
    }

    // Get PPLNS shares
    let shares = db.get_pplns_shares(window_size).await?;
    if shares.is_empty() {
        warn!("No shares found for PPLNS distribution (block {} {})", block.coin, block.id);
        return Ok(());
    }

    // Sum total difficulty
    let total_diff: f64 = shares.iter().map(|s| s.difficulty).sum();
    if total_diff <= 0.0 {
        warn!("Total share difficulty is zero for PPLNS");
        return Ok(());
    }

    // Aggregate shares per miner
    let mut miner_diff: HashMap<String, f64> = HashMap::new();
    for share in &shares {
        *miner_diff.entry(share.miner.clone()).or_insert(0.0) += share.difficulty;
    }

    info!(
        "PPLNS for {} block {} (height={}): reward={:.8} fee={:.8} distributable={:.8} shares={} miners={}",
        block.coin, block.id, block.height, reward, fee, distributable,
        shares.len(), miner_diff.len()
    );

    // Credit each miner proportionally
    for (miner, diff) in &miner_diff {
        let proportion = diff / total_diff;
        let amount = distributable * proportion;

        if amount <= 0.0 {
            continue;
        }

        // Credit the balance
        db.credit_balance(miner, &block.coin, amount).await?;

        // Record the earning
        db.insert_earning(miner, &block.coin, block.id, amount).await?;

        info!(
            "  Credited {:.8} {} to {} ({:.2}%)",
            amount, block.coin, miner, proportion * 100.0
        );
    }

    Ok(())
}

/// Start the withdrawal processor background task.
/// Processes pending withdrawals by sending coins via RPC.
///
/// NOTE: With the new withdraw endpoint, withdrawals are now processed
/// synchronously (send + debit happens in the API handler). This background
/// processor serves as a safety net for any withdrawals that were created
/// but not yet processed (e.g., if the server crashed mid-withdrawal).
pub async fn run_withdrawal_processor(
    db: Db,
    rpc_clients: Arc<HashMap<String, RpcClient>>,
    _config: Arc<Config>,
    check_interval_secs: u64,
) {
    info!("Withdrawal processor started (interval={}s)", check_interval_secs);
    let mut ticker = interval(Duration::from_secs(check_interval_secs));

    loop {
        ticker.tick().await;

        if let Err(e) = process_withdrawals(&db, &rpc_clients).await {
            error!("Withdrawal processor error: {}", e);
        }
    }
}

/// Process all pending withdrawals.
///
/// Fixed flow: balance is only deducted AFTER a successful send.
async fn process_withdrawals(
    db: &Db,
    rpc_clients: &HashMap<String, RpcClient>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pending = db.get_pending_withdrawals().await?;
    if pending.is_empty() {
        return Ok(());
    }

    info!("Processing {} pending withdrawals", pending.len());

    for withdrawal in &pending {
        let rpc = match rpc_clients.get(&withdrawal.coin) {
            Some(r) => r,
            None => {
                let msg = format!("No RPC client for coin {}", withdrawal.coin);
                warn!("{}", msg);
                db.fail_withdrawal(withdrawal.id, &msg).await?;
                continue;
            }
        };

        // Use the stored payout_address if set, otherwise fall back to miner address
        let address = withdrawal
            .payout_address
            .as_deref()
            .unwrap_or(&withdrawal.miner);

        let amount = withdrawal.amount - withdrawal.fee;

        if amount <= 0.0 {
            db.fail_withdrawal(withdrawal.id, "Amount after fee is zero or negative").await?;
            continue;
        }

        // Validate the address before sending
        match rpc.validate_address(address).await {
            Ok(result) => {
                if !result.isvalid {
                    let msg = format!(
                        "Payout address {} is not valid for {}",
                        address, withdrawal.coin
                    );
                    warn!("Withdrawal {}: {}", withdrawal.id, msg);
                    db.fail_withdrawal(withdrawal.id, &msg).await?;
                    continue;
                }
            }
            Err(e) => {
                let msg = format!("Failed to validate address: {}", e);
                warn!("Withdrawal {}: {}", withdrawal.id, msg);
                db.fail_withdrawal(withdrawal.id, &msg).await?;
                continue;
            }
        }

        info!(
            "Sending {:.8} {} to {} (withdrawal id={}, miner={})",
            amount, withdrawal.coin, address, withdrawal.id, withdrawal.miner
        );

        match rpc.send_to_address(address, amount).await {
            Ok(txid) => {
                info!(
                    "Withdrawal {} completed: txid={} to={}",
                    withdrawal.id, txid, address
                );

                // Deduct balance AFTER successful send
                if let Err(e) = db.debit_balance(&withdrawal.miner, &withdrawal.coin, withdrawal.amount).await {
                    error!(
                        "CRITICAL: Withdrawal {} sent (txid={}) but balance deduction failed: {}. \
                         Miner={} Coin={} Amount={:.8}. Manual intervention required!",
                        withdrawal.id, txid, e, withdrawal.miner, withdrawal.coin, withdrawal.amount
                    );
                }

                db.complete_withdrawal(withdrawal.id, &txid).await?;
            }
            Err(e) => {
                let msg = format!("sendtoaddress failed: {}", e);
                error!("Withdrawal {} failed: {}", withdrawal.id, msg);
                // Do NOT deduct balance on failure
                db.fail_withdrawal(withdrawal.id, &msg).await?;
            }
        }
    }

    Ok(())
}
