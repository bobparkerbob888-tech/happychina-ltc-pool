use sqlx::Row;
/// Miner-specific API endpoints.

use actix_web::{web, HttpResponse};
use serde::Serialize;

use crate::api::pool::AppState;
use super::pool::format_hashrate;

#[derive(Serialize)]
struct MinerInfoResponse {
    address: String,
    hashrate: f64,
    hashrate_formatted: String,
    worker_count: usize,
    workers_online: usize,
    balances: Vec<BalanceInfo>,
}

#[derive(Serialize)]
struct BalanceInfo {
    coin: String,
    amount: f64,
}

#[derive(Serialize)]
struct WorkerInfo {
    name: String,
    hashrate: f64,
    hashrate_formatted: String,
    difficulty: f64,
    last_seen: String,
    is_online: bool,
    user_agent: String,
}

#[derive(Serialize)]
struct WorkerListResponse {
    address: String,
    workers: Vec<WorkerInfo>,
}

#[derive(Serialize)]
struct HistoryPoint {
    timestamp: String,
    hashrate: f64,
}

#[derive(Serialize)]
struct MinerHistoryResponse {
    address: String,
    history: Vec<HistoryPoint>,
}

/// GET /api/miner/{addr}
pub async fn get_miner_info(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let addr = path.into_inner();
    let db = &data.db;

    // Check if miner exists
    match db.get_miner(&addr).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            // Return empty stats for unknown miner
            return HttpResponse::Ok().json(MinerInfoResponse {
                address: addr,
                hashrate: 0.0,
                hashrate_formatted: "0 H/s".to_string(),
                worker_count: 0,
                workers_online: 0,
                balances: Vec::new(),
            });
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": format!("Database error: {}", e)
            }));
        }
    }

    // Calculate hashrate from shares: SUM(difficulty) * 65536 / window
    let miner_diff_sum: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(difficulty), 0) FROM shares WHERE miner = $1 AND created_at > NOW() - INTERVAL '600 seconds' AND is_valid = true"
    )
    .bind(&addr)
    .fetch_one(db.pool())
    .await
    .unwrap_or(0.0);
    let hashrate = miner_diff_sum * 65536.0 / 600.0;

    // Count workers from shares
    let worker_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT worker) FROM shares WHERE miner = $1 AND created_at > NOW() - INTERVAL '600 seconds'"
    )
    .bind(&addr)
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    let workers_online = worker_count as usize;
    let worker_total = worker_count as usize;
    let balances = db.get_balances(&addr).await.unwrap_or_default();

    let balance_info: Vec<BalanceInfo> = balances
        .iter()
        .map(|b| BalanceInfo {
            coin: b.coin.clone(),
            amount: b.amount,
        })
        .collect();

    HttpResponse::Ok().json(MinerInfoResponse {
        address: addr,
        hashrate,
        hashrate_formatted: format_hashrate(hashrate),
        worker_count: worker_total,
        workers_online,
        balances: balance_info,
    })
}

/// GET /api/miner/{addr}/workers
pub async fn get_miner_workers(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let addr = path.into_inner();
    let db = &data.db;

    // Get workers from recent shares — use SUM(difficulty) for hashrate, MAX(difficulty) for current stratum diff
    let rows = sqlx::query(
        "SELECT worker, SUM(difficulty) as sum_diff, MAX(difficulty) as current_diff, MAX(created_at) as last_seen \
         FROM shares WHERE miner = $1 AND created_at > NOW() - INTERVAL '600 seconds' AND is_valid = true \
         GROUP BY worker ORDER BY worker"
    )
    .bind(&addr)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let window_secs = 600.0_f64;
    let worker_list: Vec<WorkerInfo> = rows
        .iter()
        .map(|r| {
            let name: String = r.get("worker");
            let sum_diff: f64 = r.get("sum_diff");
            let current_diff: f64 = r.get("current_diff");
            let last_seen: chrono::DateTime<chrono::Utc> = r.get("last_seen");
            // hashrate = SUM(difficulty) * 65536 / window
            let hashrate = sum_diff * 65536.0 / window_secs;
            WorkerInfo {
                name,
                hashrate,
                hashrate_formatted: format_hashrate(hashrate),
                difficulty: current_diff,
                last_seen: last_seen.to_rfc3339(),
                is_online: true,
                user_agent: String::new(),
            }
        })
        .collect();

    HttpResponse::Ok().json(WorkerListResponse {
        address: addr,
        workers: worker_list,
    })
}

/// GET /api/miner/{addr}/history
pub async fn get_miner_history(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let addr = path.into_inner();
    let db = &data.db;

    let history = db.get_miner_stats_history(&addr, 24).await.unwrap_or_default();

    let points: Vec<HistoryPoint> = history
        .iter()
        .map(|s| HistoryPoint {
            timestamp: s.created_at.to_rfc3339(),
            hashrate: s.hashrate,
        })
        .collect();

    HttpResponse::Ok().json(MinerHistoryResponse {
        address: addr,
        history: points,
    })
}
