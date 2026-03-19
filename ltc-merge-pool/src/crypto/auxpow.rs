/// Multi-chain AuxPoW merkle tree for merge-mined coins.
/// Builds a merkle tree of aux chain hashes, assigns slots by chain_id,
/// and constructs proofs for submitauxblock / getauxblock.

use super::sha256d::sha256d;
use super::encoding::{bytes_to_hex, u32_le};
use log::debug;

/// AuxPoW chain slot assignment using the Namecoin/Yiimp LCG algorithm.
/// This matches CAuxPow::getExpectedIndex exactly:
///   rand = (nonce * 1103515245 + 12345) as u32
///   rand = ((rand + chain_id) * 1103515245 + 12345) as u32
///   slot = rand % merkle_size
pub fn get_expected_index(nonce: u32, chain_id: u32, merkle_size: u32) -> u32 {
    if merkle_size <= 1 {
        return 0;
    }
    let rand = (nonce as u64)
        .wrapping_mul(1103515245)
        .wrapping_add(12345) as u32;
    let rand = ((rand as u64).wrapping_add(chain_id as u64) as u32 as u64)
        .wrapping_mul(1103515245)
        .wrapping_add(12345) as u32;
    rand % merkle_size
}

/// Find the smallest power-of-2 tree size and nonce that can hold all the
/// given chain_ids without slot collisions.
/// Returns (tree_size, nonce).
pub fn find_tree_params(chain_ids: &[u32]) -> (u32, u32) {
    if chain_ids.is_empty() || chain_ids.len() == 1 {
        return (1u32.max(chain_ids.len() as u32), 0);
    }

    for depth in 1..=16u32 {
        let tree_size = 1u32 << depth;
        for nonce in 0..100_000u32 {
            let mut slots = std::collections::HashSet::new();
            let mut collision = false;
            for &cid in chain_ids {
                let slot = get_expected_index(nonce, cid, tree_size);
                if !slots.insert(slot) {
                    collision = true;
                    break;
                }
            }
            if !collision {
                return (tree_size, nonce);
            }
        }
    }
    // Fallback (should never happen for reasonable numbers of chains)
    (1u32 << 16, 0)
}

/// Build the AuxPoW merkle tree from a set of (chain_id, block_hash) pairs.
/// block_hash must be in INTERNAL byte order (little-endian) already.
/// Returns (merkle_root, merkle_size, merkle_nonce).
/// The merkle root is in INTERNAL byte order (little-endian).
pub fn build_aux_merkle_tree(
    aux_blocks: &[(u32, [u8; 32])], // (chain_id, block_hash in LE/internal order)
) -> ([u8; 32], u32, u32) {
    if aux_blocks.is_empty() {
        return ([0u8; 32], 1, 0);
    }

    let chain_ids: Vec<u32> = aux_blocks.iter().map(|&(cid, _)| cid).collect();
    let (merkle_size, merkle_nonce) = find_tree_params(&chain_ids);

    // Build leaf array
    let mut leaves: Vec<[u8; 32]> = vec![[0u8; 32]; merkle_size as usize];

    for &(chain_id, ref block_hash) in aux_blocks {
        let slot = get_expected_index(merkle_nonce, chain_id, merkle_size) as usize;
        leaves[slot] = *block_hash;
        debug!(
            "AuxPoW: chain_id={} -> slot={} hash={}",
            chain_id,
            slot,
            bytes_to_hex(block_hash)
        );
    }

    // Compute merkle root
    let root = compute_aux_merkle_root(&leaves);

    log::info!(
        "AuxPoW tree: size={} nonce={} root={} chains={}",
        merkle_size, merkle_nonce, bytes_to_hex(&root), aux_blocks.len()
    );

    (root, merkle_size, merkle_nonce)
}

/// Compute merkle root of the aux tree leaves.
fn compute_aux_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    if leaves.len() == 1 {
        return leaves[0];
    }

    let mut current = leaves.to_vec();

    while current.len() > 1 {
        let mut next = Vec::with_capacity((current.len() + 1) / 2);
        let mut i = 0;
        while i < current.len() {
            let left = &current[i];
            let right = if i + 1 < current.len() {
                &current[i + 1]
            } else {
                &current[i]
            };
            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(left);
            combined.extend_from_slice(right);
            next.push(sha256d(&combined));
            i += 2;
        }
        current = next;
    }

    current[0]
}

