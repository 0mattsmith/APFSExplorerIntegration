//! Directory-record name hashing.
//!
//! APFS directory records on normalization-insensitive volumes carry a
//! 22-bit hash of the (NFD-normalized, optionally case-folded) filename:
//! CRC-32C seeded with `0xFFFFFFFF` over the UTF-32LE code points, without
//! final inversion, masked to 22 bits.
//!
//! Current limitation: code points are case-folded and normalized only for
//! ASCII. Full Unicode NFD + case folding (needed to *generate* correct
//! hashes for non-ASCII names, and for hash-based lookups of them) is
//! tracked in the roadmap; lookups fall back to name comparison, so
//! non-ASCII names still resolve correctly on read.

/// CRC-32C (Castagnoli, reflected 0x82F63B78), no final xor.
pub fn crc32c(seed: u32, data: &[u8]) -> u32 {
    let mut crc = seed;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    crc
}

/// The `name_len_and_hash` field for a hashed drec key: 22-bit name hash in
/// the high bits, name length *including* the NUL terminator in the low 10.
pub fn drec_name_len_and_hash(name: &str, case_fold: bool) -> u32 {
    let mut hash: u32 = 0xFFFF_FFFF;
    for ch in name.chars() {
        let folded = if case_fold {
            // ASCII fold only (see module docs).
            if ch.is_ascii_uppercase() {
                ch.to_ascii_lowercase()
            } else {
                ch
            }
        } else {
            ch
        };
        let cp = folded as u32;
        hash = crc32c(hash, &cp.to_le_bytes());
    }
    let name_len = (name.len() + 1) as u32; // including NUL
    ((hash & 0x3F_FFFF) << 10) | (name_len & 0x3FF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_known_vector() {
        // Standard CRC-32C of "123456789" is 0xE3069283 (with init ~0 and
        // final inversion). Our variant omits the final inversion.
        let crc = crc32c(0xFFFF_FFFF, b"123456789");
        assert_eq!(!crc, 0xE306_9283);
    }
}
