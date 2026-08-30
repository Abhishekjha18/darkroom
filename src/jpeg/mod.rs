//! JPEG decode, baseline and progressive (ITU-T T.81 + JFIF). Replaces
//! `jpeg-decoder` / `image` / `sharp` / `Pillow`.

pub mod bits;
pub mod color;
pub mod huffman;
pub mod idct;
pub mod scan;

use color::{ZIGZAG, ycbcr_to_rgb};
use huffman::Table;
use scan::{Component, ScanSpec, Tables};

use crate::image::{Image, ImageError, check_dimensions};

struct Quant {
    tables: [[u16; 64]; 4],
    set: [bool; 4],
}

impl Default for Quant {
    fn default() -> Self {
        Quant { tables: [[0u16; 64]; 4], set: [false; 4] }
    }
}

/// Reads a big-endian `u16`.
fn be16(d: &[u8], i: usize) -> Result<usize, ImageError> {
    if i + 1 >= d.len() {
        return Err(ImageError::Truncated { at: i, expected: "16-bit field" });
    }
    Ok(((d[i] as usize) << 8) | d[i + 1] as usize)
}

pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(ImageError::NotThisFormat);
    }

    let mut quant = Quant::default();
    let mut dc: [Option<Table>; 4] = [const { None }; 4];
    let mut ac: [Option<Table>; 4] = [const { None }; 4];
    let mut comps: Vec<Component> = Vec::new();
    let mut width = 0usize;
    let mut height = 0usize;
    let mut restart_interval = 0usize;
    let mut h_max = 1usize;
    let mut v_max = 1usize;
    let mut mcus_x = 0usize;
    let mut mcus_y = 0usize;
    let mut progressive = false;
    let mut scans = 0usize;

    let mut i = 2usize;
    while i < data.len() {
        // Markers may be preceded by any number of 0xFF fill bytes.
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        while i < data.len() && data[i] == 0xFF {
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        let marker = data[i];
        i += 1;

        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 => break, // EOI
            _ => {}
        }

        // **Segment length includes its own two bytes.** Off-by-two here
        // walks the parser into the middle of a segment and produces a
        // cascade of nonsense errors far from the real fault.
        let seg_len = be16(data, i)?;
        if seg_len < 2 || i + seg_len > data.len() {
            return Err(ImageError::Truncated { at: i, expected: "segment payload" });
        }
        let seg = &data[i + 2..i + seg_len];
        let seg_at = i;

        match marker {
            // SOF0 baseline, SOF1 extended sequential, SOF2 progressive.
            0xC0 | 0xC1 | 0xC2 => {
                if !comps.is_empty() {
                    return Err(ImageError::BadField { at: seg_at, field: "duplicate SOF", value: 0 });
                }
                progressive = marker == 0xC2;
                if seg.len() < 6 {
                    return Err(ImageError::Truncated { at: seg_at, expected: "SOF" });
                }
                if seg[0] != 8 {
                    return Err(ImageError::Unsupported { feature: "12-bit JPEG" });
                }
                height = ((seg[1] as usize) << 8) | seg[2] as usize;
                width = ((seg[3] as usize) << 8) | seg[4] as usize;
                let n = seg[5] as usize;
                if n != 1 && n != 3 {
                    return Err(ImageError::Unsupported { feature: "CMYK/YCCK JPEG" });
                }
                if seg.len() < 6 + n * 3 {
                    return Err(ImageError::Truncated { at: seg_at, expected: "SOF components" });
                }
                check_dimensions(width as u32, height as u32)?;

                let mut raw = Vec::with_capacity(n);
                for c in 0..n {
                    let b = &seg[6 + c * 3..9 + c * 3];
                    let h = (b[1] >> 4) as usize;
                    let v = (b[1] & 0x0F) as usize;
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                        return Err(ImageError::BadField {
                            at: seg_at,
                            field: "sampling factor",
                            value: b[1] as u32,
                        });
                    }
                    if b[2] as usize > 3 {
                        return Err(ImageError::BadField {
                            at: seg_at,
                            field: "quant table id",
                            value: b[2] as u32,
                        });
                    }
                    raw.push((b[0], h, v, b[2] as usize));
                }
                h_max = raw.iter().map(|r| r.1).max().unwrap_or(1);
                v_max = raw.iter().map(|r| r.2).max().unwrap_or(1);
                mcus_x = width.div_ceil(8 * h_max);
                mcus_y = height.div_ceil(8 * v_max);

                for (id, h, v, tq) in raw {
                    // Padded to whole MCUs, and cropped at the very end. An
                    // image 1281 px wide at 4:2:0 has a 16-px MCU and needs
                    // a 1296-px luma plane.
                    let blocks_w = mcus_x * h;
                    let blocks_h = mcus_y * v;
                    // The component's own extent, which a non-interleaved
                    // progressive scan walks instead of the padded grid.
                    let own_w = (width * h).div_ceil(h_max).div_ceil(8);
                    let own_h = (height * v).div_ceil(v_max).div_ceil(8);
                    comps.push(Component {
                        id,
                        h,
                        v,
                        tq,
                        dc_tbl: 0,
                        ac_tbl: 0,
                        blocks_w,
                        blocks_h,
                        own_w: own_w.min(blocks_w),
                        own_h: own_h.min(blocks_h),
                        coeffs: vec![0i16; blocks_w * blocks_h * 64],
                        pred: 0,
                    });
                }
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(ImageError::Unsupported { feature: "non-baseline JPEG mode" });
            }
            0xC4 => parse_dht(seg, seg_at, &mut dc, &mut ac)?,
            0xDB => parse_dqt(seg, seg_at, &mut quant)?,
            0xDD => restart_interval = be16(seg, 0)?,
            0xDA => {
                if comps.is_empty() {
                    return Err(ImageError::Truncated { at: seg_at, expected: "SOF before SOS" });
                }
                let spec = parse_sos(seg, seg_at, &mut comps, progressive)?;
                let scan_start = i + seg_len;
                let used = scan::decode(
                    &data[scan_start..],
                    &mut comps,
                    &spec,
                    &Tables { dc: &dc, ac: &ac },
                    mcus_x,
                    mcus_y,
                    restart_interval,
                    progressive,
                )?;
                scans += 1;
                // Baseline has exactly one scan; progressive has many, and
                // each one refines what the last left behind.
                if !progressive {
                    break;
                }
                i = scan_start + used;
                continue;
            }
            // APPn, COM and everything else with a payload: skip by length.
            _ => {}
        }

        i += seg_len;
    }

    if scans == 0 || width == 0 || height == 0 {
        return Err(ImageError::Truncated { at: data.len(), expected: "SOS scan" });
    }

    for c in &comps {
        if !quant.set[c.tq] {
            return Err(ImageError::BadField {
                at: 0,
                field: "undefined quant table",
                value: c.tq as u32,
            });
        }
    }

    let planes = render(&comps, &quant);
    Ok(assemble(&comps, &planes, width, height, h_max, v_max))
}

