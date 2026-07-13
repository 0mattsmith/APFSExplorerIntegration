//! APFS Fletcher-64 checksum (as specified in the Apple File System
//! Reference: 32-bit words, sums modulo `u32::MAX`).
//!
//! Every on-disk object stores this checksum in the first 8 bytes of its
//! block; it is computed over the rest of the object with the checksum
//! field itself treated as zero.

const MOD: u64 = 0xFFFF_FFFF;

/// Compute the checksum of an object whose first 8 bytes are the (ignored)
/// checksum field. `data` is the full object, including those 8 bytes.
pub fn fletcher64_object(data: &[u8]) -> u64 {
    let mut sum1: u64 = 0;
    let mut sum2: u64 = 0;
    // First 8 bytes (the stored checksum) count as zeros.
    sum2 = (sum2 + sum1) % MOD; // word 0 = 0
    sum2 = (sum2 + sum1) % MOD; // word 1 = 0
    let mut i = 8;
    while i + 4 <= data.len() {
        let w = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as u64;
        sum1 = (sum1 + w) % MOD;
        sum2 = (sum2 + sum1) % MOD;
        i += 4;
    }
    let ck_low = MOD - ((sum1 + sum2) % MOD);
    let ck_high = MOD - ((sum1 + ck_low) % MOD);
    (ck_high << 32) | ck_low
}

/// Validate an on-disk object: its stored checksum (first 8 LE bytes) must
/// match the computed value.
pub fn verify_object(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }
    let stored = u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    stored == fletcher64_object(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut block = vec![0u8; 4096];
        for (i, b) in block.iter_mut().enumerate() {
            *b = (i * 7 + 3) as u8;
        }
        let ck = fletcher64_object(&block);
        block[..8].copy_from_slice(&ck.to_le_bytes());
        assert!(verify_object(&block));
        // Any bit flip must be detected.
        block[100] ^= 1;
        assert!(!verify_object(&block));
    }
}
