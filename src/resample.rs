//! Box and bilinear resampling, separable two-pass. Replaces `image`'s
//! resize / `fast_image_resize`.
//!
//! **Honest cost:** this operates in gamma-encoded sRGB rather than linear
//! light. Technically wrong; visually imperceptible at thumbnail size.
//! Disclosed rather than discovered.

use crate::image::Image;

/// Thumbnails are 256 px on the long edge. Retina phones show the grid at
/// roughly 120 px CSS, so 256 covers 2x without storing 4x the bytes.
pub const THUMB_EDGE: u32 = 256;

/// Scales to fit `max_edge` on the long side, preserving aspect ratio.
pub fn thumbnail(img: &Image, max_edge: u32) -> Image {
    let (w, h) = (img.width, img.height);
    if w <= max_edge && h <= max_edge {
        return img.clone();
    }
    let (tw, th) = if w >= h {
        (max_edge, ((h as u64 * max_edge as u64) / w as u64).max(1) as u32)
    } else {
        (((w as u64 * max_edge as u64) / h as u64).max(1) as u32, max_edge)
    };
    resize(img, tw, th)
}

/// **Box down to within 2x of the target, then bilinear to the exact size.**
///
/// Bilinear alone from 4000 px to 256 px samples a tiny fraction of the
/// source pixels and produces aliased, sparkly thumbnails — the classic
/// naive-resize artifact.
pub fn resize(img: &Image, tw: u32, th: u32) -> Image {
    let tw = tw.max(1);
    let th = th.max(1);
    if img.width == tw && img.height == th {
        return img.clone();
    }

    let mut cur = img.clone();
    while cur.width / 2 >= tw && cur.height / 2 >= th && cur.width > 1 && cur.height > 1 {
        cur = halve(&cur);
    }
    bilinear(&cur, tw, th)
}