fn parse_sos(
    seg: &[u8],
    at: usize,
    comps: &mut [Component],
    progressive: bool,
) -> Result<ScanSpec, ImageError> {
    let ns = *seg.first().ok_or(ImageError::Truncated { at, expected: "SOS" })? as usize;
    if ns == 0 || ns > 4 || seg.len() < 1 + ns * 2 + 3 {
        return Err(ImageError::Truncated { at, expected: "SOS components" });
    }

    let mut chosen = Vec::with_capacity(ns);
    for s in 0..ns {
        let cid = seg[1 + s * 2];
        let tbl = seg[2 + s * 2];
        let Some(idx) = comps.iter().position(|c| c.id == cid) else {
            return Err(ImageError::BadField { at, field: "SOS component id", value: cid as u32 });
        };
        comps[idx].dc_tbl = (tbl >> 4) as usize & 3;
        comps[idx].ac_tbl = (tbl & 0x0F) as usize & 3;
        chosen.push(idx);
    }

    let tail = &seg[1 + ns * 2..];
    let (ss, se, a) = (tail[0] as usize, tail[1] as usize, tail[2]);
    let (ah, al) = ((a >> 4) as u32, (a & 0x0F) as u32);

    if progressive {
        if ss > 63 || se > 63 || (ss > 0 && se < ss) {
            return Err(ImageError::BadField { at, field: "spectral selection", value: ss as u32 });
        }
        // An AC scan may only ever name one component: the bands of
        // different components are not interleaved.
        if ss > 0 && chosen.len() != 1 {
            return Err(ImageError::BadField { at, field: "interleaved AC scan", value: ns as u32 });
        }
        Ok(ScanSpec { comps: chosen, ss, se, ah, al })
    } else {
        Ok(ScanSpec { comps: chosen, ss: 0, se: 63, ah: 0, al: 0 })
    }
}