/// Get the merkle branch (proof) for a specific slot in the aux tree.
/// Returns the sibling hashes needed to reconstruct the root.
fn get_aux_merkle_branch(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    if leaves.len() <= 1 {
        return Vec::new();
    }

    let mut branches = Vec::new();
    let mut current = leaves.to_vec();
    let mut idx = index;

    while current.len() > 1 {
        // Sibling index (XOR with 1 flips the last bit: left<->right)
        let sibling = idx ^ 1;
        if sibling < current.len() {
            branches.push(current[sibling]);
        }

        // Compute next level
        let mut next = Vec::with_capacity((current.len() + 1) / 2);
        let mut i = 0;
        while i < current.len() {
            let left = &current[i];
            let right = if i + 1 < current.len() {
                &current[i + 1]
            } else {
                &current[i]
            };
            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(left);
            combined.extend_from_slice(right);
            next.push(sha256d(&combined));
            i += 2;
        }
        current = next;
        idx /= 2;
    }

    branches
}

/// Construct the full AuxPoW proof for submitting to an aux chain daemon.
///
/// The AuxPoW serialization format (matches Yiimp client_submit.cpp exactly):
///   - coinbase tx (raw hex, non-witness serialization)
///   - parent block hash (32 bytes, big-endian/display order per Yiimp convention)
///   - varint: number of coinbase merkle branches
///   - coinbase merkle branch hashes (each 32 bytes)
///   - coinbase merkle index (u32 LE, always 0)
///   - varint: number of aux merkle branches
///   - aux merkle branch hashes (each 32 bytes)
///   - aux merkle index (u32 LE = slot in aux tree)
///   - parent block header (80 bytes, standard wire format)
///
/// Returns the hex-encoded auxpow data.
pub fn build_auxpow_proof(
    coinbase_tx: &[u8],
    coinbase_merkle_branches: &[[u8; 32]],
    parent_block_header: &[u8],
    aux_blocks: &[(u32, [u8; 32])],
    target_chain_id: u32,
    merkle_nonce: u32,
) -> String {
    let chain_ids: Vec<u32> = aux_blocks.iter().map(|&(cid, _)| cid).collect();
    let (merkle_size, _nonce) = find_tree_params(&chain_ids);
    // Use the nonce from the job (same as was used to build the commitment)
    let nonce = merkle_nonce;

    // Build aux leaves
    let mut leaves: Vec<[u8; 32]> = vec![[0u8; 32]; merkle_size as usize];
    for &(chain_id, ref block_hash) in aux_blocks {
        let slot = get_expected_index(nonce, chain_id, merkle_size) as usize;
        leaves[slot] = *block_hash;
    }

    let slot = get_expected_index(nonce, target_chain_id, merkle_size) as usize;
    let aux_branch = get_aux_merkle_branch(&leaves, slot);

    // Compute aux merkle root for verification
    let aux_root = compute_aux_merkle_root(&leaves);

    log::info!(
        "AuxPoW proof for chain_id={}: slot={} merkle_size={} nonce={} aux_branch_len={} aux_root={}",
        target_chain_id, slot, merkle_size, nonce, aux_branch.len(), bytes_to_hex(&aux_root)
    );

    let mut proof = Vec::new();

    // 1. Coinbase transaction (raw bytes)
    proof.extend_from_slice(coinbase_tx);
    log::debug!("  coinbase_tx len={}", coinbase_tx.len());

    // 2. Parent block hash (SHA256d of parent header)
    // The daemon deserializes this as uint256 via CDataStream.
    // Yiimp and the Dogecoin test framework both write this in
    // display/big-endian order (reversed from SHA256d output).
    // The daemon's CAuxPow::check() does NOT validate this field
    // (it's redundant since parentBlock is also in the proof),
    // but we match Yiimp's format for correctness.
    let parent_hash_le = sha256d(parent_block_header);
    let mut parent_hash_be = parent_hash_le;
    parent_hash_be.reverse();
    proof.extend_from_slice(&parent_hash_be);
    log::info!("  parent_hash_be={}", bytes_to_hex(&parent_hash_be));

    // 3. Coinbase merkle branch
    let branch_count = coinbase_merkle_branches.len();
    proof.extend_from_slice(&compact_size_bytes(branch_count as u64));
    for (i, branch) in coinbase_merkle_branches.iter().enumerate() {
        proof.extend_from_slice(branch);
        log::debug!("  coinbase_branch[{}]={}", i, bytes_to_hex(branch));
    }
    // Coinbase index bitmask (always 0 - coinbase is first tx)
    proof.extend_from_slice(&u32_le(0));

    // 4. Aux merkle branch
    proof.extend_from_slice(&compact_size_bytes(aux_branch.len() as u64));
    for (i, branch) in aux_branch.iter().enumerate() {
        proof.extend_from_slice(branch);
        log::debug!("  aux_branch[{}]={}", i, bytes_to_hex(branch));
    }
    // Aux branch index = slot (u32 LE)
    proof.extend_from_slice(&u32_le(slot as u32));
    log::info!("  aux_slot={} coinbase_branches={} aux_branches={}", slot, branch_count, aux_branch.len());

    // 5. Parent block header (80 bytes, standard wire format)
    proof.extend_from_slice(parent_block_header);
    log::debug!("  parent_header len={}", parent_block_header.len());

    // Verify: check that the coinbase contains the fabe6d6d magic + aux_root
    let coinbase_hex = bytes_to_hex(coinbase_tx);
    if coinbase_hex.contains("fabe6d6d") {
        let magic_pos = coinbase_hex.find("fabe6d6d").unwrap();
        let root_in_cb = &coinbase_hex[magic_pos+8..magic_pos+8+64];
        // The root in the coinbase is in BIG-ENDIAN (display order).
        // Our computed aux_root is in LITTLE-ENDIAN (internal order).
        // Reverse aux_root to BE for comparison.
        let mut aux_root_be = aux_root;
        aux_root_be.reverse();
        let aux_root_be_hex = bytes_to_hex(&aux_root_be);
        let matches = root_in_cb == aux_root_be_hex;
        log::info!("  coinbase aux_root: {} tree_root_be: {} match={}", root_in_cb, aux_root_be_hex, matches);
        if !matches {
            log::error!("  AUX ROOT MISMATCH! Coinbase commitment does not match tree root!");
        }
    } else {
        log::warn!("  coinbase does NOT contain fabe6d6d magic!");
    }

    bytes_to_hex(&proof)
}

