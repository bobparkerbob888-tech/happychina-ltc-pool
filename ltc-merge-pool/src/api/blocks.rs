/// Blocks API endpoints.

use actix_web::{web, HttpResponse};
use serde::Serialize;

use crate::api::pool::AppState;

#[derive(Serialize)]
struct BlockInfo {
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
struct BlocksResponse {
    blocks: Vec<BlockInfo>,
    total: i64,
}

/// GET /api/blocks
pub async fn get_all_blocks(data: web::Data<AppState>) -> HttpResponse {
    let db = &data.db;

    let blocks = db.get_recent_blocks(None, 100).await.unwrap_or_default();
    let total = db.count_blocks(None, None).await.unwrap_or(0);

    let block_list: Vec<BlockInfo> = blocks
        .iter()
        .map(|b| BlockInfo {
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

    HttpResponse::Ok().json(BlocksResponse {
        blocks: block_list,
        total,
    })
}

/// GET /api/blocks/{coin}
pub async fn get_coin_blocks(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let coin = path.into_inner().to_uppercase();
    let db = &data.db;

    let blocks = db.get_recent_blocks(Some(&coin), 100).await.unwrap_or_default();
    let total = db.count_blocks(Some(&coin), None).await.unwrap_or(0);

    let block_list: Vec<BlockInfo> = blocks
        .iter()
        .map(|b| BlockInfo {
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

    HttpResponse::Ok().json(BlocksResponse {
        blocks: block_list,
        total,
    })
}