fn parse_dqt(seg: &[u8], at: usize, q: &mut Quant) -> Result<(), ImageError> {
    let mut p = 0usize;
    // One segment may hold several tables.
    while p < seg.len() {
        let pq = seg[p] >> 4;
        let tq = (seg[p] & 0x0F) as usize;
        p += 1;
        if tq > 3 {
            return Err(ImageError::BadField { at, field: "quant table id", value: tq as u32 });
        }
        let n = if pq == 1 { 128 } else { 64 };
        if p + n > seg.len() {
            return Err(ImageError::Truncated { at, expected: "DQT payload" });
        }
        for k in 0..64 {
            q.tables[tq][k] = if pq == 1 {
                ((seg[p + k * 2] as u16) << 8) | seg[p + k * 2 + 1] as u16
            } else {
                seg[p + k] as u16
            };
        }
        q.set[tq] = true;
        p += n;
    }
    Ok(())
}

fn parse_dht(
    seg: &[u8],
    at: usize,
    dc: &mut [Option<Table>; 4],
    ac: &mut [Option<Table>; 4],
) -> Result<(), ImageError> {
    let mut p = 0usize;
    while p < seg.len() {
        let tc = seg[p] >> 4;
        let th = (seg[p] & 0x0F) as usize;
        p += 1;
        if tc > 1 || th > 3 {
            return Err(ImageError::BadField { at, field: "DHT class/id", value: seg[p - 1] as u32 });
        }
        if p + 16 > seg.len() {
            return Err(ImageError::Truncated { at, expected: "DHT counts" });
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&seg[p..p + 16]);
        p += 16;

        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if p + total > seg.len() {
            return Err(ImageError::Truncated { at, expected: "DHT values" });
        }
        let table = Table::build(&counts, seg[p..p + total].to_vec(), at)?;
        p += total;

        if tc == 0 {
            dc[th] = Some(table);
        } else {
            ac[th] = Some(table);
        }
    }
    Ok(())
}

/// Dequantises and transforms every block, once all scans are in.
fn render(comps: &[Component], quant: &Quant) -> Vec<Vec<u8>> {
    let mut planes = Vec::with_capacity(comps.len());
    let mut natural = [0i32; 64];
    let mut samples = [0u8; 64];

    for c in comps {
        let pw = c.blocks_w * 8;
        let ph = c.blocks_h * 8;
        let mut plane = vec![128u8; pw * ph];
        let q = &quant.tables[c.tq];

        for by in 0..c.blocks_h {
            for bx in 0..c.blocks_w {
                let at = (by * c.blocks_w + bx) * 64;
                let block = &c.coeffs[at..at + 64];
                // Coefficients are stored zig-zagged, so dequantising and
                // de-zigzagging are the same loop.
                for k in 0..64 {
                    natural[ZIGZAG[k]] = block[k] as i32 * q[k] as i32;
                }
                idct::idct8x8(&natural, &mut samples);

                for row in 0..8 {
                    let dst = (by * 8 + row) * pw + bx * 8;
                    plane[dst..dst + 8].copy_from_slice(&samples[row * 8..row * 8 + 8]);
                }
            }
        }
        planes.push(plane);
    }
    planes
}

