//! Reed–Solomon error-correction codewords.
//!
//! **Only the encoder is needed.** No syndromes, no Berlekamp–Massey, no
//! Chien search, no Forney — all of that is *decoding*, and darkroom never
//! reads a QR code. That is what makes this module short, and the honest
//! cost for `STDLIB.md` is that the substitution encodes only.

use super::gf256;

/// The generator polynomial for `n` error-correction codewords:
/// `(x - a^0)(x - a^1)...(x - a^(n-1))`, expanded in GF(256).
pub fn generator(n: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 0..n {
        // Multiply g(x) by (x - a^i).
        let mut next = vec![0u8; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] ^= c; // the x term
            next[j + 1] ^= gf256::mul(c, gf256::exp(i));
        }
        g = next;
    }
    g
}

/// Polynomial long division of the message by the generator; the remainder
/// is the EC codewords. Systematic — the data appears unchanged and the
/// check bytes follow.
pub fn encode(data: &[u8], ec_len: usize) -> Vec<u8> {
    let poly = generator(ec_len);
    let mut rem = vec![0u8; ec_len];

    for &byte in data {
        let factor = byte ^ rem[0];
        rem.rotate_left(1);
        rem[ec_len - 1] = 0;
        for (i, &g) in poly.iter().skip(1).enumerate() {
            rem[i] ^= gf256::mul(g, factor);
        }
    }
    rem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_has_the_right_degree() {
        for n in [7usize, 10, 15, 20] {
            assert_eq!(generator(n).len(), n + 1);
            assert_eq!(generator(n)[0], 1); // monic
        }
    }

    /// The published generator for 7 EC codewords.
    #[test]
    fn generator_matches_the_specification() {
        assert_eq!(generator(7), vec![1, 127, 122, 154, 164, 11, 68, 117]);
    }

    #[test]
    fn generator_for_ten_matches() {
        assert_eq!(
            generator(10),
            vec![1, 216, 194, 159, 111, 199, 94, 95, 113, 157, 193]
        );
    }

    #[test]
    fn encode_produces_the_requested_length() {
        for n in [7usize, 15, 20] {
            assert_eq!(encode(b"hello world", n).len(), n);
        }
    }

    /// The worked example from the QR tutorials: the message
    /// "HELLO WORLD" in 1-M encodes to a known set of EC codewords.
    #[test]
    fn known_codewords() {
        let data = [
            32u8, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
        ];
        let ec = encode(&data, 10);
        assert_eq!(ec, vec![196, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
    }

    #[test]
    fn different_data_gives_different_codewords() {
        assert_ne!(encode(b"http://a", 15), encode(b"http://b", 15));
    }

    #[test]
    fn handles_empty_data() {
        assert_eq!(encode(&[], 15), vec![0u8; 15]);
    }
}
