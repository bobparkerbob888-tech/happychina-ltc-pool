/// Stratum TCP server — listens on multiple ports, handles client connections.
/// One tokio task per connection with panic recovery.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use log::{debug, error, info, warn};
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{timeout, Duration};

use crate::config::{Config, StratumPort};
use crate::rpc::RpcClient;
use super::client::{ClientId, StratumClient};
use super::job::JobManager;
use super::notifications::{NotificationBroadcaster, PoolNotification};
use super::protocol::{
    mining_notify, parse_miner_worker, parse_request, response_error, response_ok,
    set_difficulty_notification, StratumRequest,
};
use super::vardiff::VardiffConfig;

/// Per-IP connection counter for rate limiting
type IpConnectionMap = Arc<DashMap<std::net::IpAddr, u32>>;

/// Key for duplicate share detection: (job_id, extranonce2, ntime, nonce)
type ShareKey = (String, String, String, String);

/// The stratum server state shared across all connections.
pub struct StratumServer {
    /// Global client ID counter
    client_id_counter: AtomicU64,
    /// Global extranonce1 counter (wraps at u32::MAX)
    extranonce1_counter: AtomicU32,
    /// Job manager (shared)
    pub job_manager: Arc<JobManager>,
    /// Notification broadcaster
    pub broadcaster: Arc<NotificationBroadcaster>,
    /// Connected clients indexed by ID
    pub clients: Arc<DashMap<ClientId, Arc<StratumClient>>>,
    /// Database connection
    pub db: Option<Arc<crate::db::Db>>,
    /// RPC clients for block submission
    pub rpc_clients: Option<Arc<std::collections::HashMap<String, RpcClient>>>,
    /// Pool config for block submission
    pub config: Option<Arc<Config>>,
    /// Share batcher for batch DB inserts
    pub share_batcher: Option<Arc<crate::db::ShareBatcher>>,
    /// Per-IP connection count (for limiting)
    ip_connections: IpConnectionMap,
    /// Maximum connections per IP
    max_connections_per_ip: u32,
    /// Connection timeout in seconds
    connection_timeout_secs: u64,
    /// Vardiff configuration
    pub vardiff_config: VardiffConfig,
    /// Duplicate share detection set with timestamps for eviction
    recent_shares: Mutex<HashSet<ShareKey>>,
    /// Timestamp of last duplicate share eviction
    last_share_eviction: Mutex<std::time::Instant>,
    /// Per-client bad share counter (client_id -> consecutive bad count)
    bad_share_counters: DashMap<ClientId, u32>,
}

impl StratumServer {
    /// Create a new stratum server.
    pub fn new(
        job_manager: Arc<JobManager>,
        broadcaster: Arc<NotificationBroadcaster>,
        max_connections_per_ip: u32,
        connection_timeout_secs: u64,
        vardiff_config: VardiffConfig,
        db: Option<Arc<crate::db::Db>>,
        rpc_clients: Option<Arc<std::collections::HashMap<String, RpcClient>>>,
        config: Option<Arc<Config>>,
    ) -> Arc<Self> {
        let share_batcher = db.as_ref().map(|d| crate::db::ShareBatcher::new(std::sync::Arc::clone(d)));
        Arc::new(Self {
            client_id_counter: AtomicU64::new(1),
            extranonce1_counter: AtomicU32::new(1),
            job_manager,
            broadcaster,
            clients: Arc::new(DashMap::new()),
            ip_connections: Arc::new(DashMap::new()),
            max_connections_per_ip,
            connection_timeout_secs,
            vardiff_config,
            db,
            rpc_clients,
            config,
            recent_shares: Mutex::new(HashSet::new()),
            last_share_eviction: Mutex::new(std::time::Instant::now()),
            bad_share_counters: DashMap::new(),
            share_batcher,
        })
    }

    /// Start listening on all configured ports.

