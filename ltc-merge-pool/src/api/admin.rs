/// Admin API endpoints — password-protected pool management.

use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::api::pool::AppState;
use super::pool::format_hashrate;

// ── Auth check ──────────────────────────────────────────────

fn check_admin(req: &HttpRequest, data: &web::Data<AppState>) -> bool {
    req.headers()
        .get("X-Admin-Key")
        .and_then(|v| v.to_str().ok())
        .map(|k| {
            data.config
                .pool
                .admin_key
                .as_deref()
                .map(|expected| k == expected)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn unauthorized() -> HttpResponse {
    HttpResponse::Unauthorized().json(serde_json::json!({
        "error": "Unauthorized — provide valid X-Admin-Key header"
    }))
}

// ── Response types ──────────────────────────────────────────

#[derive(Serialize)]
struct AdminStatsResponse {
    pool_name: String,
    hashrate: f64,
    hashrate_formatted: String,
    miners: i32,
    workers: i32,
    shares_per_sec: f64,
    total_shares: i64,
    blocks_found: i64,
    blocks_confirmed: i64,
    blocks_orphaned: i64,
    blocks_pending: i64,
    fee_percent: f64,
    total_earnings: f64,
    total_balances: f64,
    total_withdrawals: i64,
    pending_withdrawals: i64,
    coins: Vec<AdminCoinInfo>,
    db_size: String,
}

#[derive(Serialize)]
struct AdminCoinInfo {
    name: String,
    symbol: String,
    is_parent: bool,
    block_reward: f64,
    difficulty: f64,
    height: u64,
    blocks_found: i64,
    total_balance: f64,
}

#[derive(Serialize)]
struct AdminConfigResponse {
    pool_name: String,
    fee_percent: f64,
    pplns_window: u64,
    block_confirmation_depth: u64,
    pool_address: String,
    api_port: u16,
    stratum_ports: Vec<StratumPortInfo>,
    coins: Vec<CoinConfigInfo>,
}

#[derive(Serialize)]
struct StratumPortInfo {
    port: u16,
    difficulty: f64,
    vardiff: bool,
    name: String,
}

#[derive(Serialize)]
struct CoinConfigInfo {
    name: String,
    symbol: String,
    is_parent: bool,
    block_reward: f64,
    confirmation_depth: u64,
    rpc_url: String,
    reward_address: String,
}

#[derive(Serialize)]
struct AdminMinerInfo {
    address: String,
    hashrate: f64,
    hashrate_formatted: String,
    workers_online: i64,
    total_shares: i64,
    balances: Vec<MinerBalanceInfo>,
    last_seen: Option<String>,
}

#[derive(Serialize)]
struct MinerBalanceInfo {
    coin: String,
    amount: f64,
}

#[derive(Serialize)]
struct AdminBlockInfo {
    id: i32,
    coin: String,
    height: i64,
    hash: String,
    block_hash: String,
    miner: String,
    worker: String,
    reward: f64,
    difficulty: f64,
    net_difficulty: f64,
    confirmations: i32,
    status: String,
    algo: String,
    created_at: String,
    confirmed_at: Option<String>,
}

#[derive(Serialize)]
struct AdminEarningInfo {
    id: i32,
    miner: String,
    coin: String,
    block_id: Option<i32>,
    amount: f64,
    status: String,
    created_at: String,
}

#[derive(Serialize)]
struct AdminWithdrawalInfo {
    id: i32,
    miner: String,
    coin: String,
    amount: f64,
    fee: f64,
    tx_hash: Option<String>,
    status: String,
    error_message: Option<String>,
    created_at: String,
    completed_at: Option<String>,
}

#[derive(Deserialize)]
pub struct FeeUpdateRequest {
    pub fee_percent: f64,
}

#[derive(Serialize)]
struct FeeUpdateResponse {
    success: bool,
    message: String,
    new_fee: f64,
}

#[derive(Deserialize)]
pub struct PoolAddressUpdateRequest {
    pub pool_address: String,
}

#[derive(Deserialize)]
pub struct RewardAddressUpdateRequest {
    pub coin: String,
    pub reward_address: String,
}

#[derive(Serialize)]
struct AddressUpdateResponse {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct AddressesResponse {
    pool_address: String,
    coins: Vec<CoinAddressInfo>,
}

#[derive(Serialize)]
struct CoinAddressInfo {
    symbol: String,
    name: String,
    is_parent: bool,
    reward_address: String,
}

// ── Handlers ────────────────────────────────────────────────

/// GET /api/admin/stats
pub async fn get_stats(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let db = &data.db;
    let config = &data.config;

    let stat = db.get_latest_pool_stat().await.ok().flatten();
    let hashrate = stat.as_ref().map_or(0.0, |s| s.hashrate);
    let miners = stat.as_ref().map_or(0, |s| s.miners);
    let workers = stat.as_ref().map_or(0, |s| s.workers);
    let shares_per_sec = stat.as_ref().map_or(0.0, |s| s.shares_per_sec);

    let total_shares = db.count_shares().await.unwrap_or(0);
    let blocks_found = db.count_blocks(None, None).await.unwrap_or(0);
    let blocks_confirmed = db.count_blocks(None, Some("confirmed")).await.unwrap_or(0);
    let blocks_orphaned = db.count_blocks(None, Some("orphaned")).await.unwrap_or(0);
    let blocks_pending = db.count_blocks(None, Some("pending")).await.unwrap_or(0);

    // Total earnings
    let total_earnings: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount), 0) FROM earnings"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0.0);

    // Total balances across all miners
    let total_balances: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount), 0) FROM balances"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0.0);

    // Withdrawal counts
    let total_withdrawals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM withdrawals"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    let pending_withdrawals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM withdrawals WHERE status = 'pending'"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    // DB size
    let db_size: String = sqlx::query_scalar(
        "SELECT pg_size_pretty(pg_database_size(current_database()))"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or_else(|_| "unknown".to_string());

    // Per-coin info
    let mut coins = Vec::new();
    for coin_cfg in &config.coins {
        let (diff, height) = if let Some(rpc) = data.rpc_clients.get(&coin_cfg.symbol) {
            match rpc.get_blockchain_info().await {
                Ok(info) => (info.difficulty, info.blocks),
                Err(_) => (0.0, 0),
            }
        } else {
            (0.0, 0)
        };

        let coin_blocks: i64 = db.count_blocks(Some(&coin_cfg.symbol), None).await.unwrap_or(0);

        let coin_balance: f64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount), 0) FROM balances WHERE coin = $1"
        )
        .bind(&coin_cfg.symbol)
        .fetch_one(db.pool())
        .await
        .unwrap_or(0.0);

        coins.push(AdminCoinInfo {
            name: coin_cfg.name.clone(),
            symbol: coin_cfg.symbol.clone(),
            is_parent: coin_cfg.is_parent,
            block_reward: coin_cfg.block_reward,
            difficulty: diff,
            height,
            blocks_found: coin_blocks,
            total_balance: coin_balance,
        });
    }

    HttpResponse::Ok().json(AdminStatsResponse {
        pool_name: config.pool.name.clone(),
        hashrate,
        hashrate_formatted: format_hashrate(hashrate),
        miners,
        workers,
        shares_per_sec,
        total_shares,
        blocks_found,
        blocks_confirmed,
        blocks_orphaned,
        blocks_pending,
        fee_percent: config.pool.fee_percent,
        total_earnings,
        total_balances,
        total_withdrawals,
        pending_withdrawals,
        coins,
        db_size,
    })
}

