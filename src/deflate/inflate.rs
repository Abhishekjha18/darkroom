//! The decompressor (RFC 1951).
//!
//! **Built before the compressor, deliberately.** darkroom only *needs*
//! deflate — it writes PNGs, it does not have to read them. But inflate is
//! the simpler half, and the moment it works it becomes the test harness for
//! the compressor: compress a buffer, decompress it here, compare. That loop
//! closes before `gunzip` is ever involved.

use super::bits::BitReader;
use super::huffman::Decoder;
use super::tables::*;
use super::Error;

pub fn inflate(src: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    let mut r = BitReader::new(src);
    let mut out: Vec<u8> = Vec::new();

    loop {
        let final_block = r.bits(1)? == 1;
        match r.bits(2)? {
            0 => stored(&mut r, &mut out, limit)?,
            1 => {
                let lit = Decoder::from_lengths(&super::huffman::fixed_literal_lengths())?;
                let dist = Decoder::from_lengths(&super::huffman::fixed_distance_lengths())?;
                block(&mut r, &mut out, &lit, &dist, limit)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut r)?;
                block(&mut r, &mut out, &lit, &dist, limit)?;
            }
            _ => return Err(Error::BadBlockType),
        }
        if final_block {
            return Ok(out);
        }
    }
}

/// Block type 00. Also the escape hatch: a valid DEFLATE stream can be
/// nothing but stored blocks.
fn stored(r: &mut BitReader, out: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    r.align();
    let mut hdr = [0u8; 4];
    r.bytes(&mut hdr)?;
    let len = u16::from_le_bytes([hdr[0], hdr[1]]);
    let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
    if len != !nlen {
        return Err(Error::BadStoredLength);
    }
    if out.len() + len as usize > limit {
        return Err(Error::OutputTooLarge { limit });
    }
    let start = out.len();
    out.resize(start + len as usize, 0);
    r.bytes(&mut out[start..])?;
    Ok(())
}

/// Reads the dynamic block header: a Huffman table describing two more
/// Huffman tables.
fn dynamic_tables(r: &mut BitReader) -> Result<(Decoder, Decoder), Error> {
    let hlit = r.bits(5)? as usize + 257;
    let hdist = r.bits(5)? as usize + 1;
    let hclen = r.bits(4)? as usize + 4;

    let mut cl_lengths = [0u8; 19];
    for &slot in CLCL_ORDER.iter().take(hclen) {
        cl_lengths[slot] = r.bits(3)? as u8;
    }
    let cl = Decoder::from_lengths(&cl_lengths)?;

    // The literal and distance lengths are encoded as one run, with repeat
    // codes able to straddle the boundary between them.
    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0;
    while i < total {
        let sym = cl.decode(r)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(Error::BadCode); // nothing to repeat
                }
                let prev = lengths[i - 1];
                let n = 3 + r.bits(2)? as usize;
                if i + n > total {
                    return Err(Error::BadCode);
                }
                lengths[i..i + n].fill(prev);
                i += n;
            }
            17 => {
                let n = 3 + r.bits(3)? as usize;
                if i + n > total {
                    return Err(Error::BadCode);
                }
                i += n; // already zero
            }
            18 => {
                let n = 11 + r.bits(7)? as usize;
                if i + n > total {
                    return Err(Error::BadCode);
                }
                i += n;
            }
            _ => return Err(Error::BadCode),
        }
    }

    let lit = Decoder::from_lengths(&lengths[..hlit])?;
    let dist = Decoder::from_lengths(&lengths[hlit..])?;
    Ok((lit, dist))
}

fn block(
    r: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Decoder,
    dist: &Decoder,
    limit: usize,
) -> Result<(), Error> {
    loop {
        let sym = lit.decode(r)?;
        match sym {
            0..=255 => {
                if out.len() >= limit {
                    return Err(Error::OutputTooLarge { limit });
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let idx = sym as usize - 257;
                let len = LENGTH_BASE[idx] as usize + r.bits(LENGTH_EXTRA[idx] as u32)? as usize;

                let dsym = dist.decode(r)? as usize;
                if dsym >= DIST_BASE.len() {
                    return Err(Error::BadCode);
                }
                let d = DIST_BASE[dsym] as usize + r.bits(DIST_EXTRA[dsym] as u32)? as usize;

                // A distance pointing before the start of the output is the
                // classic malformed-stream case; it must be an error, never a
                // wrapping index.
                if d > out.len() || d == 0 {
                    return Err(Error::DistanceTooFar { dist: d, have: out.len() });
                }
                if out.len() + len > limit {
                    return Err(Error::OutputTooLarge { limit });
                }

                // Byte at a time: matches are allowed to overlap the output
                // they are producing (that is how runs are encoded), so a
                // block copy would be wrong.
                let start = out.len() - d;
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
            _ => return Err(Error::BadCode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `printf 'hello' | gzip | xxd` — the deflate payload of "hello",
    /// static-Huffman coded.
    #[test]
    fn inflates_a_known_static_stream() {
        let raw = [0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00];
        assert_eq!(inflate(&raw, 1 << 20).unwrap(), b"hello");
    }

    #[test]
    fn rejects_a_reserved_block_type() {
        // final=1, type=11
        assert!(matches!(inflate(&[0b111], 1 << 20), Err(Error::BadBlockType)));
    }

    #[test]
    fn rejects_truncated_input() {
        assert!(inflate(&[0xCB, 0x48], 1 << 20).is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(inflate(&[], 1 << 20).is_err());
    }

    #[test]
    fn enforces_the_output_limit() {
        let raw = [0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00];
        assert!(matches!(
            inflate(&raw, 2),
            Err(Error::OutputTooLarge { .. })
        ));
    }
}
