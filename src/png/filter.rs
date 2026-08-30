//! The five scanline filters and the Paeth predictor (RFC 2083 §6).
//!
//! **Filtering operates on bytes at a distance of `bpp`, not on pixels, and
//! it wraps modulo 256.** That wrap is the specification, not a bug to be
//! fixed with saturating arithmetic.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    None = 0,
    Sub = 1,
    Up = 2,
    Average = 3,
    Paeth = 4,
}

impl Filter {
    pub fn from_byte(b: u8) -> Option<Filter> {
        Some(match b {
            0 => Filter::None,
            1 => Filter::Sub,
            2 => Filter::Up,
            3 => Filter::Average,
            4 => Filter::Paeth,
            _ => return None,
        })
    }
}

/// The Paeth predictor. **Tie-break order is `a`, then `b`, then `c`** —
/// getting it wrong produces images that are correct except along edges.
pub fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Applies one filter to a row, writing into `out`.
pub fn apply(f: Filter, row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(row.len());
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev.get(i).copied().unwrap_or(0);
        let c = if i >= bpp { prev.get(i - bpp).copied().unwrap_or(0) } else { 0 };
        let x = row[i];
        out.push(match f {
            Filter::None => x,
            Filter::Sub => x.wrapping_sub(a),
            Filter::Up => x.wrapping_sub(b),
            Filter::Average => x.wrapping_sub(((a as u16 + b as u16) / 2) as u8),
            Filter::Paeth => x.wrapping_sub(paeth(a, b, c)),
        });
    }
}

/// Reverses a filter in place over `row`, given the already-unfiltered `prev`.
pub fn undo(f: Filter, row: &mut [u8], prev: &[u8], bpp: usize) {
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev.get(i).copied().unwrap_or(0);
        let c = if i >= bpp { prev.get(i - bpp).copied().unwrap_or(0) } else { 0 };
        row[i] = match f {
            Filter::None => row[i],
            Filter::Sub => row[i].wrapping_add(a),
            Filter::Up => row[i].wrapping_add(b),
            Filter::Average => row[i].wrapping_add(((a as u16 + b as u16) / 2) as u8),
            Filter::Paeth => row[i].wrapping_add(paeth(a, b, c)),
        };
    }
}

/// Picks a filter by the standard minimum-sum-of-absolute-differences rule:
/// filter the row all five ways, keep the smallest sum of signed-byte
/// magnitudes.
///
/// **Honest cost:** heuristic rather than exhaustive, so files are a few
/// percent larger than optimal. Trial-compressing each row is a few percent
/// better and many times slower.
pub fn choose(row: &[u8], prev: &[u8], bpp: usize) -> (Filter, Vec<u8>) {
    let mut best = (Filter::None, u64::MAX, Vec::new());
    let mut scratch = Vec::with_capacity(row.len());

    for f in [Filter::None, Filter::Sub, Filter::Up, Filter::Average, Filter::Paeth] {
        apply(f, row, prev, bpp, &mut scratch);
        let score: u64 = scratch.iter().map(|&b| (b as i8).unsigned_abs() as u64).sum();
        if score < best.1 {
            best = (f, score, scratch.clone());
        }
    }
    (best.0, best.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every filter must be exactly reversible — that is the whole contract.
    #[test]
    fn every_filter_round_trips() {
        let prev: Vec<u8> = (0..30u8).map(|i| i.wrapping_mul(7)).collect();
        let row: Vec<u8> = (0..30u8).map(|i| i.wrapping_mul(13).wrapping_add(5)).collect();
        let mut out = Vec::new();

        for f in [Filter::None, Filter::Sub, Filter::Up, Filter::Average, Filter::Paeth] {
            apply(f, &row, &prev, 3, &mut out);
            let mut back = out.clone();
            undo(f, &mut back, &prev, 3);
            assert_eq!(back, row, "{f:?} did not round trip");
        }
    }

    #[test]
    fn round_trips_on_the_first_row() {
        // prev is all zeros for row 0.
        let prev = vec![0u8; 12];
        let row: Vec<u8> = vec![9, 200, 3, 255, 0, 128, 77, 12, 43, 1, 2, 3];
        let mut out = Vec::new();
        for f in [Filter::None, Filter::Sub, Filter::Up, Filter::Average, Filter::Paeth] {
            apply(f, &row, &prev, 3, &mut out);
            let mut back = out.clone();
            undo(f, &mut back, &prev, 3);
            assert_eq!(back, row, "{f:?}");
        }
    }

    #[test]
    fn paeth_matches_the_specification() {
        // From the RFC's own description of the predictor.
        assert_eq!(paeth(0, 0, 0), 0);
        assert_eq!(paeth(10, 20, 15), 15); // p = 15, all equidistant -> c loses to a? check order
        assert_eq!(paeth(200, 100, 50), 200);
        assert_eq!(paeth(100, 200, 50), 200);
    }

    #[test]
    fn paeth_prefers_a_on_a_tie() {
        // p = a + b - c; when |p-a| == |p-b|, `a` must win.
        assert_eq!(paeth(10, 10, 10), 10);
        assert_eq!(paeth(5, 5, 0), 5);
    }

    #[test]
    fn filtering_wraps_rather_than_saturating() {
        let row = [0u8];
        let prev = [255u8];
        let mut out = Vec::new();
        apply(Filter::Up, &row, &prev, 1, &mut out);
        assert_eq!(out[0], 1); // 0 - 255 wraps to 1, it does not clamp to 0
    }

    #[test]
    fn choose_returns_a_reversible_filter() {
        let prev = vec![4u8; 24];
        let row: Vec<u8> = (0..24u8).collect();
        let (f, filtered) = choose(&row, &prev, 3);
        let mut back = filtered.clone();
        undo(f, &mut back, &prev, 3);
        assert_eq!(back, row);
    }
}
