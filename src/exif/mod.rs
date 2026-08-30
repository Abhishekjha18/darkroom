//! EXIF / TIFF IFD parsing (CIPA DC-008). Replaces `kamadak-exif` /
//! `exifread`.
//!
//! **Never fatal.** A missing tag is normal, not an error — most PNGs have
//! no EXIF at all, and a photo with no `DateTimeOriginal` still belongs in
//! the timeline under its mtime.

pub mod isobmff;
pub mod tiff;

use crate::civil;
use tiff::{IfdEntry, Tiff};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    /// Seconds since the epoch, local wall clock. See `civil`.
    pub taken: Option<i64>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Kept as an exact rational — "1/250", never 0.004.
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<(u32, u32)>,
    /// 1..=8, defaulting to 1.
    pub orientation: u8,
    pub gps: Option<(f64, f64)>,
    pub dims: Option<(u32, u32)>,
    /// Which tag the date came from, for the UI to mark a guess.
    pub date_source: DateSource,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateSource {
    Original,
    Digitized,
    ModifiedTag,
    /// Fell through to filesystem mtime. "This date is a guess" is
    /// information the user wants.
    #[default]
    FileTime,
}

impl DateSource {
    pub fn name(self) -> &'static str {
        match self {
            DateSource::Original => "taken",
            DateSource::Digitized => "digitized",
            DateSource::ModifiedTag => "file-tag",
            DateSource::FileTime => "mtime",
        }
    }
}

impl Meta {
    pub fn camera(&self) -> Option<String> {
        match (&self.make, &self.model) {
            (Some(mk), Some(md)) => {
                // Many models already repeat the make: "Canon EOS 5D".
                if md.to_lowercase().starts_with(&mk.to_lowercase()) {
                    Some(md.clone())
                } else {
                    Some(format!("{mk} {md}"))
                }
            }
            (None, Some(md)) => Some(md.clone()),
            (Some(mk), None) => Some(mk.clone()),
            (None, None) => None,
        }
    }
}

// Tag numbers. Twelve of them, not the standard.
const TAG_MAKE: u16 = 0x010F;
const TAG_MODEL: u16 = 0x0110;
const TAG_ORIENTATION: u16 = 0x0112;
const TAG_DATETIME: u16 = 0x0132;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_GPS_IFD: u16 = 0x8825;
const TAG_EXPOSURE: u16 = 0x829A;
const TAG_FNUMBER: u16 = 0x829D;
const TAG_ISO: u16 = 0x8827;
const TAG_DATETIME_ORIGINAL: u16 = 0x9003;
const TAG_DATETIME_DIGITIZED: u16 = 0x9004;
const TAG_PIXEL_X: u16 = 0xA002;
const TAG_PIXEL_Y: u16 = 0xA003;
const TAG_LENS_MODEL: u16 = 0xA434;

/// Extracts metadata from a whole image file. Returns defaults when there is
/// nothing to read, which is the common case.
pub fn parse(bytes: &[u8]) -> Meta {
    let mut meta = Meta { orientation: 1, ..Default::default() };
    let Some(tiff_start) = find_tiff(bytes) else { return meta };
    let Some((t, ifd0)) = Tiff::new(&bytes[tiff_start..]) else { return meta };

    let mut exif_ifd = None;
    let mut gps_ifd = None;
    let mut dt_original = None;
    let mut dt_digitized = None;
    let mut dt_modified = None;

    // **Cap the IFD chain.** A malformed file can point IFD1 back at IFD0.
    let mut visited: Vec<u32> = Vec::new();
    let mut next = Some(ifd0);
    let mut hops = 0;
    while let Some(off) = next {
        if hops >= 8 || visited.contains(&off) {
            break;
        }
        visited.push(off);
        hops += 1;

        for e in t.entries(off) {
            match e.tag {
                TAG_MAKE => meta.make = t.ascii(&e),
                TAG_MODEL => meta.model = t.ascii(&e),
                TAG_ORIENTATION => {
                    if let Some(v) = t.uint(&e)
                        && (1..=8).contains(&v)
                    {
                        meta.orientation = v as u8;
                    }
                }
                TAG_DATETIME => dt_modified = t.ascii(&e),
                TAG_EXIF_IFD => exif_ifd = Some(t.u32_of(&e.raw)),
                TAG_GPS_IFD => gps_ifd = Some(t.u32_of(&e.raw)),
                _ => {}
            }
        }
        // IFD1 is the embedded thumbnail. Present, and deliberately ignored
        // — see the module note in LLD-EXIF.md §8.
        next = t.next_ifd(off);
    }

    if let Some(off) = exif_ifd {
        for e in t.entries(off) {
            match e.tag {
                TAG_DATETIME_ORIGINAL => dt_original = t.ascii(&e),
                TAG_DATETIME_DIGITIZED => dt_digitized = t.ascii(&e),
                TAG_EXPOSURE => meta.exposure = t.rational(&e, 0),
                TAG_FNUMBER => meta.f_number = t.rational(&e, 0),
                TAG_ISO => meta.iso = t.uint(&e),
                TAG_LENS_MODEL => meta.lens = t.ascii(&e),
                TAG_PIXEL_X => {
                    let w = t.uint(&e).unwrap_or(0);
                    meta.dims = Some((w, meta.dims.map(|d| d.1).unwrap_or(0)));
                }
                TAG_PIXEL_Y => {
                    let h = t.uint(&e).unwrap_or(0);
                    meta.dims = Some((meta.dims.map(|d| d.0).unwrap_or(0), h));
                }
                _ => {}
            }
        }
    }

    if let Some(off) = gps_ifd {
        meta.gps = parse_gps(&t, off);
    }

    // Fallback chain: DateTimeOriginal -> DateTimeDigitized -> DateTime ->
    // file mtime, which the caller supplies.
    for (raw, src) in [
        (dt_original, DateSource::Original),
        (dt_digitized, DateSource::Digitized),
        (dt_modified, DateSource::ModifiedTag),
    ] {
        if let Some(s) = raw
            && let Some(ts) = civil::parse_exif_datetime(&s)
        {
            meta.taken = Some(ts);
            meta.date_source = src;
            break;
        }
    }

    if meta.dims == Some((0, 0)) {
        meta.dims = None;
    }
    meta
}

