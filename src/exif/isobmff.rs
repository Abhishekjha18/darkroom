//! Locating the Exif item inside an ISOBMFF container (HEIC/HEIF).
//!
//! The container is ordinary ISO/IEC 14496-12 and parses in a few hundred
//! lines. The image inside is HEVC intra-coded and darkroom deliberately
//! does not decode it — but **the date, camera and GPS live out here**, and
//! reading them is what keeps an iPhone photo in the timeline in the right
//! place instead of vanishing from it.
//!
//! The path is `meta` → `iinf` (which item is the Exif one) → `iloc` (where
//! its bytes are).

/// A parsed box header: what it is, and where its body lies.
struct BoxHeader {
    kind: [u8; 4],
    body: usize,
    end: usize,
}

/// Reads one box at `pos`, bounds-checked against `bytes`.
fn read_box(bytes: &[u8], pos: usize) -> Option<BoxHeader> {
    let head = bytes.get(pos..pos + 8)?;
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let kind = [head[4], head[5], head[6], head[7]];

    let (body, total) = match size {
        // 1 means the real size is a 64-bit field after the type.
        1 => {
            let ext = bytes.get(pos + 8..pos + 16)?;
            let large = u64::from_be_bytes(ext.try_into().ok()?);
            // A 64-bit size that does not fit a usize cannot be inside this
            // file, so refusing is the same as bounds-checking it.
            (pos + 16, usize::try_from(large).ok()?)
        }
        // 0 means the box runs to the end of the file.
        0 => (pos + 8, bytes.len().checked_sub(pos)?),
        n => (pos + 8, n),
    };

    let end = pos.checked_add(total)?;
    if end > bytes.len() || body > end {
        return None;
    }
    Some(BoxHeader { kind, body, end })
}

