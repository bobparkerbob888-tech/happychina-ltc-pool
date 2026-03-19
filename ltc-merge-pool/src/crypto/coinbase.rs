/// Coinbase transaction builder with AuxPoW commitment.
/// Splits into coinbase1/coinbase2 for stratum protocol.

use super::encoding::{bip34_height, bytes_to_hex, compact_size, hex_to_bytes, i32_le, u32_le, u64_le};
use super::sha256d::sha256d;

/// Result of building a coinbase transaction, split for stratum.
#[derive(Debug, Clone)]
pub struct CoinbaseParts {
    /// Everything before the extranonce placeholder (hex)
    pub coinbase1: String,
    /// Everything after extranonce1+extranonce2 to end of TX (hex)
    pub coinbase2: String,
    /// Size of extranonce1 in bytes
    pub extranonce1_size: usize,
    /// Size of extranonce2 in bytes
    pub extranonce2_size: usize,
}

impl CoinbaseParts {
    /// Reconstruct the full coinbase transaction given extranonce1 and extranonce2 (hex).
    /// Returns the full coinbase TX as bytes.
    pub fn build_coinbase_tx(&self, extranonce1: &str, extranonce2: &str) -> Vec<u8> {
        let mut hex = String::new();
        hex.push_str(&self.coinbase1);
        hex.push_str(extranonce1);
        hex.push_str(extranonce2);
        hex.push_str(&self.coinbase2);
        hex_to_bytes(&hex)
    }

    /// Compute the coinbase txid (SHA256d of the serialized TX, then reverse for display).
    pub fn coinbase_txid(&self, extranonce1: &str, extranonce2: &str) -> [u8; 32] {
        let tx_bytes = self.build_coinbase_tx(extranonce1, extranonce2);
        sha256d(&tx_bytes)
    }
}

