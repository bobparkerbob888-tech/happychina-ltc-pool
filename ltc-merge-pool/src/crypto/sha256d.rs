/// Double SHA-256 hashing (SHA256(SHA256(data)))

use sha2::{Digest, Sha256};

/// Compute SHA256d (double SHA-256) of the input data.
/// Returns 32 bytes.
pub fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    let second = Sha256::digest(first);
    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

/// Compute a single SHA-256 of the input data.
/// Returns 32 bytes.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let hash = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hash);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256d_empty() {
        // SHA256d of empty input is a well-known value
        let result = sha256d(&[]);
        let hex = hex::encode(result);
        assert_eq!(
            hex,
            "5df6e0e2761359d30a8275058e299fcc0381534545f55cf43e41983f5d4c9456"
        );
    }

    #[test]
    fn test_sha256_empty() {
        let result = sha256(&[]);
        let hex = hex::encode(result);
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