/// Locates the start of the TIFF block inside a container.
fn find_tiff(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return find_tiff_in_jpeg(bytes);
    }
    if bytes.len() > 12 && &bytes[4..8] == b"ftyp" {
        return find_tiff_in_isobmff(bytes);
    }
    None
}

/// Walks JPEG markers for the `APP1` segment whose payload begins
/// `Exif\0\0`. The TIFF header starts six bytes after that.
fn find_tiff_in_jpeg(bytes: &[u8]) -> Option<usize> {
    let mut i = 2usize;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] == 0xFF {
            j += 1;
        }
        let marker = *bytes.get(j)?;
        let after = j + 1;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                i = after;
                continue;
            }
            // Once the scan starts there is no more metadata to find.
            0xDA | 0xD9 => return None,
            _ => {}
        }
        let len = ((*bytes.get(after)? as usize) << 8) | *bytes.get(after + 1)? as usize;
        if len < 2 {
            return None;
        }
        let payload = bytes.get(after + 2..after + len)?;
        if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
            return Some(after + 2 + 6);
        }
        i = after + len;
    }
    None
}

/// HEIC and friends: the Exif item lives in the `meta` box, reachable via
/// `iinf` and `iloc`. See `isobmff`.
///
/// **A real box walk, with a bounded scan behind it.** The walk is the
/// correct answer and handles every file that follows the specification.
/// The fallback exists because HEIC in the wild is less uniform than the
/// standard suggests — and it is safe to fall back to, because a wrong guess
/// has to survive the TIFF header check below before anything reads it.
fn find_tiff_in_isobmff(bytes: &[u8]) -> Option<usize> {
    if let Some(at) = isobmff::find_exif_tiff(bytes) {
        return Some(at);
    }
    let window = &bytes[..bytes.len().min(1 << 20)];
    let sig = b"Exif\0\0";
    window
        .windows(sig.len())
        .position(|w| w == sig)
        .map(|p| p + sig.len())
        .filter(|&start| {
            matches!(bytes.get(start..start + 2), Some(b"II") | Some(b"MM"))
                && start + 8 <= bytes.len()
        })
}

/// Latitude and longitude arrive as three RATIONALs plus a separate ASCII
/// reference tag.
fn parse_gps(t: &Tiff, off: u32) -> Option<(f64, f64)> {
    let entries = t.entries(off);
    let find = |tag: u16| entries.iter().find(|e| e.tag == tag).copied();

    let lat_ref = find(0x0001).and_then(|e| t.ascii(&e))?;
    let lat = find(0x0002)?;
    let lon_ref = find(0x0003).and_then(|e| t.ascii(&e))?;
    let lon = find(0x0004)?;

    let lat = dms(t, &lat)?;
    let lon = dms(t, &lon)?;

    let lat = if lat_ref.starts_with('S') { -lat } else { lat };
    let lon = if lon_ref.starts_with('W') { -lon } else { lon };

    // Reject impossible coordinates rather than plotting them.
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then_some((lat, lon))
}

