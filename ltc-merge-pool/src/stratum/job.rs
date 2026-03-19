/// Job manager: creates stratum jobs from block templates,
/// caches recent jobs for share validation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::RwLock;
use log::info;

use crate::crypto::coinbase::{build_coinbase, CoinbaseParts};
use crate::crypto::encoding::{ser_string_be, 
    bytes_to_hex, hex_to_bytes, reverse_bytes, reverse_hex, ser_string_be2,
    bits_to_target, difficulty_to_target_scrypt, hash_le_target,
};
use crate::crypto::merkle::{get_merkle_branches, compute_root_from_branches};
use crate::crypto::sha256d::sha256d;
use crate::crypto::scrypt::scrypt_hash_be;
use crate::crypto::auxpow::{build_aux_merkle_tree, build_auxpow_proof};
use crate::types::BlockTemplate;

/// A stratum mining job, ready to be sent to clients.
#[derive(Debug, Clone)]
pub struct StratumJob {
    /// Unique job ID (hex-encoded counter)
    pub job_id: String,
    /// Previous block hash, word-reversed for stratum (Yiimp format)
    pub prevhash: String,
    /// Previous block hash, original hex
    pub prevhash_raw: String,
    /// Coinbase split parts
    pub coinbase: CoinbaseParts,
    /// Merkle branches (hex strings)
    pub merkle_branches: Vec<String>,
    /// Block version (hex, 4 bytes BE — Yiimp format, ser_string_be converts to LE wire format)
    pub version: String,
    /// nBits (hex, 4 bytes BE — Yiimp format)
    pub nbits: String,
    /// nTime (hex, 4 bytes BE — Yiimp format)
    pub ntime: String,
    /// Whether this job should cause clients to drop old work
    pub clean_jobs: bool,
    /// Block height
    pub height: u64,
    /// Block target (32 bytes, big-endian) for network share validation
    pub block_target: [u8; 32],
    /// Coinbase value (satoshis)
    pub coinbase_value: u64,
    /// Transaction data (raw hex) for block assembly
    pub tx_data: Vec<String>,
    /// Transaction txids for merkle root verification
    pub tx_hashes: Vec<[u8; 32]>,
    /// Timestamp when this job was created
    pub created_at: std::time::Instant,
    /// AuxPoW data: list of (chain_id, block_hash) for aux chains
    pub aux_blocks: Vec<(u32, [u8; 32])>,
    /// AuxPoW merkle size
    pub aux_merkle_size: u32,
    /// AuxPoW merkle nonce
    pub aux_merkle_nonce: u32,
    /// Default witness commitment (if segwit)
    pub default_witness_commitment: Option<String>,
    pub mweb: Option<String>,
    /// AuxPoW: display-order hash hex for each chain_id (for submitauxblock calls)
    pub aux_display_hashes: std::collections::HashMap<u32, String>,
    /// AuxPoW: mapping from chain_id to coin symbol
    pub chain_id_to_symbol: std::collections::HashMap<u32, String>,
    /// AuxPoW: mapping from chain_id to target (32 bytes, big-endian) for per-chain difficulty check
    pub aux_targets: std::collections::HashMap<u32, [u8; 32]>,
    /// AuxPoW: height for each chain_id
    pub aux_heights: std::collections::HashMap<u32, u64>,
}

/// Result of validating a submitted share.
#[derive(Debug)]
pub struct ShareResult {
    /// Whether the share meets the client's difficulty target
    pub is_valid_share: bool,
    /// Whether the share meets the network block target
    pub is_block: bool,
    /// The scrypt hash (big-endian hex) for logging/PoW
    pub hash_hex: String,
    /// The SHA-256d block hash (display order hex) for RPC getblock lookups
    pub block_hash_hex: String,
    /// Share difficulty
    pub share_difficulty: f64,
    /// The full block header bytes (for block submission)
    pub header_bytes: Vec<u8>,
    /// The full coinbase TX bytes (for block submission)
    pub coinbase_tx: Vec<u8>,
}

/// The job manager holds the current job and a cache of recent jobs.
pub struct JobManager {
    /// Current job (the latest)
    current_job: RwLock<Option<StratumJob>>,
    /// Cache of recent jobs indexed by job_id
    job_cache: RwLock<HashMap<String, StratumJob>>,
    /// Job ID counter
    job_counter: AtomicU64,
    /// Maximum number of cached jobs
    max_cached_jobs: usize,
    /// Pool address scriptPubKey (hex)
    pool_script: String,
    /// Pool fee percentage
    pool_fee_percent: f64,
    /// Extranonce1 size in bytes
    pub extranonce1_size: usize,
    /// Extranonce2 size in bytes
    pub extranonce2_size: usize,
}

