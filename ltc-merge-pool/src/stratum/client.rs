/// Per-client state for a stratum connection.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;
use std::time::Instant;
use parking_lot::Mutex;

/// Unique client identifier
pub type ClientId = u64;

/// A connected stratum client.
pub struct StratumClient {
    /// Unique ID for this client
    pub id: ClientId,
    /// Remote address
    pub addr: SocketAddr,
    /// Assigned extranonce1 (4 bytes, hex-encoded = 8 chars)
    pub extranonce1: String,
    /// Current difficulty (stored as f64 bits in AtomicU64 for lock-free sharing)
    difficulty_bits: AtomicU64,
    /// Miner address (set on authorize)
    pub miner_address: Mutex<Option<String>>,
    /// Worker name (set on authorize)
    pub worker_name: Mutex<Option<String>>,
    /// Whether the client has sent mining.subscribe
    pub subscribed: AtomicBool,
    /// Whether the client has authorized
    pub authorized: AtomicBool,
    /// User agent string
    pub user_agent: Mutex<String>,
    /// Channel to send messages to the client's write task
    pub write_tx: mpsc::Sender<String>,
    /// Last activity time (for timeout)
    pub last_activity: Mutex<Instant>,
    /// Vardiff state
    pub vardiff: Mutex<super::vardiff::VardiffState>,
    /// Port difficulty (initial)
    pub port_difficulty: f64,
    /// Whether vardiff is enabled for this port
    pub vardiff_enabled: bool,
}

impl StratumClient {
    /// Create a new client with the given parameters.
    pub fn new(
        id: ClientId,
        addr: SocketAddr,
        extranonce1: String,
        difficulty: f64,
        vardiff_enabled: bool,
        write_tx: mpsc::Sender<String>,
    ) -> Self {
        Self {
            id,
            addr,
            extranonce1,
            difficulty_bits: AtomicU64::new(difficulty.to_bits()),
            miner_address: Mutex::new(None),
            worker_name: Mutex::new(None),
            subscribed: AtomicBool::new(false),
            authorized: AtomicBool::new(false),
            user_agent: Mutex::new(String::new()),
            write_tx,
            last_activity: Mutex::new(Instant::now()),
            vardiff: Mutex::new(super::vardiff::VardiffState::new(difficulty)),
            port_difficulty: difficulty,
            vardiff_enabled,
        }
    }

    /// Get the current difficulty (atomically).
    pub fn get_difficulty(&self) -> f64 {
        f64::from_bits(self.difficulty_bits.load(Ordering::Relaxed))
    }

    /// Set the current difficulty (atomically).
    pub fn set_difficulty(&self, diff: f64) {
        self.difficulty_bits.store(diff.to_bits(), Ordering::Relaxed);
    }

    /// Check if this client is subscribed.
    pub fn is_subscribed(&self) -> bool {
        self.subscribed.load(Ordering::Relaxed)
    }

    /// Check if this client is authorized.
    pub fn is_authorized(&self) -> bool {
        self.authorized.load(Ordering::Relaxed)
    }

    /// Mark the client as subscribed.
    pub fn set_subscribed(&self) {
        self.subscribed.store(true, Ordering::Relaxed);
    }

    /// Mark the client as authorized with the given miner/worker.
    pub fn set_authorized(&self, miner: &str, worker: &str) {
        *self.miner_address.lock() = Some(miner.to_string());
        *self.worker_name.lock() = Some(worker.to_string());
        self.authorized.store(true, Ordering::Relaxed);
    }

    /// Get the miner address (if authorized).
    pub fn get_miner(&self) -> Option<String> {
        self.miner_address.lock().clone()
    }

    /// Get the worker name (if authorized).
    pub fn get_worker(&self) -> Option<String> {
        self.worker_name.lock().clone()
    }

    /// Update last activity timestamp.
    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Check if the client has timed out.
    pub fn is_timed_out(&self, timeout_secs: u64) -> bool {
        self.last_activity.lock().elapsed().as_secs() > timeout_secs
    }

    /// Send a JSON message to this client (non-blocking).
    pub async fn send(&self, msg: &str) -> bool {
        self.write_tx.try_send(msg.to_string()).is_ok()
    }

    /// Set the user agent.
    pub fn set_user_agent(&self, ua: &str) {
        *self.user_agent.lock() = ua.to_string();
    }

    /// Get a display string for logging.
    pub fn display(&self) -> String {
        let miner = self.get_miner().unwrap_or_else(|| "?".to_string());
        let worker = self.get_worker().unwrap_or_else(|| "?".to_string());
        format!(
            "client#{} [{}] {}.{} diff={}",
            self.id, self.addr, miner, worker, self.get_difficulty()
        )
    }
}
