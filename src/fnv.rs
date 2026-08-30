//! FNV-1a, 64-bit. The only hash that touches disk.
//!
//! **Never `DefaultHasher`.** `std`'s is `RandomState`-seeded and explicitly
//! documented as unstable across releases. Persisting one of its outputs
//! produces an index that silently fails to match itself on the next launch,
//! and the symptom — "it re-indexes everything every time" — reads like a
//! caching bug rather than a hashing one. See ARCHITECTURE.md §7.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn hash64(bytes: &[u8]) -> u64 {
    let mut h = OFFSET_BASIS;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published FNV-1a 64 vectors. If these drift, every index on disk is void.
    #[test]
    fn known_vectors() {
        assert_eq!(hash64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn stable_across_calls() {
        assert_eq!(hash64(b"D:/Photos/IMG_0001.jpg"), hash64(b"D:/Photos/IMG_0001.jpg"));
    }
}
