pub mod pool;
pub mod miner;
pub mod blocks;
pub mod withdraw;
pub mod admin;

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
            .route("/withdraw", web::post().to(withdraw::create_withdrawal))
            // Payout address endpoints
            .route("/payout-address", web::post().to(withdraw::set_payout_address))
            .route("/payout-addresses/{miner}", web::get().to(withdraw::get_payout_addresses))
            // Admin endpoints
            .route("/admin/stats", web::get().to(admin::get_stats))
            .route("/admin/config", web::get().to(admin::get_config))
            .route("/admin/fee", web::post().to(admin::update_fee))
            .route("/admin/miners", web::get().to(admin::get_miners))
            .route("/admin/blocks", web::get().to(admin::get_blocks))
            .route("/admin/earnings", web::get().to(admin::get_earnings))
            .route("/admin/withdrawals", web::get().to(admin::get_withdrawals))
            .route("/admin/pool-address", web::post().to(admin::update_pool_address))
            .route("/admin/addresses", web::get().to(admin::get_addresses))
            .route("/admin/reward-address", web::post().to(admin::update_reward_address))
            .route("/admin/password", web::post().to(admin::update_password))
            .route("/admin/reconcile-balances", web::post().to(admin::reconcile_balances)),
    );
}
