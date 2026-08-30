//! Perceptual hashing and near-duplicate clustering. Replaces `img_hash` /
//! `imagehash`.
//!
//! This is the payload. Everything else in darkroom is infrastructure for
//! one moment: a folder of thousands of photos collapsing into clusters,
//! and a number for how much space the duplicates are wasting.
//!
//! Byte-identical duplicates are unremarkable — every file manager finds
//! those. **Near-duplicates are the thing.**

use crate::image::Image;
use crate::resample::grey_resize;

/// Both hashes, both stored. dHash is nearly free and is a fast pre-filter;
/// pHash is the one that actually decides. Disagreement between them is
/// itself signal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sig {
    pub dhash: u64,
    pub phash: u64,
    /// pHashes of the same image turned 90, 180 and 270 degrees, so a
    /// pixel-rotated copy still matches its original. See `rotation_hashes`.
    pub rots: [u64; 3],
}

pub fn signature(img: &Image) -> Sig {
    Sig { dhash: dhash(img), phash: phash(img), rots: rotation_hashes(img) }
}

/// pHashes of the image turned 90, 180 and 270 degrees.
///
/// **This is what makes clustering rotation-invariant**, and it is far
/// cheaper than it sounds: the grey reduction is computed once and the three
/// rotations are index permutations of a 32x32 grid, so the cost is three
/// extra DCTs on 1024 samples — trivial beside decoding the JPEG that
/// produced them.
///
/// EXIF orientation is already applied before hashing, so this is not about
/// phone photos held sideways. It is about a copy whose *pixels* were
/// rotated — a crop-and-rotate saved beside its original.
fn rotation_hashes(img: &Image) -> [u64; 3] {
    const N: usize = 32;
    let g = grey_resize(img, N as u32, N as u32);
    let mut out = [0u64; 3];
    let mut cur = g;
    for slot in out.iter_mut() {
        cur = rotate90(&cur, N);
        *slot = phash_of_grey(&cur, N);
    }
    out
}

/// Rotates a square grey grid 90 degrees clockwise.
fn rotate90(g: &[u8], n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            out[x * n + (n - 1 - y)] = g[y * n + x];
        }
    }
    out
}

/// Gradient direction between horizontally adjacent pixels.
///
/// Encodes *relative* gradients, so it survives resizing and JPEG
/// re-encoding almost perfectly.
pub fn dhash(img: &Image) -> u64 {
    let g = grey_resize(img, 9, 8);
    let mut h = 0u64;
    let mut bit = 0;
    for y in 0..8usize {
        for x in 0..8usize {
            if g[y * 9 + x] > g[y * 9 + x + 1] {
                h |= 1 << bit;
            }
            bit += 1;
        }
    }
    h
}

/// DCT-based perceptual hash: 32x32 grey, 2-D DCT-II, the top-left 8x8 block
/// excluding the DC term, thresholded at the median.
pub fn phash(img: &Image) -> u64 {
    const N: usize = 32;
    phash_of_grey(&grey_resize(img, N as u32, N as u32), N)
}

/// The hash proper, over an already-reduced grey grid.
fn phash_of_grey(g: &[u8], n: usize) -> u64 {
    const N: usize = 32;
    debug_assert_eq!(n, N);

    // Separable DCT-II, rows then columns.
    let cos = dct_table::<N>();
    let mut rows = [[0f32; N]; N];
    for y in 0..N {
        for u in 0..N {
            let mut s = 0f32;
            for x in 0..N {
                s += g[y * N + x] as f32 * cos[u][x];
            }
            rows[y][u] = s;
        }
    }
    let mut out = [[0f32; N]; N];
    for u in 0..N {
        for v in 0..N {
            let mut s = 0f32;
            for y in 0..N {
                s += rows[y][u] * cos[v][y];
            }
            out[v][u] = s;
        }
    }

    // **Excluding [0][0].** The DC term is average brightness, and including
    // it makes the hash brightness-sensitive — the whole thing pHash exists
    // to fix.
    let mut vals = [0f32; 64];
    let mut n = 0;
    for v in 0..8 {
        for u in 0..8 {
            if u == 0 && v == 0 {
                continue;
            }
            vals[n] = out[v][u];
            n += 1;
        }
    }
    let coeffs = &vals[..n]; // 63 of them

    // **Median, not mean.** The mean is dragged by a single large
    // coefficient and produces degenerate all-ones or all-zeros hashes on
    // high-contrast images.
    let mut sorted = vals;
    let s = &mut sorted[..n];
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = s[n / 2];

    let mut h = 0u64;
    for (i, &c) in coeffs.iter().enumerate() {
        if c > median {
            h |= 1 << i;
        }
    }
    h
}

