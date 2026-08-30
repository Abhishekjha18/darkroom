//! Zig-zag order, chroma upsampling, and YCbCr to RGB.

/// Zig-zag position → natural (row-major) index.
pub const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// JFIF conversion in 16-bit fixed point, with rounding.
///
/// **Clamp, do not wrap.** Wrapping produces the psychedelic-pixel artifact
/// that looks like a Huffman bug and is not one.
#[inline]
pub fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let y = y as i32;
    let cb = cb as i32 - 128;
    let cr = cr as i32 - 128;

    let r = y + ((91881 * cr + 32768) >> 16);
    let g = y - ((22554 * cb + 46802 * cr + 32768) >> 16);
    let b = y + ((116130 * cb + 32768) >> 16);

    [r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_is_a_permutation() {
        let mut seen = ZIGZAG.to_vec();
        seen.sort_unstable();
        assert_eq!(seen, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn zigzag_starts_and_ends_correctly() {
        assert_eq!(ZIGZAG[0], 0);
        assert_eq!(ZIGZAG[1], 1);
        assert_eq!(ZIGZAG[2], 8);
        assert_eq!(ZIGZAG[63], 63);
    }

    #[test]
    fn neutral_chroma_is_grey() {
        for y in [0u8, 1, 64, 128, 200, 255] {
            assert_eq!(ycbcr_to_rgb(y, 128, 128), [y, y, y]);
        }
    }

    #[test]
    fn primaries_land_in_the_right_corner() {
        // Full red in JFIF: Y=76, Cb=85, Cr=255
        let [r, g, b] = ycbcr_to_rgb(76, 85, 255);
        assert!(r > 240 && g < 20 && b < 20, "{r} {g} {b}");
    }

    #[test]
    fn clamps_rather_than_wrapping() {
        // Each of these overshoots the 0..255 range before clamping, which
        // is where wrapping arithmetic would produce a wildly wrong colour
        // rather than a saturated one.
        assert_eq!(ycbcr_to_rgb(255, 128, 255)[0], 255); // R over  (~433)
        assert_eq!(ycbcr_to_rgb(0, 128, 0)[0], 0); // R under (~-179)
        assert_eq!(ycbcr_to_rgb(255, 255, 128)[2], 255); // B over  (~480)
        assert_eq!(ycbcr_to_rgb(0, 0, 128)[2], 0); // B under (~-226)
        assert_eq!(ycbcr_to_rgb(0, 255, 255)[1], 0); // G under both chromas
    }
}
