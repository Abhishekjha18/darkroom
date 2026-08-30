//! TIFF header and IFD traversal, both byte orders (TIFF 6.0).

/// A TIFF block. **`buf` starts at the TIFF header, which is the origin every
/// offset inside is relative to** — not the segment, not the file. Getting
/// that origin wrong is the single commonest EXIF bug and it produces
/// plausible-looking garbage rather than an error.
pub struct Tiff<'a> {
    pub buf: &'a [u8],
    pub little: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct IfdEntry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// The raw 4 bytes. Whether this is the value or an offset depends on
    /// `count * sizeof(typ)`.
    pub raw: [u8; 4],
}

/// Byte width of each TIFF field type, 0 for types we do not read.
pub fn type_size(typ: u16) -> u32 {
    match typ {
        1 | 2 | 6 | 7 => 1, // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => 2,         // SHORT, SSHORT
        4 | 9 | 11 => 4,    // LONG, SLONG, FLOAT
        5 | 10 | 12 => 8,   // RATIONAL, SRATIONAL, DOUBLE
        _ => 0,
    }
}

impl<'a> Tiff<'a> {
    /// Validates the 8-byte TIFF header and returns the IFD0 offset.
    pub fn new(buf: &'a [u8]) -> Option<(Tiff<'a>, u32)> {
        if buf.len() < 8 {
            return None;
        }
        // Canon writes "II", many Nikons write "MM". Both are common and
        // both are real.
        let little = match &buf[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let t = Tiff { buf, little };
        if t.u16_at(2)? != 42 {
            return None;
        }
        let ifd0 = t.u32_at(4)?;
        Some((t, ifd0))
    }

    pub fn u16_at(&self, off: usize) -> Option<u16> {
        let b = self.buf.get(off..off + 2)?;
        Some(if self.little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    pub fn u32_at(&self, off: usize) -> Option<u32> {
        let b = self.buf.get(off..off + 4)?;
        Some(if self.little {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    }

    pub fn u16_of(&self, raw: &[u8; 4]) -> u16 {
        if self.little {
            u16::from_le_bytes([raw[0], raw[1]])
        } else {
            // **A value that sits inline in a big-endian file is
            // left-justified in those 4 bytes** — a single SHORT is in bytes
            // 0..1, not 2..3. This is the second commonest EXIF bug and it
            // produces values wrong by a factor of 65536.
            u16::from_be_bytes([raw[0], raw[1]])
        }
    }

    pub fn u32_of(&self, raw: &[u8; 4]) -> u32 {
        if self.little {
            u32::from_le_bytes(*raw)
        } else {
            u32::from_be_bytes(*raw)
        }
    }

    /// Reads the entries of the IFD at `off`.
    pub fn entries(&self, off: u32) -> Vec<IfdEntry> {
        let mut out = Vec::new();
        let base = off as usize;
        let Some(count) = self.u16_at(base) else { return out };

        // A malformed file can claim tens of thousands of entries.
        let count = (count as usize).min(512);
        for i in 0..count {
            let e = base + 2 + i * 12;
            let (Some(tag), Some(typ), Some(cnt)) =
                (self.u16_at(e), self.u16_at(e + 2), self.u32_at(e + 4))
            else {
                break;
            };
            let Some(raw) = self.buf.get(e + 8..e + 12) else { break };
            out.push(IfdEntry {
                tag,
                typ,
                count: cnt,
                raw: [raw[0], raw[1], raw[2], raw[3]],
            });
        }
        out
    }

    /// Offset to the next IFD, or `None` at the end of the chain.
    pub fn next_ifd(&self, off: u32) -> Option<u32> {
        let count = self.u16_at(off as usize)? as usize;
        let n = self.u32_at(off as usize + 2 + count.min(512) * 12)?;
        (n != 0).then_some(n)
    }

    /// The bytes of an entry's value, resolving the value-vs-offset rule.
    ///
    /// The last 4 bytes are **the value itself** when
    /// `count * sizeof(type) <= 4`, and an **offset to the value** otherwise.
    /// So a SHORT sits inline; a RATIONAL never does.
    // The inline case borrows from the entry, the offset case from the TIFF
    // buffer. Tying the result to the shorter of the two lets one signature
    // serve both without copying.
    pub fn value_bytes<'e>(&'e self, e: &'e IfdEntry) -> Option<&'e [u8]> {
        let size = type_size(e.typ);
        if size == 0 {
            return None;
        }
        let total = size.checked_mul(e.count)? as usize;
        if total <= 4 {
            return Some(&e.raw[..total.max(1).min(4)]);
        }
        // Every offset is file-supplied, and this is the module most likely
        // to be handed a hostile one. Bounds-check before slicing.
        let off = self.u32_of(&e.raw) as usize;
        self.buf.get(off..off.checked_add(total)?)
    }

    /// An ASCII value, with NUL and space padding trimmed.
    ///
    /// `"Canon\0\0\0"` and `"Canon      "` are the same camera and must
    /// group as one in the UI.
    pub fn ascii(&self, e: &IfdEntry) -> Option<String> {
        let b = self.value_bytes(e)?;
        let s: String = b
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as char)
            .collect();
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// A single unsigned integer from a BYTE, SHORT or LONG entry.
    pub fn uint(&self, e: &IfdEntry) -> Option<u32> {
        match e.typ {
            1 | 7 => Some(e.raw[0] as u32),
            3 => Some(self.u16_of(&e.raw) as u32),
            4 => Some(self.u32_of(&e.raw)),
            _ => None,
        }
    }

    /// A RATIONAL as `(numerator, denominator)`.
    pub fn rational(&self, e: &IfdEntry, index: usize) -> Option<(u32, u32)> {
        if e.typ != 5 && e.typ != 10 {
            return None;
        }
        let b = self.value_bytes(e)?;
        let at = index * 8;
        let n = b.get(at..at + 4)?;
        let d = b.get(at + 4..at + 8)?;
        let to_u32 = |x: &[u8]| {
            if self.little {
                u32::from_le_bytes([x[0], x[1], x[2], x[3]])
            } else {
                u32::from_be_bytes([x[0], x[1], x[2], x[3]])
            }
        };
        Some((to_u32(n), to_u32(d)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal TIFF block with one IFD0 entry.
    fn build(little: bool, tag: u16, typ: u16, count: u32, raw: [u8; 4]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(if little { b"II" } else { b"MM" });
        let u16b = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        b.extend_from_slice(&u16b(42));
        b.extend_from_slice(&u32b(8)); // IFD0 at offset 8
        b.extend_from_slice(&u16b(1)); // one entry
        b.extend_from_slice(&u16b(tag));
        b.extend_from_slice(&u16b(typ));
        b.extend_from_slice(&u32b(count));
        b.extend_from_slice(&raw);
        b.extend_from_slice(&u32b(0)); // no next IFD
        b
    }

    #[test]
    fn reads_both_byte_orders() {
        for little in [true, false] {
            let raw = if little { [0x34, 0x12, 0, 0] } else { [0x12, 0x34, 0, 0] };
            let data = build(little, 0x0112, 3, 1, raw);
            let (t, ifd0) = Tiff::new(&data).unwrap();
            assert_eq!(t.little, little);
            let e = t.entries(ifd0);
            assert_eq!(e.len(), 1);
            assert_eq!(e[0].tag, 0x0112);
            assert_eq!(t.uint(&e[0]), Some(0x1234));
        }
    }

    /// The big-endian left-justification trap, asserted directly.
    #[test]
    fn big_endian_inline_shorts_are_left_justified() {
        let data = build(false, 0x0112, 3, 1, [0x00, 0x06, 0xFF, 0xFF]);
        let (t, ifd0) = Tiff::new(&data).unwrap();
        let e = t.entries(ifd0);
        // Orientation 6, not 6 * 65536 and not 0xFFFF.
        assert_eq!(t.uint(&e[0]), Some(6));
    }

    #[test]
    fn rejects_a_bad_header() {
        assert!(Tiff::new(b"XX\x2a\x00\x08\x00\x00\x00").is_none());
        assert!(Tiff::new(b"II\x00\x00\x08\x00\x00\x00").is_none()); // magic != 42
        assert!(Tiff::new(b"II").is_none());
        assert!(Tiff::new(b"").is_none());
    }

    #[test]
    fn value_vs_offset_rule() {
        // A SHORT (2 bytes) sits inline.
        let data = build(true, 1, 3, 1, [9, 0, 0, 0]);
        let (t, ifd0) = Tiff::new(&data).unwrap();
        let e = t.entries(ifd0);
        assert_eq!(t.value_bytes(&e[0]).unwrap().len(), 2);

        // A RATIONAL (8 bytes) never does: raw is an offset.
        let mut data = build(true, 1, 5, 1, [0, 0, 0, 0]);
        let off = data.len() as u32;
        data[8 + 2 + 8..8 + 2 + 12].copy_from_slice(&off.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&250u32.to_le_bytes());
        let (t, ifd0) = Tiff::new(&data).unwrap();
        let e = t.entries(ifd0);
        assert_eq!(t.rational(&e[0], 0), Some((1, 250)));
    }

    /// An offset pointing 2 GB out of bounds must return None, not panic.
    #[test]
    fn out_of_bounds_offsets_are_refused() {
        let data = build(true, 1, 5, 1, [0xFF, 0xFF, 0xFF, 0x7F]);
        let (t, ifd0) = Tiff::new(&data).unwrap();
        let e = t.entries(ifd0);
        assert!(t.value_bytes(&e[0]).is_none());
        assert!(t.rational(&e[0], 0).is_none());
    }

    #[test]
    fn an_absurd_entry_count_is_capped() {
        let mut data = build(true, 1, 3, 1, [0; 4]);
        data[8] = 0xFF;
        data[9] = 0xFF; // claim 65535 entries
        let (t, _) = Tiff::new(&data).unwrap();
        assert!(t.entries(8).len() <= 512);
    }

    #[test]
    fn ascii_trims_nul_and_space_padding() {
        let mut data = build(true, 0x010F, 2, 12, [0; 4]);
        let off = data.len() as u32;
        data[8 + 2 + 8..8 + 2 + 12].copy_from_slice(&off.to_le_bytes());
        data.extend_from_slice(b"Canon   \0\0\0\0");
        let (t, ifd0) = Tiff::new(&data).unwrap();
        let e = t.entries(ifd0);
        assert_eq!(t.ascii(&e[0]).as_deref(), Some("Canon"));
    }
}
