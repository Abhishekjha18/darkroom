//! MSB-first bit reader with `0xFF00` unstuffing.
//!
//! **The opposite order to DEFLATE's reader.** See `deflate/bits.rs` — the
//! two must never be merged.

use crate::image::ImageError;

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
    /// Set when a marker was found in the entropy stream. The MCU loop
    /// decides what it means: `RSTn` is expected, `EOI` means the scan ended
    /// early, anything else is a corrupt file.
    pub marker: Option<u8>,
    /// Offset of the `0xFF` that began `marker`. Progressive images carry
    /// several scans, so the outer parser has to know exactly where the
    /// entropy data stopped in order to read the next header.
    pub marker_at: Option<usize>,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, acc: 0, n: 0, marker: None, marker_at: None }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Refills one byte, applying the stuffing rule.
    ///
    /// Entropy-coded data cannot contain a raw `FF`, so encoders stuff a
    /// zero after every one. Getting this wrong produces images that are
    /// perfect for the first few hundred blocks and then dissolve — which
    /// reads like an IDCT bug and is not one.
    fn fill(&mut self) {
        if self.marker.is_some() {
            // Past a marker the stream is over; feed zeros so a truncated
            // scan yields a partial image instead of an error.
            self.acc = (self.acc << 8) & 0xFFFF_FFFF;
            self.n += 8;
            return;
        }
        let byte = match self.data.get(self.pos) {
            Some(&b) => b,
            None => {
                self.marker = Some(0xD9); // treat EOF as EOI
                self.marker_at = Some(self.data.len());
                self.acc <<= 8;
                self.n += 8;
                return;
            }
        };
        self.pos += 1;

        if byte == 0xFF {
            match self.data.get(self.pos) {
                Some(0x00) => {
                    self.pos += 1; // stuffed byte, the 0xFF is data
                }
                Some(&m) => {
                    self.marker = Some(m);
                    self.marker_at = Some(self.pos - 1);
                    self.pos += 1;
                    self.acc <<= 8;
                    self.n += 8;
                    return;
                }
                None => {
                    self.marker = Some(0xD9);
                    self.marker_at = Some(self.pos - 1);
                    self.acc <<= 8;
                    self.n += 8;
                    return;
                }
            }
        }
        self.acc = (self.acc << 8) | byte as u32;
        self.n += 8;
    }

    pub fn bit(&mut self) -> u32 {
        if self.n == 0 {
            self.fill();
        }
        self.n -= 1;
        (self.acc >> self.n) & 1
    }

    pub fn bits(&mut self, k: u32) -> u32 {
        let mut v = 0;
        for _ in 0..k {
            v = (v << 1) | self.bit();
        }
        v
    }

    /// Discards to the next byte boundary, before a restart marker.
    pub fn align(&mut self) {
        self.n = 0;
        self.acc = 0;
    }

    /// Sign-extends `s` additional bits (T.81 `EXTEND`).
    pub fn receive_extend(&mut self, s: u8) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s as u32) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }

    /// Scans forward for the next restart marker after a desync.
    ///
    /// Restart markers exist precisely so a corrupt stream can resync, so a
    /// mismatch scans forward rather than failing the file — which is what
    /// turns `truncated-scan.jpg` into a partial image instead of an error.
    pub fn resync_to_restart(&mut self) -> bool {
        self.align();
        if let Some(m) = self.marker
            && (0xD0..=0xD7).contains(&m)
        {
            self.marker = None;
            self.marker_at = None;
            return true;
        }
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF {
                let m = self.data[self.pos + 1];
                if (0xD0..=0xD7).contains(&m) {
                    self.pos += 2;
                    self.marker = None;
                    self.marker_at = None;
                    return true;
                }
                if m != 0x00 && m != 0xFF {
                    return false; // a different marker: the scan is over
                }
            }
            self.pos += 1;
        }
        false
    }

    pub fn hit_marker(&self) -> bool {
        self.marker.is_some()
    }
}

/// Errors are only produced by table construction, so the reader itself is
/// infallible by design: it degrades to zeros rather than failing a file.
pub fn _assert_error_type(_: ImageError) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first() {
        let mut r = BitReader::new(&[0b1011_0000]);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 0);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 1);
    }

    #[test]
    fn unstuffs_ff00() {
        // 0xFF 0x00 means a literal 0xFF byte of data.
        let mut r = BitReader::new(&[0xFF, 0x00, 0x0F]);
        assert_eq!(r.bits(8), 0xFF);
        assert_eq!(r.bits(8), 0x0F);
        assert!(!r.hit_marker());
    }

    #[test]
    fn stops_at_a_real_marker() {
        let mut r = BitReader::new(&[0x12, 0xFF, 0xD9]);
        assert_eq!(r.bits(8), 0x12);
        let _ = r.bits(8);
        assert_eq!(r.marker, Some(0xD9));
    }

    #[test]
    fn running_out_of_data_yields_zeros_not_a_panic() {
        let mut r = BitReader::new(&[0xAA]);
        assert_eq!(r.bits(8), 0xAA);
        for _ in 0..64 {
            let _ = r.bit();
        }
        assert!(r.hit_marker());
    }

    #[test]
    fn receive_extend_sign_extends() {
        // For category s, values >= 2^(s-1) are positive and the rest are
        // negative: s=3 covers -7..-4 and 4..7, never 0..3.
        // 0b110 = 6, which is >= 4, so it stays 6.
        let mut r = BitReader::new(&[0b1100_0000]);
        assert_eq!(r.receive_extend(3), 6);
        // 0b011 = 3, below 4, so 3 - 8 + 1 = -4.
        let mut r = BitReader::new(&[0b0110_0000]);
        assert_eq!(r.receive_extend(3), -4);
        // 0b001 = 1 -> 1 - 8 + 1 = -6
        let mut r = BitReader::new(&[0b0010_0000]);
        assert_eq!(r.receive_extend(3), -6);
        // The category's two extremes.
        let mut r = BitReader::new(&[0b1110_0000]);
        assert_eq!(r.receive_extend(3), 7);
        let mut r = BitReader::new(&[0b0000_0000]);
        assert_eq!(r.receive_extend(3), -7);
    }

    #[test]
    fn receive_extend_zero_is_zero() {
        let mut r = BitReader::new(&[0xFF, 0x00]);
        assert_eq!(r.receive_extend(0), 0);
    }

    #[test]
    fn finds_a_restart_marker() {
        let mut r = BitReader::new(&[0x01, 0x02, 0xFF, 0xD2, 0x03]);
        assert!(r.resync_to_restart());
        assert_eq!(r.bits(8), 0x03);
    }
}