/// `deg + min/60 + sec/3600`.
///
/// **Denominators vary wildly by vendor** — some write seconds as `4230/100`,
/// some push all precision into minutes. Any implementation that assumes
/// `/1` produces coordinates in the wrong ocean. A `0` denominator is real
/// in the wild, so every division is guarded.
fn dms(t: &Tiff, e: &IfdEntry) -> Option<f64> {
    let mut total = 0f64;
    for (i, scale) in [1.0f64, 60.0, 3600.0].iter().enumerate() {
        let (n, d) = t.rational(e, i)?;
        if d == 0 {
            if n == 0 {
                continue; // 0/0 is a legitimate "no seconds"
            }
            return None;
        }
        total += (n as f64 / d as f64) / scale;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a JPEG carrying an APP1 Exif block with the given IFD0 and
    /// ExifIFD entries.
    struct Builder {
        little: bool,
        ifd0: Vec<(u16, u16, u32, [u8; 4])>,
        exif: Vec<(u16, u16, u32, [u8; 4])>,
        heap: Vec<u8>,
    }

    impl Builder {
        fn new(little: bool) -> Self {
            Builder { little, ifd0: Vec::new(), exif: Vec::new(), heap: Vec::new() }
        }

        fn u16b(&self, v: u16) -> [u8; 2] {
            if self.little { v.to_le_bytes() } else { v.to_be_bytes() }
        }
        fn u32b(&self, v: u32) -> [u8; 4] {
            if self.little { v.to_le_bytes() } else { v.to_be_bytes() }
        }

        /// Adds a value to the heap and returns its placeholder offset slot.
        fn heap_push(&mut self, data: &[u8]) -> usize {
            let at = self.heap.len();
            self.heap.extend_from_slice(data);
            while self.heap.len() % 2 != 0 {
                self.heap.push(0);
            }
            at
        }

        fn ascii(&mut self, tag: u16, s: &str, into_exif: bool) {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            let n = v.len() as u32;
            let at = self.heap_push(&v);
            let slot = (tag, 2u16, n, [0xFF, 0xFF, 0xFF, 0xFF]);
            if into_exif {
                self.exif.push(slot);
            } else {
                self.ifd0.push(slot);
            }
            // Remember where the heap entry went by re-encoding later.
            let idx = if into_exif { self.exif.len() - 1 } else { self.ifd0.len() - 1 };
            let marker = (at as u32).to_le_bytes();
            if into_exif {
                self.exif[idx].3 = marker;
            } else {
                self.ifd0[idx].3 = marker;
            }
        }

        fn short(&mut self, tag: u16, v: u16, into_exif: bool) {
            let raw = if self.little {
                [v.to_le_bytes()[0], v.to_le_bytes()[1], 0, 0]
            } else {
                [v.to_be_bytes()[0], v.to_be_bytes()[1], 0, 0]
            };
            let slot = (tag, 3u16, 1u32, raw);
            if into_exif {
                self.exif.push(slot);
            } else {
                self.ifd0.push(slot);
            }
        }

        fn build(mut self) -> Vec<u8> {
            // Layout: header(8) | IFD0 | ExifIFD | heap
            let ifd0_at = 8u32;
            let ifd0_len = 2 + (self.ifd0.len() as u32 + 1) * 12 + 4; // +1 for the ExifIFD pointer
            let exif_at = ifd0_at + ifd0_len;
            let exif_len = 2 + self.exif.len() as u32 * 12 + 4;
            let heap_at = exif_at + exif_len;

            let mut t = Vec::new();
            t.extend_from_slice(if self.little { b"II" } else { b"MM" });
            t.extend_from_slice(&self.u16b(42));
            t.extend_from_slice(&self.u32b(ifd0_at));

            let fix = |raw: [u8; 4], little: bool, heap_at: u32| -> [u8; 4] {
                // Heap placeholders were stored little-endian; re-encode.
                let rel = u32::from_le_bytes(raw);
                let abs = heap_at + rel;
                if little { abs.to_le_bytes() } else { abs.to_be_bytes() }
            };

            t.extend_from_slice(&self.u16b(self.ifd0.len() as u16 + 1));
            let ifd0 = std::mem::take(&mut self.ifd0);
            for (tag, typ, count, raw) in ifd0 {
                t.extend_from_slice(&self.u16b(tag));
                t.extend_from_slice(&self.u16b(typ));
                t.extend_from_slice(&self.u32b(count));
                let raw = if typ == 2 { fix(raw, self.little, heap_at) } else { raw };
                t.extend_from_slice(&raw);
            }
            t.extend_from_slice(&self.u16b(TAG_EXIF_IFD));
            t.extend_from_slice(&self.u16b(4));
            t.extend_from_slice(&self.u32b(1));
            t.extend_from_slice(&self.u32b(exif_at));
            t.extend_from_slice(&self.u32b(0)); // no IFD1

            t.extend_from_slice(&self.u16b(self.exif.len() as u16));
            let exif = std::mem::take(&mut self.exif);
            for (tag, typ, count, raw) in exif {
                t.extend_from_slice(&self.u16b(tag));
                t.extend_from_slice(&self.u16b(typ));
                t.extend_from_slice(&self.u32b(count));
                let raw = if typ == 2 { fix(raw, self.little, heap_at) } else { raw };
                t.extend_from_slice(&raw);
            }
            t.extend_from_slice(&self.u32b(0));
            t.extend_from_slice(&self.heap);

            // Wrap in a JPEG APP1 segment.
            let mut payload = b"Exif\0\0".to_vec();
            payload.extend_from_slice(&t);
            let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
            let len = (payload.len() + 2) as u16;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&payload);
            out.extend_from_slice(&[0xFF, 0xD9]);
            out
        }
    }

    #[test]
    fn reads_nothing_from_a_bare_file() {
        let m = parse(b"");
        assert_eq!(m.orientation, 1);
        assert!(m.taken.is_none());
        assert_eq!(m.date_source, DateSource::FileTime);

        let m = parse(&[0xFF, 0xD8, 0xFF, 0xD9]);
        assert!(m.taken.is_none());
    }

    #[test]
    fn reads_camera_and_date_in_both_byte_orders() {
        for little in [true, false] {
            let mut b = Builder::new(little);
            b.ascii(TAG_MAKE, "Canon", false);
            b.ascii(TAG_MODEL, "Canon EOS 5D", false);
            b.short(TAG_ORIENTATION, 6, false);
            b.ascii(TAG_DATETIME_ORIGINAL, "2026:08:18 14:30:05", true);
            let data = b.build();

            let m = parse(&data);
            assert_eq!(m.make.as_deref(), Some("Canon"), "little={little}");
            assert_eq!(m.model.as_deref(), Some("Canon EOS 5D"));
            assert_eq!(m.orientation, 6);
            assert_eq!(m.date_source, DateSource::Original);
            assert_eq!(civil::date_string(m.taken.unwrap()), "2026-08-18");
            // Model already repeats the make, so the camera name must not
            // become "Canon Canon EOS 5D".
            assert_eq!(m.camera().as_deref(), Some("Canon EOS 5D"));
        }
    }

    #[test]
    fn falls_back_through_the_date_chain() {
        let mut b = Builder::new(true);
        b.ascii(TAG_DATETIME_DIGITIZED, "2020:01:02 03:04:05", true);
        let m = parse(&b.build());
        assert_eq!(m.date_source, DateSource::Digitized);
        assert_eq!(civil::date_string(m.taken.unwrap()), "2020-01-02");

        let mut b = Builder::new(true);
        b.ascii(TAG_DATETIME, "2019:05:06 07:08:09", false);
        let m = parse(&b.build());
        assert_eq!(m.date_source, DateSource::ModifiedTag);
    }

    #[test]
    fn an_unset_camera_clock_does_not_become_1970() {
        let mut b = Builder::new(true);
        b.ascii(TAG_DATETIME_ORIGINAL, "0000:00:00 00:00:00", true);
        let m = parse(&b.build());
        assert!(m.taken.is_none());
        assert_eq!(m.date_source, DateSource::FileTime);
    }

    #[test]
    fn combines_make_and_model_when_they_differ() {
        let mut m = Meta::default();
        m.make = Some("NIKON CORPORATION".into());
        m.model = Some("NIKON D850".into());
        assert_eq!(m.camera().as_deref(), Some("NIKON CORPORATION NIKON D850"));

        m.make = Some("NIKON".into());
        assert_eq!(m.camera().as_deref(), Some("NIKON D850"));
    }

    #[test]
    fn ignores_an_out_of_range_orientation() {
        let mut b = Builder::new(true);
        b.short(TAG_ORIENTATION, 99, false);
        assert_eq!(parse(&b.build()).orientation, 1);
    }

    #[test]
    fn survives_a_truncated_exif_block() {
        let mut b = Builder::new(true);
        b.ascii(TAG_MAKE, "Canon", false);
        let mut data = b.build();
        data.truncate(data.len() / 2);
        let _ = parse(&data); // must not panic
    }

    /// The corpus file that separates a careful parser from a crashing one.
    #[test]
    fn survives_an_ifd_pointing_out_of_bounds() {
        let mut b = Builder::new(true);
        b.ifd0.push((TAG_MAKE, 2u16, 0x7FFF_FFFF, [0xFF, 0xFF, 0xFF, 0x7F]));
        let m = parse(&b.build());
        assert!(m.make.is_none());
    }
}
