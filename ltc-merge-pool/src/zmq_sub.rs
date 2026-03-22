/// ZMQ subscriber for instant block notifications from coin daemons.
/// Subscribes to hashblock events and sends a signal to the job loop.

use log::{info, warn, error};
use tokio::sync::mpsc;

/// Spawns ZMQ subscribers for all configured coins.
/// Returns a receiver that gets a message whenever ANY coin has a new block.
pub fn spawn_zmq_subscribers(
    zmq_endpoints: Vec<(String, String)>,  // (symbol, zmq_url)
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>(64);

    for (symbol, endpoint) in zmq_endpoints {
        let tx = tx.clone();
        let symbol = symbol.clone();
        let endpoint = endpoint.clone();

        tokio::spawn(async move {
            info!("[ZMQ] Subscribing to {} at {}", symbol, endpoint);
            loop {
                match subscribe_loop(&symbol, &endpoint, &tx).await {
                    Ok(_) => warn!("[ZMQ] {} subscriber ended, reconnecting...", symbol),
                    Err(e) => error!("[ZMQ] {} subscriber error: {}, reconnecting...", symbol, e),
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            }
        });
    }

    rx
}

async fn subscribe_loop(
    symbol: &str,
    endpoint: &str,
    tx: &mpsc::Sender<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use zeromq::{SocketRecv, SubSocket, Socket};

    let mut socket = SubSocket::new();
    socket.connect(endpoint).await?;
    socket.subscribe("hashblock").await?;
    info!("[ZMQ] {} connected to {}", symbol, endpoint);

    loop {
        let msg = socket.recv().await?;
        // ZMQ hashblock message: frame[0]="hashblock", frame[1]=32-byte hash, frame[2]=sequence
        let frames: Vec<_> = msg.iter().collect();
        if frames.len() >= 2 {
            let hash_hex = if frames[1].len() == 32 {
                hex::encode(frames[1].as_ref())
            } else {
                String::from("?")
            };
            info!("[ZMQ] {} new block: {}", symbol, &hash_hex[..std::cmp::min(16, hash_hex.len())]);
            // Signal the job loop to refresh
            let _ = tx.try_send(symbol.to_string());
        }
    }
}