fn dct_table<const N: usize>() -> [[f32; N]; N] {
    let mut t = [[0f32; N]; N];
    for (u, row) in t.iter_mut().enumerate() {
        let cu = if u == 0 { (1.0f32 / N as f32).sqrt() } else { (2.0f32 / N as f32).sqrt() };
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = cu
                * ((2.0 * x as f32 + 1.0) * u as f32 * std::f32::consts::PI / (2.0 * N as f32))
                    .cos();
        }
    }
    t
}

/// One instruction on any modern CPU.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Distance between two signatures, **taking rotation into account**.
///
/// Only one side needs turning: rotations form a group, so comparing every
/// orientation of `a` against `b` upright covers all four relative angles.
pub fn distance(a: &Sig, b: &Sig) -> u32 {
    let mut best = hamming(a.phash, b.phash);
    for &r in &a.rots {
        best = best.min(hamming(r, b.phash));
    }
    best
}

pub struct Item {
    pub id: u64,
    /// `None` when the pixels never decoded — HEIC, or a corrupt file.
    ///
    /// **Absence is explicit rather than a magic value.** An all-zero
    /// signature was the obvious sentinel and it is wrong: a solid-colour
    /// image genuinely hashes to `0/0`, because every AC coefficient is zero
    /// and so is the median. Encoding "no signature" as a value a real image
    /// can produce silently drops blank screenshots and black frames out of
    /// clustering.
    pub sig: Option<Sig>,
    pub bytes: u64,
    /// Resolution, for picking the keeper.
    pub pixels: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Cluster {
    pub ids: Vec<u64>,
    /// The largest-resolution member, tie-broken by file size.
    pub best: u64,
    /// Total minus the keeper. **This is the number in the demo.**
    pub wasted_bytes: u64,
}

/// Distance at or below which two photos are the same photo.
pub const DEFAULT_THRESHOLD: u32 = 10;

/// Single-linkage clustering over the pHash distance, via union-find.
///
/// **Single-linkage chains, and that is a real failure mode**: A near B,
/// B near C, A far from C, all one cluster. The mitigation is a cap on
/// cluster diameter — a merge that would put two members more than
/// `2 * threshold` apart is rejected. It prevents the demo's worst possible
/// moment, which is one cluster containing everything.
pub fn cluster(items: &[Item], threshold: u32) -> Vec<Cluster> {
    let n = items.len();
    // Pull the pHashes out once; only entries that actually have one take
    // part in clustering.
    let ph: Vec<Option<Sig>> = items.iter().map(|i| i.sig).collect();
    let live: Vec<usize> = (0..n).filter(|&i| ph[i].is_some()).collect();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

    fn find(parent: &mut Vec<usize>, mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }

    // All candidate pairs, closest first, so tight matches merge before
    // loose ones and chaining has less room to start.
    let mut pairs: Vec<(u32, usize, usize)> = Vec::new();
    for (ai, &i) in live.iter().enumerate() {
        for &j in &live[ai + 1..] {
            let d = distance(&ph[i].unwrap(), &ph[j].unwrap());
            if d <= threshold {
                pairs.push((d, i, j));
            }
        }
    }
    pairs.sort_unstable();

    let cap = threshold * 2;
    for (_, i, j) in pairs {
        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
        if ri == rj {
            continue;
        }
        // Diameter check across the whole prospective union.
        let ok = members[ri].iter().all(|&a| {
            members[rj].iter().all(|&b| match (ph[a], ph[b]) {
                (Some(x), Some(y)) => distance(&x, &y) <= cap,
                _ => false,
            })
        });
        if !ok {
            continue;
        }
        let moved = std::mem::take(&mut members[rj]);
        members[ri].extend(moved);
        parent[rj] = ri;
    }

    let mut out = Vec::new();
    for r in 0..n {
        if find(&mut parent, r) != r || members[r].len() < 2 {
            continue;
        }
        let group = &members[r];
        // Keeper: highest resolution, tie-broken by file size.
        let best_idx = *group
            .iter()
            .max_by_key(|&&i| (items[i].pixels, items[i].bytes))
            .unwrap();
        let total: u64 = group.iter().map(|&i| items[i].bytes).sum();
        let mut ids: Vec<u64> = group.iter().map(|&i| items[i].id).collect();
        ids.sort_unstable();
        out.push(Cluster {
            ids,
            best: items[best_idx].id,
            wasted_bytes: total - items[best_idx].bytes,
        });
    }
    // Biggest saving first: that is the order the UI wants.
    out.sort_by(|a, b| b.wasted_bytes.cmp(&a.wasted_bytes).then(a.best.cmp(&b.best)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, v: u8) -> Image {
        let mut img = Image::new(w, h);
        img.px.fill(v);
        img
    }

    fn gradient(w: u32, h: u32, seed: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 3;
                let v = ((x * 7 + y * 13 + seed) % 256) as u8;
                img.px[i] = v;
                img.px[i + 1] = v.wrapping_add(40);
                img.px[i + 2] = v.wrapping_sub(30);
            }
        }
        img
    }

