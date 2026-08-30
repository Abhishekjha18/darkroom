//! The compressor. Ship order is **stored → static → dynamic**, and each is
//! independently correct: every one of them produces a stream real `gunzip`
//! accepts.

use super::bits::BitWriter;
use super::huffman::{canonical_codes, code_lengths, fixed_literal_lengths, MAX_BITS};
use super::lz77::{tokenize, Token};
use super::tables::*;

const LIT_SYMS: usize = 288;
const DIST_SYMS: usize = 30;
/// Stored blocks carry a `u16` length, so they cap here.
const MAX_STORED: usize = 65535;

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let tokens = tokenize(data);
    let (lit_freq, dist_freq) = frequencies(&tokens);

    // Cost all three encodings and take the cheapest. The comparison is what
    // makes "stored" an honest escape hatch rather than dead code.
    let stored_bits = stored_cost(data.len());
    let static_bits = static_cost(&lit_freq, &dist_freq);
    let (dyn_bits, dyn_tables) = dynamic_cost(&lit_freq, &dist_freq);

    let mut w = BitWriter::new();
    if stored_bits <= static_bits && stored_bits <= dyn_bits {
        emit_stored(&mut w, data);
    } else if static_bits <= dyn_bits {
        emit_static(&mut w, &tokens);
    } else {
        emit_dynamic(&mut w, &tokens, dyn_tables);
    }
    w.finish()
}

fn frequencies(tokens: &[Token]) -> (Vec<u32>, Vec<u32>) {
    let mut lit = vec![0u32; LIT_SYMS];
    let mut dist = vec![0u32; DIST_SYMS];
    for t in tokens {
        match *t {
            Token::Lit(b) => lit[b as usize] += 1,
            Token::Match { len, dist: d } => {
                lit[257 + length_code(len)] += 1;
                dist[dist_code(d)] += 1;
            }
        }
    }
    lit[256] += 1; // end-of-block, always present
    (lit, dist)
}

fn stored_cost(len: usize) -> u64 {
    let blocks = len.div_ceil(MAX_STORED).max(1) as u64;
    // 3 header bits + padding to a byte + LEN/NLEN, per block
    blocks * (3 + 7 + 32) + len as u64 * 8
}

fn token_bits(lit_freq: &[u32], dist_freq: &[u32], lit_len: &[u8], dist_len: &[u8]) -> u64 {
    let mut bits = 0u64;
    for (sym, &f) in lit_freq.iter().enumerate() {
        if f == 0 {
            continue;
        }
        bits += f as u64 * lit_len[sym] as u64;
        if sym >= 257 {
            bits += f as u64 * LENGTH_EXTRA[sym - 257] as u64;
        }
    }
    for (sym, &f) in dist_freq.iter().enumerate() {
        if f == 0 {
            continue;
        }
        bits += f as u64 * (dist_len[sym] as u64 + DIST_EXTRA[sym] as u64);
    }
    bits
}

fn static_cost(lit_freq: &[u32], dist_freq: &[u32]) -> u64 {
    let lit_len = fixed_literal_lengths();
    let dist_len = vec![5u8; DIST_SYMS];
    3 + token_bits(lit_freq, dist_freq, &lit_len, &dist_len)
}

/// Code lengths for a dynamic block, plus everything needed to write its
/// header.
struct DynTables {
    lit_len: Vec<u8>,
    dist_len: Vec<u8>,
    hlit: usize,
    hdist: usize,
    hclen: usize,
    cl_len: Vec<u8>,
    rle: Vec<(u8, u8, u8)>, // (symbol, extra_bits, extra_value)
}

fn dynamic_cost(lit_freq: &[u32], dist_freq: &[u32]) -> (u64, DynTables) {
    let mut lit_len = code_lengths(lit_freq, MAX_BITS as u8);
    let mut dist_len = code_lengths(dist_freq, MAX_BITS as u8);

    // A table with a single code is incomplete, and some decoders reject it.
    // A second one-bit code fixes that — but it has to be *added beside* the
    // symbol already in use, never assigned to a fixed slot: promoting slots
    // 0 and 1 when the live symbol is, say, 5 leaves three one-bit codes,
    // which is over-subscribed and rejected by every decoder including ours.
    complete_table(&mut lit_len);
    complete_table(&mut dist_len);

    let mut hlit = LIT_SYMS;
    while hlit > 257 && lit_len[hlit - 1] == 0 {
        hlit -= 1;
    }
    let mut hdist = DIST_SYMS;
    while hdist > 1 && dist_len[hdist - 1] == 0 {
        hdist -= 1;
    }

    let mut combined = lit_len[..hlit].to_vec();
    combined.extend_from_slice(&dist_len[..hdist]);
    let rle = rle_encode(&combined);

    let mut cl_freq = vec![0u32; 19];
    for &(sym, _, _) in &rle {
        cl_freq[sym as usize] += 1;
    }
    // The code-length alphabet is itself Huffman-coded, capped at 7 bits.
    let cl_len = code_lengths(&cl_freq, 7);

    let mut hclen = 19;
    while hclen > 4 && cl_len[CLCL_ORDER[hclen - 1]] == 0 {
        hclen -= 1;
    }

    let mut header = 3 + 5 + 5 + 4 + (hclen as u64 * 3);
    for &(sym, extra, _) in &rle {
        header += cl_len[sym as usize] as u64 + extra as u64;
    }

    let total = header + token_bits(lit_freq, dist_freq, &lit_len, &dist_len);
    (total, DynTables { lit_len, dist_len, hlit, hdist, hclen, cl_len, rle })
}