/// Compact size encoding
fn compact_size_bytes(n: u64) -> Vec<u8> {
    if n < 0xFD {
        vec![n as u8]
    } else if n <= 0xFFFF {
        let mut out = vec![0xFD];
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out
    } else if n <= 0xFFFF_FFFF {
        let mut out = vec![0xFE];
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out
    } else {
        let mut out = vec![0xFF];
        out.extend_from_slice(&n.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lcg_slot_assignment() {
        // With nonce=0, treeSize=32:
        // DOGE chainId=98 -> slot 24
        // PEPE chainId=63 -> slot 17
        // BELLS chainId=16 -> slot 14
        // DINGO chainId=50 -> slot 8
        // SHIC chainId=74 -> slot 0
        // TRMP chainId=168 -> slot 6
        assert_eq!(get_expected_index(0, 98, 32), 24);
        assert_eq!(get_expected_index(0, 63, 32), 17);
        assert_eq!(get_expected_index(0, 16, 32), 14);
        assert_eq!(get_expected_index(0, 50, 32), 8);
        assert_eq!(get_expected_index(0, 74, 32), 0);
        assert_eq!(get_expected_index(0, 168, 32), 6);
    }

    #[test]
    fn test_find_tree_params() {
        let chain_ids = vec![98, 63, 16, 50, 74, 168];
        let (size, nonce) = find_tree_params(&chain_ids);
        // Should find size=32, nonce=0 (no collisions)
        assert_eq!(size, 32);
        assert_eq!(nonce, 0);
    }

    #[test]
    fn test_build_aux_merkle_tree() {
        let hash1 = [0x11u8; 32];
        let hash2 = [0x22u8; 32];
        let aux_blocks = vec![(1u32, hash1), (2u32, hash2)];
        let (root, size, nonce) = build_aux_merkle_tree(&aux_blocks);
        assert!(size >= 2);
        // Root should be non-trivial
        assert!(root.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_empty_aux_tree() {
        let (root, size, nonce) = build_aux_merkle_tree(&[]);
        assert_eq!(root, [0u8; 32]);
        assert_eq!(size, 1);
        assert_eq!(nonce, 0);
    }

    #[test]
    fn test_single_chain_slot() {
        // Single chain: tree size=1, any chain_id should get slot 0
        assert_eq!(get_expected_index(0, 98, 1), 0);
        assert_eq!(get_expected_index(0, 1, 1), 0);
    }
}