/// Upsamples chroma and converts to RGB, cropping the whole-MCU planes to
/// the declared dimensions.
fn assemble(
    comps: &[Component],
    planes: &[Vec<u8>],
    width: usize,
    height: usize,
    h_max: usize,
    v_max: usize,
) -> Image {
    let mut img = Image::new(width as u32, height as u32);

    if comps.len() == 1 {
        // Greyscale: replicate luma, skipping colour conversion entirely.
        let c = &comps[0];
        let pw = c.blocks_w * 8;
        for y in 0..height {
            for x in 0..width {
                let v = planes[0][y.min(c.blocks_h * 8 - 1) * pw + x.min(pw - 1)];
                let i = (y * width + x) * 3;
                img.px[i] = v;
                img.px[i + 1] = v;
                img.px[i + 2] = v;
            }
        }
        return img;
    }

    for y in 0..height {
        for x in 0..width {
            // Nearest-neighbour upsampling. Fancy (triangular) upsampling is
            // visibly better on hard edges and not worth the lines at
            // 256-px thumbnail scale.
            let s = |ci: usize| -> u8 {
                let c = &comps[ci];
                let pw = c.blocks_w * 8;
                let sx = (x * c.h / h_max).min(pw - 1);
                let sy = (y * c.v / v_max).min(c.blocks_h * 8 - 1);
                planes[ci][sy * pw + sx]
            };
            let [r, g, b] = ycbcr_to_rgb(s(0), s(1), s(2));
            let i = (y * width + x) * 3;
            img.px[i] = r;
            img.px[i + 1] = g;
            img.px[i + 2] = b;
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_non_jpeg() {
        assert!(matches!(decode(b"not a jpeg"), Err(ImageError::NotThisFormat)));
        assert!(matches!(decode(&[]), Err(ImageError::NotThisFormat)));
        assert!(matches!(decode(&[0xFF, 0xD8]), Err(ImageError::NotThisFormat)));
    }

    #[test]
    fn rejects_absurd_dimensions_before_allocating() {
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
        d.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 1, 1, 0x11, 0]);
        assert!(matches!(decode(&d), Err(ImageError::TooLarge { .. })));
    }

    #[test]
    fn rejects_a_bad_quant_table_id() {
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x09];
        d.extend_from_slice(&[0u8; 64]);
        assert!(decode(&d).is_err());
    }

    #[test]
    fn truncated_segment_is_an_error_not_a_panic() {
        let d = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0xFF, 0x08];
        assert!(matches!(decode(&d), Err(ImageError::Truncated { .. })));
    }

    /// Progressive is no longer refused: a SOF2 header must be accepted and
    /// only fail later for reasons unrelated to being progressive.
    #[test]
    fn progressive_is_no_longer_unsupported() {
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xC2, 0x00, 0x0B, 0x08];
        d.extend_from_slice(&[0, 16, 0, 16, 1, 1, 0x11, 0]);
        d.extend_from_slice(&[0xFF, 0xD9]);
        match decode(&d) {
            Err(ImageError::Unsupported { feature }) => {
                panic!("progressive should be supported, got {feature}")
            }
            _ => {}
        }
    }

    #[test]
    fn still_refuses_modes_that_were_never_implemented() {
        // SOF3, lossless.
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x0B, 0x08];
        d.extend_from_slice(&[0, 16, 0, 16, 1, 1, 0x11, 0]);
        assert!(matches!(decode(&d), Err(ImageError::Unsupported { .. })));
    }
}
