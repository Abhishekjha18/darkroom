//! The on-disk record layout, and the checked cursors that read it.
//!
//! **Every read goes through a bounds-checked cursor**, never through
//! `&bytes[i..j]`. A truncated index is detected at the record that runs off
//! the end rather than by reading garbage into a `u64`.

use std::fmt;
use std::path::PathBuf;

use crate::catalog::{Entry, EntryState};
use crate::exif::{DateSource, Meta};
use crate::phash::Sig;
use crate::probe::Format;

#[derive(Debug, PartialEq, Eq)]
pub enum IndexError {
    NotAnIndex,
    VersionMismatch { found: u32, expected: u32 },
    Truncated { at: usize },
    BadField(&'static str),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::NotAnIndex => write!(f, "not a darkroom index"),
            IndexError::VersionMismatch { found, expected } => {
                write!(f, "index version {found}, expected {expected}")
            }
            IndexError::Truncated { at } => write!(f, "index truncated at byte {at}"),
            IndexError::BadField(what) => write!(f, "bad index field: {what}"),
        }
    }
}

// ------------------------------------------------------------------ write

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    /// Strings are length-prefixed, never NUL-terminated: a path may
    /// legitimately contain anything except a NUL, and on Windows the lossy
    /// conversion can produce replacement characters.
    pub fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u16(b.len().min(u16::MAX as usize) as u16);
        self.buf.extend_from_slice(&b[..b.len().min(u16::MAX as usize)]);
    }
}

// ------------------------------------------------------------------- read

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], IndexError> {
        let end = self.pos.checked_add(n).ok_or(IndexError::Truncated { at: self.pos })?;
        let s = self.buf.get(self.pos..end).ok_or(IndexError::Truncated { at: self.pos })?;
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, IndexError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, IndexError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self) -> Result<u32, IndexError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn u64(&mut self) -> Result<u64, IndexError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64, IndexError> {
        Ok(self.u64()? as i64)
    }
    pub fn f64(&mut self) -> Result<f64, IndexError> {
        Ok(f64::from_bits(self.u64()?))
    }
    pub fn str(&mut self) -> Result<String, IndexError> {
        let n = self.u16()? as usize;
        let b = self.take(n)?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }
    pub fn slice(&mut self, n: usize) -> Result<&'a [u8], IndexError> {
        self.take(n)
    }
}

// --------------------------------------------------------------- entries

/// Presence bits for the optional `Meta` fields.
mod present {
    pub const TAKEN: u16 = 1 << 0;
    pub const MAKE: u16 = 1 << 1;
    pub const MODEL: u16 = 1 << 2;
    pub const LENS: u16 = 1 << 3;
    pub const ISO: u16 = 1 << 4;
    pub const EXPOSURE: u16 = 1 << 5;
    pub const FNUMBER: u16 = 1 << 6;
    pub const GPS: u16 = 1 << 7;
    pub const DIMS: u16 = 1 << 8;
    pub const SIG: u16 = 1 << 9;
    pub const THUMB: u16 = 1 << 10;
}

pub fn format_code(f: Format) -> u8 {
    match f {
        Format::Jpeg => 0,
        Format::Png => 1,
        Format::Gif => 2,
        Format::Heic => 3,
    }
}

pub fn format_from(code: u8) -> Result<Format, IndexError> {
    Ok(match code {
        0 => Format::Jpeg,
        1 => Format::Png,
        2 => Format::Gif,
        3 => Format::Heic,
        _ => return Err(IndexError::BadField("format")),
    })
}

fn source_code(s: DateSource) -> u8 {
    match s {
        DateSource::Original => 0,
        DateSource::Digitized => 1,
        DateSource::ModifiedTag => 2,
        DateSource::FileTime => 3,
    }
}

fn source_from(c: u8) -> DateSource {
    match c {
        0 => DateSource::Original,
        1 => DateSource::Digitized,
        2 => DateSource::ModifiedTag,
        _ => DateSource::FileTime,
    }
}

