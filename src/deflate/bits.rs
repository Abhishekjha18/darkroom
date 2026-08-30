//! LSB-first bit IO, for DEFLATE only.
//!
//! **The opposite order to JPEG's reader**, which is MSB-first. The two are
//! about forty lines each and look almost identical, and sharing them is the
//! single most tempting wrong refactor in the project: the bit orders are
//! incompatible and the failure is a garbage stream two thousand bytes in.
//! They live in separate modules with different type names for that reason.

use super::Error;

pub struct BitReader<'a> {
    src: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        BitReader { src, pos: 0, acc: 0, n: 0 }
    }

    /// Reads `k` bits, least-significant first. `k <= 24`.
    pub fn bits(&mut self, k: u32) -> Result<u32, Error> {
        while self.n < k {
            let byte = *self.src.get(self.pos).ok_or(Error::Truncated { at: self.pos })?;
            self.pos += 1;
            self.acc |= (byte as u32) << self.n;
            self.n += 8;
        }
        let v = self.acc & ((1u32 << k) - 1);
        self.acc >>= k;
        self.n -= k;
        Ok(v)
    }

    /// One bit. Hot path for Huffman decoding, so it avoids the shift dance.
    pub fn bit(&mut self) -> Result<u32, Error> {
        if self.n == 0 {
            let byte = *self.src.get(self.pos).ok_or(Error::Truncated { at: self.pos })?;
            self.pos += 1;
            self.acc = byte as u32;
            self.n = 8;
        }
        let v = self.acc & 1;
        self.acc >>= 1;
        self.n -= 1;
        Ok(v)
    }

    /// Bits still obtainable: what is buffered plus what is unread.
    pub fn available(&self) -> usize {
        self.n as usize + (self.src.len() - self.pos) * 8
    }

    /// Buffers at least `k` bits when the input can supply them.
    fn fill_to(&mut self, k: u32) {
        while self.n < k && self.pos < self.src.len() {
            self.acc |= (self.src[self.pos] as u32) << self.n;
            self.pos += 1;
            self.n += 8;
        }
    }

    /// Reads `k` bits **without consuming them**. Only meaningful when
    /// `available() >= k`; the caller checks.
    pub fn peek(&mut self, k: u32) -> u32 {
        self.fill_to(k);
        self.acc & ((1u32 << k) - 1)
    }

    /// Drops `k` bits previously seen by `peek`.
    pub fn consume(&mut self, k: u32) {
        self.acc >>= k;
        self.n -= k;
    }

    /// Discards bits up to the next byte boundary. Stored blocks begin here.
    pub fn align(&mut self) {
        let drop = self.n % 8;
        self.acc >>= drop;
        self.n -= drop;
    }

    /// Reads whole bytes, only valid immediately after `align`.
    pub fn bytes(&mut self, out: &mut [u8]) -> Result<(), Error> {
        for slot in out.iter_mut() {
            *slot = if self.n >= 8 {
                let b = (self.acc & 0xFF) as u8;
                self.acc >>= 8;
                self.n -= 8;
                b
            } else {
                let b = *self.src.get(self.pos).ok_or(Error::Truncated { at: self.pos })?;
                self.pos += 1;
                b
            };
        }
        Ok(())
    }
}

pub struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter { out: Vec::new(), acc: 0, n: 0 }
    }

    pub fn bits(&mut self, value: u32, k: u32) {
        self.acc |= (value & ((1u32 << k) - 1)) << self.n;
        self.n += k;
        while self.n >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Huffman codes are packed **most-significant bit first**, unlike every
    /// other field in DEFLATE. Reversing here is what reconciles the two.
    pub fn huff(&mut self, code: u16, len: u8) {
        let mut v = 0u32;
        for i in 0..len {
            v |= ((code as u32 >> i) & 1) << (len - 1 - i);
        }
        self.bits(v, len as u32);
    }

    pub fn align(&mut self) {
        if self.n > 0 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc = 0;
            self.n = 0;
        }
    }

    pub fn raw(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.n, 0, "raw bytes require a byte-aligned writer");
        self.out.extend_from_slice(bytes);
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_bit_runs() {
        let mut w = BitWriter::new();
        w.bits(0b101, 3);
        w.bits(0b1111_0000, 8);
        w.bits(1, 1);
        let buf = w.finish();

        let mut r = BitReader::new(&buf);
        assert_eq!(r.bits(3).unwrap(), 0b101);
        assert_eq!(r.bits(8).unwrap(), 0b1111_0000);
        assert_eq!(r.bits(1).unwrap(), 1);
    }

    #[test]
    fn single_bits_match_runs() {
        let mut w = BitWriter::new();
        w.bits(0b1011, 4);
        let buf = w.finish();
        let mut r = BitReader::new(&buf);
        // LSB-first: the low bit of the value comes out first.
        assert_eq!(r.bit().unwrap(), 1);
        assert_eq!(r.bit().unwrap(), 1);
        assert_eq!(r.bit().unwrap(), 0);
        assert_eq!(r.bit().unwrap(), 1);
    }

    #[test]
    fn huffman_codes_are_written_msb_first() {
        let mut w = BitWriter::new();
        w.huff(0b110, 3); // must emit 1,1,0 in that order
        let buf = w.finish();
        let mut r = BitReader::new(&buf);
        assert_eq!(r.bit().unwrap(), 1);
        assert_eq!(r.bit().unwrap(), 1);
        assert_eq!(r.bit().unwrap(), 0);
    }

    #[test]
    fn align_skips_to_byte_boundary() {
        let mut w = BitWriter::new();
        w.bits(1, 1);
        w.align();
        w.raw(&[0xAB, 0xCD]);
        let buf = w.finish();

        let mut r = BitReader::new(&buf);
        assert_eq!(r.bit().unwrap(), 1);
        r.align();
        let mut got = [0u8; 2];
        r.bytes(&mut got).unwrap();
        assert_eq!(got, [0xAB, 0xCD]);
    }

    #[test]
    fn running_off_the_end_is_an_error_not_a_panic() {
        let mut r = BitReader::new(&[0x01]);
        assert!(r.bits(8).is_ok());
        assert!(matches!(r.bits(8), Err(Error::Truncated { .. })));
    }
}
