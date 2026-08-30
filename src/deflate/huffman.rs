//! Canonical Huffman construction and decoding for DEFLATE (RFC 1951 §3.2).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::Error;
use super::bits::BitReader;

/// RFC 1951 caps code lengths at 15 bits.
pub const MAX_BITS: usize = 15;

/// Decoding side: counts per length plus symbols in canonical order.
///
/// No tree nodes and no pointer chasing — this is the form the specification
/// itself describes, and it makes a malformed table detectable at
/// *construction* time rather than as a null pointer twenty thousand codes
/// later.
/// Width of the direct-lookup table. Nine bits covers the overwhelming
/// majority of real codes in one indexed read.
const FAST_BITS: u32 = 9;
const FAST_SIZE: usize = 1 << FAST_BITS;
/// No code of `FAST_BITS` or fewer matches this slot.
const FAST_MISS: u16 = u16::MAX;

pub struct Decoder {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
    /// `(symbol << 4) | length`, or `FAST_MISS`. Indexed by the next
    /// `FAST_BITS` bits of the stream as they arrive — which, because
    /// DEFLATE packs Huffman codes MSB-first inside an LSB-first stream, is
    /// the code *reversed*.
    fast: Vec<u16>,
}

/// Reverses the low `len` bits of `code`.
fn reverse(code: u16, len: u8) -> u16 {
    let mut out = 0u16;
    for i in 0..len {
        out |= ((code >> i) & 1) << (len - 1 - i);
    }
    out
}

impl Decoder {
    pub fn from_lengths(lengths: &[u8]) -> Result<Decoder, Error> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &l in lengths {
            if l as usize > MAX_BITS {
                return Err(Error::BadCodeLength { len: l });
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;

        // Over-subscription check. A table claiming more codes than the bit
        // length can address is malformed, and catching it here is what stops
        // the decoder looping on garbage.
        let mut left = 1i32;
        for len in 1..=MAX_BITS {
            left <<= 1;
            left -= counts[len] as i32;
            if left < 0 {
                return Err(Error::OverSubscribed);
            }
        }

        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }

        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offsets[l as usize] as usize] = sym as u16;
                offsets[l as usize] += 1;
            }
        }

        // Direct-lookup table for short codes. Every longer code falls
        // through to the bit-at-a-time path, which stays the reference
        // implementation and the only path near end-of-input.
        let codes = canonical_codes(lengths);
        let mut fast = vec![FAST_MISS; FAST_SIZE];
        for (sym, &l) in lengths.iter().enumerate() {
            if l == 0 || l as u32 > FAST_BITS {
                continue;
            }
            let base = reverse(codes[sym], l) as usize;
            let entry = ((sym as u16) << 4) | l as u16;
            // Every combination of the bits above the code maps to the same
            // symbol, because those bits belong to whatever comes next.
            let step = 1usize << l;
            let mut slot = base;
            while slot < FAST_SIZE {
                fast[slot] = entry;
                slot += step;
            }
        }

        Ok(Decoder { counts, symbols, fast })
    }

    /// Reads one symbol.
    pub fn decode(&self, r: &mut BitReader) -> Result<u16, Error> {
        // Only safe when the whole window is really present; near the end of
        // the stream the slow path reports truncation properly instead of
        // reading padding.
        if r.available() >= FAST_BITS as usize {
            let entry = self.fast[r.peek(FAST_BITS) as usize];
            if entry != FAST_MISS {
                r.consume((entry & 0xF) as u32);
                return Ok(entry >> 4);
            }
        }
        self.decode_slow(r)
    }

    /// One bit at a time, comparing against the canonical first-code of each
    /// length.
    fn decode_slow(&self, r: &mut BitReader) -> Result<u16, Error> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=MAX_BITS {
            code |= r.bit()? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::BadCode)
    }
}

/// Assigns canonical codes to a set of lengths (RFC 1951 §3.2.2).
///
/// Codes are returned **unreversed**; `BitWriter::huff` reverses them on the
/// way out, because Huffman codes are the one field DEFLATE packs MSB-first.
pub fn canonical_codes(lengths: &[u8]) -> Vec<u16> {
    let mut bl_count = [0u16; MAX_BITS + 1];
    for &l in lengths {
        bl_count[l as usize] += 1;
    }
    bl_count[0] = 0;

    let mut next = [0u16; MAX_BITS + 2];
    let mut code = 0u16;
    for bits in 1..=MAX_BITS {
        code = (code + bl_count[bits - 1]) << 1;
        next[bits] = code;
    }

    let mut codes = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l != 0 {
            codes[sym] = next[l as usize];
            next[l as usize] += 1;
        }
    }
    codes
}

