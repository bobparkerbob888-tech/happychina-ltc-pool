/// Broadcast notifications to all connected stratum clients.
/// Uses a tokio broadcast channel for fan-out.

use log::debug;
use tokio::sync::broadcast;

/// A notification message to broadcast to all clients.
#[derive(Debug, Clone)]
pub enum PoolNotification {
    /// New mining job — broadcast mining.notify to all clients
    NewJob {
        /// Pre-formatted mining.notify JSON
        notify_json: String,
        /// Whether this is a clean job (new block)
        clean_jobs: bool,
    },
    /// Set difficulty for a specific client (not broadcast)
    SetDifficulty {
        client_id: u64,
        difficulty: f64,
    },
    /// Shutdown all connections
    Shutdown,
}

/// The notification broadcaster.
pub struct NotificationBroadcaster {
    /// Send side of the broadcast channel
    tx: broadcast::Sender<PoolNotification>,
}

impl NotificationBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Get a new receiver (subscribe to notifications).
    pub fn subscribe(&self) -> broadcast::Receiver<PoolNotification> {
        self.tx.subscribe()
    }

    /// Broadcast a new job notification to all clients.
    pub fn broadcast_job(&self, notify_json: String, clean_jobs: bool) {
        let msg = PoolNotification::NewJob {
            notify_json,
            clean_jobs,
        };
        match self.tx.send(msg) {
            Ok(n) => {
                debug!("Broadcast job to {} receivers", n);
            }
            Err(_) => {
                debug!("No receivers for job broadcast");
            }
        }
    }

    /// Broadcast a shutdown notification.
    pub fn broadcast_shutdown(&self) {
        let _ = self.tx.send(PoolNotification::Shutdown);
    }

    /// Get the number of active receivers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Clone for NotificationBroadcaster {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}