impl JobManager {
    /// Create a new job manager.
    pub fn new(
        pool_script: String,
        pool_fee_percent: f64,
        extranonce1_size: usize,
        extranonce2_size: usize,
    ) -> Self {
        Self {
            current_job: RwLock::new(None),
            job_cache: RwLock::new(HashMap::new()),
            job_counter: AtomicU64::new(1),
            max_cached_jobs: 16,
            pool_script,
            pool_fee_percent,
            extranonce1_size,
            extranonce2_size,
        }
    }

    /// Create a new stratum job from a block template and optional aux blocks.
    pub fn create_job(
        &self,
        template: &BlockTemplate,
        aux_blocks: &[(u32, [u8; 32])],
        clean_jobs: bool,
    ) -> StratumJob {
        let job_num = self.job_counter.fetch_add(1, Ordering::Relaxed);
        let job_id = format!("{:x}", job_num);

        // Build aux merkle tree
        let (aux_merkle_root, aux_merkle_size, aux_merkle_nonce) =
            build_aux_merkle_tree(aux_blocks);

        let aux_root_opt = if aux_blocks.is_empty() {
            None
        } else {
            Some(&aux_merkle_root)
        };

        // Build coinbase transaction
        let coinbase = build_coinbase(
            template.height,
            template.coinbasevalue,
            &self.pool_script,
            self.pool_fee_percent,
            aux_root_opt,
            aux_merkle_size,
            aux_merkle_nonce,
            self.extranonce1_size,
            self.extranonce2_size,
            template.default_witness_commitment.as_deref(),
        );

        // Get transaction hashes for merkle tree
        let tx_hashes: Vec<[u8; 32]> = template
            .transactions
            .iter()
            .map(|tx| {
                let txid_bytes = hex_to_bytes(&tx.txid);
                let mut hash = [0u8; 32];
                // txid is displayed in reversed byte order; we need internal order
                let reversed = reverse_bytes(&txid_bytes);
                if reversed.len() == 32 {
                    hash.copy_from_slice(&reversed);
                }
                hash
            })
            .collect();

        // Merkle branches (without coinbase)
        let merkle_branches = if tx_hashes.is_empty() {
            Vec::new()
        } else {
            get_merkle_branches_for_stratum(&tx_hashes)
        };

        let merkle_branch_hex: Vec<String> = merkle_branches
            .iter()
            .map(|h| bytes_to_hex(h))
            .collect();

        // Prevhash: word-ORDER-reversed for stratum (Yiimp ser_string_be2).
        // Combined with ser_string_be in header construction, this produces LE wire format.
        let prevhash = ser_string_be2(&template.previousblockhash);

        // Version as BE hex (Yiimp format: sprintf("%08x", version)).
        // ser_string_be in header construction will convert this to LE wire format.
        let version = format!("{:08x}", template.version as u32);

        // nTime as BE hex (Yiimp format: sprintf("%08x", curtime)).
        let ntime = format!("{:08x}", template.curtime as u32);

        // nBits stays in BE hex (as returned by getblocktemplate).
        // Yiimp also keeps it in BE hex; ser_string_be converts to LE wire format.
        let nbits_be = template.bits.clone();

        // Block target from bits
        let block_target = bits_to_target(&template.bits);

        // Transaction data for block assembly
        let tx_data: Vec<String> = template
            .transactions
            .iter()
            .map(|tx| tx.data.clone())
            .collect();

        let stored_tx_hashes = tx_hashes.clone();

        let job = StratumJob {
            job_id: job_id.clone(),
            prevhash,
            prevhash_raw: template.previousblockhash.clone(),
            coinbase,
            merkle_branches: merkle_branch_hex,
            version,
            nbits: nbits_be,
            ntime,
            clean_jobs,
            height: template.height,
            block_target,
            coinbase_value: template.coinbasevalue,
            tx_data,
            tx_hashes: stored_tx_hashes,
            created_at: std::time::Instant::now(),
            aux_blocks: aux_blocks.to_vec(),
            aux_merkle_size,
            aux_merkle_nonce,
            default_witness_commitment: template.default_witness_commitment.clone(),
            mweb: template.mweb.clone(),
            aux_display_hashes: std::collections::HashMap::new(),
            chain_id_to_symbol: std::collections::HashMap::new(),
            aux_targets: std::collections::HashMap::new(),
            aux_heights: std::collections::HashMap::new(),
        };

        // Cache the job
        let mut cache = self.job_cache.write();
        cache.insert(job_id.clone(), job.clone());

        // Evict old jobs if too many
        if cache.len() > self.max_cached_jobs {
            let oldest_key = cache
                .iter()
                .min_by_key(|(_, j)| j.created_at)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_key {
                cache.remove(&key);
            }
        }
        drop(cache);

        // Update current job
        *self.current_job.write() = Some(job.clone());

        info!(
            "New job {} at height {} with {} txns, {} aux chains, aux_tree_size={}, aux_nonce={}",
            job.job_id,
            job.height,
            job.tx_data.len(),
            job.aux_blocks.len(),
            job.aux_merkle_size,
            job.aux_merkle_nonce,
        );

        job
    }