    #[test]
    fn hamming_counts_differing_bits() {
        assert_eq!(hamming(0, 0), 0);
        assert_eq!(hamming(0b1011, 0b1000), 2);
        assert_eq!(hamming(u64::MAX, 0), 64);
    }

    #[test]
    fn identical_images_hash_identically() {
        let a = gradient(200, 150, 3);
        assert_eq!(signature(&a), signature(&a.clone()));
    }

    /// The property the whole feature rests on: a scaled copy must hash
    /// close to its original.
    #[test]
    fn scaling_barely_moves_the_hash() {
        let big = gradient(640, 480, 11);
        let small = crate::resample::resize(&big, 160, 120);
        let d = hamming(phash(&big), phash(&small));
        assert!(d <= DEFAULT_THRESHOLD, "phash moved by {d}");
    }

    #[test]
    fn different_images_hash_far_apart() {
        let a = gradient(320, 240, 1);
        let mut b = Image::new(320, 240);
        for y in 0..240u32 {
            for x in 0..320u32 {
                let i = (y as usize * 320 + x as usize) * 3;
                let v = if (x / 20 + y / 20) % 2 == 0 { 20u8 } else { 230 };
                b.px[i] = v;
                b.px[i + 1] = v;
                b.px[i + 2] = v;
            }
        }
        assert!(hamming(phash(&a), phash(&b)) > DEFAULT_THRESHOLD);
    }

    /// A flat image has no structure; the hash must still be well-formed
    /// rather than degenerate in a way that clusters everything.
    #[test]
    fn a_flat_image_is_handled() {
        let s = signature(&solid(64, 64, 128));
        assert_eq!(s.dhash, 0); // no gradients anywhere
        let _ = s.phash;
    }

    #[test]
    fn degenerate_sizes_do_not_panic() {
        for (w, h) in [(1u32, 1u32), (2, 3), (1, 400), (400, 1)] {
            let _ = signature(&gradient(w, h, 5));
        }
    }

    fn item(id: u64, phash: u64, bytes: u64, pixels: u64) -> Item {
        Item { id, sig: Some(Sig { dhash: phash, phash, rots: [phash; 3] }), bytes, pixels }
    }