/// Exact 2x2 box reduction. Cheap, and correct for large ratios.
fn halve(img: &Image) -> Image {
    let nw = (img.width / 2).max(1);
    let nh = (img.height / 2).max(1);
    let mut out = Image::new(nw, nh);
    let sw = img.width as usize;

    for y in 0..nh as usize {
        for x in 0..nw as usize {
            let (x0, y0) = (x * 2, y * 2);
            let x1 = (x0 + 1).min(img.width as usize - 1);
            let y1 = (y0 + 1).min(img.height as usize - 1);
            let idx = |px: usize, py: usize| (py * sw + px) * 3;
            let (a, b, c, d) = (idx(x0, y0), idx(x1, y0), idx(x0, y1), idx(x1, y1));
            let o = (y * nw as usize + x) * 3;
            for k in 0..3 {
                let sum = img.px[a + k] as u32
                    + img.px[b + k] as u32
                    + img.px[c + k] as u32
                    + img.px[d + k] as u32;
                out.px[o + k] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

/// Separable bilinear: horizontal pass, then vertical.
fn bilinear(img: &Image, tw: u32, th: u32) -> Image {
    // Horizontal pass into an intermediate of the source height.
    let mut mid = Image::new(tw, img.height);
    let sw = img.width as usize;
    let x_ratio = if tw > 1 { (img.width as f32 - 1.0) / (tw as f32 - 1.0) } else { 0.0 };

    for y in 0..img.height as usize {
        for x in 0..tw as usize {
            let fx = x as f32 * x_ratio;
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let t = fx - x0 as f32;
            let a = (y * sw + x0) * 3;
            let b = (y * sw + x1) * 3;
            let o = (y * tw as usize + x) * 3;
            for k in 0..3 {
                let v = img.px[a + k] as f32 * (1.0 - t) + img.px[b + k] as f32 * t;
                mid.px[o + k] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    let mut out = Image::new(tw, th);
    let sh = img.height as usize;
    let y_ratio = if th > 1 { (sh as f32 - 1.0) / (th as f32 - 1.0) } else { 0.0 };

    for y in 0..th as usize {
        let fy = y as f32 * y_ratio;
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let t = fy - y0 as f32;
        for x in 0..tw as usize {
            let a = (y0 * tw as usize + x) * 3;
            let b = (y1 * tw as usize + x) * 3;
            let o = (y * tw as usize + x) * 3;
            for k in 0..3 {
                let v = mid.px[a + k] as f32 * (1.0 - t) + mid.px[b + k] as f32 * t;
                out.px[o + k] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Greyscale reduction for the perceptual hashes. Box filtering only —
/// precision is pointless at 32x32 and below.
pub fn grey_resize(img: &Image, tw: u32, th: u32) -> Vec<u8> {
    let small = resize(img, tw, th);
    let mut out = vec![0u8; (tw * th) as usize];
    for y in 0..th {
        for x in 0..tw {
            out[(y * tw + x) as usize] = small.luma(x, y);
        }
    }
    out
}

/// Applies an EXIF orientation, 1..=8.
///
/// **Applied before the thumbnail and before the hash.** Skip it and a
/// portrait photo thumbnails sideways *and* fails to cluster against its own
/// rotated copy — one bug that shows up as two unrelated ones.
pub fn apply_orientation(img: &Image, orientation: u8) -> Image {
    if orientation <= 1 || orientation > 8 {
        return img.clone();
    }
    // 5..=8 involve a transpose, so width and height swap.
    let swap = matches!(orientation, 5 | 6 | 7 | 8);
    let (ow, oh) = if swap { (img.height, img.width) } else { (img.width, img.height) };
    let mut out = Image::new(ow, oh);

    for y in 0..img.height {
        for x in 0..img.width {
            let (nx, ny) = match orientation {
                2 => (img.width - 1 - x, y),                 // mirror horizontal
                3 => (img.width - 1 - x, img.height - 1 - y), // 180
                4 => (x, img.height - 1 - y),                // mirror vertical
                5 => (y, x),                                 // transpose
                6 => (img.height - 1 - y, x),                // 90 CW
                7 => (img.height - 1 - y, img.width - 1 - x), // transverse
                8 => (y, img.width - 1 - x),                 // 90 CCW
                _ => (x, y),
            };
            let src = (y as usize * img.width as usize + x as usize) * 3;
            let dst = (ny as usize * ow as usize + nx as usize) * 3;
            out.px[dst..dst + 3].copy_from_slice(&img.px[src..src + 3]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> Image {
        let mut img = Image::new(w, h);
        for p in img.px.chunks_exact_mut(3) {
            p.copy_from_slice(&c);
        }
        img
    }

    fn hgradient(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (x * 255 / (w - 1).max(1)) as u8;
                let i = (y as usize * w as usize + x as usize) * 3;
                img.px[i] = v;
                img.px[i + 1] = v;
                img.px[i + 2] = v;
            }
        }
        img
    }

    #[test]
    fn a_solid_colour_survives_any_resize() {
        let img = solid(640, 480, [17, 200, 99]);
        for (w, h) in [(320u32, 240u32), (256, 192), (64, 48), (7, 5), (1, 1)] {
            let out = resize(&img, w, h);
            assert_eq!((out.width, out.height), (w, h));
            assert!(out.is_consistent());
            for p in out.px.chunks_exact(3) {
                assert_eq!(p, [17, 200, 99], "colour drifted at {w}x{h}");
            }
        }
    }

    #[test]
    fn thumbnail_preserves_aspect_ratio() {
        let out = thumbnail(&solid(1000, 500, [1, 2, 3]), 256);
        assert_eq!((out.width, out.height), (256, 128));

        let out = thumbnail(&solid(500, 1000, [1, 2, 3]), 256);
        assert_eq!((out.width, out.height), (128, 256));

        // Already small enough: untouched.
        let out = thumbnail(&solid(100, 80, [1, 2, 3]), 256);
        assert_eq!((out.width, out.height), (100, 80));
    }

    #[test]
    fn thumbnail_never_produces_a_zero_dimension() {
        let out = thumbnail(&solid(4000, 3, [9, 9, 9]), 256);
        assert!(out.width >= 1 && out.height >= 1);
        assert!(out.is_consistent());
    }

    #[test]
    fn downscaled_gradient_stays_monotonic() {
        let out = resize(&hgradient(512, 8), 64, 8);
        for x in 1..64u32 {
            assert!(
                out.get(x, 0)[0] >= out.get(x - 1, 0)[0],
                "not monotonic at {x}"
            );
        }
        // And it still spans most of the range.
        assert!(out.get(0, 0)[0] < 20 && out.get(63, 0)[0] > 235);
    }

    #[test]
    fn box_reduction_averages() {
        // A 2x2 checker of 0 and 255 must average to ~128, not to 0 or 255.
        let mut img = Image::new(2, 2);
        img.px = vec![0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0];
        let out = resize(&img, 1, 1);
        assert!((127..=128).contains(&out.px[0]), "got {}", out.px[0]);
    }

    #[test]
    fn grey_resize_produces_the_right_length() {
        let g = grey_resize(&solid(640, 480, [255, 255, 255]), 32, 32);
        assert_eq!(g.len(), 32 * 32);
        assert!(g.iter().all(|&v| v == 255));
    }

    #[test]
    fn orientation_1_is_identity() {
        let img = hgradient(4, 3);
        assert_eq!(apply_orientation(&img, 1).px, img.px);
    }

    #[test]
    fn orientation_swaps_dimensions_when_transposed() {
        let img = hgradient(4, 3);
        for o in [5u8, 6, 7, 8] {
            let out = apply_orientation(&img, o);
            assert_eq!((out.width, out.height), (3, 4), "orientation {o}");
            assert!(out.is_consistent());
        }
        for o in [2u8, 3, 4] {
            let out = apply_orientation(&img, o);
            assert_eq!((out.width, out.height), (4, 3), "orientation {o}");
        }
    }

    #[test]
    fn orientation_3_is_a_180_rotation() {
        let img = hgradient(4, 3);
        let out = apply_orientation(&img, 3);
        assert_eq!(out.get(0, 0), img.get(3, 2));
        assert_eq!(out.get(3, 2), img.get(0, 0));
    }

    #[test]
    fn orientation_6_rotates_90_clockwise() {
        let img = hgradient(4, 3);
        let out = apply_orientation(&img, 6);
        // The top-left of a 90-CW rotation is the bottom-left of the source.
        assert_eq!(out.get(0, 0), img.get(0, 2));
    }

    #[test]
    fn every_orientation_round_trips_through_its_inverse() {
        let img = hgradient(6, 4);
        // 2,4,5,7 are their own inverses; 6 and 8 invert each other; 3 too.
        for (o, inv) in [(2u8, 2u8), (3, 3), (4, 4), (5, 5), (6, 8), (7, 7), (8, 6)] {
            let there = apply_orientation(&img, o);
            let back = apply_orientation(&there, inv);
            assert_eq!(back.px, img.px, "orientation {o} then {inv}");
        }
    }

    #[test]
    fn out_of_range_orientation_is_ignored() {
        let img = hgradient(4, 3);
        assert_eq!(apply_orientation(&img, 0).px, img.px);
        assert_eq!(apply_orientation(&img, 9).px, img.px);
    }
}
