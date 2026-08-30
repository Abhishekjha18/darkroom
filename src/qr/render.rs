//! Terminal and PNG rendering of a QR code.

use super::Qr;
use crate::image::Image;

/// **Mandatory, on every side.** It is the first thing an implementation
/// drops when the code doesn't fit the window — and without it phones fail
/// to detect the code at all, a failure that looks like a bad encoder rather
/// than bad framing.
pub const QUIET_ZONE: usize = 4;

const RESET: &str = "\x1b[0m";
const WHITE_BG: &str = "\x1b[47m";
const BLACK_BG: &str = "\x1b[40m";
const WHITE_FG: &str = "\x1b[37m";
const BLACK_FG: &str = "\x1b[30m";

/// Renders with **half-block glyphs, two QR rows per text row**.
///
/// Terminal cells are roughly 1:2, so full-block rendering produces a code
/// twice as tall as it is wide, which many scanners reject. Half-blocks with
/// foreground and background colours give square modules.
///
/// `invert` swaps the polarity: on a dark-themed terminal the naive
/// rendering comes out inverted, and **most scanners refuse inverted codes**.
pub fn terminal(qr: &Qr, invert: bool) -> String {
    let n = qr.size + QUIET_ZONE * 2;
    let dark = |row: usize, col: usize| -> bool {
        if row < QUIET_ZONE || col < QUIET_ZONE {
            return false;
        }
        let (r, c) = (row - QUIET_ZONE, col - QUIET_ZONE);
        r < qr.size && c < qr.size && qr.dark(r, c)
    };

    let (d_fg, l_fg) = if invert { (WHITE_FG, BLACK_FG) } else { (BLACK_FG, WHITE_FG) };
    let (d_bg, l_bg) = if invert { (WHITE_BG, BLACK_BG) } else { (BLACK_BG, WHITE_BG) };

    let mut out = String::with_capacity(n * n * 4);
    for row in (0..n).step_by(2) {
        for col in 0..n {
            // The upper half-block paints the foreground on the top row and
            // the background on the bottom one.
            let top = dark(row, col);
            let bottom = row + 1 < n && dark(row + 1, col);
            out.push_str(if top { d_fg } else { l_fg });
            out.push_str(if bottom { d_bg } else { l_bg });
            out.push('\u{2580}'); // upper half block
        }
        out.push_str(RESET);
        out.push('\n');
    }
    out
}

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
    fn terminal_output_has_the_right_shape() {
        let q = code();
        let s = terminal(&q, false);
        let lines: Vec<&str> = s.lines().collect();
        let n = q.size + QUIET_ZONE * 2;
        // Two module rows per text row, rounded up.
        assert_eq!(lines.len(), n.div_ceil(2));
        // Every line carries one glyph per column.
        for l in &lines {
            assert_eq!(l.matches('\u{2580}').count(), n);
        }
    }

    #[test]
    fn inverting_swaps_the_colour_codes() {
        let q = code();
        let normal = terminal(&q, false);
        let inverted = terminal(&q, true);
        assert_ne!(normal, inverted);
        assert_eq!(normal.len(), inverted.len());
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
