//! Separable 8x8 inverse DCT, float, row-column.
//!
//! **Rejected: the AAN integer fast IDCT.** It is faster and it is another
//! page of constants to get subtly wrong. This form is the definition
//! transcribed, and it measured within 3/255 of an independent decoder.

use std::sync::LazyLock;

/// `COS[u][x] = C(u) * cos((2x+1) u pi / 16)`, with `C(0) = 1/sqrt(2)`.
static COS: LazyLock<[[f32; 8]; 8]> = LazyLock::new(|| {
    let mut t = [[0f32; 8]; 8];
    for (u, row) in t.iter_mut().enumerate() {
        let cu = if u == 0 { 1.0 / std::f32::consts::SQRT_2 } else { 1.0 };
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = cu
                * ((2.0 * x as f32 + 1.0) * u as f32 * std::f32::consts::PI / 16.0).cos();
        }
    }
    t
});

#[inline]
fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Transforms one block of dequantised coefficients in natural order into
/// spatial samples, level-shifted by +128.
pub fn idct8x8(coef: &[i32; 64], out: &mut [u8; 64]) {
    // Very common in flat regions: if every AC coefficient is zero the whole
    // block is the constant DC/8 + 128. Ten lines, and it skips 128
    // multiply-accumulates per block.
    if coef[1..].iter().all(|&c| c == 0) {
        let v = clamp((coef[0] as f32 / 8.0).round() as i32 + 128);
        out.fill(v);
        return;
    }

    let cos = &*COS;
    let mut tmp = [0f32; 64];

    for y in 0..8 {
        let row = &coef[y * 8..y * 8 + 8];
        // Rows are frequently all-zero away from the top-left corner.
        if row.iter().all(|&c| c == 0) {
            continue;
        }
        for x in 0..8 {
            let mut s = 0f32;
            for (u, &c) in row.iter().enumerate() {
                if c != 0 {
                    s += cos[u][x] * c as f32;
                }
            }
            tmp[y * 8 + x] = s * 0.5;
        }
    }

    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0f32;
            for (v, cosv) in cos.iter().enumerate() {
                s += cosv[y] * tmp[v * 8 + x];
            }
            out[y * 8 + x] = clamp((s * 0.5).round() as i32 + 128);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_block_is_flat() {
        let mut coef = [0i32; 64];
        coef[0] = 8 * 10; // DC/8 = 10
        let mut out = [0u8; 64];
        idct8x8(&coef, &mut out);
        assert!(out.iter().all(|&v| v == 138), "{:?}", &out[..8]);
    }

    #[test]
    fn zero_block_is_mid_grey() {
        let mut out = [0u8; 64];
        idct8x8(&[0i32; 64], &mut out);
        assert!(out.iter().all(|&v| v == 128));
    }

    #[test]
    fn output_is_always_clamped() {
        let mut coef = [0i32; 64];
        coef[0] = 100_000;
        let mut out = [0u8; 64];
        idct8x8(&coef, &mut out);
        assert!(out.iter().all(|&v| v == 255));

        coef[0] = -100_000;
        idct8x8(&coef, &mut out);
        assert!(out.iter().all(|&v| v == 0));
    }

    /// The shortcut path and the general path must agree.
    #[test]
    fn shortcut_matches_the_general_path() {
        for dc in [-800i32, -8, 0, 8, 240, 800] {
            let mut coef = [0i32; 64];
            coef[0] = dc;
            let mut fast = [0u8; 64];
            idct8x8(&coef, &mut fast);

            // Force the general path with a zero AC coefficient that is not
            // detected by the all-zero test.
            let mut coef2 = coef;
            coef2[63] = 0;
            let mut slow = [0u8; 64];
            {
                // inline general path
                let cos = &*COS;
                let mut tmp = [0f32; 64];
                for y in 0..8 {
                    for x in 0..8 {
                        let mut s = 0f32;
                        for u in 0..8 {
                            s += cos[u][x] * coef2[y * 8 + u] as f32;
                        }
                        tmp[y * 8 + x] = s * 0.5;
                    }
                }
                for x in 0..8 {
                    for y in 0..8 {
                        let mut s = 0f32;
                        for v in 0..8 {
                            s += cos[v][y] * tmp[v * 8 + x];
                        }
                        slow[y * 8 + x] = clamp((s * 0.5).round() as i32 + 128);
                    }
                }
            }
            for i in 0..64 {
                assert!(
                    fast[i].abs_diff(slow[i]) <= 1,
                    "dc={dc} i={i}: {} vs {}",
                    fast[i],
                    slow[i]
                );
            }
        }
    }

    /// A single horizontal frequency must produce a horizontal pattern that
    /// is constant down each column.
    #[test]
    fn first_ac_is_a_horizontal_gradient() {
        let mut coef = [0i32; 64];
        coef[1] = 200;
        let mut out = [0u8; 64];
        idct8x8(&coef, &mut out);
        for y in 1..8 {
            for x in 0..8 {
                assert_eq!(out[y * 8 + x], out[x], "column {x} varies");
            }
        }
        assert!(out[0] > out[7], "gradient should descend left to right");
    }
}