/// GET /api/admin/config
pub async fn get_config(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let config = &data.config;

    let stratum_ports: Vec<StratumPortInfo> = config
        .stratum
        .ports
        .iter()
        .map(|p| StratumPortInfo {
            port: p.port,
            difficulty: p.difficulty,
            vardiff: p.vardiff,
            name: p.name.clone(),
        })
        .collect();

    let coins: Vec<CoinConfigInfo> = config
        .coins
        .iter()
        .map(|c| CoinConfigInfo {
            name: c.name.clone(),
            symbol: c.symbol.clone(),
            is_parent: c.is_parent,
            block_reward: c.block_reward,
            confirmation_depth: c.confirmation_depth,
            rpc_url: c.rpc_url.clone(),
            reward_address: c.reward_address.clone().unwrap_or_default(),
        })
        .collect();

    HttpResponse::Ok().json(AdminConfigResponse {
        pool_name: config.pool.name.clone(),
        fee_percent: config.pool.fee_percent,
        pplns_window: config.pool.pplns_window,
        block_confirmation_depth: config.pool.block_confirmation_depth,
        pool_address: config.pool.pool_address.clone(),
        api_port: config.pool.api_port,
        stratum_ports,
        coins,
    })
}

/// POST /api/admin/fee
pub async fn update_fee(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<FeeUpdateRequest>,
) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let new_fee = body.fee_percent;

    if new_fee < 0.0 || new_fee > 100.0 {
        return HttpResponse::BadRequest().json(FeeUpdateResponse {
            success: false,
            message: "Fee must be between 0 and 100".to_string(),
            new_fee: data.config.pool.fee_percent,
        });
    }

    // Update config.toml on disk
    let config_path = "config.toml";
    match std::fs::read_to_string(config_path) {
        Ok(content) => {
            // Replace the fee_percent line
            let mut updated = String::new();
            for line in content.lines() {
                if line.trim_start().starts_with("fee_percent") && line.contains('=') {
                    updated.push_str(&format!("fee_percent = {}\n", new_fee));
                } else {
                    updated.push_str(line);
                    updated.push('\n');
                }
            }
            if let Err(e) = std::fs::write(config_path, &updated) {
                return HttpResponse::InternalServerError().json(FeeUpdateResponse {
                    success: false,
                    message: format!("Failed to write config: {}", e),
                    new_fee: data.config.pool.fee_percent,
                });
            }
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(FeeUpdateResponse {
                success: false,
                message: format!("Failed to read config: {}", e),
                new_fee: data.config.pool.fee_percent,
            });
        }
    }

    // Note: the in-memory config is behind Arc and won't update until restart.
    // The file has been updated so the next restart will pick it up.
    HttpResponse::Ok().json(FeeUpdateResponse {
        success: true,
        message: format!(
            "Fee updated to {}% in config.toml. Takes effect on next restart.",
            new_fee
        ),
        new_fee,
    })
}

