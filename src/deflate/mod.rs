//! DEFLATE (RFC 1951) and the zlib container (RFC 1950).
//!
//! Replaces `flate2` / `miniz_oxide`. `std` has no compression of any kind —
//! unlike Python's and Go's standard libraries, which both ship zlib. Without
//! this module the thumbnail path does not exist, and without thumbnails
//! darkroom is a directory listing.

pub mod bits;
pub mod compress;
pub mod huffman;
pub mod inflate;
pub mod lz77;
pub mod tables;

use std::fmt;

pub use compress::deflate;
pub use inflate::inflate;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Truncated { at: usize },
    BadBlockType,
    BadStoredLength,
    BadCode,
    BadCodeLength { len: u8 },
    OverSubscribed,
    DistanceTooFar { dist: usize, have: usize },
    OutputTooLarge { limit: usize },
    BadZlibHeader { cmf: u8, flg: u8 },
    ChecksumMismatch { want: u32, got: u32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { at } => write!(f, "truncated deflate stream at byte {at}"),
            Error::BadBlockType => write!(f, "reserved deflate block type"),
            Error::BadStoredLength => write!(f, "stored block LEN/NLEN mismatch"),
            Error::BadCode => write!(f, "invalid huffman code"),
            Error::BadCodeLength { len } => write!(f, "code length {len} exceeds 15"),
            Error::OverSubscribed => write!(f, "over-subscribed huffman table"),
            Error::DistanceTooFar { dist, have } => {
                write!(f, "match distance {dist} exceeds {have} bytes of output")
            }
            Error::OutputTooLarge { limit } => {
                write!(f, "decompressed output exceeds the {limit}-byte cap")
            }
            Error::BadZlibHeader { cmf, flg } => {
                write!(f, "bad zlib header {cmf:#04x} {flg:#04x}")
            }
            Error::ChecksumMismatch { want, got } => {
                write!(f, "adler-32 mismatch: expected {want:#010x}, got {got:#010x}")
            }
        }
    }
}

/// Adler-32 over the *uncompressed* bytes (RFC 1950).
///
/// **Adler-32 is not CRC-32, and PNG needs both** — Adler inside the zlib
/// stream, CRC on every chunk. Confusing them produces a file every decoder
/// rejects with a checksum error and no hint as to which checksum.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    // Chunked so the accumulators cannot overflow before the modulo.
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// Wraps a deflate stream in the zlib container PNG's `IDAT` requires.
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let cmf = 0x78u8; // deflate, 32 KiB window
    let flevel = 2u8; // default compression
    let base = (flevel as u16) << 6;
    // FLG is chosen so that (CMF << 8 | FLG) is a multiple of 31.
    let rem = ((cmf as u16) << 8 | base) % 31;
    let flg = (base + (31 - rem)) as u8;
    debug_assert_eq!(((cmf as u16) << 8 | flg as u16) % 31, 0);

    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    out.push(cmf);
    out.push(flg);
    out.extend_from_slice(&deflate(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

pub fn zlib_decompress(src: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    if src.len() < 6 {
        return Err(Error::Truncated { at: src.len() });
    }
    let (cmf, flg) = (src[0], src[1]);
    if cmf & 0x0F != 8 || ((cmf as u16) << 8 | flg as u16) % 31 != 0 || flg & 0x20 != 0 {
        return Err(Error::BadZlibHeader { cmf, flg });
    }
    let body = &src[2..src.len() - 4];
    let out = inflate(body, limit)?;

    let want = u32::from_be_bytes([
        src[src.len() - 4],
        src[src.len() - 3],
        src[src.len() - 2],
        src[src.len() - 1],
    ]);
    let got = adler32(&out);
    if want != got {
        return Err(Error::ChecksumMismatch { want, got });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published Adler-32 vectors.
    #[test]
    fn adler32_known_vectors() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"abc"), 0x024d_0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn adler32_survives_long_input() {
        // Exercises the chunked accumulator boundary.
        let data = vec![0xFFu8; 100_000];
        let a = adler32(&data);
        assert_ne!(a, 0);
    }

    #[test]
    fn zlib_round_trips() {
        for case in [
            b"".to_vec(),
            b"hello world".to_vec(),
            vec![0u8; 50_000],
            (0..30_000u32).map(|i| (i >> 3) as u8).collect(),
        ] {
            let packed = zlib_compress(&case);
            assert_eq!(zlib_decompress(&packed, 1 << 26).unwrap(), case);
        }
    }

    #[test]
    fn zlib_header_is_a_multiple_of_31() {
        let packed = zlib_compress(b"x");
        assert_eq!(((packed[0] as u16) << 8 | packed[1] as u16) % 31, 0);
        assert_eq!(packed[0], 0x78);
    }

    #[test]
    fn rejects_a_corrupt_zlib_header() {
        let mut packed = zlib_compress(b"hello");
        packed[1] ^= 0xFF;
        assert!(matches!(
            zlib_decompress(&packed, 1 << 20),
            Err(Error::BadZlibHeader { .. })
        ));
    }

    #[test]
    fn detects_a_corrupted_payload() {
        let mut packed = zlib_compress(&vec![7u8; 4096]);
        let n = packed.len();
        packed[n - 5] ^= 0x01; // last byte of the deflate stream
        // Either the stream fails to decode or the checksum catches it.
        // Silently returning wrong bytes is the only unacceptable outcome.
        match zlib_decompress(&packed, 1 << 20) {
            Ok(out) => assert_ne!(out, vec![7u8; 4096]),
            Err(_) => {}
        }
    }
}
