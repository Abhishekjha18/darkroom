//! Format sniffing by content, never by extension.
//!
//! `corpus/pathological/` holds a `.png` that is a JPEG, a `.jpg` that is a
//! PNG, a `double-extension.jpg.png` and a file with no extension at all.
//! All four must be classified correctly, which rules out looking at the name.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    /// Container parses; the pixels are HEVC and are deliberately not decoded.
    /// See ARCHITECTURE.md §13.
    Heic,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Gif => "gif",
            Format::Heic => "heic",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Format::Jpeg => "image/jpeg",
            Format::Png => "image/png",
            Format::Gif => "image/gif",
            Format::Heic => "image/heic",
        }
    }
}

/// The longest prefix any check below needs.
pub const SNIFF_LEN: usize = 16;

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// ISOBMFF brands that carry HEIF/HEIC image items.
const HEIF_BRANDS: [&[u8; 4]; 8] =
    [b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevm", b"hevs", b"mif1"];

pub fn probe(head: &[u8]) -> Option<Format> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if head.starts_with(PNG_MAGIC) {
        return Some(Format::Png);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    // ISOBMFF: a big-endian box length, then "ftyp", then the major brand.
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand: &[u8] = &head[8..12];
        if HEIF_BRANDS.iter().any(|b| b.as_slice() == brand) {
            return Some(Format::Heic);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_jpeg() {
        assert_eq!(probe(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
    }

    #[test]
    fn sniffs_png() {
        assert_eq!(probe(PNG_MAGIC), Some(Format::Png));
    }

    #[test]
    fn sniffs_gif() {
        assert_eq!(probe(b"GIF89a....."), Some(Format::Gif));
    }

    #[test]
    fn sniffs_heic() {
        let mut b = vec![0, 0, 0, 0x18];
        b.extend_from_slice(b"ftypheic");
        b.extend_from_slice(b"\0\0\0\0");
        assert_eq!(probe(&b), Some(Format::Heic));
    }

    /// A short read must never index out of bounds.
    #[test]
    fn short_input_is_not_a_panic() {
        for n in 0..12 {
            assert_eq!(probe(&vec![0xFFu8; n]), None);
        }
        assert_eq!(probe(b""), None);
    }

    #[test]
    fn rejects_non_images() {
        assert_eq!(probe(b"#!/bin/sh\n"), None);
        assert_eq!(probe(&[0u8; 16]), None);
    }
}