    /// Get the current (latest) job.
    pub fn current_job(&self) -> Option<StratumJob> {
        self.current_job.read().clone()
    }

    /// Look up a job by ID (for share validation).
    pub fn get_job(&self, job_id: &str) -> Option<StratumJob> {
        self.job_cache.read().get(job_id).cloned()
    }

    /// Validate a submitted share.
    pub fn validate_share(
        &self,
        job_id: &str,
        extranonce1: &str,
        extranonce2: &str,
        ntime: &str,
        nonce: &str,
        client_difficulty: f64,
    ) -> Result<ShareResult, String> {
        let job = self
            .get_job(job_id)
            .ok_or_else(|| format!("Job not found: {}", job_id))?;

        // Validate extranonce2 length
        if extranonce2.len() != self.extranonce2_size * 2 {
            return Err(format!(
                "Invalid extranonce2 length: expected {}, got {}",
                self.extranonce2_size * 2,
                extranonce2.len()
            ));
        }

        if ntime.len() != 8 {
            return Err(format!("Invalid ntime length: {}", ntime.len()));
        }

        if nonce.len() != 8 {
            return Err(format!("Invalid nonce length: {}", nonce.len()));
        }

        // [Fix #7] ntime range validation: reject if >1800s from current time
        if let Ok(submitted_ntime) = u32::from_str_radix(ntime, 16) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(0);
            if now > 0 {
                let diff = if submitted_ntime > now {
                    submitted_ntime - now
                } else {
                    now - submitted_ntime
                };
                if diff > 1800 {
                    return Err(format!(
                        "ntime {} is too far from current time {} (delta={}s, max=1800s)",
                        submitted_ntime, now, diff
                    ));
                }
            }
        } else {
            return Err(format!("Invalid ntime hex: {}", ntime));
        }

        // Build the coinbase transaction
        let coinbase_tx = job.coinbase.build_coinbase_tx(extranonce1, extranonce2);
        let coinbase_hash = sha256d(&coinbase_tx);

        // Compute the merkle root
        let merkle_branches_bytes: Vec<[u8; 32]> = job
            .merkle_branches
            .iter()
            .map(|h| {
                let bytes = hex_to_bytes(h);
                let mut arr = [0u8; 32];
                if bytes.len() == 32 {
                    arr.copy_from_slice(&bytes);
                }
                arr
            })
            .collect();

        let merkle_root = compute_root_from_branches(&coinbase_hash, &merkle_branches_bytes);

        // Build the 80-byte block header using EXACT Yiimp method:
        //
        // All single-word fields (version, ntime, nbits, nonce) are in BE hex.
        // Prevhash is word-ORDER-reversed (ser_string_be2 of display-order hash).
        // Merkle root is byte-reversed-within-words (ser_string_be of LE hash hex).
        //
        // Concatenation: version_BE + prevhash_WR + merkle_WR + ntime_BE + nbits_BE + nonce
        // Then ser_string_be reverses bytes within each 4-byte word:
        //   - BE single-word fields -> LE (correct wire format)
        //   - word-ORDER-reversed prevhash -> full byte reversal = LE (correct wire format)
        //   - byte-reversed-within-words merkle -> back to LE (correct wire format)
        let merkle_hex = bytes_to_hex(&merkle_root);
        let merkle_wr = ser_string_be(&merkle_hex);

