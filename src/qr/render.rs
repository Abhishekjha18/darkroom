//! PNG rendering of a QR code.

use super::Qr;
use crate::image::Image;

/// **Mandatory, on every side.** It is the first thing an implementation
/// drops when the code doesn't fit the window — and without it phones fail
/// to detect the code at all, a failure that looks like a bad encoder rather
/// than bad framing.
pub const QUIET_ZONE: usize = 4;

/// Renders to an RGB image, `scale` pixels per module, with the quiet zone.
///
/// Written to disk unconditionally as the fallback path: it costs nothing
/// because the PNG encoder exists anyway, and it is the answer to a terminal
/// that renders the glyphs badly during recording.
pub fn image(qr: &Qr, scale: u32) -> Image {
    let scale = scale.max(1);
    let n = (qr.size + QUIET_ZONE * 2) as u32;
    let px = n * scale;
    let mut img = Image::new(px, px);
    img.px.fill(255);

    for r in 0..qr.size {
        for c in 0..qr.size {
            if !qr.dark(r, c) {
                continue;
            }
            let x0 = (c + QUIET_ZONE) as u32 * scale;
            let y0 = (r + QUIET_ZONE) as u32 * scale;
            for y in y0..y0 + scale {
                for x in x0..x0 + scale {
                    let i = (y as usize * px as usize + x as usize) * 3;
                    img.px[i] = 0;
                    img.px[i + 1] = 0;
                    img.px[i + 2] = 0;
                }
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qr;

    fn code() -> Qr {
        qr::encode("http://192.168.0.105:8080").unwrap()
    }

    #[test]
    fn the_quiet_zone_is_light_on_every_side() {
        let q = code();
        let img = image(&q, 1);
        let n = img.width as usize;
        // Top and bottom rows, left and right columns, all white.
        for k in 0..n {
            for (x, y) in [(k, 0), (k, n - 1), (0, k), (n - 1, k)] {
                let i = (y * n + x) * 3;
                assert_eq!(img.px[i], 255, "quiet zone not light at ({x},{y})");
            }
        }
    }

    #[test]
    fn image_scales_by_module() {
        let q = code();
        let n = (q.size + QUIET_ZONE * 2) as u32;
        for scale in [1u32, 4, 8] {
            let img = image(&q, scale);
            assert_eq!(img.width, n * scale);
            assert_eq!(img.height, n * scale);
            assert!(img.is_consistent());
        }
    }

    #[test]
    fn the_image_contains_dark_modules() {
        let img = image(&code(), 2);
        assert!(img.px.iter().any(|&v| v == 0), "no dark modules rendered");
    }

    /// The rendered PNG must survive our own encoder and decoder unchanged —
    /// a QR that is corrupted by the write path scans as nothing.
    #[test]
    fn renders_through_the_png_encoder_intact() {
        let img = image(&code(), 3);
        let png = crate::png::encode(&img);
        let back = crate::png::decode(&png).unwrap();
        assert_eq!(back.px, img.px);
    }
}
