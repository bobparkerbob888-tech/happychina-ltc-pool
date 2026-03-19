/// Merkle tree computation and branch extraction for stratum mining.

use super::sha256d::sha256d;

/// Compute the merkle root from a list of transaction hashes (as 32-byte arrays).
/// The first entry should be the coinbase txid.
/// Returns the merkle root as 32 bytes.
pub fn compute_merkle_root(hashes: &[[u8; 32]]) -> [u8; 32] {
    if hashes.is_empty() {
        return [0u8; 32];
    }
    if hashes.len() == 1 {
        return hashes[0];
    }

    let mut current: Vec<[u8; 32]> = hashes.to_vec();

    while current.len() > 1 {
        let mut next = Vec::with_capacity((current.len() + 1) / 2);
        let mut i = 0;
        while i < current.len() {
            let left = &current[i];
            let right = if i + 1 < current.len() {
                &current[i + 1]
            } else {
                &current[i] // duplicate last if odd
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

/// Extract merkle branches for the coinbase transaction (index 0).
/// Returns a list of sibling hashes needed to recompute the merkle root
/// given a modified coinbase hash.
/// The stratum protocol sends these branches so miners can compute
/// the merkle root after inserting their extranonce.
pub fn get_merkle_branches(hashes: &[[u8; 32]]) -> Vec<[u8; 32]> {
    if hashes.len() <= 1 {
        return Vec::new();
    }

    let mut branches = Vec::new();
    let mut current: Vec<[u8; 32]> = hashes.to_vec();

    while current.len() > 1 {
        // The sibling of index 0 is index 1 (or self if only one element)
        if current.len() > 1 {
            branches.push(current[1]);
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
    }

    branches
}

/// Compute the merkle root from a coinbase hash and merkle branches.
/// This is what a stratum miner does after computing its coinbase hash.
pub fn compute_root_from_branches(coinbase_hash: &[u8; 32], branches: &[[u8; 32]]) -> [u8; 32] {
    let mut current = *coinbase_hash;
    for branch in branches {
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(&current);
        combined.extend_from_slice(branch);
        current = sha256d(&combined);
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_tx_merkle() {
        let hash = sha256d(b"coinbase");
        let root = compute_merkle_root(&[hash]);
        assert_eq!(root, hash);
    }

    #[test]
    fn test_two_tx_merkle() {
        let h1 = sha256d(b"tx1");
        let h2 = sha256d(b"tx2");
        let root = compute_merkle_root(&[h1, h2]);

        // Manually compute
        let mut combined = Vec::new();
        combined.extend_from_slice(&h1);
        combined.extend_from_slice(&h2);
        let expected = sha256d(&combined);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_branches_reconstruct() {
        let h1 = sha256d(b"tx1");
        let h2 = sha256d(b"tx2");
        let h3 = sha256d(b"tx3");

        let hashes = vec![h1, h2, h3];
        let root = compute_merkle_root(&hashes);
        let branches = get_merkle_branches(&hashes);
        let reconstructed = compute_root_from_branches(&h1, &branches);
        assert_eq!(root, reconstructed);
    }
}
