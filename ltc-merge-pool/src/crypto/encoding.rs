/// Hex/byte utilities, varint encoding, byte reversal, BIP34 height serialization.

/// Decode a hex string into bytes
pub fn hex_to_bytes(hex: &str) -> Vec<u8> {
    hex::decode(hex).unwrap_or_default()
}

/// Encode bytes as a lowercase hex string
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Reverse a byte slice (returns new vec)
pub fn reverse_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    out.reverse();
    out
}

/// Reverse a hex string byte-by-byte (e.g. "aabbcc" -> "ccbbaa")
pub fn reverse_hex(hex: &str) -> String {
    let bytes = hex_to_bytes(hex);
    bytes_to_hex(&reverse_bytes(&bytes))
}

/// Yiimp ser_string_be2: reverse the ORDER of 4-byte (8-hex-char) words.
/// The bytes within each word are NOT reversed — only the word positions change.
/// For a 32-byte hash (8 words), word 0 goes to position 7, word 1 to position 6, etc.
/// This matches the Yiimp C++ ser_string_be2() used for stratum prevhash.
///
/// Combined with ser_string_be (which reverses bytes within each word), this produces
/// a full byte reversal: ser_string_be(ser_string_be2(display_hex)) = LE wire format.
pub fn ser_string_be2(hex: &str) -> String {
    let chars: Vec<char> = hex.chars().collect();
    let num_words = chars.len() / 8;
    if num_words == 0 || chars.len() % 8 != 0 {
        return hex.to_string();
    }
    let mut result = String::with_capacity(chars.len());
    // Copy words in reverse order (last word first)
    for i in 0..num_words {
        let src = (num_words - 1 - i) * 8;
        for j in 0..8 {
            result.push(chars[src + j]);
        }
    }
    result
}