/// GET /api/admin/addresses — Returns all addresses (pool_address + per-coin reward_address)
pub async fn get_addresses(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let config = &data.config;

    let coins: Vec<CoinAddressInfo> = config
        .coins
        .iter()
        .map(|c| CoinAddressInfo {
            symbol: c.symbol.clone(),
            name: c.name.clone(),
            is_parent: c.is_parent,
            reward_address: c.reward_address.clone().unwrap_or_default(),
        })
        .collect();

    HttpResponse::Ok().json(AddressesResponse {
        pool_address: config.pool.pool_address.clone(),
        coins,
    })
}

/// POST /api/admin/pool-address — Update the pool_address (fee address) in config.toml
pub async fn update_pool_address(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<PoolAddressUpdateRequest>,
) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let new_address = body.pool_address.trim().to_string();

    // Validate: non-empty, reasonable length (20-100 chars covers all crypto address formats)
    if new_address.is_empty() {
        return HttpResponse::BadRequest().json(AddressUpdateResponse {
            success: false,
            message: "Pool address cannot be empty".to_string(),
        });
    }
    if new_address.len() < 20 || new_address.len() > 100 {
        return HttpResponse::BadRequest().json(AddressUpdateResponse {
            success: false,
            message: "Pool address must be between 20 and 100 characters".to_string(),
        });
    }

    let config_path = "config.toml";
    match std::fs::read_to_string(config_path) {
        Ok(content) => {
            let mut updated = String::new();
            let mut in_pool_section = false;
            let mut replaced = false;

            for line in content.lines() {
                let trimmed = line.trim();

                // Track which section we're in
                if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
                    in_pool_section = trimmed == "[pool]";
                } else if trimmed.starts_with("[[") {
                    in_pool_section = false;
                }

                if in_pool_section && trimmed.starts_with("pool_address") && trimmed.contains('=') && !replaced {
                    updated.push_str(&format!("pool_address = \"{}\"\n", new_address));
                    replaced = true;
                } else {
                    updated.push_str(line);
                    updated.push('\n');
                }
            }

            if !replaced {
                return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                    success: false,
                    message: "Could not find pool_address in config.toml".to_string(),
                });
            }

            if let Err(e) = std::fs::write(config_path, &updated) {
                return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                    success: false,
                    message: format!("Failed to write config: {}", e),
                });
            }
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                success: false,
                message: format!("Failed to read config: {}", e),
            });
        }
    }

    HttpResponse::Ok().json(AddressUpdateResponse {
        success: true,
        message: format!(
            "Pool address updated to {} in config.toml. Takes effect on next restart.",
            new_address
        ),
    })
}