    pub async fn start(
        self: &Arc<Self>,
        listen_addr: &str,
        ports: &[StratumPort],
    ) -> Result<(), Box<dyn std::error::Error>> {
        for port_config in ports {
            let addr = format!("{}:{}", listen_addr, port_config.port);
            let listener = TcpListener::bind(&addr).await?;
            info!(
                "Stratum listening on {} — {} (diff={}, vardiff={})",
                addr, port_config.name, port_config.difficulty, port_config.vardiff
            );

            let server = Arc::clone(self);
            let difficulty = port_config.difficulty;
            let vardiff_enabled = port_config.vardiff;

            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, addr)) => {
                            let server = Arc::clone(&server);
                            tokio::spawn(async move {
                                server.handle_connection(stream, addr, difficulty, vardiff_enabled).await;
                            });
                        }
                        Err(e) => {
                            error!("Accept error: {}", e);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            });
        }
        Ok(())
    }

    /// Evict old entries from the duplicate share set (older than 5 minutes).
    /// Since we don't store timestamps per-entry, we do a full clear every 5 minutes.
    fn maybe_evict_shares(&self) {
        let mut last = self.last_share_eviction.lock();
        if last.elapsed() > std::time::Duration::from_secs(300) {
            let mut shares = self.recent_shares.lock();
            shares.clear();
            *last = std::time::Instant::now();
            debug!("Evicted duplicate share detection set");
        }
    }

    /// Check if a share is a duplicate. Returns true if duplicate (already seen).
    fn is_duplicate_share(&self, job_id: &str, extranonce2: &str, ntime: &str, nonce: &str) -> bool {
        self.maybe_evict_shares();
        let key: ShareKey = (
            job_id.to_string(),
            extranonce2.to_lowercase(),
            ntime.to_lowercase(),
            nonce.to_lowercase(),
        );
        let mut shares = self.recent_shares.lock();
        !shares.insert(key) // insert returns false if already present
    }

    /// Check if a string contains only valid hex characters.
    fn is_valid_hex(s: &str) -> bool {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Handle a single client connection.
    async fn handle_connection(
        self: &Arc<Self>,
        stream: TcpStream,
        addr: SocketAddr,
        difficulty: f64,
        vardiff_enabled: bool,
    ) {
        let ip = addr.ip();

        // Per-IP rate limiting
        {
            let mut count = self.ip_connections.entry(ip).or_insert(0);
            if *count >= self.max_connections_per_ip {
                warn!("Per-IP limit reached for {} ({} connections)", ip, *count);
                return;
            }
            *count += 1;
        }

        // Assign client ID and extranonce1
        let client_id = self.client_id_counter.fetch_add(1, Ordering::Relaxed);
        let en1_val = self.extranonce1_counter.fetch_add(1, Ordering::Relaxed);
        let extranonce1 = format!("{:08x}", en1_val);

        // Create write channel
        let (write_tx, mut write_rx) = mpsc::channel::<String>(64);

        // Create the client
        let client = Arc::new(StratumClient::new(
            client_id,
            addr,
            extranonce1.clone(),
            difficulty,
            vardiff_enabled,
            write_tx,
        ));

        // Register client
        self.clients.insert(client_id, Arc::clone(&client));

        info!("Client connected: {} (id={}, en1={})", addr, client_id, extranonce1);

        // Split TCP stream
        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);

        // Subscribe to broadcast notifications
        let mut broadcast_rx = self.broadcaster.subscribe();

        // Writer task: reads from write_rx and broadcast_rx, writes to TCP
        let write_client = Arc::clone(&client);
        let write_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Messages from the handler (responses, per-client notifications)
                    msg = write_rx.recv() => {
                        match msg {
                            Some(data) => {
                                let line = if data.ends_with("\n") {
                                    data
                                } else {
                                    format!("{}\n", data)
                                };
                                if writer.write_all(line.as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            None => break, // Channel closed
                        }
                    }
                    // Broadcast notifications (new jobs)
                    notification = broadcast_rx.recv() => {
                        match notification {
                            Ok(PoolNotification::NewJob { notify_json, .. }) => {
                                if write_client.is_subscribed() && write_client.is_authorized() {
                                    let line = format!("{}\n", notify_json);
                                    if writer.write_all(line.as_bytes()).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Ok(PoolNotification::Shutdown) => {
                                break;
                            }
                            Ok(PoolNotification::SetDifficulty { client_id: cid, difficulty: diff }) => {
                                if cid == write_client.id {
                                    let msg = set_difficulty_notification(diff);
                                    let line = format!("{}\n", msg);
                                    let _ = writer.write_all(line.as_bytes()).await;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Client {} lagged {} broadcast messages", write_client.id, n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Reader loop: read JSON lines from client
        let timeout_duration = Duration::from_secs(self.connection_timeout_secs);
        let mut line_buf = String::new();
        let mut should_disconnect = false;

        loop {
            line_buf.clear();
            let read_result = timeout(timeout_duration, buf_reader.read_line(&mut line_buf)).await;

            match read_result {
                Ok(Ok(0)) => {
                    // EOF
                    debug!("Client {} disconnected (EOF)", client_id);
                    break;
                }
                Ok(Ok(_n)) => {
                    let line = line_buf.trim();
                    if line.is_empty() {
                        continue;
                    }
                    client.touch();

                    // Parse and handle the request
                    if let Some(req) = parse_request(line) {
                        should_disconnect = self.handle_request(&client, req).await;
                        if should_disconnect {
                            warn!("Disconnecting client {} due to excessive bad shares", client_id);
                            break;
                        }
                    } else {
                        debug!("Client {} sent unparseable line: {}", client_id, line);
                    }
                }
                Ok(Err(e)) => {
                    debug!("Client {} read error: {}", client_id, e);
                    break;
                }
                Err(_) => {
                    debug!("Client {} timed out ({}s)", client_id, self.connection_timeout_secs);
                    break;
                }
            }
        }

        // Cleanup
        self.clients.remove(&client_id);
        self.bad_share_counters.remove(&client_id);
        {
            if let Some(mut count) = self.ip_connections.get_mut(&ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    drop(count);
                    self.ip_connections.remove(&ip);
                }
            }
        }

        // Drop the writer task
        write_handle.abort();

        info!("Client disconnected: {} (id={})", addr, client_id);
    }

    /// Handle a parsed stratum request. Returns true if client should be disconnected.
    async fn handle_request(self: &Arc<Self>, client: &Arc<StratumClient>, req: StratumRequest) -> bool {
        match req.method.as_str() {
            "mining.subscribe" => {
                self.handle_subscribe(client, &req).await;
                false
            }
            "mining.authorize" => {
                self.handle_authorize(client, &req).await;
                false
            }
            "login" => {
                // MRR/NiceHash compat — treat as subscribe + authorize
                self.handle_login(client, &req).await;
                false
            }
            "mining.submit" => {
                self.handle_submit(client, &req).await
            }
            "mining.configure" => {
                self.handle_configure(client, &req).await;
                false
            }
            "mining.extranonce.subscribe" => {
                self.handle_extranonce_subscribe(client, &req).await;
                false
            }
            "mining.multi_version" => {
                self.handle_multi_version(client, &req).await;
                false
            }
            "mining.suggest_difficulty" => {
                self.handle_suggest_difficulty(client, &req).await;
                false
            }
            _ => {
                // Unknown method — log and ignore, NEVER disconnect
                debug!(
                    "Client {} sent unknown method: {}",
                    client.id, req.method
                );
                // Send a generic OK response if it has an id
                if !req.id.is_null() {
                    let resp = response_ok(&req.id, serde_json::Value::Bool(true));
                    let _ = client.send(&resp).await;
                }
                false
            }
        }
    }

    /// Handle mining.subscribe
    async fn handle_subscribe(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        // Extract user agent if provided
        if let Some(ua) = req.params.first().and_then(|v| v.as_str()) {
            client.set_user_agent(ua);
        }

        client.set_subscribed();

        // Response: [[["mining.set_difficulty", "sub_id"], ["mining.notify", "sub_id"]], extranonce1, extranonce2_size]
        let sub_id = format!("{:x}", client.id);
        let result = serde_json::json!([
            [
                ["mining.set_difficulty", &sub_id],
                ["mining.notify", &sub_id],
            ],
            &client.extranonce1,
            self.job_manager.extranonce2_size,
        ]);

        let resp = response_ok(&req.id, result);
        let _ = client.send(&resp).await;

        // Send initial difficulty
        let diff_msg = set_difficulty_notification(client.get_difficulty());
        let _ = client.send(&diff_msg).await;

        // Send current job if available
        if let Some(job) = self.job_manager.current_job() {
            let notify = mining_notify(
                &job.job_id,
                &job.prevhash,
                &job.coinbase.coinbase1,
                &job.coinbase.coinbase2,
                &job.merkle_branches,
                &job.version,
                &job.nbits,
                &job.ntime,
                true,
            );
            let _ = client.send(&notify).await;
        }
    }

    /// Handle mining.authorize
    async fn handle_authorize(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        let user = req
            .params
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let _pass = req
            .params
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("x");

        if user.is_empty() {
            let resp = response_error(&req.id, 24, "Missing username");
            let _ = client.send(&resp).await;
            return;
        }

        let (miner, worker) = parse_miner_worker(user);

        // Basic address validation (length check — actual RPC validation can be done async)
        if miner.len() < 20 || miner.len() > 128 {
            let resp = response_error(&req.id, 24, "Invalid address format");
            let _ = client.send(&resp).await;
            return;
        }

        client.set_authorized(&miner, &worker);
        info!(
            "Client {} authorized: {}.{} (en1={})",
            client.id, miner, worker, client.extranonce1
        );

        let resp = response_ok(&req.id, serde_json::Value::Bool(true));
        let _ = client.send(&resp).await;
    }

    /// Handle "login" (MRR/NiceHash object-params compat)
    async fn handle_login(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        // First, do subscribe implicitly
        if !client.is_subscribed() {
            client.set_subscribed();

            let sub_id = format!("{:x}", client.id);
            // Send subscribe result
            let sub_result = serde_json::json!([
                [
                    ["mining.set_difficulty", &sub_id],
                    ["mining.notify", &sub_id],
                ],
                &client.extranonce1,
                self.job_manager.extranonce2_size,
            ]);
            let sub_resp = serde_json::json!({
                "id": req.id,
                "result": sub_result,
                "error": null,
            })
            .to_string();
            let _ = client.send(&sub_resp).await;
        }

        // Then authorize
        let user = req
            .params
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if user.is_empty() {
            let resp = response_error(&req.id, 24, "Missing login");
            let _ = client.send(&resp).await;
            return;
        }

        let (miner, worker) = parse_miner_worker(user);

        if miner.len() < 20 || miner.len() > 128 {
            let resp = response_error(&req.id, 24, "Invalid address format");
            let _ = client.send(&resp).await;
            return;
        }

        client.set_authorized(&miner, &worker);
        info!(
            "Client {} login: {}.{} (en1={})",
            client.id, miner, worker, client.extranonce1
        );

        // Send difficulty + job
        let diff_msg = set_difficulty_notification(client.get_difficulty());
        let _ = client.send(&diff_msg).await;

        if let Some(job) = self.job_manager.current_job() {
            let notify = mining_notify(
                &job.job_id,
                &job.prevhash,
                &job.coinbase.coinbase1,
                &job.coinbase.coinbase2,
                &job.merkle_branches,
                &job.version,
                &job.nbits,
                &job.ntime,
                true,
            );
            let _ = client.send(&notify).await;
        }
    }

    /// Handle mining.submit. Returns true if client should be disconnected.
    async fn handle_submit(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) -> bool {
        if !client.is_authorized() {
            let resp = response_error(&req.id, 24, "Not authorized");
            let _ = client.send(&resp).await;
            return false;
        }

        // Params: [worker_name, job_id, extranonce2, ntime, nonce]
        let _worker_name = req.params.first().and_then(|v| v.as_str()).unwrap_or("");
        let job_id = req.params.get(1).and_then(|v| v.as_str()).unwrap_or("");
        let extranonce2 = req.params.get(2).and_then(|v| v.as_str()).unwrap_or("");
        let ntime = req.params.get(3).and_then(|v| v.as_str()).unwrap_or("");
        let nonce = req.params.get(4).and_then(|v| v.as_str()).unwrap_or("");

        if job_id.is_empty() || extranonce2.is_empty() || ntime.is_empty() || nonce.is_empty() {
            let resp = response_error(&req.id, 23, "Missing submit parameters");
            let _ = client.send(&resp).await;
            return false;
        }

        // [Fix #10] Hex character validation
        if !Self::is_valid_hex(nonce) || !Self::is_valid_hex(ntime) || !Self::is_valid_hex(extranonce2) {
            let resp = response_error(&req.id, 23, "Invalid hex in submit parameters");
            let _ = client.send(&resp).await;
            debug!("Client {} sent non-hex submit params: nonce={} ntime={} en2={}", client.id, nonce, ntime, extranonce2);
            return self.increment_bad_shares(client.id);
        }

        // [Fix #1] Duplicate share detection
        if self.is_duplicate_share(job_id, extranonce2, ntime, nonce) {
            let resp = response_error(&req.id, 22, "Duplicate share");
            let _ = client.send(&resp).await;
            debug!("Duplicate share from client {}: job={} nonce={}", client.id, job_id, nonce);
            return self.increment_bad_shares(client.id);
        }

        // Read the current difficulty BEFORE validation (vardiff-aware)
        let current_difficulty = client.get_difficulty();

        // Validate the share
        match self.job_manager.validate_share(
            job_id,
            &client.extranonce1,
            extranonce2,
            ntime,
            nonce,
            current_difficulty,
        ) {
            Ok(result) => {
                if result.is_valid_share {
                    let resp = response_ok(&req.id, serde_json::Value::Bool(true));
                    let _ = client.send(&resp).await;

                    // [Fix #6] Reset bad share counter on valid share
                    self.bad_share_counters.insert(client.id, 0);

                    let miner = client.get_miner().unwrap_or_default();
                    let worker = client.get_worker().unwrap_or_default();

                    debug!(
                        "Valid share from {}.{}: diff={:.4} hash={}...",
                        miner,
                        worker,
                        result.share_difficulty,
                        &result.hash_hex[..16],
                    );

                    // Record share in database — use current_difficulty (vardiff-aware)
                    if let Some(ref db) = self.db {
                        let share = crate::db::ShareInsert {
                            miner: miner.clone(),
                            worker: worker.clone(),
                            difficulty: current_difficulty,
                            share_difficulty: result.share_difficulty,
                            ip_address: String::new(),
                            user_agent: String::new(),
                            is_valid: true,
                        };
                        if let Some(ref batcher) = self.share_batcher { batcher.submit(share).await; } else if let Err(e) = db.insert_share(&share).await { log::warn!("Failed to insert share: {}", e);
                        }
                    }
                    // Vardiff: record share and check for retarget
                    if client.vardiff_enabled {
                        let new_diff = {
                            let mut vd = client.vardiff.lock();
                            vd.on_share(&self.vardiff_config)
                        };
                        if let Some(diff) = new_diff {
                            info!(
                                "Vardiff: client {} retarget {:.4} -> {:.4}",
                                client.id, current_difficulty, diff
                            );
                            // Update the client difficulty atomically
                            client.set_difficulty(diff);
                            // Send the new difficulty notification via the write channel.
                            let diff_msg = set_difficulty_notification(diff);
                            let _ = client.send(&diff_msg).await;
                        }
                    }

                    // Check if this share meets any chain's network difficulty.
                    // Parent chain (LTC): only submit if is_block (scrypt hash <= LTC target)
                    // Aux chains: compare scrypt hash against each chain's target from createauxblock
                    if let Some(job) = self.job_manager.get_job(job_id) {
                        if let (Some(ref rpc_clients), Some(ref config)) = (&self.rpc_clients, &self.config) {
                            let parent_symbol = config.parent_coin().symbol.clone();

                            // Parent chain: only submit when hash meets LTC network target
                            if result.is_block {
                                info!(
                                    "*** PARENT BLOCK FOUND by {}.{} ! hash={} diff={:.4} ***",
                                    miner, worker, result.hash_hex, result.share_difficulty,
                                );
                                if let Some(parent_rpc) = rpc_clients.get(&parent_symbol) {
                                    let block_hex = self.job_manager.assemble_block(&job, &result.header_bytes, &result.coinbase_tx);
                                    info!("Submitting parent block ({}) at height {}...", parent_symbol, job.height);
                                    match parent_rpc.submit_block(&block_hex).await {
                                        Ok(()) => {
                                            info!("*** PARENT BLOCK {} ACCEPTED at height {} ***", parent_symbol, job.height);
                                            if let Some(ref db) = self.db {
                                                let bi = crate::db::BlockInsert {
                                                    coin: parent_symbol.clone(),
                                                    height: job.height as i64,
                                                    hash: result.hash_hex.clone(),
                                                    block_hash: result.block_hash_hex.clone(),
                                                    miner: miner.clone(),
                                                    worker: worker.clone(),
                                                    reward: config.parent_coin().block_reward,
                                                    difficulty: result.share_difficulty,
                                                    net_difficulty: 0.0,
                                                };
                                                match db.insert_block(&bi).await {
                                                    Ok(id) => info!("Block recorded id={}", id),
                                                    Err(e) => error!("DB block insert err: {}", e),
                                                }
                                            }
                                        }
                                        Err(e) => error!("PARENT BLOCK SUBMIT FAILED h={}: {}", job.height, e),
                                    }
                                }
                            }

                            // Aux chains: check each chain's target independently.
                            // The scrypt hash (big-endian) must be <= the aux chain's target.
                            // Convert share hash_hex back to bytes for comparison.
                            let hash_be_bytes = crate::crypto::encoding::hex_to_bytes(&result.hash_hex);
                            if hash_be_bytes.len() == 32 {
                                let mut hash_arr = [0u8; 32];
                                hash_arr.copy_from_slice(&hash_be_bytes);

                                // Debug: log aux target check details for high-diff shares
                                if result.share_difficulty > 1_000_000.0 {
                                    debug!(
                                        "Aux target check: job={} aux_blocks={} aux_targets={} share_diff={:.0} hash={}...",
                                        job_id, job.aux_blocks.len(), job.aux_targets.len(),
                                        result.share_difficulty, &result.hash_hex[..16]
                                    );
                                }

                                for &(chain_id, ref _aux_hash) in &job.aux_blocks {
                                    // Check if this share meets the aux chain's difficulty
                                    let meets_aux_target = match job.aux_targets.get(&chain_id) {
                                        Some(aux_target) => {
                                            let meets = crate::crypto::encoding::hash_le_target(&hash_arr, aux_target);
                                            if result.share_difficulty > 1_000_000.0 {
                                                let sym = job.chain_id_to_symbol.get(&chain_id).map(|s| s.as_str()).unwrap_or("?");
                                                debug!(
                                                    "  chain_id={} ({}) hash={}.. target={}.. meets={}",
                                                    chain_id, sym,
                                                    &result.hash_hex[..16],
                                                    &crate::crypto::encoding::bytes_to_hex(aux_target)[..16],
                                                    meets
                                                );
                                            }
                                            meets
                                        }
                                        None => {
                                            // No target stored - skip (shouldn't happen)
                                            warn!("No aux target for chain_id={}, skipping (aux_targets has {} entries)", chain_id, job.aux_targets.len());
                                            false
                                        }
                                    };

                                    if !meets_aux_target {
                                        continue;
                                    }

                                    // Look up which coin this chain_id belongs to
                                    let coin_symbol = match job.chain_id_to_symbol.get(&chain_id) {
                                        Some(s) => s.clone(),
                                        None => {
                                            warn!("No coin symbol for chain_id={}, skipping", chain_id);
                                            continue;
                                        }
                                    };
                                    // Get the display-order hash for submitauxblock
                                    let aux_hash_hex = match job.aux_display_hashes.get(&chain_id) {
                                        Some(h) => h.clone(),
                                        None => {
                                            warn!("No display hash for chain_id={} ({}), skipping", chain_id, coin_symbol);
                                            continue;
                                        }
                                    };
                                    let aux_coin_config = match config.coin_by_symbol(&coin_symbol) {
                                        Some(c) => c,
                                        None => {
                                            warn!("No config for coin {}, skipping", coin_symbol);
                                            continue;
                                        }
                                    };

                                    info!(
                                        "*** AUX BLOCK {} FOUND by {}.{} ! chain_id={} diff={:.4} ***",
                                        coin_symbol, miner, worker, chain_id, result.share_difficulty,
                                    );

                                    if let Some(aux_rpc) = rpc_clients.get(&coin_symbol) {
                                        let auxpow_hex = self.job_manager.build_aux_proof(
                                            &job, &result.header_bytes, &result.coinbase_tx, chain_id,
                                        );
                                        info!(
                                            "Submitting aux block {} chain_id={} hash={}...",
                                            coin_symbol, chain_id, &aux_hash_hex[..16]
                                        );
                                        debug!(
                                            "  auxpow len={} first80={}...",
                                            auxpow_hex.len(),
                                            &auxpow_hex[..std::cmp::min(80, auxpow_hex.len())]
                                        );
                                        match aux_rpc.submit_aux_block(&aux_hash_hex,
                                            &auxpow_hex).await {
                                            Ok(()) => {
                                                info!("*** AUX BLOCK {} ACCEPTED ***", coin_symbol);
                                                if let Some(ref db) = self.db {
                                                    let bi = crate::db::BlockInsert {
                                                        coin: coin_symbol.clone(),
                                                        height: job.aux_heights.get(&chain_id).copied().unwrap_or(0) as i64,
                                                        hash: aux_hash_hex.clone(),
                                                        block_hash: String::new(),
                                                        miner: miner.clone(),
                                                        worker: worker.clone(),
                                                        reward: aux_coin_config.block_reward,
                                                        difficulty: result.share_difficulty,
                                                        net_difficulty: 0.0,
                                                    };
                                                    match db.insert_block(&bi).await {
                                                        Ok(id) => info!("Aux {} recorded id={}", coin_symbol, id),
                                                        Err(e) => error!("DB aux insert {} err: {}", coin_symbol, e),
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error!(
                                                    "[{}] submitauxblock FAILED chain_id={} hash={}: {}",
                                                    coin_symbol, chain_id, &aux_hash_hex[..16], e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Invalid share (above difficulty target)
                    let resp = response_error(&req.id, 23, "Share above target");
                    let _ = client.send(&resp).await;
                    debug!(
                        "Rejected share from client {}: diff={:.4} (needed {:.4})",
                        client.id, result.share_difficulty, current_difficulty,
                    );
                    // [Fix #6] Increment bad share counter
                    return self.increment_bad_shares(client.id);
                }
            }
            Err(ref e) if e.starts_with("Job not found:") => {
                // [Fix #3] Stale share auto-accept: job expired from cache
                // Return success to prevent hash rental penalties, but don't record
                let resp = response_ok(&req.id, serde_json::Value::Bool(true));
                let _ = client.send(&resp).await;
                debug!("Stale share accepted (job expired) from client {}: {}", client.id, e);
            }
            Err(e) => {
                let resp = response_error(&req.id, 23, &e);
                let _ = client.send(&resp).await;
                warn!("Share validation error from client {}: {}", client.id, e);
                // [Fix #6] Increment bad share counter
                return self.increment_bad_shares(client.id);
            }
        }
        false
    }

    /// [Fix #6] Increment bad share counter for a client. Returns true if client should be disconnected.
    fn increment_bad_shares(&self, client_id: ClientId) -> bool {
        let mut entry = self.bad_share_counters.entry(client_id).or_insert(0);
        *entry += 1;
        let count = *entry;
        if count >= 100 {
            warn!("Client {} exceeded 100 consecutive bad shares, will disconnect", client_id);
            true
        } else {
            false
        }
    }

    /// Handle mining.configure (version rolling, etc.)
    async fn handle_configure(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        // Acknowledge with empty result (we don't support version rolling yet)
        let resp = response_ok(&req.id, serde_json::json!({}));
        let _ = client.send(&resp).await;
    }

    /// Handle mining.extranonce.subscribe
    async fn handle_extranonce_subscribe(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        let resp = response_ok(&req.id, serde_json::Value::Bool(true));
        let _ = client.send(&resp).await;
    }

    /// [Fix #11] Handle mining.multi_version — respond false since we don't support version rolling
    async fn handle_multi_version(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        let resp = response_ok(&req.id, serde_json::Value::Bool(false));
        let _ = client.send(&resp).await;
    }

    /// [Fix #4] Handle mining.suggest_difficulty
    async fn handle_suggest_difficulty(
        self: &Arc<Self>,
        client: &Arc<StratumClient>,
        req: &StratumRequest,
    ) {
        let suggested = req.params.first()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        if suggested <= 0.0 {
            debug!("Client {} sent invalid suggest_difficulty: {:?}", client.id, req.params.first());
            let resp = response_ok(&req.id, serde_json::Value::Bool(true));
            let _ = client.send(&resp).await;
            return;
        }

        // Clamp to min/max from vardiff config
        let clamped = suggested
            .max(self.vardiff_config.min_difficulty)
            .min(self.vardiff_config.max_difficulty);

        let old_diff = client.get_difficulty();
        client.set_difficulty(clamped);

        info!(
            "Client {} suggest_difficulty: requested={:.4} set={:.4} (was {:.4})",
            client.id, suggested, clamped, old_diff
        );

        // Send mining.set_difficulty notification with the new difficulty
        let diff_msg = set_difficulty_notification(clamped);
        let _ = client.send(&diff_msg).await;

        // Respond OK
        let resp = response_ok(&req.id, serde_json::Value::Bool(true));
        let _ = client.send(&resp).await;
    }

    /// Get the number of connected clients.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}
