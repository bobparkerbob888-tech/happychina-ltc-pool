/// Pool-wide stats API endpoint.

use actix_web::{web, HttpResponse};
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use crate::db::Db;
use crate::rpc::RpcClient;

/// Shared application state for API handlers.
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub rpc_clients: Arc<HashMap<String, RpcClient>>,
}

#[derive(Serialize)]
struct PoolStatsResponse {
    pool_name: String,
    hashrate: f64,
    hashrate_formatted: String,
    miners: i32,
    workers: i32,
    shares_per_sec: f64,
    network_difficulty: f64,
    best_share: f64,
    total_shares: i64,
    blocks_found: i64,
    blocks_confirmed: i64,
    fee_percent: f64,
    coins: Vec<CoinInfo>,
    est_time_to_find: String,
}

#[derive(Serialize)]
struct CoinInfo {
    name: String,
    symbol: String,
    is_parent: bool,
    block_reward: f64,
    difficulty: f64,
    height: u64,
}

/// GET /api/pool
pub async fn get_pool_stats(data: web::Data<AppState>) -> HttpResponse {
    let db = &data.db;
    let config = &data.config;

    // Get latest pool stats
    let stat = db.get_latest_pool_stat().await.ok().flatten();
    let hashrate = stat.as_ref().map_or(0.0, |s| s.hashrate);
    let miners = stat.as_ref().map_or(0, |s| s.miners);
    let workers = stat.as_ref().map_or(0, |s| s.workers);
    let shares_per_sec = stat.as_ref().map_or(0.0, |s| s.shares_per_sec);

    let total_shares = db.count_shares().await.unwrap_or(0);
    let blocks_found = db.count_blocks(None, None).await.unwrap_or(0);
    let blocks_confirmed = db.count_blocks(None, Some("confirmed")).await.unwrap_or(0);

    // Get best share ever
    let best_share: f64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(share_difficulty), 0) FROM shares WHERE is_valid = true"
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0.0);

    // Get network info for parent coin
    let mut network_difficulty = 0.0;
    let mut coins = Vec::new();

    for coin_cfg in &config.coins {
        if let Some(rpc) = data.rpc_clients.get(&coin_cfg.symbol) {
            let (diff, height) = match rpc.get_blockchain_info().await {
                Ok(info) => {
                    if coin_cfg.is_parent {
                        network_difficulty = info.difficulty;
                    }
                    (info.difficulty, info.blocks)
                }
                Err(_) => (0.0, 0),
            };
            coins.push(CoinInfo {
                name: coin_cfg.name.clone(),
                symbol: coin_cfg.symbol.clone(),
                is_parent: coin_cfg.is_parent,
                block_reward: coin_cfg.block_reward,
                difficulty: diff,
                height,
            });
        }
    }

    // Estimate time to find a block (seconds)
    let est_ttf = if hashrate > 0.0 && network_difficulty > 0.0 {
        // For scrypt: TTF = difficulty * 2^32 / hashrate
        let ttf_secs = network_difficulty * 4294967296.0 / hashrate;
        format_duration(ttf_secs)
    } else {
        "N/A".to_string()
    };

    HttpResponse::Ok().json(PoolStatsResponse {
        pool_name: config.pool.name.clone(),
        hashrate,
        hashrate_formatted: format_hashrate(hashrate),
        miners,
        workers,
        shares_per_sec,
        network_difficulty,
        best_share: best_share,
        total_shares,
        blocks_found,
        blocks_confirmed,
        fee_percent: config.pool.fee_percent,
        coins,
        est_time_to_find: est_ttf,
    })
}

/// Format hashrate for display (H/s, KH/s, MH/s, GH/s, TH/s, PH/s).
pub fn format_hashrate(hashrate: f64) -> String {
    if hashrate <= 0.0 {
        return "0 H/s".to_string();
    }
    let units = ["H/s", "KH/s", "MH/s", "GH/s", "TH/s", "PH/s", "EH/s"];
    let mut value = hashrate;
    let mut unit_idx = 0;
    while value >= 1000.0 && unit_idx < units.len() - 1 {
        value /= 1000.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", value, units[unit_idx])
}

/// Format seconds into a human-readable duration string.
fn format_duration(secs: f64) -> String {
    if secs < 0.0 || secs.is_nan() || secs.is_infinite() {
        return "N/A".to_string();
    }
    let total = secs as u64;
    if total < 60 {
        format!("{}s", total)
    } else if total < 3600 {
        format!("{}m {}s", total / 60, total % 60)
    } else if total < 86400 {
        format!("{}h {}m", total / 3600, (total % 3600) / 60)
    } else {
        format!("{}d {}h", total / 86400, (total % 86400) / 3600)
    }
}