/// POST /api/admin/reward-address — Update a coin's reward_address in config.toml
pub async fn update_reward_address(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<RewardAddressUpdateRequest>,
) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let coin_symbol = body.coin.trim().to_uppercase();
    let new_address = body.reward_address.trim().to_string();

    // Validate coin exists
    if data.config.coin_by_symbol(&coin_symbol).is_none() {
        return HttpResponse::BadRequest().json(AddressUpdateResponse {
            success: false,
            message: format!("Unknown coin: {}", coin_symbol),
        });
    }

    // Validate address
    if new_address.is_empty() {
        return HttpResponse::BadRequest().json(AddressUpdateResponse {
            success: false,
            message: "Reward address cannot be empty".to_string(),
        });
    }
    if new_address.len() < 20 || new_address.len() > 100 {
        return HttpResponse::BadRequest().json(AddressUpdateResponse {
            success: false,
            message: "Reward address must be between 20 and 100 characters".to_string(),
        });
    }

    let config_path = "config.toml";
    match std::fs::read_to_string(config_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();

            // Two-pass approach: first find which [[coins]] section contains the target symbol,
            // then find the reward_address line within that section.
            // This handles any field ordering within the section.

            // Pass 1: identify [[coins]] section boundaries and which one has our symbol
            let mut section_starts: Vec<usize> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if line.trim() == "[[coins]]" {
                    section_starts.push(i);
                }
            }

            // Find which section contains our target symbol
            let mut target_section_start: Option<usize> = None;
            for (idx, &start) in section_starts.iter().enumerate() {
                let end = if idx + 1 < section_starts.len() {
                    section_starts[idx + 1]
                } else {
                    lines.len()
                };
                for i in start..end {
                    let trimmed = lines[i].trim();
                    if trimmed.starts_with("symbol") && trimmed.contains('=') {
                        if let Some(val) = trimmed.split('=').nth(1) {
                            let sym = val.trim().trim_matches('"').trim();
                            if sym.eq_ignore_ascii_case(&coin_symbol) {
                                target_section_start = Some(start);
                                break;
                            }
                        }
                    }
                }
                if target_section_start.is_some() {
                    break;
                }
            }

            let section_start = match target_section_start {
                Some(s) => s,
                None => {
                    return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                        success: false,
                        message: format!("Could not find [[coins]] section for {} in config.toml", coin_symbol),
                    });
                }
            };

            // Find the end of this section
            let section_end = section_starts
                .iter()
                .find(|&&s| s > section_start)
                .copied()
                .unwrap_or(lines.len());

            // Pass 2: find the reward_address line within this section
            let mut reward_line_idx: Option<usize> = None;
            for i in section_start..section_end {
                let trimmed = lines[i].trim();
                if trimmed.starts_with("reward_address") && trimmed.contains('=') {
                    reward_line_idx = Some(i);
                    break;
                }
            }

            let reward_idx = match reward_line_idx {
                Some(idx) => idx,
                None => {
                    return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                        success: false,
                        message: format!("Could not find reward_address for {} in config.toml", coin_symbol),
                    });
                }
            };

            // Build updated content
            let mut updated = String::new();
            for (i, line) in lines.iter().enumerate() {
                if i == reward_idx {
                    updated.push_str(&format!("reward_address = \"{}\"\n", new_address));
                } else {
                    updated.push_str(line);
                    updated.push('\n');
                }
            }

            if let Err(e) = std::fs::write(config_path, &updated) {
                return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                    success: false,
                    message: format!("Failed to write config: {}", e),
                });
            }
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(AddressUpdateResponse {
                success: false,
                message: format!("Failed to read config: {}", e),
            });
        }
    }

    HttpResponse::Ok().json(AddressUpdateResponse {
        success: true,
        message: format!(
            "{} reward address updated to {} in config.toml. Takes effect on next restart.",
            coin_symbol, new_address
        ),
    })
}

