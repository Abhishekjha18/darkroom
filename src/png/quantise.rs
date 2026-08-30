//! Colour quantisation for palette PNG (colour type 3).
//!
//! Thumbnails are photographs, and lossless truecolour PNG is poor at those:
//! a 256x192 tile measures ~60 KB, which makes a cold 200-tile grid ~13 MB.
//! An 8-bit palette stores one byte per pixel instead of three and typically
//! halves that again after DEFLATE.
//!
//! **Nothing here is used unless it actually wins.** `png::encode_thumbnail`
//! encodes both ways and keeps the smaller, the same way the DEFLATE
//! compressor costs stored, static and dynamic blocks before choosing.

use crate::image::Image;

pub const MAX_COLOURS: usize = 256;

/// Colours are bucketed to 5 bits per channel while choosing the palette.
/// 32768 buckets is a small enough working set to median-cut quickly and
/// fine enough that the chosen palette is indistinguishable from one picked
/// over all 16.7 M.
const BUCKET_BITS: u32 = 5;
const BUCKET_SIDE: usize = 1 << BUCKET_BITS;
const BUCKETS: usize = BUCKET_SIDE * BUCKET_SIDE * BUCKET_SIDE;

pub struct Quantised {
    pub palette: Vec<[u8; 3]>,
    pub indices: Vec<u8>,
    /// True when the image had 256 or fewer distinct colours and the palette
    /// reproduces it exactly. Screenshots and flat graphics land here.
    pub exact: bool,
}

fn bucket_of(c: [u8; 3]) -> usize {
    let shift = 8 - BUCKET_BITS;
    ((c[0] as usize >> shift) << (2 * BUCKET_BITS))
        | ((c[1] as usize >> shift) << BUCKET_BITS)
        | (c[2] as usize >> shift)
}

/// Quantises to at most 256 colours.
/// **No dithering, measured rather than assumed.** Floyd-Steinberg error
/// diffusion was implemented and dropped: across the near-duplicate set it
/// produced files **36% larger** (184 KB against 135 KB) because the noise it
/// spreads is exactly what DEFLATE cannot model. What it buys is smoother
/// gradients — and these are 256 px tiles that a phone renders at ~112 px,
/// where the browser's own downscale hides the banding anyway. Undithered
/// nearest-colour also measures *closer* to the source than Pillow's
/// quantiser (mean 4.89 against 5.58).
pub fn quantise(img: &Image) -> Quantised {
    if let Some(q) = exact_palette(img) {
        return q;
    }
    let entries = histogram(img);
    let palette = median_cut(entries);
    let indices = map_nearest(img, &palette);
    Quantised { palette, indices, exact: false }
}

/// If the image has 256 or fewer distinct colours, the palette *is* the
/// image and the encoding is lossless.
fn exact_palette(img: &Image) -> Option<Quantised> {
    let mut seen: Vec<[u8; 3]> = Vec::with_capacity(MAX_COLOURS);
    for px in img.px.chunks_exact(3) {
        let c = [px[0], px[1], px[2]];
        if seen.binary_search(&c).is_err() {
            if seen.len() == MAX_COLOURS {
                return None;
            }
            let pos = seen.binary_search(&c).unwrap_err();
            seen.insert(pos, c);
        }
    }
    let indices = img
        .px
        .chunks_exact(3)
        .map(|px| seen.binary_search(&[px[0], px[1], px[2]]).unwrap() as u8)
        .collect();
    Some(Quantised { palette: seen, indices, exact: true })
}

/// A colour plus how many pixels it stands for.
#[derive(Clone, Copy)]
struct Entry {
    c: [u8; 3],
    n: u32,
}

fn histogram(img: &Image) -> Vec<Entry> {
    // Sums per bucket, so each bucket's representative is the mean of the
    // colours that fell in it rather than the corner of the cube.
    let mut sums = vec![[0u32; 4]; BUCKETS];
    for px in img.px.chunks_exact(3) {
        let b = bucket_of([px[0], px[1], px[2]]);
        sums[b][0] += px[0] as u32;
        sums[b][1] += px[1] as u32;
        sums[b][2] += px[2] as u32;
        sums[b][3] += 1;
    }
    sums.iter()
        .filter(|s| s[3] > 0)
        .map(|s| Entry {
            c: [
                (s[0] / s[3]).min(255) as u8,
                (s[1] / s[3]).min(255) as u8,
                (s[2] / s[3]).min(255) as u8,
            ],
            n: s[3],
        })
        .collect()
}

