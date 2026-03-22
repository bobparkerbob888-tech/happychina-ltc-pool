mod config;
mod crypto;
mod stratum;
mod db;
mod rpc;
mod types;
mod jobs;
mod api;
mod zmq_sub;

use std::collections::HashMap;
use std::sync::Arc;
use log::{error, info, warn};
use actix_web::{App, HttpServer, web};
use actix_files as fs;
use actix_cors::Cors;

use crate::api::pool::AppState;
use crate::stratum::job::JobManager;
use crate::stratum::notifications::NotificationBroadcaster;
use crate::stratum::server::StratumServer;
use crate::stratum::vardiff::VardiffConfig;
use crate::stratum::protocol::mining_notify;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    // Support --config flag for alternate config file
    let config_path = args.iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("config.toml");

    let cfg = config::Config::load(config_path)?;
    info!(
        "Loaded config: {} with {} coins",
        cfg.pool.name,
        cfg.coins.len()
    );
    info!(
        "Parent chain: {} ({})",
        cfg.parent_coin().name,
        cfg.parent_coin().symbol
    );
    for aux in cfg.aux_coins() {
        info!("  Aux chain: {} ({})", aux.name, aux.symbol);
    }
    info!(
        "Stratum ports: {:?}",
        cfg.stratum
            .ports
            .iter()
            .map(|p| p.port)
            .collect::<Vec<_>>()
    );

    // Create RPC clients for all coin daemons
    let rpc_map = rpc::create_rpc_clients(&cfg.coins);

    // ── Test modes ──────────────────────────────────────
    if args.iter().any(|a| a == "--test-rpc") {
        info!("=== Testing RPC connections to all {} daemons ===", rpc_map.len());
        let results = rpc::test_all_connections(&rpc_map).await;
        let mut all_ok = true;

        for (symbol, result) in &results {
            match result {
                Ok(info) => {
                    info!("[{}] OK - chain: {}, height: {}, difficulty: {:.4}", symbol, info.chain, info.blocks, info.difficulty);
                }
                Err(e) => {
                    error!("[{}] FAILED - {}", symbol, e);
                    all_ok = false;
                }
            }
        }

        let parent_client = rpc_map.get(&cfg.parent_coin().symbol).unwrap();
        match parent_client.get_block_template().await {
            Ok(template) => {
                info!("[{}] Block template: height={}, txns={}", cfg.parent_coin().symbol, template.height, template.transactions.len());
            }
            Err(e) => {
                error!("[{}] getblocktemplate FAILED: {}", cfg.parent_coin().symbol, e);
                all_ok = false;
            }
        }

        if all_ok {
            info!("=== All RPC connections successful! ===");
        } else {
            error!("=== Some RPC connections failed ===");
            std::process::exit(1);
        }
        return Ok(());
    }

    if args.iter().any(|a| a == "--test-db") {
        info!("=== Testing database connection ===");
        let database = db::Db::connect(&cfg.database.url).await?;
        let migration_sql = std::fs::read_to_string("migrations/001_initial.sql")?;
        database.run_migration(&migration_sql).await?;
        // Run payout addresses migration
        if let Ok(migration2) = std::fs::read_to_string("migrations/002_payout_addresses.sql") {
            database.run_migration(&migration2).await?;
        }
        info!("=== Database connection and migration successful! ===");
        return Ok(());
    }

    // ── Production startup ──────────────────────────────
    info!("HappyChina Pool starting up...");
    info!("Pool address: {}", cfg.pool.pool_address);
    info!("Fee: {}%", cfg.pool.fee_percent);
    info!("PPLNS window: {} shares", cfg.pool.pplns_window);

    // 1. Connect to database
    let database = db::Db::connect(&cfg.database.url).await?;

    // Run migration
    let migration_sql = std::fs::read_to_string("migrations/001_initial.sql")?;
    database.run_migration(&migration_sql).await?;
    // Run payout addresses migration
    if let Ok(migration2) = std::fs::read_to_string("migrations/002_payout_addresses.sql") {
        database.run_migration(&migration2).await?;
    }
    info!("Database migration complete");

    // Wrap config and RPC clients in Arc for sharing
    let config = Arc::new(cfg.clone());
    let rpc_clients = Arc::new(rpc_map);

    // 2. Get pool address scriptPubKey
    let parent_rpc = rpc_clients.get(&cfg.parent_coin().symbol).unwrap();
    let pool_script = match parent_rpc.validate_address(&cfg.pool.pool_address).await {
        Ok(result) => {
            if !result.isvalid {
                error!("Pool address is not valid!");
                std::process::exit(1);
            }
            result.script_pub_key.unwrap_or_else(|| {
                error!("Pool address has no scriptPubKey!");
                std::process::exit(1);
            })
        }
        Err(e) => {
            error!("Failed to validate pool address: {}", e);
            std::process::exit(1);
        }
    };
    info!("Pool scriptPubKey: {}", pool_script);

    // 3. Create job manager
    let job_manager = Arc::new(JobManager::new(
        pool_script,
        cfg.pool.fee_percent,
        4, // extranonce1 size
        4, // extranonce2 size
    ));

    // 4. Create notification broadcaster
    let broadcaster = Arc::new(NotificationBroadcaster::new(256));

    // 5. Create vardiff config
    let vardiff_config = VardiffConfig {
        target_time: cfg.stratum.vardiff_target_time,
        retarget_shares: cfg.stratum.vardiff_retarget_shares,
        min_difficulty: cfg.stratum.min_difficulty,
        max_difficulty: cfg.stratum.max_difficulty,
    };

    // 6. Create stratum server
    let stratum_server = StratumServer::new(
        Arc::clone(&job_manager),
        Arc::clone(&broadcaster),
        cfg.stratum.max_connections,
        cfg.stratum.connection_timeout_secs,
        vardiff_config,
        Some(Arc::new(database.clone())),
        Some(Arc::clone(&rpc_clients)),
        Some(Arc::clone(&config)),
    );

    // 7. Start stratum listeners
    stratum_server
        .start(&cfg.stratum.listen_address, &cfg.stratum.ports)
        .await?;
    info!("Stratum server started on all ports");

    // 8. Start block tracker background task
    {
        let db_clone = database.clone();
        let rpc_clone = Arc::clone(&rpc_clients);
        let cfg_clone = Arc::clone(&config);
        tokio::spawn(async move {
            jobs::block_tracker::run_block_tracker(db_clone, rpc_clone, cfg_clone, 60).await;
        });
        info!("Block tracker background task started (60s interval)");
    }

    // 9. Start stats updater background task
    {
        let db_clone = database.clone();
        let timeout = cfg.stratum.connection_timeout_secs as i64;
        tokio::spawn(async move {
            jobs::stats::run_stats_updater(db_clone, 120, timeout).await;
        });
        info!("Stats updater background task started (120s interval)");
    }

    // 10. Start withdrawal processor background task
    {
        let db_clone = database.clone();
        let rpc_clone = Arc::clone(&rpc_clients);
        let cfg_clone = Arc::clone(&config);
        tokio::spawn(async move {
            jobs::payments::run_withdrawal_processor(db_clone, rpc_clone, cfg_clone, 30).await;
        });
        info!("Withdrawal processor background task started (30s interval)");
    }

    // 11. Start ZMQ subscribers for instant block notifications
    let zmq_endpoints: Vec<(String, String)> = config.coins.iter()
        .filter_map(|c| c.zmq_hashblock.as_ref().map(|z| (c.symbol.clone(), z.clone())))
        .collect();
    let zmq_rx = zmq_sub::spawn_zmq_subscribers(zmq_endpoints);
    info!("ZMQ subscribers started for {} coins", config.coins.len());

    // 12. Start job creation loop (ZMQ-triggered + fallback polling)
    {
        let jm = Arc::clone(&job_manager);
        let bc = Arc::clone(&broadcaster);
        let rpc_clone = Arc::clone(&rpc_clients);
        let cfg_clone = Arc::clone(&config);
        let db_clone = database.clone();
        let ss = Arc::clone(&stratum_server);

        tokio::spawn(async move {
            run_job_loop(jm, bc, rpc_clone, cfg_clone, db_clone, ss, zmq_rx).await;
        });
        info!("Job creation loop started (ZMQ + 10s fallback polling)");
    }

    // 13. Start HTTP API + static file server
    let http_db = database.clone();
    let http_config = Arc::clone(&config);
    let http_rpc = Arc::clone(&rpc_clients);

    let api_port = cfg.pool.api_port;
    info!("Starting HTTP server on 0.0.0.0:{}", api_port);

    HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);

        let app_state = web::Data::new(AppState {
            db: http_db.clone(),
            config: Arc::clone(&http_config),
            rpc_clients: Arc::clone(&http_rpc),
        });

        App::new()
            .wrap(cors)
            .app_data(app_state)
            .configure(api::configure_routes)
            .service(fs::Files::new("/", "./web").index_file("index.html"))
    })
    .bind(format!("0.0.0.0:{}", api_port))?
    .workers(4)
    .run()
    .await?;

    Ok(())
}

