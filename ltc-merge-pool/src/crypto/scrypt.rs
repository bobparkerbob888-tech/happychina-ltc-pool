/// Scrypt hash for Litecoin/scrypt-coin share validation.
/// Parameters: N=1024, r=1, p=1, output=32 bytes.

/// Compute scrypt hash of an 80-byte block header.
/// Returns 32 bytes in internal byte order (little-endian, same as Bitcoin hash convention).
pub fn scrypt_1024_1_1(header: &[u8]) -> [u8; 32] {
    assert!(
        header.len() == 80,
        "scrypt_1024_1_1: header must be exactly 80 bytes, got {}",
        header.len()
    );

    let params = scrypt::Params::new(10, 1, 1, 32).expect("valid scrypt params"); // log2(1024)=10
    let mut output = [0u8; 32];
    scrypt::scrypt(header, header, &params, &mut output).expect("scrypt hash failed");
    output
}

/// Compute scrypt hash and return the result as big-endian (for target comparison).
/// Bitcoin/Litecoin targets are compared big-endian.
pub fn scrypt_hash_be(header: &[u8]) -> [u8; 32] {
    let mut hash = scrypt_1024_1_1(header);
    hash.reverse();
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrypt_hash_length() {
        let header = [0u8; 80];
        let hash = scrypt_1024_1_1(&header);
        assert_eq!(hash.len(), 32);
        // The hash of all-zeros header should be non-zero
        assert!(hash.iter().any(|&b| b != 0));
    }

    #[test]
    #[should_panic(expected = "header must be exactly 80 bytes")]
    fn test_scrypt_wrong_length() {
        let header = [0u8; 40];
        let _ = scrypt_1024_1_1(&header);
    }
}
