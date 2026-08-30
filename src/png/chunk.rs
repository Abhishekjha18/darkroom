//! PNG chunk framing (RFC 2083 §5).
//!
//! **Two traps in one sentence:** the length field excludes the type and the
//! CRC; the CRC includes the type. Both are easy to get backwards and both
//! produce "not a PNG" from every reader.

use super::crc;
use crate::image::ImageError;

pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub fn write(out: &mut Vec<u8>, ctype: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(ctype);
    out.extend_from_slice(payload);

    let c = crc::update(crc::INIT, ctype);
    let c = crc::update(c, payload);
    out.extend_from_slice(&crc::finish(c).to_be_bytes());
}

pub struct Chunk<'a> {
    pub ctype: [u8; 4],
    pub payload: &'a [u8],
    pub at: usize,
}

impl Chunk<'_> {
    /// A lowercase first letter means ancillary: safe to skip. That is what
    /// the case bit is for.
    pub fn is_critical(&self) -> bool {
        self.ctype[0].is_ascii_uppercase()
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Reader<'a>, ImageError> {
        if data.len() < 8 {
            return Err(ImageError::Truncated { at: 0, expected: "PNG signature" });
        }
        if data[..8] != SIGNATURE {
            return Err(ImageError::NotThisFormat);
        }
        Ok(Reader { data, pos: 8 })
    }

    /// Returns the next chunk, verifying its CRC.
    pub fn next(&mut self) -> Result<Option<Chunk<'a>>, ImageError> {
        if self.pos == self.data.len() {
            return Ok(None);
        }
        let at = self.pos;
        if self.pos + 8 > self.data.len() {
            return Err(ImageError::Truncated { at, expected: "chunk header" });
        }
        let len = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]) as usize;

        // Guard before it is used as a slice bound.
        if len > self.data.len() {
            return Err(ImageError::BadField { at, field: "chunk length", value: len as u32 });
        }
        let ctype = [
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ];
        let body = self.pos + 8;
        if body + len + 4 > self.data.len() {
            return Err(ImageError::Truncated { at, expected: "chunk payload" });
        }
        let payload = &self.data[body..body + len];

        let want = u32::from_be_bytes([
            self.data[body + len],
            self.data[body + len + 1],
            self.data[body + len + 2],
            self.data[body + len + 3],
        ]);
        let c = crc::update(crc::INIT, &ctype);
        let got = crc::finish(crc::update(c, payload));
        if want != got {
            return Err(ImageError::BadField {
                at,
                field: chunk_name(&ctype),
                value: got,
            });
        }

        self.pos = body + len + 4;
        Ok(Some(Chunk { ctype, payload, at }))
    }
}

/// Names the chunk in an error rather than printing raw bytes.
fn chunk_name(ctype: &[u8; 4]) -> &'static str {
    match ctype {
        b"IHDR" => "IHDR CRC",
        b"PLTE" => "PLTE CRC",
        b"IDAT" => "IDAT CRC",
        b"IEND" => "IEND CRC",
        b"tRNS" => "tRNS CRC",
        _ => "chunk CRC",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(ctype: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        write(&mut out, ctype, payload);
        out
    }

    #[test]
    fn writes_and_reads_a_chunk() {
        let buf = framed(b"IHDR", &[1, 2, 3, 4]);
        let mut r = Reader::new(&buf).unwrap();
        let c = r.next().unwrap().unwrap();
        assert_eq!(&c.ctype, b"IHDR");
        assert_eq!(c.payload, &[1, 2, 3, 4]);
        assert!(r.next().unwrap().is_none());
    }

    #[test]
    fn length_excludes_type_and_crc() {
        let buf = framed(b"IDAT", &[9; 10]);
        // 8 signature + 4 length + 4 type + 10 payload + 4 crc
        assert_eq!(buf.len(), 30);
        assert_eq!(&buf[8..12], &10u32.to_be_bytes());
    }

    #[test]
    fn rejects_a_bad_crc() {
        let mut buf = framed(b"IDAT", &[1, 2, 3]);
        let n = buf.len();
        buf[n - 1] ^= 0xFF;
        let mut r = Reader::new(&buf).unwrap();
        assert!(matches!(r.next(), Err(ImageError::BadField { .. })));
    }

    #[test]
    fn rejects_a_bad_signature() {
        let mut buf = framed(b"IHDR", &[0]);
        buf[1] = b'X';
        assert!(matches!(Reader::new(&buf), Err(ImageError::NotThisFormat)));
    }

    #[test]
    fn rejects_a_truncated_payload() {
        let mut buf = framed(b"IDAT", &[7; 20]);
        buf.truncate(buf.len() - 5);
        let mut r = Reader::new(&buf).unwrap();
        assert!(matches!(r.next(), Err(ImageError::Truncated { .. })));
    }

    /// A length field of 4 GB must not become a slice bound.
    #[test]
    fn rejects_an_absurd_length_without_panicking() {
        let mut buf = SIGNATURE.to_vec();
        buf.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        buf.extend_from_slice(b"IDAT");
        buf.extend_from_slice(&[0; 8]);
        let mut r = Reader::new(&buf).unwrap();
        assert!(r.next().is_err());
    }

    #[test]
    fn recognises_ancillary_chunks() {
        let buf = framed(b"tEXt", b"hi");
        let mut r = Reader::new(&buf).unwrap();
        assert!(!r.next().unwrap().unwrap().is_critical());
    }
}
