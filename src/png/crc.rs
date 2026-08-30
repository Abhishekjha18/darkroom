//! CRC-32/ISO-HDLC. Replaces `crc32fast`.
//!
//! Genuinely required: without it, no PNG darkroom writes is valid.

/// Polynomial `0xEDB88320` — the reflected form of `0x04C11DB7`.
const POLY: u32 = 0xEDB8_8320;

/// Built at compile time, so there is no initialisation ordering question at
/// all and the table costs nothing at startup.
const TABLE: [u32; 256] = make_table();

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { POLY ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

pub fn crc32(data: &[u8]) -> u32 {
    update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

/// Incremental form, for a CRC spanning several buffers (a chunk's type and
/// its payload, which are not contiguous).
pub fn update(crc: u32, data: &[u8]) -> u32 {
    let mut c = crc;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

pub fn finish(crc: u32) -> u32 {
    crc ^ 0xFFFF_FFFF
}

pub const INIT: u32 = 0xFFFF_FFFF;

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical CRC-32 check value: "123456789" → 0xCBF43926.
    #[test]
    fn known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn incremental_matches_one_shot() {
        let data = b"the quick brown fox jumps over the lazy dog";
        let split = 17;
        let c = update(INIT, &data[..split]);
        let c = update(c, &data[split..]);
        assert_eq!(finish(c), crc32(data));
    }

    #[test]
    fn detects_single_bit_flips() {
        let a = crc32(b"IDAT payload here");
        let b = crc32(b"IDAT payload herf");
        assert_ne!(a, b);
    }
}