/// Ensures a code table has at least two codes, so it is complete.
///
/// Zero or one used symbol both leave a table a strict decoder can refuse.
/// The existing symbol keeps its slot — it still has to be encodable — and a
/// neighbour is promoted to one bit alongside it.
fn complete_table(lengths: &mut [u8]) {
    let used: Vec<usize> = (0..lengths.len()).filter(|&i| lengths[i] > 0).collect();
    match used.len() {
        0 => {
            lengths[0] = 1;
            lengths[1] = 1;
        }
        1 => {
            let u = used[0];
            lengths[u] = 1;
            lengths[usize::from(u == 0)] = 1;
        }
        _ => {}
    }
}

/// Run-length encodes a code-length sequence using symbols 16, 17 and 18.
fn rle_encode(lengths: &[u8]) -> Vec<(u8, u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let mut run = 1;
        while i + run < lengths.len() && lengths[i + run] == v {
            run += 1;
        }

        if v == 0 {
            while run >= 3 {
                let n = if run >= 11 { run.min(138) } else { run.min(10) };
                if n >= 11 {
                    out.push((18, 7, (n - 11) as u8));
                } else {
                    out.push((17, 3, (n - 3) as u8));
                }
                run -= n;
                i += n;
            }
        } else {
            out.push((v, 0, 0));
            run -= 1;
            i += 1;
            while run >= 3 {
                let n = run.min(6);
                out.push((16, 2, (n - 3) as u8));
                run -= n;
                i += n;
            }
        }
        for _ in 0..run {
            out.push((v, 0, 0));
            i += 1;
        }
    }
    out
}

fn emit_stored(w: &mut BitWriter, data: &[u8]) {
    if data.is_empty() {
        w.bits(1, 1);
        w.bits(0, 2);
        w.align();
        w.raw(&[0, 0, 0xFF, 0xFF]);
        return;
    }
    for (i, chunk) in data.chunks(MAX_STORED).enumerate() {
        let last = (i + 1) * MAX_STORED >= data.len();
        w.bits(last as u32, 1);
        w.bits(0, 2);
        w.align();
        let len = chunk.len() as u16;
        w.raw(&len.to_le_bytes());
        w.raw(&(!len).to_le_bytes());
        w.raw(chunk);
    }
}

fn emit_static(w: &mut BitWriter, tokens: &[Token]) {
    let lit_len = fixed_literal_lengths();
    let lit_code = canonical_codes(&lit_len);
    let dist_len = vec![5u8; DIST_SYMS];
    let dist_code_tbl = canonical_codes(&dist_len);

    w.bits(1, 1);
    w.bits(1, 2);
    write_tokens(w, tokens, &lit_code, &lit_len, &dist_code_tbl, &dist_len);
}

fn emit_dynamic(w: &mut BitWriter, tokens: &[Token], t: DynTables) {
    let lit_code = canonical_codes(&t.lit_len);
    let dist_code_tbl = canonical_codes(&t.dist_len);
    let cl_code = canonical_codes(&t.cl_len);

    w.bits(1, 1);
    w.bits(2, 2);
    w.bits((t.hlit - 257) as u32, 5);
    w.bits((t.hdist - 1) as u32, 5);
    w.bits((t.hclen - 4) as u32, 4);

    for &slot in CLCL_ORDER.iter().take(t.hclen) {
        w.bits(t.cl_len[slot] as u32, 3);
    }
    for &(sym, extra, val) in &t.rle {
        w.huff(cl_code[sym as usize], t.cl_len[sym as usize]);
        if extra > 0 {
            w.bits(val as u32, extra as u32);
        }
    }

    write_tokens(w, tokens, &lit_code, &t.lit_len, &dist_code_tbl, &t.dist_len);
}