        let header_hex = format!("{}{}{}{}{}{}",
            job.version,        // version BE hex (e.g. "20000000")
            job.prevhash,       // prevhash word-ORDER-reversed
            merkle_wr,          // merkle root byte-reversed-within-words
            ntime,              // ntime BE hex (echoed from mining.notify)
            job.nbits,          // nbits BE hex
            nonce               // nonce hex
        );

        // Apply ser_string_be to entire header (reverse bytes in each 4-byte word)
        // This converts all fields to proper LE wire format.
        let header_wire = ser_string_be(&header_hex);
        let header = hex_to_bytes(&header_wire);

        if header.len() != 80 {
            return Err(format!("Header is {} bytes, expected 80", header.len()));
        }

        // Compute scrypt hash (big-endian for target comparison)
        let hash_be = scrypt_hash_be(&header);
        let hash_hex = bytes_to_hex(&hash_be);

        // [Fix #9] Minimum hash sanity check: last 2 bytes of BE hash must be zero
        // In big-endian representation, bytes[0] and [1] are the most significant.
        // A valid scrypt share at any reasonable difficulty should have leading zero bytes.
        // We check the first 2 bytes (most significant in BE) are zero.
        if hash_be[0] != 0 || hash_be[1] != 0 {
            return Err(format!(
                "Hash fails sanity check: first bytes are {:02x}{:02x} (expected 0000)",
                hash_be[0], hash_be[1]
            ));
        }

        // Compute SHA-256d block hash (the actual block identity hash used by RPC)
        let block_id_hash = sha256d(&header);
        let block_hash_hex = bytes_to_hex(&reverse_bytes(&block_id_hash));



        // Compute share difficulty from hash
        let share_difficulty = hash_to_difficulty(&hash_be);

        // Check against client difficulty target
        let client_target = difficulty_to_target_scrypt(client_difficulty);
        let is_valid_share = hash_le_target(&hash_be, &client_target);

        // Check against network block target.
        let is_block = hash_le_target(&hash_be, &job.block_target);

        Ok(ShareResult {
            is_valid_share,
            is_block,
            hash_hex,
            block_hash_hex,
            share_difficulty,
            header_bytes: header,
            coinbase_tx,
        })
    }

    /// Assemble a full block for submission to the parent chain.
    /// Returns the hex-encoded block.
    pub fn assemble_block(
        &self,
        job: &StratumJob,
        header: &[u8],
        coinbase_tx: &[u8],
    ) -> String {
        use crate::crypto::encoding::compact_size;

        let mut block = Vec::new();

        // Block header (80 bytes) - standard wire format from validate_share
        block.extend_from_slice(header);

        // Transaction count (coinbase + regular txns)
        let tx_count = 1 + job.tx_data.len();
        block.extend_from_slice(&compact_size(tx_count as u64));

        // Coinbase transaction - add segwit witness for submitblock
        // Non-witness: version(4) + inputs + outputs + locktime(4)
        // Witness:     version(4) + marker(00) + flag(01) + inputs + outputs + witness + locktime(4)
        if coinbase_tx.len() >= 8 {
            let version = &coinbase_tx[0..4];
            let middle = &coinbase_tx[4..coinbase_tx.len()-4];
            let locktime = &coinbase_tx[coinbase_tx.len()-4..];
            
            block.extend_from_slice(version);           // version
            block.push(0x00);                            // segwit marker
            block.push(0x01);                            // segwit flag
            block.extend_from_slice(middle);             // inputs + outputs
            block.push(0x01);                            // witness stack count: 1
            block.push(0x20);                            // witness item length: 32
            block.extend_from_slice(&[0u8; 32]);         // witness reserved value: 32 zero bytes
            block.extend_from_slice(locktime);           // locktime
        } else {
            block.extend_from_slice(coinbase_tx);
        }

        // Regular transactions
        for tx_hex in &job.tx_data {
            block.extend_from_slice(&hex_to_bytes(tx_hex));
        }

        // MWEB extension block (required for Litecoin)
        if let Some(ref mweb_hex) = job.mweb {
            if !mweb_hex.is_empty() {
                block.push(0x01);  // MWEB marker
                block.extend_from_slice(&hex_to_bytes(mweb_hex));
            }
        }

        bytes_to_hex(&block)
    }

    /// Build AuxPoW proof for submitting to an aux chain.
    pub fn build_aux_proof(
        &self,
        job: &StratumJob,
        header: &[u8],
        coinbase_tx: &[u8],
        target_chain_id: u32,
    ) -> String {
        // Get coinbase merkle branches (for proving coinbase is in the parent block)
        let coinbase_hash = sha256d(coinbase_tx);

        // Build the full list of tx hashes (coinbase + regular)
        let mut all_hashes = vec![coinbase_hash];
        for h in &job.tx_hashes {
            all_hashes.push(*h);
        }

        let coinbase_branches = get_merkle_branches(&all_hashes);

        build_auxpow_proof(
            coinbase_tx,
            &coinbase_branches,
            header,
            &job.aux_blocks,
            target_chain_id,
            job.aux_merkle_nonce,
        )
    }

    /// Clear all cached jobs (e.g. on new block).
    pub fn clear_cache(&self) {
        self.job_cache.write().clear();
    }

    /// Update a job's metadata (display hashes, symbol map, targets) after creation.
    pub fn update_job_metadata(&self, job: &StratumJob) {
        let mut cache = self.job_cache.write();
        if let Some(cached_job) = cache.get_mut(&job.job_id) {
            cached_job.aux_display_hashes = job.aux_display_hashes.clone();
            cached_job.chain_id_to_symbol = job.chain_id_to_symbol.clone();
            cached_job.aux_targets = job.aux_targets.clone();
            cached_job.aux_heights = job.aux_heights.clone();
        }
        drop(cache);
        let mut current = self.current_job.write();
        if let Some(ref mut cj) = *current {
            if cj.job_id == job.job_id {
                cj.aux_display_hashes = job.aux_display_hashes.clone();
                cj.chain_id_to_symbol = job.chain_id_to_symbol.clone();
                cj.aux_targets = job.aux_targets.clone();
                cj.aux_heights = job.aux_heights.clone();
            }
        }
    }
}