/// Optimal code lengths for the given symbol frequencies, capped at
/// `max_bits`.
///
/// When the natural tree is deeper than the cap, frequencies are halved and
/// the tree rebuilt. That converges quickly and keeps the result a valid
/// prefix code, which package-merge would also give but in far more lines.
pub fn code_lengths(freqs: &[u32], max_bits: u8) -> Vec<u8> {
    let n = freqs.len();
    let mut lengths = vec![0u8; n];
    let used: Vec<usize> = (0..n).filter(|&i| freqs[i] > 0).collect();

    match used.len() {
        0 => return lengths,
        // A single symbol still needs one bit: a zero-length code cannot be
        // written, and decoders reject a one-symbol table with length 0.
        1 => {
            lengths[used[0]] = 1;
            return lengths;
        }
        _ => {}
    }

    let mut scaled: Vec<u64> = freqs.iter().map(|&f| f as u64).collect();

    loop {
        struct Node {
            left: i32,
            right: i32,
        }
        let mut nodes: Vec<Node> = (0..n).map(|_| Node { left: -1, right: -1 }).collect();
        // (freq, id) ordering makes ties deterministic, so two runs over the
        // same image produce the same bytes.
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> =
            used.iter().map(|&i| Reverse((scaled[i], i))).collect();

        while heap.len() > 1 {
            let Reverse((f1, i1)) = heap.pop().unwrap();
            let Reverse((f2, i2)) = heap.pop().unwrap();
            let id = nodes.len();
            nodes.push(Node { left: i1 as i32, right: i2 as i32 });
            heap.push(Reverse((f1 + f2, id)));
        }
        let root = heap.pop().unwrap().0.1;

        for l in lengths.iter_mut() {
            *l = 0;
        }
        let mut deepest = 0u8;
        let mut stack = vec![(root, 0u8)];
        while let Some((id, depth)) = stack.pop() {
            let node = &nodes[id];
            if node.left < 0 {
                lengths[id] = depth.max(1);
                deepest = deepest.max(lengths[id]);
            } else {
                stack.push((node.left as usize, depth + 1));
                stack.push((node.right as usize, depth + 1));
            }
        }

        if deepest <= max_bits {
            return lengths;
        }
        for f in scaled.iter_mut() {
            if *f > 0 {
                *f = (*f + 1) / 2;
            }
        }
    }
}

/// The fixed literal/length table: 0–143 are 8 bits, 144–255 are 9,
/// 256–279 are 7, 280–287 are 8. Copied from the RFC rather than derived.
pub fn fixed_literal_lengths() -> Vec<u8> {
    let mut l = vec![8u8; 288];
    l[144..256].fill(9);
    l[256..280].fill(7);
    l
}

/// Fixed distance codes are all 5 bits.
pub fn fixed_distance_lengths() -> Vec<u8> {
    vec![5u8; 30]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deflate::bits::BitWriter;

    #[test]
    fn canonical_codes_match_the_rfc_example() {
        // RFC 1951 §3.2.2: lengths 2,1,3,3 over symbols A,B,C,D give
        // A=10, B=0, C=110, D=111.
        let lengths = [2u8, 1, 3, 3];
        let codes = canonical_codes(&lengths);
        assert_eq!(codes[0], 0b10);
        assert_eq!(codes[1], 0b0);
        assert_eq!(codes[2], 0b110);
        assert_eq!(codes[3], 0b111);
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let lengths = [2u8, 1, 3, 3];
        let codes = canonical_codes(&lengths);
        let dec = Decoder::from_lengths(&lengths).unwrap();

        let mut w = BitWriter::new();
        for sym in [1usize, 0, 3, 2, 1] {
            w.huff(codes[sym], lengths[sym]);
        }
        let buf = w.finish();

        let mut r = BitReader::new(&buf);
        let got: Vec<u16> = (0..5).map(|_| dec.decode(&mut r).unwrap()).collect();
        assert_eq!(got, [1, 0, 3, 2, 1]);
    }

    #[test]
    fn rejects_an_oversubscribed_table() {
        // Three symbols of length 1 cannot exist.
        assert!(matches!(
            Decoder::from_lengths(&[1, 1, 1]),
            Err(Error::OverSubscribed)
        ));
    }

    #[test]
    fn rejects_a_length_beyond_15() {
        assert!(Decoder::from_lengths(&[16]).is_err());
    }

    #[test]
    fn lengths_respect_the_cap() {
        // A brutally skewed distribution would naturally exceed 15 bits.
        let mut freqs = vec![1u32; 300];
        for i in 0..40 {
            freqs[i] = 1 << i.min(30);
        }
        let lengths = code_lengths(&freqs, MAX_BITS as u8);
        assert!(lengths.iter().all(|&l| l as usize <= MAX_BITS));
        // Still a valid prefix code.
        assert!(Decoder::from_lengths(&lengths).is_ok());
    }

    #[test]
    fn a_single_used_symbol_gets_one_bit() {
        let mut freqs = vec![0u32; 10];
        freqs[3] = 99;
        let lengths = code_lengths(&freqs, 15);
        assert_eq!(lengths[3], 1);
    }

    /// The lookup table is an optimisation, so it must be indistinguishable
    /// from the reference path on every symbol.
    #[test]
    fn fast_and_slow_paths_agree() {
        let lengths = fixed_literal_lengths();
        let codes = canonical_codes(&lengths);
        let dec = Decoder::from_lengths(&lengths).unwrap();

        let syms: Vec<usize> = (0..288).chain([0, 255, 256, 279, 280, 287]).collect();
        let mut w = BitWriter::new();
        for &s in &syms {
            w.huff(codes[s], lengths[s]);
        }
        // Padding so the fast path stays enabled through the last symbol.
        for _ in 0..8 {
            w.bits(0, 8);
        }
        let buf = w.finish();

        let mut fast = BitReader::new(&buf);
        let mut slow = BitReader::new(&buf);
        for &s in &syms {
            let a = dec.decode(&mut fast).unwrap();
            let b = dec.decode_slow(&mut slow).unwrap();
            assert_eq!(a, b, "paths disagree on symbol {s}");
            assert_eq!(a as usize, s);
        }
    }

    #[test]
    fn reversal_is_its_own_inverse() {
        for len in 1..=9u8 {
            for code in 0..(1u16 << len) {
                assert_eq!(reverse(reverse(code, len), len), code);
            }
        }
    }

    #[test]
    fn fixed_tables_are_valid() {
        assert!(Decoder::from_lengths(&fixed_literal_lengths()).is_ok());
        assert!(Decoder::from_lengths(&fixed_distance_lengths()).is_ok());
    }
}
