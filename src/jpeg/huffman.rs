//! Canonical Huffman tables from BITS/HUFFVAL (T.81 Annex C).
//!
//! **Validation happens at construction, not at use.** A malformed table
//! detected here is one specific diagnostic; the same table detected lazily
//! is a null lookup twenty thousand blocks later.

use super::bits::BitReader;
use crate::image::ImageError;

pub struct Table {
    /// Indexed by code length 1..=16. `max_code[l] == -1` means "no codes of
    /// this length".
    min_code: [i32; 17],
    max_code: [i32; 17],
    val_ptr: [i32; 17],
    values: Vec<u8>,
}

impl Table {
    pub fn build(bits: &[u8; 16], values: Vec<u8>, at: usize) -> Result<Table, ImageError> {
        let total: usize = bits.iter().map(|&b| b as usize).sum();

        // The spike's `truncated DHT values` diagnostic comes from exactly
        // this check.
        if total != values.len() {
            return Err(ImageError::Truncated { at, expected: "DHT values" });
        }
        if total > 256 {
            return Err(ImageError::BadField { at, field: "DHT code count", value: total as u32 });
        }
        if total == 0 {
            return Err(ImageError::BadField { at, field: "empty DHT", value: 0 });
        }

        let mut min_code = [0i32; 17];
        let mut max_code = [-1i32; 17];
        let mut val_ptr = [0i32; 17];

        let mut code = 0i32;
        let mut k = 0i32;
        for l in 1..=16usize {
            let n = bits[l - 1] as i32;
            if n == 0 {
                max_code[l] = -1;
                code <<= 1;
                continue;
            }
            val_ptr[l] = k;
            min_code[l] = code;
            code += n;
            k += n;
            max_code[l] = code - 1;

            // An over-subscribed table: more codes than this length can hold.
            // Rejecting here is what stops the decoder looping on garbage.
            if code > (1 << l) {
                return Err(ImageError::BadField {
                    at,
                    field: "over-subscribed DHT",
                    value: l as u32,
                });
            }
            code <<= 1;
        }

        Ok(Table { min_code, max_code, val_ptr, values })
    }

    /// Reads one symbol. Bails past length 16 rather than looping.
    pub fn decode(&self, r: &mut BitReader) -> Result<u8, ImageError> {
        let mut code = r.bit() as i32;
        for l in 1..=16usize {
            if self.max_code[l] >= 0 && code <= self.max_code[l] {
                let idx = self.val_ptr[l] + code - self.min_code[l];
                return self
                    .values
                    .get(idx as usize)
                    .copied()
                    .ok_or(ImageError::BadField { at: r.pos(), field: "huffman index", value: idx as u32 });
            }
            code = (code << 1) | r.bit() as i32;
        }
        Err(ImageError::BadField { at: r.pos(), field: "huffman code length", value: 17 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two symbols: one 1-bit code, one 2-bit code.
    fn simple() -> Table {
        let mut bits = [0u8; 16];
        bits[0] = 1; // one code of length 1
        bits[1] = 1; // one code of length 2
        Table::build(&bits, vec![0xAA, 0xBB], 0).unwrap()
    }

    #[test]
    fn decodes_canonical_codes() {
        let t = simple();
        // code 0 -> 0xAA ; code 10 -> 0xBB
        let mut r = BitReader::new(&[0b0100_0000]);
        assert_eq!(t.decode(&mut r).unwrap(), 0xAA);
        assert_eq!(t.decode(&mut r).unwrap(), 0xBB);
    }

    #[test]
    fn rejects_a_values_length_mismatch() {
        let mut bits = [0u8; 16];
        bits[0] = 2;
        assert!(matches!(
            Table::build(&bits, vec![1], 0x2A1),
            Err(ImageError::Truncated { at: 0x2A1, expected: "DHT values" })
        ));
    }

    #[test]
    fn rejects_an_oversubscribed_table() {
        let mut bits = [0u8; 16];
        bits[0] = 3; // three codes of length 1 cannot exist
        assert!(Table::build(&bits, vec![1, 2, 3], 0).is_err());
    }

    #[test]
    fn rejects_an_empty_table() {
        assert!(Table::build(&[0u8; 16], vec![], 0).is_err());
    }

    #[test]
    fn rejects_more_than_256_codes() {
        let mut bits = [0u8; 16];
        bits[15] = 255;
        bits[14] = 255;
        assert!(Table::build(&bits, vec![0; 510], 0).is_err());
    }

    /// A 16-deep table is legal and must decode.
    #[test]
    fn handles_maximum_code_length() {
        let mut bits = [0u8; 16];
        bits[15] = 2;
        let t = Table::build(&bits, vec![0x11, 0x22], 0).unwrap();
        let mut r = BitReader::new(&[0x00, 0x00, 0xFF, 0x00]);
        assert_eq!(t.decode(&mut r).unwrap(), 0x11);
    }
}