    #[test]
    fn clusters_close_items_and_leaves_far_ones_out() {
        let items = vec![
            item(1, 0b0000, 100, 1000),
            item(2, 0b0001, 50, 500),  // distance 1
            item(3, 0b0011, 40, 400),  // distance 2 from #1
            item(4, u64::MAX, 90, 900), // distance 64 - unrelated
        ];
        let cs = cluster(&items, 5);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].ids, vec![1, 2, 3]);
        // Keeper is the highest resolution.
        assert_eq!(cs[0].best, 1);
        assert_eq!(cs[0].wasted_bytes, 90);
    }

    #[test]
    fn a_lone_item_is_not_a_cluster() {
        let items = vec![item(1, 0, 10, 10), item(2, u64::MAX, 10, 10)];
        assert!(cluster(&items, 5).is_empty());
    }

    #[test]
    fn entries_without_a_signature_never_cluster() {
        // Three files whose pixels failed to decode. Without the guard they
        // would be a perfect cluster of "things that are equally unreadable".
        let items = vec![
            Item { id: 1, sig: None, bytes: 10, pixels: 1 },
            Item { id: 2, sig: None, bytes: 10, pixels: 1 },
            Item { id: 3, sig: None, bytes: 10, pixels: 1 },
        ];
        assert!(cluster(&items, 10).is_empty());
    }

    /// The bug the `Option` exists to prevent: a solid-colour image really
    /// does hash to 0/0, and two of them really are duplicates.
    #[test]
    fn solid_colour_images_still_cluster_with_each_other() {
        let black = signature(&solid(64, 64, 0));
        assert_eq!(black.dhash, 0, "premise of this test");
        assert_eq!(black.phash, 0, "premise of this test");

        let items = vec![
            Item { id: 1, sig: Some(black), bytes: 100, pixels: 4096 },
            Item { id: 2, sig: Some(black), bytes: 80, pixels: 4096 },
        ];
        let cs = cluster(&items, DEFAULT_THRESHOLD);
        assert_eq!(cs.len(), 1, "two identical blank images must cluster");
        assert_eq!(cs[0].ids, vec![1, 2]);
    }

    #[test]
    fn a_missing_signature_does_not_block_others() {
        let items = vec![
            Item { id: 1, sig: None, bytes: 10, pixels: 1 },
            item(2, 0b0011, 100, 1000),
            item(3, 0b0001, 50, 500),
        ];
        let cs = cluster(&items, 5);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].ids, vec![2, 3]);
    }

    /// The chaining guard: A-B and B-C are close, A-C is far.
    #[test]
    fn the_diameter_cap_stops_chaining() {
        let a = 0u64;
        let b = 0b0000_1111u64; // 4 from a
        let c = 0b1111_1111u64; // 4 from b, 8 from a
        let items = vec![item(1, a, 10, 10), item(2, b, 10, 10), item(3, c, 10, 10)];

        // Threshold 4, cap 8: A-C at exactly 8 is allowed.
        let cs = cluster(&items, 4);
        assert_eq!(cs[0].ids, vec![1, 2, 3]);

        // A longer chain must be refused rather than swallowing everything.
        let d = 0b1111_1111_1111u64; // 4 from c, 12 from a
        let items = vec![
            item(1, a, 10, 10),
            item(2, b, 10, 10),
            item(3, c, 10, 10),
            item(4, d, 10, 10),
        ];
        let cs = cluster(&items, 4);
        assert!(
            cs.iter().all(|c| c.ids.len() < 4),
            "chained into one cluster: {cs:?}"
        );
    }

    #[test]
    fn keeper_tie_breaks_on_file_size() {
        let items = vec![item(1, 0, 100, 1000), item(2, 0, 250, 1000)];
        let cs = cluster(&items, 2);
        assert_eq!(cs[0].best, 2);
        assert_eq!(cs[0].wasted_bytes, 100);
    }

    #[test]
    fn clusters_are_ordered_by_saving() {
        let items = vec![
            item(1, 0, 10, 10),
            item(2, 0, 10, 10),
            item(3, 0xFF00, 900, 10),
            item(4, 0xFF00, 900, 10),
        ];
        let cs = cluster(&items, 2);
        assert_eq!(cs.len(), 2);
        assert!(cs[0].wasted_bytes >= cs[1].wasted_bytes);
    }
}
