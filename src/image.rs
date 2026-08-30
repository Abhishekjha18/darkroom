//! The one pixel format in the program, and the error every decoder returns.

use std::fmt;

/// Every variant names the byte offset it died at, because "invalid JPEG" is
/// useless in a bug report and `truncated DHT values at 0x2A1` is not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ImageError {
    NotThisFormat,
    Truncated { at: usize, expected: &'static str },
    BadField { at: usize, field: &'static str, value: u32 },
    /// **A first-class outcome, not a failure.** Progressive JPEG and HEIC
    /// both land here and both produce a catalogued entry with a stated
    /// reason.
    Unsupported { feature: &'static str },
    TooLarge { w: u32, h: u32 },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::NotThisFormat => write!(f, "not this format"),
            ImageError::Truncated { at, expected } => {
                write!(f, "truncated {expected} at 0x{at:X}")
            }
            ImageError::BadField { at, field, value } => {
                write!(f, "bad {field} ({value}) at 0x{at:X}")
            }
            ImageError::Unsupported { feature } => write!(f, "unsupported: {feature}"),
            ImageError::TooLarge { w, h } => write!(f, "refusing {w}x{h}: too large"),
        }
    }
}

/// A header claiming 65535x65535 is 12 GB and must be refused before
/// allocating, not attempted. A 20000x150 panorama is legitimate and passes.
pub const MAX_PIXELS: u64 = 64_000_000;

pub fn check_dimensions(w: u32, h: u32) -> Result<(), ImageError> {
    if w == 0 || h == 0 {
        return Err(ImageError::BadField { at: 0, field: "dimensions", value: w.max(h) });
    }
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(ImageError::TooLarge { w, h });
    }
    Ok(())
}


/// 8-bit RGB, tightly packed, no stride padding.
///
/// **One format, deliberately.** Every alternative — planar YCbCr, 16-bit,
/// palettes, alpha — buys nothing darkroom needs and multiplies the
/// conversion matrix between modules. Alpha is composited onto white at PNG
/// decode time.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub px: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Image {
        Image { width, height, px: vec![0; width as usize * height as usize * 3] }
    }

    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Every decoder must produce a buffer of exactly this size.
    pub fn is_consistent(&self) -> bool {
        self.px.len() == self.pixels() * 3
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        [self.px[i], self.px[i + 1], self.px[i + 2]]
    }

    /// Luma, for the hashes. `0.299R + 0.587G + 0.114B` — **not** the mean of
    /// the channels, which is a different and worse grey.
    pub fn luma(&self, x: u32, y: u32) -> u8 {
        let [r, g, b] = self.get(x, y);
        ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000).min(255) as u8
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{}, {} bytes)", self.width, self.height, self.px.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_consistent() {
        let img = Image::new(7, 5);
        assert!(img.is_consistent());
        assert_eq!(img.px.len(), 7 * 5 * 3);
    }

    #[test]
    fn luma_uses_rec601_weights() {
        let mut img = Image::new(1, 1);
        img.px = vec![255, 255, 255];
        assert_eq!(img.luma(0, 0), 255);
        img.px = vec![0, 0, 0];
        assert_eq!(img.luma(0, 0), 0);
        img.px = vec![255, 0, 0];
        assert_eq!(img.luma(0, 0), 76); // 0.299 * 255
        img.px = vec![0, 255, 0];
        assert_eq!(img.luma(0, 0), 149); // 0.587 * 255
    }
}