/// GET /api/admin/miners
pub async fn get_miners(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let db = &data.db;

    // Get all miners with their stats
    let rows = sqlx::query(
        "SELECT m.address, m.created_at,
                COALESCE(w.worker_count, 0) as workers_online,
                COALESCE(w.last_seen, m.created_at) as last_seen,
                COALESCE(s.share_count, 0) as total_shares,
                COALESCE(s.sum_diff, 0) as sum_diff
         FROM miners m
         LEFT JOIN (
             SELECT miner, COUNT(*) as worker_count, MAX(last_seen) as last_seen
             FROM workers WHERE is_online = true GROUP BY miner
         ) w ON m.address = w.miner
         LEFT JOIN (
             SELECT miner, COUNT(*) as share_count, SUM(difficulty) as sum_diff
             FROM shares WHERE is_valid = true AND created_at > NOW() - INTERVAL '600 seconds'
             GROUP BY miner
         ) s ON m.address = s.miner
         ORDER BY COALESCE(s.sum_diff, 0) DESC"
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let mut miners_list = Vec::new();
    for row in &rows {
        let address: String = row.get("address");
        let workers_online: i64 = row.get("workers_online");
        let total_shares: i64 = row.get("total_shares");
        let sum_diff: f64 = row.get("sum_diff");
        let last_seen: chrono::DateTime<chrono::Utc> = row.get("last_seen");
        let hashrate = sum_diff * 65536.0 / 600.0;

        // Get balances
        let balances = db.get_balances(&address).await.unwrap_or_default();
        let balance_info: Vec<MinerBalanceInfo> = balances
            .iter()
            .map(|b| MinerBalanceInfo {
                coin: b.coin.clone(),
                amount: b.amount,
            })
            .collect();

        miners_list.push(AdminMinerInfo {
            address,
            hashrate,
            hashrate_formatted: format_hashrate(hashrate),
            workers_online,
            total_shares,
            balances: balance_info,
            last_seen: Some(last_seen.to_rfc3339()),
        });
    }

    HttpResponse::Ok().json(serde_json::json!({
        "miners": miners_list,
        "total": miners_list.len()
    }))
}

/// GET /api/admin/blocks
pub async fn get_blocks(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let db = &data.db;

    // Get ALL blocks (not just last 100)
    let blocks = sqlx::query_as::<_, crate::db::BlockRow>(
        "SELECT id, coin, height, hash, block_hash, miner, worker, reward, difficulty,
                net_difficulty, confirmations, status, algo, created_at, confirmed_at
         FROM blocks ORDER BY created_at DESC LIMIT 500"
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let total: i64 = db.count_blocks(None, None).await.unwrap_or(0);

    let block_list: Vec<AdminBlockInfo> = blocks
        .iter()
        .map(|b| AdminBlockInfo {
            id: b.id,
            coin: b.coin.clone(),
            height: b.height,
            hash: b.hash.clone(),
            block_hash: b.block_hash.clone(),
            miner: b.miner.clone(),
            worker: b.worker.clone(),
            reward: b.reward,
            difficulty: b.difficulty,
            net_difficulty: b.net_difficulty,
            confirmations: b.confirmations,
            status: b.status.clone(),
            algo: b.algo.clone(),
            created_at: b.created_at.to_rfc3339(),
            confirmed_at: b.confirmed_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    HttpResponse::Ok().json(serde_json::json!({
        "blocks": block_list,
        "total": total
    }))
}

/// GET /api/admin/earnings
pub async fn get_earnings(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let db = &data.db;

    let earnings = sqlx::query_as::<_, crate::db::EarningRow>(
        "SELECT id, miner, coin, block_id, amount, status, created_at
         FROM earnings ORDER BY created_at DESC LIMIT 500"
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM earnings")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    let earning_list: Vec<AdminEarningInfo> = earnings
        .iter()
        .map(|e| AdminEarningInfo {
            id: e.id,
            miner: e.miner.clone(),
            coin: e.coin.clone(),
            block_id: e.block_id,
            amount: e.amount,
            status: e.status.clone(),
            created_at: e.created_at.to_rfc3339(),
        })
        .collect();

    HttpResponse::Ok().json(serde_json::json!({
        "earnings": earning_list,
        "total": total
    }))
}

/// GET /api/admin/withdrawals
pub async fn get_withdrawals(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    if !check_admin(&req, &data) {
        return unauthorized();
    }

    let db = &data.db;

    let withdrawals = sqlx::query_as::<_, crate::db::WithdrawalRow>(
        "SELECT id, miner, coin, amount, fee, tx_hash, status, error_message, created_at, completed_at
         FROM withdrawals ORDER BY created_at DESC LIMIT 500"
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM withdrawals")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM withdrawals WHERE status = 'pending'"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    let withdrawal_list: Vec<AdminWithdrawalInfo> = withdrawals
        .iter()
        .map(|w| AdminWithdrawalInfo {
            id: w.id,
            miner: w.miner.clone(),
            coin: w.coin.clone(),
            amount: w.amount,
            fee: w.fee,
            tx_hash: w.tx_hash.clone(),
            status: w.status.clone(),
            error_message: w.error_message.clone(),
            created_at: w.created_at.to_rfc3339(),
            completed_at: w.completed_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    HttpResponse::Ok().json(serde_json::json!({
        "withdrawals": withdrawal_list,
        "total": total,
        "pending": pending
    }))
}


#[derive(Deserialize)]
pub struct PasswordUpdateRequest {
    pub current_password: String,
    pub new_password: String,
}

/// POST /api/admin/password - Change admin password
pub async fn update_password(
    req: HttpRequest,
    data: web::Data<AppState>,
    body: web::Json<PasswordUpdateRequest>,
) -> HttpResponse {
    if !check_admin(&req, &data) {
        return HttpResponse::Unauthorized().json(serde_json::json!({"error": "Unauthorized"}));
    }
    let new_pw = body.new_password.trim();
    if new_pw.len() < 4 {
        return HttpResponse::BadRequest().json(serde_json::json!({"error": "Password must be at least 4 characters"}));
    }
    // Update config.toml
    let config_path = "config.toml";
    if let Ok(content) = std::fs::read_to_string(config_path) {
        let updated = content.lines().map(|line| {
            if line.trim().starts_with("admin_key") {
                format!("admin_key = \"{}\"", new_pw)
            } else {
                line.to_string()
            }
        }).collect::<Vec<_>>().join("\n");
        if std::fs::write(config_path, &updated).is_ok() {
            return HttpResponse::Ok().json(serde_json::json!({"success": true, "message": "Password updated. Use new password on next login."}));
        }
    }
    HttpResponse::InternalServerError().json(serde_json::json!({"error": "Failed to update config"}))
}