/// Median cut: repeatedly split the box with the widest colour spread along
/// its longest axis, at the population median.
fn median_cut(mut entries: Vec<Entry>) -> Vec<[u8; 3]> {
    if entries.is_empty() {
        return vec![[0, 0, 0]];
    }
    let mut boxes: Vec<(usize, usize)> = vec![(0, entries.len())];

    while boxes.len() < MAX_COLOURS {
        // Pick the box worth splitting: widest extent, weighted by how many
        // pixels it covers, so a large flat region does not hoard entries.
        let mut best: Option<(usize, u64)> = None;
        for (i, &(start, end)) in boxes.iter().enumerate() {
            if end - start < 2 {
                continue;
            }
            let (axis_len, _) = widest_axis(&entries[start..end]);
            let pop: u32 = entries[start..end].iter().map(|e| e.n).sum();
            // Split whichever box costs the most: how far its colours spread
            // times how many pixels are paying for that spread.
            let score = axis_len as u64 * pop as u64;
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((i, score));
            }
        }
        let Some((idx, _)) = best else { break };
        let (start, end) = boxes[idx];

        let (_, axis) = widest_axis(&entries[start..end]);
        entries[start..end].sort_unstable_by_key(|e| e.c[axis]);

        // Split where half the population lies, not half the entries: the
        // point is to balance pixels, not distinct colours.
        let total: u32 = entries[start..end].iter().map(|e| e.n).sum();
        let mut acc = 0u32;
        let mut split = start + 1;
        for i in start..end - 1 {
            acc += entries[i].n;
            if acc * 2 >= total {
                split = i + 1;
                break;
            }
            split = i + 2;
        }
        // **Both halves must be non-empty.** A single entry holding more
        // than half the box's pixels — a large flat region, which is
        // extremely common — walks this loop off the end, and an empty box
        // averages to black: it burns a palette slot and puts a colour in
        // the palette that appears nowhere in the image.
        let split = split.clamp(start + 1, end - 1);

        boxes[idx] = (start, split);
        boxes.push((split, end));
    }

    boxes
        .iter()
        .map(|&(start, end)| {
            let slice = &entries[start..end];
            let n: u64 = slice.iter().map(|e| e.n as u64).sum::<u64>().max(1);
            let mut acc = [0u64; 3];
            for e in slice {
                for k in 0..3 {
                    acc[k] += e.c[k] as u64 * e.n as u64;
                }
            }
            [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]
        })
        .collect()
}

fn widest_axis(entries: &[Entry]) -> (u8, usize) {
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    for e in entries {
        for k in 0..3 {
            lo[k] = lo[k].min(e.c[k]);
            hi[k] = hi[k].max(e.c[k]);
        }
    }
    let mut axis = 0;
    let mut len = 0u8;
    for k in 0..3 {
        let d = hi[k] - lo[k];
        if d > len {
            len = d;
            axis = k;
        }
    }
    (len, axis)
}

/// A lazily filled bucket-to-palette cache.
///
/// Building all 32768 entries up front would cost 8.4 M distance
/// computations per thumbnail; filling only the buckets an image actually
/// uses costs a small fraction of that.
struct NearestCache {
    slots: Vec<i16>,
}

impl NearestCache {
    fn new() -> Self {
        NearestCache { slots: vec![-1; BUCKETS] }
    }