/// Compute merkle branches for stratum (without coinbase).
/// Mirrors the Yiimp C++ merkle_steps() algorithm:
/// L includes a placeholder at index 0 for the coinbase. At each level,
/// L[1] is the sibling of the running hash. Pairs are hashed starting
/// from index 2, skipping the coinbase pair (which the miner computes).
fn get_merkle_branches_for_stratum(tx_hashes: &[[u8; 32]]) -> Vec<[u8; 32]> {
    if tx_hashes.is_empty() {
        return Vec::new();
    }

    let mut branches = Vec::new();
    // Prepend a dummy placeholder for the coinbase at index 0
    let mut level: Vec<[u8; 32]> = Vec::with_capacity(1 + tx_hashes.len());
    level.push([0u8; 32]); // placeholder for coinbase
    level.extend_from_slice(tx_hashes);

    while level.len() > 1 {
        // The sibling of the running hash (coinbase side) is at index 1
        branches.push(level[1]);

        // If odd count, duplicate last element
        if level.len() % 2 != 0 {
            let last = *level.last().unwrap();
            level.push(last);
        }

        // Hash pairs starting from index 2 (skip the first pair which includes coinbase)
        let mut next: Vec<[u8; 32]> = Vec::new();
        next.push([0u8; 32]); // placeholder for the hashed coinbase pair
        let mut i = 2;
        while i < level.len() {
            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(&level[i]);
            combined.extend_from_slice(&level[i + 1]);
            next.push(sha256d(&combined));
            i += 2;
        }
        level = next;
    }

    branches
}

/// Convert a big-endian hash to approximate difficulty.
/// difficulty = maxTarget / hashTarget
/// For scrypt: maxTarget = 0x0000FFFF * 2^224
fn hash_to_difficulty(hash_be: &[u8; 32]) -> f64 {
    // Use full 256-bit hash for difficulty calculation
    // maxTarget = 0xFFFF * 2^224 (same scale as pool difficulty settings)
    let mut value = 0.0f64;
    for (i, &b) in hash_be.iter().enumerate() {
        value += (b as f64) * (256.0f64).powi(31 - i as i32);
    }
    if value == 0.0 {
        return f64::MAX;
    }
    let max_target: f64 = 0xFFFFu64 as f64 * (2.0f64).powi(208);
    max_target / value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_to_difficulty() {
        let hash = [0u8; 32];
        let diff = hash_to_difficulty(&hash);
        assert!(diff == f64::MAX);
    }

    #[test]
    fn test_get_merkle_branches_empty() {
        let branches = get_merkle_branches_for_stratum(&[]);
        assert!(branches.is_empty());
    }
}