/// Encode a Bitcoin compact-size (varint) for transaction counts, etc.
pub fn compact_size(n: u64) -> Vec<u8> {
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

/// BIP34 height serialization for coinbase scriptSig.
/// Encodes the block height as a minimal CScriptNum push.
pub fn bip34_height(height: u64) -> Vec<u8> {
    if height == 0 {
        return vec![0x01, 0x00]; // OP_0 would be 0x00, but BIP34 uses push 0x00
    }
    // Serialize as little-endian, minimal encoding
    let mut h = height;
    let mut data = Vec::new();
    while h > 0 {
        data.push((h & 0xFF) as u8);
        h >>= 8;
    }
    // If the top bit is set, add a 0x00 byte to keep it positive
    if data.last().map_or(false, |&b| b & 0x80 != 0) {
        data.push(0x00);
    }
    let mut out = Vec::new();
    out.push(data.len() as u8); // push length
    out.extend_from_slice(&data);
    out
}

/// Encode a u32 as 4 bytes little-endian
pub fn u32_le(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Encode a u64 as 8 bytes little-endian
pub fn u64_le(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Encode an i32 as 4 bytes little-endian
pub fn i32_le(v: i32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Encode a u32 as 4 bytes big-endian
pub fn u32_be(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

/// Encode an i64 as 8 bytes little-endian
pub fn i64_le(v: i64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Parse a 256-bit target from hex string into [u8; 32] (big-endian)
pub fn target_from_hex(hex: &str) -> [u8; 32] {
    let bytes = hex_to_bytes(hex);
    let mut target = [0u8; 32];
    // The hex target is big-endian (most significant byte first)
    // Pad on the left if shorter than 32 bytes
    let offset = 32usize.saturating_sub(bytes.len());
    for (i, &b) in bytes.iter().enumerate() {
        if offset + i < 32 {
            target[offset + i] = b;
        }
    }
    target
}

/// Convert "bits" (nBits compact target) to a 256-bit target [u8; 32] big-endian
pub fn bits_to_target(bits_hex: &str) -> [u8; 32] {
    let bits_bytes = hex_to_bytes(bits_hex);
    if bits_bytes.len() != 4 {
        return [0u8; 32];
    }
    let exponent = bits_bytes[0] as usize;
    let mut target = [0u8; 32];
    if exponent == 0 || exponent > 32 {
        return target;
    }
    // mantissa occupies 3 bytes at position (exponent-3) from the end
    let byte_offset = 32 - exponent;
    if byte_offset < 32 {
        target[byte_offset] = bits_bytes[1];
    }
    if byte_offset + 1 < 32 {
        target[byte_offset + 1] = bits_bytes[2];
    }
    if byte_offset + 2 < 32 {
        target[byte_offset + 2] = bits_bytes[3];
    }
    target
}

/// Compare two 32-byte big-endian values: returns true if hash <= target
pub fn hash_le_target(hash: &[u8; 32], target: &[u8; 32]) -> bool {
    for i in 0..32 {
        if hash[i] < target[i] {
            return true;
        }
        if hash[i] > target[i] {
            return false;
        }
    }
    true // equal
}

/// Convert difficulty to a 256-bit target.
/// For scrypt: maxTarget = 0x0000ffff << 224 (i.e., 0x0000ffff00000000...0)
/// target = maxTarget / difficulty
pub fn difficulty_to_target_scrypt(difficulty: f64) -> [u8; 32] {
    if difficulty <= 0.0 {
        return [0xFF; 32];
    }
    // maxTarget for scrypt = 0x0000FFFF * 2^208 (standard scrypt diff-1)
    // As a f64: 0xFFFF * 2^208
    // target = maxTarget / difficulty
    // We compute this as big integer arithmetic using f64 approximation,
    // then convert to 32 bytes.
    //
    // A simpler approach: maxTarget bytes are [0x00, 0x00, 0xFF, 0xFF, 0, 0, ... 0]
    // We scale by 1/difficulty.

    // Use 256-bit multiplication via u128 pairs
    // maxTarget = 0xFFFF << 224 = 0xFFFF * 2^224
    // In practice, for pool difficulty we use float approximation
    let max_target_f64: f64 = 0xFFFFu64 as f64 * (2.0f64).powi(224);
    let target_f64 = max_target_f64 / difficulty;

    // Convert f64 to 32-byte big-endian
    float_to_target_bytes(target_f64)
}

/// Convert a large float to a 32-byte big-endian target representation.
fn float_to_target_bytes(mut value: f64) -> [u8; 32] {
    let mut target = [0u8; 32];
    if value <= 0.0 || value.is_nan() || value.is_infinite() {
        return target;
    }
    // Fill bytes from most significant to least significant
    for i in 0..32 {
        let byte_val = (value / (256.0f64).powi(31 - i as i32)).floor();
        let byte_clamped = byte_val.min(255.0).max(0.0) as u8;
        target[i] = byte_clamped;
        value -= (byte_clamped as f64) * (256.0f64).powi(31 - i as i32);
        if value < 0.0 {
            value = 0.0;
        }
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverse_hex() {
        assert_eq!(reverse_hex("aabbcc"), "ccbbaa");
    }

    #[test]
    fn test_compact_size() {
        assert_eq!(compact_size(0), vec![0x00]);
        assert_eq!(compact_size(252), vec![0xFC]);
        assert_eq!(compact_size(253), vec![0xFD, 0xFD, 0x00]);
        assert_eq!(compact_size(0x1234), vec![0xFD, 0x34, 0x12]);
    }

    #[test]
    fn test_bip34_height() {
        // Height 1
        let h1 = bip34_height(1);
        assert_eq!(h1, vec![0x01, 0x01]);
        // Height 500000 = 0x07A120
        let h = bip34_height(500000);
        assert_eq!(h, vec![0x03, 0x20, 0xA1, 0x07]);
    }

    #[test]
    fn test_ser_string_be2() {
        // 32 zero bytes
        let zeros = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(ser_string_be2(zeros), zeros);

        // Verify word-ORDER reversal (not byte reversal within words).
        // Input:  word0="aabbccdd" word1="11223344"
        // Output: word1="11223344" word0="aabbccdd"
        assert_eq!(ser_string_be2("aabbccdd11223344"), "11223344aabbccdd");

        // Full 32-byte example: ser_string_be(ser_string_be2(BE)) should equal full byte reversal (LE).
        let be_hash = "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f";
        let le_hash = "6fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000";
        let wr = ser_string_be2(be_hash);
        let le_result = ser_string_be(&wr);
        assert_eq!(le_result, le_hash);
    }

    #[test]
    fn test_hash_le_target() {
        let hash = [0u8; 32];
        let target = [0xFFu8; 32];
        assert!(hash_le_target(&hash, &target));

        let hash2 = [0xFFu8; 32];
        let target2 = [0u8; 32];
        assert!(!hash_le_target(&hash2, &target2));
    }
}

/// Yiimp ser_string_be: reverse bytes within each 4-byte (8 hex char) word.
/// Input: hex string like "aabbccdd11223344"
/// Output: "ddccbbaa44332211"
/// This is NOT the same as full byte reversal - it reverses within each 32-bit word.
pub fn ser_string_be(hex: &str) -> String {
    let mut result = String::with_capacity(hex.len());
    let chars: Vec<char> = hex.chars().collect();
    // Process 8 hex chars (4 bytes) at a time
    for chunk in chars.chunks(8) {
        if chunk.len() == 8 {
            // Reverse byte pairs within this 4-byte word
            result.push(chunk[6]);
            result.push(chunk[7]);
            result.push(chunk[4]);
            result.push(chunk[5]);
            result.push(chunk[2]);
            result.push(chunk[3]);
            result.push(chunk[0]);
            result.push(chunk[1]);
        } else {
            // Partial chunk - just append as-is
            for c in chunk {
                result.push(*c);
            }
        }
    }
    result
}