/// Encodes one entry. `thumb_off` addresses the concatenated thumbnail blob.
pub fn encode(e: &Entry, thumb_off: u64) -> Vec<u8> {
    let mut w = Writer::default();
    w.u64(e.id);
    w.str(&e.path.to_string_lossy());
    w.str(&e.rel);
    w.u64(e.bytes);
    w.i64(e.mtime);
    w.u8(format_code(e.format));

    match &e.state {
        EntryState::Ok => {
            w.u8(0);
            w.str("");
        }
        EntryState::NoPreview { reason } => {
            w.u8(1);
            w.str(reason);
        }
        EntryState::Failed { reason } => {
            w.u8(2);
            w.str(reason);
        }
    }

    let m = &e.meta;
    let mut mask = 0u16;
    if m.taken.is_some() {
        mask |= present::TAKEN;
    }
    if m.make.is_some() {
        mask |= present::MAKE;
    }
    if m.model.is_some() {
        mask |= present::MODEL;
    }
    if m.lens.is_some() {
        mask |= present::LENS;
    }
    if m.iso.is_some() {
        mask |= present::ISO;
    }
    if m.exposure.is_some() {
        mask |= present::EXPOSURE;
    }
    if m.f_number.is_some() {
        mask |= present::FNUMBER;
    }
    if m.gps.is_some() {
        mask |= present::GPS;
    }
    if e.dims.is_some() {
        mask |= present::DIMS;
    }
    if e.sig.is_some() {
        mask |= present::SIG;
    }
    if e.thumb.is_some() {
        mask |= present::THUMB;
    }
    w.u16(mask);
    w.u8(m.orientation);
    w.u8(source_code(m.date_source));

    if let Some(v) = m.taken {
        w.i64(v);
    }
    if let Some(s) = &m.make {
        w.str(s);
    }
    if let Some(s) = &m.model {
        w.str(s);
    }
    if let Some(s) = &m.lens {
        w.str(s);
    }
    if let Some(v) = m.iso {
        w.u32(v);
    }
    if let Some((n, d)) = m.exposure {
        w.u32(n);
        w.u32(d);
    }
    if let Some((n, d)) = m.f_number {
        w.u32(n);
        w.u32(d);
    }
    if let Some((lat, lon)) = m.gps {
        w.f64(lat);
        w.f64(lon);
    }
    if let Some((x, y)) = e.dims {
        w.u32(x);
        w.u32(y);
    }
    if let Some(s) = e.sig {
        w.u64(s.dhash);
        w.u64(s.phash);
        for r in s.rots {
            w.u64(r);
        }
    }
    if let Some(t) = &e.thumb {
        w.u64(thumb_off);
        w.u32(t.len() as u32);
    }
    w.buf
}