/// Walks sibling boxes in `[from, to)`, calling `f` with each.
fn for_each_box(bytes: &[u8], from: usize, to: usize, mut f: impl FnMut(&BoxHeader)) {
    let mut pos = from;
    // A malformed file can nest or chain boxes indefinitely; cap the walk.
    let mut guard = 0;
    while pos < to && guard < 4096 {
        let Some(b) = read_box(bytes, pos) else { return };
        if b.end <= pos || b.end > to {
            return; // zero-length or overrunning box: stop rather than loop
        }
        f(&b);
        pos = b.end;
        guard += 1;
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a big-endian unsigned integer of `n` bytes (0, 4 or 8 in `iloc`).
fn be_var(b: &[u8], at: usize, n: usize) -> Option<u64> {
    if n == 0 {
        return Some(0);
    }
    let raw = b.get(at..at + n)?;
    Some(raw.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64))
}

/// The item id whose `item_type` is `Exif`, from the `iinf` box.
fn exif_item_id(bytes: &[u8], iinf: &BoxHeader) -> Option<u32> {
    let version = *bytes.get(iinf.body)?;
    let mut pos = iinf.body + 4; // version + flags

    // Entry count widens at version 1.
    let count = if version == 0 {
        let n = be16(bytes, pos)? as u32;
        pos += 2;
        n
    } else {
        let n = be32(bytes, pos)?;
        pos += 4;
        n
    };

    let mut found = None;
    let mut seen = 0u32;
    for_each_box(bytes, pos, iinf.end, |b| {
        seen += 1;
        if found.is_some() || seen > count.max(1) || &b.kind != b"infe" {
            return;
        }
        let Some(&v) = bytes.get(b.body) else { return };
        // Only version 2 and up carry `item_type`; earlier ones describe
        // the item by MIME name instead and cannot be matched this way.
        if v < 2 {
            return;
        }
        let mut p = b.body + 4;
        let id = if v == 2 {
            let id = be16(bytes, p).map(u32::from);
            p += 2;
            id
        } else {
            let id = be32(bytes, p);
            p += 4;
            id
        };
        p += 2; // item_protection_index
        let Some(id) = id else { return };
        if bytes.get(p..p + 4) == Some(b"Exif") {
            found = Some(id);
        }
    });
    found
}

/// The byte range of `item` in the file, from the `iloc` box.
fn item_extent(bytes: &[u8], iloc: &BoxHeader, item: u32) -> Option<(usize, usize)> {
    let version = *bytes.get(iloc.body)?;
    let mut pos = iloc.body + 4;

    let sizes = *bytes.get(pos)?;
    let offset_size = (sizes >> 4) as usize;
    let length_size = (sizes & 0x0F) as usize;
    let base_sizes = *bytes.get(pos + 1)?;
    let base_offset_size = (base_sizes >> 4) as usize;
    let index_size = if version >= 1 { (base_sizes & 0x0F) as usize } else { 0 };
    pos += 2;

    let count = if version < 2 {
        let n = be16(bytes, pos)? as u32;
        pos += 2;
        n
    } else {
        let n = be32(bytes, pos)?;
        pos += 4;
        n
    };

    for _ in 0..count.min(4096) {
        let id = if version < 2 {
            let v = be16(bytes, pos)? as u32;
            pos += 2;
            v
        } else {
            let v = be32(bytes, pos)?;
            pos += 4;
            v
        };
        if version >= 1 {
            pos += 2; // construction_method
        }
        pos += 2; // data_reference_index
        let base = be_var(bytes, pos, base_offset_size)?;
        pos += base_offset_size;
        let extents = be16(bytes, pos)?;
        pos += 2;

        for e in 0..extents {
            pos += index_size;
            let offset = be_var(bytes, pos, offset_size)?;
            pos += offset_size;
            let length = be_var(bytes, pos, length_size)?;
            pos += length_size;

            // The first extent of the Exif item is the one that matters;
            // fragmented metadata is not something cameras produce.
            if id == item && e == 0 {
                let start = usize::try_from(base.checked_add(offset)?).ok()?;
                let len = usize::try_from(length).ok()?;
                let end = start.checked_add(len)?;
                if end <= bytes.len() {
                    return Some((start, end));
                }
                return None;
            }
        }
    }
    None
}

/// Returns the offset of the TIFF header inside an ISOBMFF file.
pub fn find_exif_tiff(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 12 || bytes.get(4..8)? != b"ftyp" {
        return None;
    }

    let mut meta: Option<(usize, usize)> = None;
    for_each_box(bytes, 0, bytes.len(), |b| {
        if &b.kind == b"meta" && meta.is_none() {
            // `meta` is a FullBox: its children start after version+flags.
            meta = Some((b.body + 4, b.end));
        }
    });
    let (meta_start, meta_end) = meta?;

    let mut iinf: Option<BoxHeader> = None;
    let mut iloc: Option<BoxHeader> = None;
    for_each_box(bytes, meta_start, meta_end, |b| match &b.kind {
        b"iinf" if iinf.is_none() => {
            iinf = Some(BoxHeader { kind: b.kind, body: b.body, end: b.end })
        }
        b"iloc" if iloc.is_none() => {
            iloc = Some(BoxHeader { kind: b.kind, body: b.body, end: b.end })
        }
        _ => {}
    });

    let item = exif_item_id(bytes, &iinf?)?;
    let (start, end) = item_extent(bytes, &iloc?, item)?;
    tiff_start(bytes, start, end)
}

/// Resolves the `ExifDataBlock` preamble to the TIFF header itself.
///
/// The spec puts a 32-bit `exif_tiff_header_offset` first, counting the
/// bytes before the TIFF header. **Real files disagree about what fills
/// them:** some write 6 and the string `Exif\0\0`, some write 0 and start
/// the TIFF immediately. Both are accepted, and the result is only returned
/// if a TIFF header is actually there.
fn tiff_start(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let skip = be32(bytes, start)? as usize;
    let mut at = start.checked_add(4)?.checked_add(skip)?;

    if bytes.get(at..at + 6) == Some(b"Exif\0\0") {
        at += 6;
    }
    if at + 8 > end || at + 8 > bytes.len() {
        return None;
    }
    // Validate rather than trust: a wrong offset must fail here instead of
    // producing plausible-looking garbage further down.
    match bytes.get(at..at + 2)? {
        b"II" | b"MM" => Some(at),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal HEIC-shaped file around a TIFF block.
    fn container(tiff: &[u8], header_offset: u32, prefix: &[u8]) -> Vec<u8> {
        fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        }
        fn full(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
            let mut b = vec![version, 0, 0, 0];
            b.extend_from_slice(body);
            boxed(kind, &b)
        }

        let mut payload = header_offset.to_be_bytes().to_vec();
        payload.extend_from_slice(prefix);
        payload.extend_from_slice(tiff);

        let ftyp = boxed(b"ftyp", b"heic\0\0\0\0heicmif1");

        let mut infe_body = 1u16.to_be_bytes().to_vec();
        infe_body.extend_from_slice(&0u16.to_be_bytes());
        infe_body.extend_from_slice(b"Exif\0");
        let infe = full(b"infe", 2, &infe_body);

        let mut iinf_body = 1u16.to_be_bytes().to_vec();
        iinf_body.extend_from_slice(&infe);
        let iinf = full(b"iinf", 0, &iinf_body);

        let make_iloc = |offset: u32| {
            let mut b = vec![0x44u8, 0x00];
            b.extend_from_slice(&1u16.to_be_bytes());
            b.extend_from_slice(&1u16.to_be_bytes()); // item id
            b.extend_from_slice(&0u16.to_be_bytes()); // data ref
            b.extend_from_slice(&1u16.to_be_bytes()); // extent count
            b.extend_from_slice(&offset.to_be_bytes());
            b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            full(b"iloc", 0, &b)
        };

        // Settle the offset: it depends on the size of the box that holds it.
        let mut offset = 0u32;
        let mut meta;
        for _ in 0..3 {
            let mut body = Vec::new();
            body.extend_from_slice(&iinf);
            body.extend_from_slice(&make_iloc(offset));
            meta = full(b"meta", 0, &body);
            offset = (ftyp.len() + meta.len() + 8) as u32;
        }
        let mut body = Vec::new();
        body.extend_from_slice(&iinf);
        body.extend_from_slice(&make_iloc(offset));
        let meta = full(b"meta", 0, &body);

        let mut out = ftyp;
        out.extend_from_slice(&meta);
        out.extend_from_slice(&boxed(b"mdat", &payload));
        out
    }

    fn tiff_le() -> Vec<u8> {
        let mut t = b"II".to_vec();
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes()); // no entries
        t.extend_from_slice(&0u32.to_le_bytes());
        t
    }

    #[test]
    fn finds_the_exif_item_with_an_exif_prefix() {
        let tiff = tiff_le();
        let file = container(&tiff, 6, b"Exif\0\0");
        let at = find_exif_tiff(&file).expect("should locate the TIFF");
        assert_eq!(&file[at..at + 2], b"II");
    }

    /// The other convention seen in the wild: offset 0, no prefix.
    #[test]
    fn finds_the_exif_item_without_a_prefix() {
        let tiff = tiff_le();
        let file = container(&tiff, 0, b"");
        let at = find_exif_tiff(&file).unwrap();
        assert_eq!(&file[at..at + 2], b"II");
    }

    #[test]
    fn rejects_a_non_isobmff_file() {
        assert!(find_exif_tiff(b"").is_none());
        assert!(find_exif_tiff(b"\xff\xd8\xff\xe0 not a container").is_none());
        assert!(find_exif_tiff(&[0u8; 64]).is_none());
    }

    /// Truncation at every length must return None, never panic.
    #[test]
    fn truncation_never_panics() {
        let file = container(&tiff_le(), 6, b"Exif\0\0");
        for cut in 0..file.len() {
            let _ = find_exif_tiff(&file[..cut]);
        }
    }

    /// Corrupting any single byte must not panic either.
    #[test]
    fn corruption_never_panics() {
        let base = container(&tiff_le(), 6, b"Exif\0\0");
        for i in 0..base.len() {
            let mut f = base.clone();
            f[i] ^= 0xFF;
            let _ = find_exif_tiff(&f);
        }
    }

    #[test]
    fn a_box_claiming_a_huge_size_is_refused() {
        let mut f = b"\0\0\0\x10ftypheic\0\0\0\0heic".to_vec();
        f.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        f.extend_from_slice(b"meta");
        f.extend_from_slice(&[0u8; 16]);
        assert!(find_exif_tiff(&f).is_none());
    }

    /// A zero-length box would otherwise spin the walker forever.
    #[test]
    fn a_zero_length_box_does_not_loop() {
        let mut f = b"\0\0\0\x10ftypheic\0\0\0\0heic".to_vec();
        f.extend_from_slice(&[0, 0, 0, 0]);
        f.extend_from_slice(b"junk");
        assert!(find_exif_tiff(&f).is_none());
    }
}
