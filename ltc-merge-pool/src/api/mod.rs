pub mod pool;
pub mod miner;
pub mod blocks;
pub mod withdraw;

use actix_web::web;

/// Configure all API routes.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api")
            .route("/pool", web::get().to(pool::get_pool_stats))
            .route("/blocks", web::get().to(blocks::get_all_blocks))
            .route("/blocks/{coin}", web::get().to(blocks::get_coin_blocks))
            .route("/miner/{addr}", web::get().to(miner::get_miner_info))
            .route("/miner/{addr}/workers", web::get().to(miner::get_miner_workers))
            .route("/miner/{addr}/history", web::get().to(miner::get_miner_history))
            .route("/withdraw", web::post().to(withdraw::create_withdrawal)),
    );
}