    fn lookup(&mut self, palette: &[[u8; 3]], c: [u8; 3]) -> u8 {
        let b = bucket_of(c);
        if self.slots[b] >= 0 {
            return self.slots[b] as u8;
        }
        let mut best = 0usize;
        let mut best_d = u32::MAX;
        for (i, p) in palette.iter().enumerate() {
            let dr = c[0] as i32 - p[0] as i32;
            let dg = c[1] as i32 - p[1] as i32;
            let db = c[2] as i32 - p[2] as i32;
            let d = (dr * dr + dg * dg + db * db) as u32;
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        self.slots[b] = best as i16;
        best as u8
    }
}

fn map_nearest(img: &Image, palette: &[[u8; 3]]) -> Vec<u8> {
    let mut cache = NearestCache::new();
    img.px
        .chunks_exact(3)
        .map(|px| cache.lookup(palette, [px[0], px[1], px[2]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> Image {
        let mut img = Image::new(w, h);
        for px in img.px.chunks_exact_mut(3) {
            px.copy_from_slice(&c);
        }
        img
    }

    fn gradient(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 3;
                img.px[i] = (x * 255 / w.max(1)) as u8;
                img.px[i + 1] = (y * 255 / h.max(1)) as u8;
                img.px[i + 2] = ((x * y) % 256) as u8;
            }
        }
        img
    }

    /// Rebuilds the image from palette plus indices.
    fn rebuild(img: &Image, q: &Quantised) -> Image {
        let mut out = Image::new(img.width, img.height);
        for (i, &idx) in q.indices.iter().enumerate() {
            let c = q.palette[idx as usize];
            out.px[i * 3..i * 3 + 3].copy_from_slice(&c);
        }
        out
    }

    #[test]
    fn a_few_colours_quantise_exactly() {
        let mut img = Image::new(8, 8);
        for (i, px) in img.px.chunks_exact_mut(3).enumerate() {
            let v = (i % 5) as u8 * 50;
            px.copy_from_slice(&[v, 255 - v, 128]);
        }
        let q = quantise(&img);
        assert!(q.exact, "5 colours must be reproduced exactly");
        assert!(q.palette.len() <= 5);
        assert_eq!(rebuild(&img, &q).px, img.px);
    }

    #[test]
    fn a_solid_image_needs_one_colour() {
        let img = solid(32, 32, [17, 200, 99]);
        let q = quantise(&img);
        assert!(q.exact);
        assert_eq!(q.palette.len(), 1);
        assert!(q.indices.iter().all(|&i| i == 0));
    }

    #[test]
    fn exactly_256_colours_is_still_exact() {
        let mut img = Image::new(16, 16);
        for (i, px) in img.px.chunks_exact_mut(3).enumerate() {
            px.copy_from_slice(&[i as u8, 0, 0]);
        }
        let q = quantise(&img);
        assert!(q.exact);
        assert_eq!(q.palette.len(), 256);
        assert_eq!(rebuild(&img, &q).px, img.px);
    }

    #[test]
    fn more_than_256_colours_falls_back_to_median_cut() {
        let img = gradient(64, 64);
        let q = quantise(&img);
        assert!(!q.exact);
        assert!(q.palette.len() <= MAX_COLOURS);
        assert_eq!(q.indices.len(), img.pixels());
    }

    /// Quantisation is lossy, but it must stay close: a palette that drifts
    /// badly would break the thumbnails it exists to shrink.
    #[test]
    fn quantised_colours_stay_close_to_the_original() {
        // A synthetic gradient with a high-frequency blue channel is far
        // harder than a photograph: `oracles::quantiser_is_competitive_with_pillow`
        // measures the real case at mean 4.89 on a corpus thumbnail.
        let img = gradient(96, 96);
        let q = quantise(&img);
        let back = rebuild(&img, &q);

        let mut worst = 0i32;
        let mut total = 0i64;
        for (a, b) in img.px.iter().zip(back.px.iter()) {
            let d = (*a as i32 - *b as i32).abs();
            worst = worst.max(d);
            total += d as i64;
        }
        let mean = total as f64 / img.px.len() as f64;
        assert!(mean < 12.0, "mean drift {mean:.2} is too high");
        assert!(worst < 90, "worst drift {worst} is too high");
    }

    #[test]
    fn every_index_addresses_a_real_palette_entry() {
        let img = gradient(48, 33);
        let q = quantise(&img);
        assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));
    }

    #[test]
    fn degenerate_sizes_do_not_panic() {
        for (w, h) in [(1u32, 1u32), (1, 64), (64, 1), (3, 2)] {
            let q = quantise(&gradient(w, h));
            assert_eq!(q.indices.len(), (w * h) as usize);
        }
    }

    #[test]
    fn quantisation_is_deterministic() {
        let img = gradient(64, 64);
        let a = quantise(&img);
        let b = quantise(&img);
        assert_eq!(a.palette, b.palette);
        assert_eq!(a.indices, b.indices);
    }
}