/// Build a coinbase transaction for the parent chain (LTC).
///
/// The aux_merkle_root is in LE (internal byte order) as computed by the merkle tree.
/// It is stored in BIG-ENDIAN (display order) in the coinbase commitment, because
/// the daemon computes the expected root in LE, reverses to BE, then searches
/// The aux merkle root is stored in sha256d natural byte order after fabe6d6d.
pub fn build_coinbase(
    height: u64,
    coinbase_value: u64,
    pool_address_script: &str,
    pool_fee_percent: f64,
    aux_merkle_root: Option<&[u8; 32]>,
    aux_merkle_size: u32,
    aux_merkle_nonce: u32,
    extranonce1_size: usize,
    extranonce2_size: usize,
    default_witness_commitment: Option<&str>,
) -> CoinbaseParts {
    // Calculate fee split
    let fee_satoshis = ((coinbase_value as f64) * (pool_fee_percent / 100.0)) as u64;
    let miner_satoshis = coinbase_value - fee_satoshis;

    // Build scriptSig
    let mut script_sig = Vec::new();

    // BIP34 height
    script_sig.extend_from_slice(&bip34_height(height));

    // AuxPoW commitment: magic(4) + merkle_root(32) + merkle_size_LE(4) + merkle_nonce_LE(4)
    // The merkle root bytes are written in their natural sha256d output order.
    // The aux daemon searches for these exact bytes in the coinbase scriptSig.
    if let Some(aux_root) = aux_merkle_root {
        // fabe6d6d magic
        script_sig.extend_from_slice(&[0xFA, 0xBE, 0x6D, 0x6D]);
        let mut aux_root_be = *aux_root;
        aux_root_be.reverse();
        script_sig.extend_from_slice(&aux_root_be);
        script_sig.extend_from_slice(&u32_le(aux_merkle_size));
        script_sig.extend_from_slice(&u32_le(aux_merkle_nonce));
    }

    // Pool tag (before extranonce)
    let pool_tag = b"/HappyChina/";
    script_sig.extend_from_slice(pool_tag);

    // Mark the extranonce position
    let extranonce_offset_in_scriptsig = script_sig.len();
    let total_extranonce_size = extranonce1_size + extranonce2_size;

    // Placeholder for extranonce (will be replaced by miner)
    script_sig.extend(std::iter::repeat(0u8).take(total_extranonce_size));

    let scriptsig_len = script_sig.len();

    // Build the coinbase transaction
    let mut tx = Vec::new();

    // Version (1)
    tx.extend_from_slice(&i32_le(1));

    // Input count
    tx.extend_from_slice(&compact_size(1));

    // Previous output (null for coinbase)
    tx.extend_from_slice(&[0u8; 32]); // prev txid
    tx.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // prev vout = 0xFFFFFFFF

    // ScriptSig length
    tx.extend_from_slice(&compact_size(scriptsig_len as u64));

    // Mark where extranonce starts in the full TX
    let extranonce_offset_in_tx = tx.len() + extranonce_offset_in_scriptsig;

    // ScriptSig
    tx.extend_from_slice(&script_sig);

    // Sequence
    tx.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);

    // Outputs
    let mut num_outputs: u64 = 1;
    if fee_satoshis > 0 {
        num_outputs += 1;
    }
    if default_witness_commitment.is_some() {
        num_outputs += 1;
    }

    let mut outputs = Vec::new();

    // Output 1: miner reward
    outputs.extend_from_slice(&u64_le(miner_satoshis));
    let script_bytes = hex_to_bytes(pool_address_script);
    outputs.extend_from_slice(&compact_size(script_bytes.len() as u64));
    outputs.extend_from_slice(&script_bytes);

    // Output 2: pool fee (if > 0)
    if fee_satoshis > 0 {
        outputs.extend_from_slice(&u64_le(fee_satoshis));
        outputs.extend_from_slice(&compact_size(script_bytes.len() as u64));
        outputs.extend_from_slice(&script_bytes);
    }

    // Output 3: segwit witness commitment (if present)
    if let Some(commitment_hex) = default_witness_commitment {
        let commitment_bytes = hex_to_bytes(commitment_hex);
        outputs.extend_from_slice(&u64_le(0));
        outputs.extend_from_slice(&compact_size(commitment_bytes.len() as u64));
        outputs.extend_from_slice(&commitment_bytes);
    }

    // Write output count + outputs
    tx.extend_from_slice(&compact_size(num_outputs));
    tx.extend_from_slice(&outputs);

    // Locktime
    tx.extend_from_slice(&u32_le(0));

    // Split at extranonce position
    let tx_hex = bytes_to_hex(&tx);
    let hex_offset = extranonce_offset_in_tx * 2;
    let hex_end = hex_offset + (total_extranonce_size * 2);

    let coinbase1 = tx_hex[..hex_offset].to_string();
    let coinbase2 = tx_hex[hex_end..].to_string();

    CoinbaseParts {
        coinbase1,
        coinbase2,
        extranonce1_size,
        extranonce2_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_coinbase_splits() {
        let parts = build_coinbase(
            500000,
            1250000000,
            "76a914000000000000000000000000000000000000000088ac",
            1.0,
            None,
            0,
            0,
            4,
            4,
            None,
        );

        assert!(!parts.coinbase1.is_empty());
        assert!(!parts.coinbase2.is_empty());
        assert_eq!(parts.extranonce1_size, 4);
        assert_eq!(parts.extranonce2_size, 4);

        let tx = parts.build_coinbase_tx("00000001", "00000000");
        assert!(!tx.is_empty());

        let txid = parts.coinbase_txid("00000001", "00000000");
        assert!(txid.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_coinbase_with_auxpow() {
        let aux_root = [0xABu8; 32];
        let parts = build_coinbase(
            100000,
            5000000000,
            "76a914000000000000000000000000000000000000000088ac",
            0.0,
            Some(&aux_root),
            8,
            0,
            4,
            4,
            None,
        );

        let tx_bytes = parts.build_coinbase_tx("00000001", "00000000");
        let tx_hex = hex::encode(&tx_bytes);

        // Should contain the AuxPoW magic
        assert!(tx_hex.contains("fabe6d6d"));
    }
}