/// Main job creation loop.
/// Polls getblocktemplate and createauxblock to create new stratum jobs.
async fn run_job_loop(
    job_manager: Arc<JobManager>,
    broadcaster: Arc<NotificationBroadcaster>,
    rpc_clients: Arc<HashMap<String, rpc::RpcClient>>,
    config: Arc<config::Config>,
    db: db::Db,
    stratum_server: Arc<StratumServer>,
    mut zmq_rx: tokio::sync::mpsc::Receiver<String>,
) {
    let mut last_prevhash = String::new();
    let mut poll_interval = tokio::time::interval(tokio::time::Duration::from_secs(10));

    loop {
        // Wait for either ZMQ notification (instant) or poll timeout (10s fallback)
        tokio::select! {
            _ = poll_interval.tick() => {
                log::debug!("Job loop: fallback poll tick");
            }
            Some(coin) = zmq_rx.recv() => {
                log::info!("Job loop: ZMQ trigger from {}", coin);
                // Drain any queued notifications to avoid redundant refreshes
                while zmq_rx.try_recv().is_ok() {}
            }
        }

        // 1. Get parent block template
        let parent_rpc = match rpc_clients.get(&config.parent_coin().symbol) {
            Some(r) => r,
            None => continue,
        };

        let template = match parent_rpc.get_block_template().await {
            Ok(t) => t,
            Err(e) => {
                warn!("getblocktemplate failed: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        // Detect new block (prevhash changed)
        let is_new_block = template.previousblockhash != last_prevhash;
        if is_new_block {
            info!(
                "New block detected! height={} prevhash={}...",
                template.height,
                &template.previousblockhash[..16]
            );
            last_prevhash = template.previousblockhash.clone();
        }

        // 2. Get aux blocks from all aux chains
        let mut aux_blocks: Vec<(u32, [u8; 32])> = Vec::new();
        let mut aux_display_hashes: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
        let mut chain_id_to_symbol: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
        let mut aux_targets: std::collections::HashMap<u32, [u8; 32]> = std::collections::HashMap::new();
                    let mut aux_heights: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        for aux_coin in config.aux_coins() {
            let aux_rpc = match rpc_clients.get(&aux_coin.symbol) {
                Some(r) => r,
                None => continue,
            };

            match aux_rpc.create_aux_block(&aux_coin.reward_address.clone().unwrap_or(config.pool.pool_address.clone())).await {
                Ok(aux_block) => {
                    let hash_bytes = crypto::encoding::hex_to_bytes(&aux_block.hash);
                    if hash_bytes.len() == 32 {
                        // createauxblock returns hash in display order (big-endian).
                        // The AuxPoW merkle tree uses internal byte order (little-endian).
                        // Reverse the bytes from display to internal order.
                        let reversed = crypto::encoding::reverse_bytes(&hash_bytes);
                        let mut hash_internal = [0u8; 32];
                        hash_internal.copy_from_slice(&reversed);

                        aux_blocks.push((aux_block.chainid, hash_internal));
                        // Store the display-order hash for submitauxblock
                        aux_display_hashes.insert(aux_block.chainid, aux_block.hash.clone());
                        chain_id_to_symbol.insert(aux_block.chainid, aux_coin.symbol.clone());

                        // Extract the aux chain target for per-chain difficulty checking.
                        // createauxblock returns target in "reversed byte order" (LE hex).
                        // We need BE (big-endian) for hash_le_target comparison.
                        // The target field may be "target" or "_target" depending on daemon.
                        let target_be: [u8; 32] = if let Some(target_hex) = aux_block.get_target() {
                            // Target from createauxblock is in LE hex (reversed byte order).
                            // Reverse to BE for comparison with scrypt hash (which is BE).
                            let target_bytes = crypto::encoding::hex_to_bytes(target_hex);
                            if target_bytes.len() == 32 {
                                let reversed_target = crypto::encoding::reverse_bytes(&target_bytes);
                                let mut t = [0u8; 32];
                                t.copy_from_slice(&reversed_target);
                                t
                            } else {
                                // Fallback: compute from bits
                                crypto::encoding::bits_to_target(&aux_block.bits)
                            }
                        } else {
                            // No target field, compute from bits
                            crypto::encoding::bits_to_target(&aux_block.bits)
                        };
                        aux_targets.insert(aux_block.chainid, target_be);
                        aux_heights.insert(aux_block.chainid, aux_block.height);

                        info!(
                            "[{}] createauxblock: chainid={} hash={} height={} target={}...",
                            aux_coin.symbol, aux_block.chainid, &aux_block.hash, aux_block.height,
                            &crypto::encoding::bytes_to_hex(&target_be)[..16]
                        );
                    }
                }
                Err(e) => {
                    log::debug!("[{}] createauxblock failed: {}", aux_coin.symbol, e);
                }
            }
        }

        // 3. Create stratum job
        let mut job = job_manager.create_job(&template, &aux_blocks, is_new_block);
        job.aux_display_hashes = aux_display_hashes;
        job.chain_id_to_symbol = chain_id_to_symbol;
        job.aux_targets = aux_targets;
                    job.aux_heights = aux_heights;
        // Update the job in the cache with the display hashes, symbol map, and targets
        job_manager.update_job_metadata(&job);

        // 4. Broadcast to all miners
        let notify = mining_notify(
            &job.job_id,
            &job.prevhash,
            &job.coinbase.coinbase1,
            &job.coinbase.coinbase2,
            &job.merkle_branches,
            &job.version,
            &job.nbits,
            &job.ntime,
            is_new_block,
        );
        broadcaster.broadcast_job(notify, is_new_block);

        // [Fix #8] Faster polling: no additional sleep — the 1-second interval tick handles pacing.
        // Previously slept 9 extra seconds when no new block was detected (total 10s cycle).
        // Now polls every 1 second consistently for faster block detection.
    }
}
