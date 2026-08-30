//! The length and distance alphabets (RFC 1951 §3.2.5).
//!
//! **Table-driven, copied from the RFC.** Deriving these arithmetically is
//! where implementations get code 284/285 wrong — 285 encodes exactly 258
//! with no extra bits, breaking the pattern the other codes follow.

/// Length codes 257..=285 → base length.
pub const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];

/// Extra bits for each length code. Note the final entry is 0, not 5.
pub const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Distance codes 0..=29 → base distance.
pub const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

pub const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];

/// The code-length alphabet is written in this permuted order so that
/// trailing zeros can be truncated. **Copy it verbatim from the RFC** —
/// every implementation that types it from memory gets it wrong, and the
/// symptom is a stream that inflates to garbage only for certain inputs.
pub const CLCL_ORDER: [usize; 19] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Maps a match length (3..=258) to its code index into `LENGTH_BASE`.
pub fn length_code(len: u16) -> usize {
    debug_assert!((3..=258).contains(&len));
    if len == 258 {
        return 28;
    }
    let mut i = 0;
    while i + 1 < LENGTH_BASE.len() && LENGTH_BASE[i + 1] <= len {
        i += 1;
    }
    i
}

/// Maps a match distance (1..=32768) to its code index into `DIST_BASE`.
pub fn dist_code(dist: u16) -> usize {
    let mut i = 0;
    while i + 1 < DIST_BASE.len() && DIST_BASE[i + 1] <= dist {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_codes_bracket_correctly() {
        assert_eq!(length_code(3), 0);
        assert_eq!(length_code(10), 7);
        assert_eq!(length_code(11), 8); // first code with an extra bit
        assert_eq!(length_code(12), 8);
        assert_eq!(length_code(258), 28);
    }

    /// 285 is the irregular one: exactly 258, no extra bits.
    #[test]
    fn the_last_length_code_is_irregular() {
        assert_eq!(LENGTH_BASE[28], 258);
        assert_eq!(LENGTH_EXTRA[28], 0);
    }

    #[test]
    fn distance_codes_bracket_correctly() {
        assert_eq!(dist_code(1), 0);
        assert_eq!(dist_code(4), 3);
        assert_eq!(dist_code(5), 4);
        assert_eq!(dist_code(32768), 29);
    }

    /// Every length in range must round-trip through its code and extra bits.
    #[test]
    fn every_length_round_trips() {
        for len in 3u16..=258 {
            let c = length_code(len);
            let base = LENGTH_BASE[c];
            let extra = LENGTH_EXTRA[c];
            assert!(base <= len, "len {len} code {c}");
            assert!(len - base < (1u16 << extra) || extra == 0, "len {len}");
        }
    }

    #[test]
    fn every_distance_round_trips() {
        for dist in [1u16, 2, 3, 4, 5, 100, 1000, 4096, 24576, 32768] {
            let c = dist_code(dist);
            let base = DIST_BASE[c];
            assert!(base <= dist);
            assert!(dist - base < (1u16 << DIST_EXTRA[c]));
        }
    }

    #[test]
    fn the_permutation_is_complete() {
        let mut seen = CLCL_ORDER.to_vec();
        seen.sort_unstable();
        assert_eq!(seen, (0..19).collect::<Vec<_>>());
    }
}