fn write_tokens(
    w: &mut BitWriter,
    tokens: &[Token],
    lit_code: &[u16],
    lit_len: &[u8],
    dist_code_tbl: &[u16],
    dist_len: &[u8],
) {
    for t in tokens {
        match *t {
            Token::Lit(b) => w.huff(lit_code[b as usize], lit_len[b as usize]),
            Token::Match { len, dist } => {
                let lc = length_code(len);
                let sym = 257 + lc;
                w.huff(lit_code[sym], lit_len[sym]);
                if LENGTH_EXTRA[lc] > 0 {
                    w.bits((len - LENGTH_BASE[lc]) as u32, LENGTH_EXTRA[lc] as u32);
                }
                let dc = dist_code(dist);
                w.huff(dist_code_tbl[dc], dist_len[dc]);
                if DIST_EXTRA[dc] > 0 {
                    w.bits((dist - DIST_BASE[dc]) as u32, DIST_EXTRA[dc] as u32);
                }
            }
        }
    }
    // **The missing final symbol is the classic bug**, and it presents as
    // `gunzip: unexpected end of file`.
    w.huff(lit_code[256], lit_len[256]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deflate::inflate::inflate;

    fn round_trip(data: &[u8]) {
        let packed = deflate(data);
        let back = inflate(&packed, 1 << 26).expect("own inflate must accept own deflate");
        assert_eq!(back, data, "round trip mismatch on {} bytes", data.len());
    }

    #[test]
    fn round_trips_empty() {
        round_trip(b"");
    }

    #[test]
    fn round_trips_short_strings() {
        for s in ["a", "ab", "hello", "hello hello hello"] {
            round_trip(s.as_bytes());
        }
    }

    #[test]
    fn round_trips_highly_repetitive() {
        round_trip(&vec![0u8; 100_000]);
        round_trip(&b"abcabcabc".repeat(5000));
    }

    #[test]
    fn round_trips_incompressible() {
        let data: Vec<u8> =
            (0..70_000u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
        round_trip(&data);
    }

    #[test]
    fn round_trips_all_byte_values() {
        let data: Vec<u8> = (0..=255u8).cycle().take(9999).collect();
        round_trip(&data);
    }

    #[test]
    fn round_trips_every_small_length() {
        for n in 0..300usize {
            let data: Vec<u8> = (0..n).map(|i| (i % 7) as u8).collect();
            round_trip(&data);
        }
    }

    /// Regression: exactly one distance code in use, and not at slot 0 or 1.
    /// `abcdefghabc` matches at distance 8, which is distance code 5 — the
    /// shape that made the completion guard emit three one-bit codes.
    #[test]
    fn round_trips_a_single_high_distance_code() {
        round_trip(b"abcdefghabc");
        for dist in 1..=40usize {
            let mut data: Vec<u8> = (0..dist).map(|i| (i % 251) as u8).collect();
            let head: Vec<u8> = data[..3.min(data.len())].to_vec();
            data.extend_from_slice(&head);
            round_trip(&data);
        }
    }

    #[test]
    fn every_table_we_emit_is_complete() {
        // If any emitted table were over-subscribed or short, our own
        // decoder would reject it — which is the assertion in round_trip.
        for n in [1usize, 2, 3, 5, 9, 17, 33, 129, 1025] {
            round_trip(&vec![0xABu8; n]);
        }
    }

    #[test]
    fn compresses_repetitive_data_substantially() {
        let data = vec![b'x'; 50_000];
        assert!(deflate(&data).len() < 500);
    }

    #[test]
    fn rle_encodes_zero_runs() {
        let lengths = vec![0u8; 20];
        let rle = rle_encode(&lengths);
        assert_eq!(rle.len(), 1);
        assert_eq!(rle[0].0, 18);
    }

    #[test]
    fn rle_encodes_repeats_of_a_value() {
        let mut lengths = vec![4u8; 7];
        lengths.push(0);
        let rle = rle_encode(&lengths);
        assert_eq!(rle[0], (4, 0, 0));
        assert_eq!(rle[1].0, 16);
    }

    /// Every RLE encoding must reproduce the lengths it came from.
    #[test]
    fn rle_round_trips() {
        let cases: Vec<Vec<u8>> = vec![
            vec![0; 200],
            vec![5; 200],
            (0..100).map(|i| (i % 16) as u8).collect(),
            {
                let mut v = vec![0u8; 50];
                v.extend(vec![7u8; 9]);
                v.extend(vec![0u8; 140]);
                v
            },
        ];
        for lengths in cases {
            let mut out = Vec::new();
            for &(sym, _, val) in &rle_encode(&lengths) {
                match sym {
                    16 => {
                        let prev = *out.last().unwrap();
                        out.extend(std::iter::repeat_n(prev, val as usize + 3));
                    }
                    17 => out.extend(std::iter::repeat_n(0u8, val as usize + 3)),
                    18 => out.extend(std::iter::repeat_n(0u8, val as usize + 11)),
                    v => out.push(v),
                }
            }
            assert_eq!(out, lengths);
        }
    }
}