/// Decodes one entry. The thumbnail is resolved by the caller from the blob.
pub fn decode(r: &mut Reader) -> Result<(Entry, Option<(u64, u32)>), IndexError> {
    let id = r.u64()?;
    let path = PathBuf::from(r.str()?);
    let rel = r.str()?;
    let bytes = r.u64()?;
    let mtime = r.i64()?;
    let format = format_from(r.u8()?)?;

    let state_code = r.u8()?;
    let reason = r.str()?;
    let state = match state_code {
        0 => EntryState::Ok,
        // `reason` is `&'static str` on the live type; on the way back in it
        // is only ever displayed, so the owned string is kept in `Failed`.
        1 => EntryState::NoPreview { reason: "HEVC" },
        2 => EntryState::Failed { reason },
        _ => return Err(IndexError::BadField("state")),
    };

    let mask = r.u16()?;
    let orientation = r.u8()?;
    let date_source = source_from(r.u8()?);

    let mut meta = Meta { orientation, date_source, ..Default::default() };
    if mask & present::TAKEN != 0 {
        meta.taken = Some(r.i64()?);
    }
    if mask & present::MAKE != 0 {
        meta.make = Some(r.str()?);
    }
    if mask & present::MODEL != 0 {
        meta.model = Some(r.str()?);
    }
    if mask & present::LENS != 0 {
        meta.lens = Some(r.str()?);
    }
    if mask & present::ISO != 0 {
        meta.iso = Some(r.u32()?);
    }
    if mask & present::EXPOSURE != 0 {
        meta.exposure = Some((r.u32()?, r.u32()?));
    }
    if mask & present::FNUMBER != 0 {
        meta.f_number = Some((r.u32()?, r.u32()?));
    }
    if mask & present::GPS != 0 {
        meta.gps = Some((r.f64()?, r.f64()?));
    }

    let dims = if mask & present::DIMS != 0 { Some((r.u32()?, r.u32()?)) } else { None };
    let sig = if mask & present::SIG != 0 {
        Some(Sig {
            dhash: r.u64()?,
            phash: r.u64()?,
            rots: [r.u64()?, r.u64()?, r.u64()?],
        })
    } else {
        None
    };
    let thumb_ref = if mask & present::THUMB != 0 { Some((r.u64()?, r.u32()?)) } else { None };

    Ok((
        Entry { id, path, rel, bytes, mtime, format, meta, sig, thumb: None, dims, state },
        thumb_ref,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Entry {
        Entry {
            id: 0x1234_5678_9abc_def0,
            path: PathBuf::from("D:/Photos/a b/写真.jpg"),
            rel: "a b/写真.jpg".into(),
            bytes: 123456,
            mtime: 1_786_882_193,
            format: Format::Jpeg,
            meta: Meta {
                taken: Some(1_786_000_000),
                make: Some("Canon".into()),
                model: Some("EOS 5D".into()),
                lens: Some("EF 50mm".into()),
                iso: Some(400),
                exposure: Some((1, 250)),
                f_number: Some((28, 10)),
                orientation: 6,
                gps: Some((51.5074, -0.1278)),
                dims: None,
                date_source: DateSource::Original,
            },
            sig: Some(Sig { dhash: 0xAAAA, phash: 0x5555, rots: [1, 2, 3] }),
            thumb: Some(vec![1, 2, 3, 4]),
            dims: Some((4000, 3000)),
            state: EntryState::Ok,
        }
    }

    #[test]
    fn round_trips_a_full_entry() {
        let e = sample();
        let buf = encode(&e, 4096);
        let mut r = Reader::new(&buf);
        let (back, thumb) = decode(&mut r).unwrap();

        assert_eq!(back.id, e.id);
        assert_eq!(back.path, e.path);
        assert_eq!(back.rel, e.rel);
        assert_eq!(back.bytes, e.bytes);
        assert_eq!(back.mtime, e.mtime);
        assert_eq!(back.format, e.format);
        assert_eq!(back.meta, e.meta);
        assert_eq!(back.sig, e.sig);
        assert_eq!(back.dims, e.dims);
        assert_eq!(thumb, Some((4096, 4)));
        assert_eq!(r.remaining(), 0, "decoder must consume exactly the record");
    }

    #[test]
    fn round_trips_an_empty_entry() {
        let e = Entry {
            id: 1,
            path: PathBuf::from("x"),
            rel: "x".into(),
            bytes: 0,
            mtime: 0,
            format: Format::Png,
            meta: Meta { orientation: 1, ..Default::default() },
            sig: None,
            thumb: None,
            dims: None,
            state: EntryState::NoPreview { reason: "HEVC" },
        };
        let buf = encode(&e, 0);
        let mut r = Reader::new(&buf);
        let (back, thumb) = decode(&mut r).unwrap();
        assert!(back.sig.is_none());
        assert!(thumb.is_none());
        assert_eq!(back.state, EntryState::NoPreview { reason: "HEVC" });
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn preserves_a_failure_reason() {
        let mut e = sample();
        e.state = EntryState::Failed { reason: "truncated DHT values at 0x2A1".into() };
        let buf = encode(&e, 0);
        let (back, _) = decode(&mut Reader::new(&buf)).unwrap();
        assert_eq!(
            back.state,
            EntryState::Failed { reason: "truncated DHT values at 0x2A1".into() }
        );
    }

    /// Truncation must be an error at the boundary, never a wrong value.
    #[test]
    fn truncation_is_detected_not_guessed() {
        let buf = encode(&sample(), 0);
        for cut in [1usize, 8, 20, 40, buf.len() - 1] {
            let mut r = Reader::new(&buf[..cut]);
            assert!(decode(&mut r).is_err(), "accepted a {cut}-byte record");
        }
    }

    #[test]
    fn rejects_an_unknown_state_byte() {
        let mut buf = encode(&sample(), 0);
        // Locate the state byte: after id, path, rel, bytes, mtime, format.
        let mut r = Reader::new(&buf);
        r.u64().unwrap();
        r.str().unwrap();
        r.str().unwrap();
        r.u64().unwrap();
        r.i64().unwrap();
        r.u8().unwrap();
        let at = r.pos();
        buf[at] = 99;
        assert!(matches!(
            decode(&mut Reader::new(&buf)),
            Err(IndexError::BadField("state"))
        ));
    }

    #[test]
    fn format_codes_round_trip() {
        for f in [Format::Jpeg, Format::Png, Format::Gif, Format::Heic] {
            assert_eq!(format_from(format_code(f)).unwrap(), f);
        }
        assert!(format_from(200).is_err());
    }

    #[test]
    fn a_cursor_never_reads_past_the_end() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert!(r.u64().is_err());
        assert!(r.slice(99).is_err());
    }
}
