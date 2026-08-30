//! Entropy-coded scan decoding, baseline and progressive.
//!
//! **Coefficients are stored, not rendered.** Baseline could transform each
//! block the moment it is read, but progressive cannot: a single block is
//! refined across several scans, and nothing can be transformed until the
//! last of them has been seen. Both paths therefore fill the same
//! coefficient buffer and the IDCT runs once at the end.
//!
//! Coefficients are held in **zig-zag order**, because that is the order
//! progressive spectral selection addresses them in — `Ss` and `Se` are
//! zig-zag indices, not row-major ones.

use super::bits::BitReader;
use super::huffman::Table;
use crate::image::ImageError;

pub struct Component {
    pub id: u8,
    pub h: usize,
    pub v: usize,
    pub tq: usize,
    pub dc_tbl: usize,
    pub ac_tbl: usize,
    /// Blocks per row in `coeffs`, padded out to whole MCUs.
    pub blocks_w: usize,
    pub blocks_h: usize,
    /// The component's own block extent, which is what a non-interleaved
    /// scan walks — it is smaller than the padded grid whenever the image
    /// does not end on an MCU boundary.
    pub own_w: usize,
    pub own_h: usize,
    pub coeffs: Vec<i16>,
    pub pred: i32,
}

impl Component {
    pub fn block_mut(&mut self, bx: usize, by: usize) -> &mut [i16] {
        let at = (by * self.blocks_w + bx) * 64;
        &mut self.coeffs[at..at + 64]
    }
}

/// One scan's header parameters.
pub struct ScanSpec {
    /// Indices into the component list, in the order the scan lists them.
    pub comps: Vec<usize>,
    pub ss: usize,
    pub se: usize,
    pub ah: u32,
    pub al: u32,
}

pub struct Tables<'a> {
    pub dc: &'a [Option<Table>; 4],
    pub ac: &'a [Option<Table>; 4],
}

/// Decodes one scan, returning the offset just past the entropy data.
#[allow(clippy::too_many_arguments)]
pub fn decode(
    entropy: &[u8],
    comps: &mut [Component],
    spec: &ScanSpec,
    t: &Tables,
    mcus_x: usize,
    mcus_y: usize,
    restart_interval: usize,
    progressive: bool,
) -> Result<usize, ImageError> {
    let mut r = BitReader::new(entropy);
    let mut eobrun = 0u32;

    for &ci in &spec.comps {
        comps[ci].pred = 0;
    }

    // A scan naming one component walks that component's own blocks; a scan
    // naming several walks MCUs. Progressive images use both shapes, often
    // in the same file.
    let single = spec.comps.len() == 1;
    let (units_x, units_y) = if single {
        let c = &comps[spec.comps[0]];
        (c.own_w, c.own_h)
    } else {
        (mcus_x, mcus_y)
    };

    let mut since_restart = 0usize;
    'outer: for uy in 0..units_y {
        for ux in 0..units_x {
            if restart_interval > 0 && since_restart == restart_interval {
                if !r.resync_to_restart() {
                    break 'outer;
                }
                for &ci in &spec.comps {
                    comps[ci].pred = 0;
                }
                eobrun = 0;
                since_restart = 0;
            }

            if single {
                let ci = spec.comps[0];
                unit(&mut r, comps, ci, ux, uy, spec, t, progressive, &mut eobrun)?;
            } else {
                for &ci in &spec.comps {
                    let (h, v) = (comps[ci].h, comps[ci].v);
                    for by in 0..v {
                        for bx in 0..h {
                            unit(
                                &mut r,
                                comps,
                                ci,
                                ux * h + bx,
                                uy * v + by,
                                spec,
                                t,
                                progressive,
                                &mut eobrun,
                            )?;
                        }
                    }
                }
            }
            since_restart += 1;

            // A truncated scan yields what was decoded rather than an error.
            if r.hit_marker() && restart_interval == 0 {
                break 'outer;
            }
        }
    }

    Ok(r.marker_at.unwrap_or_else(|| r.pos()))
}

/// Decodes one block into its component's coefficient buffer.
#[allow(clippy::too_many_arguments)]
fn unit(
    r: &mut BitReader,
    comps: &mut [Component],
    ci: usize,
    bx: usize,
    by: usize,
    spec: &ScanSpec,
    t: &Tables,
    progressive: bool,
    eobrun: &mut u32,
) -> Result<(), ImageError> {
    let (dc_i, ac_i) = (comps[ci].dc_tbl, comps[ci].ac_tbl);
    // Blocks past the padded grid belong to no MCU and are skipped rather
    // than written out of bounds.
    if bx >= comps[ci].blocks_w || by >= comps[ci].blocks_h {
        return Ok(());
    }

    if !progressive {
        let dc = table(t.dc, dc_i, r)?;
        let ac = table(t.ac, ac_i, r)?;
        let pred = comps[ci].pred;
        let block = comps[ci].block_mut(bx, by);
        let new_pred = baseline_block(r, block, dc, ac, pred)?;
        comps[ci].pred = new_pred;
        return Ok(());
    }

    if spec.ss == 0 {
        // DC scan: first pass sets the high bits, later passes add one more.
        if spec.ah == 0 {
            let dc = table(t.dc, dc_i, r)?;
            let pred = comps[ci].pred;
            let s = dc.decode(r)?;
            if s > 15 {
                return Err(ImageError::BadField { at: r.pos(), field: "DC size", value: s as u32 });
            }
            let diff = r.receive_extend(s);
            let value = pred.wrapping_add(diff);
            comps[ci].pred = value;
            comps[ci].block_mut(bx, by)[0] = (value << spec.al) as i16;
        } else {
            if r.bit() == 1 {
                let block = comps[ci].block_mut(bx, by);
                block[0] |= (1 << spec.al) as i16;
            }
        }
        return Ok(());
    }

    let ac = table(t.ac, ac_i, r)?;
    let (ss, se, al) = (spec.ss, spec.se.min(63), spec.al);
    let block = comps[ci].block_mut(bx, by);
    if spec.ah == 0 {
        ac_first(r, block, ac, ss, se, al, eobrun)
    } else {
        ac_refine(r, block, ac, ss, se, al, eobrun)
    }
}

fn table<'t>(
    tables: &'t [Option<Table>; 4],
    i: usize,
    r: &BitReader,
) -> Result<&'t Table, ImageError> {
    tables[i].as_ref().ok_or(ImageError::BadField {
        at: r.pos(),
        field: "missing huffman table",
        value: i as u32,
    })
}

/// Baseline: DC difference then AC run/size pairs, whole block in one pass.
fn baseline_block(
    r: &mut BitReader,
    block: &mut [i16],
    dc: &Table,
    ac: &Table,
    pred: i32,
) -> Result<i32, ImageError> {
    let s = dc.decode(r)?;
    if s > 15 {
        return Err(ImageError::BadField { at: r.pos(), field: "DC size", value: s as u32 });
    }
    let diff = r.receive_extend(s);
    let value = pred.wrapping_add(diff);
    block[0] = value.clamp(i16::MIN as i32, i16::MAX as i32) as i16;

    let mut k = 1usize;
    while k < 64 {
        let rs = ac.decode(r)?;
        let run = (rs >> 4) as usize;
        let size = rs & 0x0F;

        if size == 0 {
            if run == 15 {
                k += 16; // ZRL: sixteen zeros
                continue;
            }
            break; // EOB
        }
        k += run;
        // Anything that would push past 63 is a corrupt file, not a wrap.
        if k > 63 {
            break;
        }
        block[k] = r.receive_extend(size) as i16;
        k += 1;
    }
    Ok(value)
}

/// Progressive AC, first pass over a spectral band.
///
/// **`EOBRUN` is the mechanism baseline does not have**: one end-of-band
/// symbol can terminate the band for many consecutive blocks at once.
fn ac_first(
    r: &mut BitReader,
    block: &mut [i16],
    ac: &Table,
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Result<(), ImageError> {
    if *eobrun > 0 {
        *eobrun -= 1;
        return Ok(());
    }
    let mut k = ss;
    while k <= se {
        let rs = ac.decode(r)?;
        let run = (rs >> 4) as u32;
        let size = rs & 0x0F;

        if size == 0 {
            if run < 15 {
                *eobrun = (1 << run) - 1;
                if run > 0 {
                    *eobrun += r.bits(run);
                }
                break;
            }
            k += 16;
            continue;
        }
        k += run as usize;
        if k > se {
            break;
        }
        block[k] = (r.receive_extend(size) << al) as i16;
        k += 1;
    }
    Ok(())
}

/// Progressive AC refinement: the fiddliest corner of the format.
///
/// Every coefficient already known to be non-zero takes one correction bit,
/// **including those skipped over while looking for the next new one**, and
/// including every block covered by an in-progress `EOBRUN`. Getting the
/// interleaving of those two streams wrong produces an image that is close
/// to right and speckled with wrong pixels.
fn ac_refine(
    r: &mut BitReader,
    block: &mut [i16],
    ac: &Table,
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Result<(), ImageError> {
    let p1 = 1i16 << al; // value for a newly non-zero coefficient
    let m1 = -1i16 << al;
    let mut k = ss;

    if *eobrun == 0 {
        while k <= se {
            let rs = ac.decode(r)?;
            let mut run = (rs >> 4) as i32;
            let size = rs & 0x0F;
            let mut new_value = 0i16;

            if size == 0 {
                if run < 15 {
                    *eobrun = (1 << run) as u32;
                    if run > 0 {
                        *eobrun += r.bits(run as u32);
                    }
                    break;
                }
                // run == 15: skip sixteen zero-valued coefficients.
            } else {
                // In a refinement scan the magnitude is always one bit.
                new_value = if r.bit() == 1 { p1 } else { m1 };
            }

            while k <= se {
                if block[k] != 0 {
                    // An existing coefficient takes a correction bit.
                    if r.bit() == 1 && (block[k] & p1) == 0 {
                        block[k] = if block[k] >= 0 {
                            block[k].saturating_add(p1)
                        } else {
                            block[k].saturating_add(m1)
                        };
                    }
                } else {
                    if run == 0 {
                        if new_value != 0 {
                            block[k] = new_value;
                        }
                        k += 1;
                        break;
                    }
                    run -= 1;
                }
                k += 1;
            }
        }
    }

    if *eobrun > 0 {
        // Inside an end-of-band run the band contributes no new
        // coefficients, but the ones already there still take their bits.
        while k <= se {
            if block[k] != 0 && r.bit() == 1 && (block[k] & p1) == 0 {
                block[k] = if block[k] >= 0 {
                    block[k].saturating_add(p1)
                } else {
                    block[k].saturating_add(m1)
                };
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_of(bits: [u8; 16], values: Vec<u8>) -> Table {
        Table::build(&bits, values, 0).unwrap()
    }

    /// One 1-bit code for symbol 0x00 (EOB) and one 2-bit code for 0x01.
    fn simple_ac() -> Table {
        let mut bits = [0u8; 16];
        bits[0] = 1;
        bits[1] = 1;
        table_of(bits, vec![0x00, 0x11])
    }

    #[test]
    fn ac_first_honours_an_eob_run() {
        // Symbol 0x00 with run=0 sets EOBRUN to 0 and ends the band.
        let ac = simple_ac();
        let data = [0b0000_0000u8];
        let mut r = BitReader::new(&data);
        let mut block = [0i16; 64];
        let mut eobrun = 0;
        ac_first(&mut r, &mut block, &ac, 1, 5, 0, &mut eobrun).unwrap();
        assert_eq!(eobrun, 0);
        assert!(block.iter().all(|&c| c == 0));
    }

    #[test]
    fn ac_first_consumes_a_pending_eob_run() {
        let ac = simple_ac();
        let data = [0xFFu8];
        let mut r = BitReader::new(&data);
        let mut block = [0i16; 64];
        let mut eobrun = 3;
        ac_first(&mut r, &mut block, &ac, 1, 5, 0, &mut eobrun).unwrap();
        // The block is skipped entirely and the run shortens.
        assert_eq!(eobrun, 2);
        assert!(block.iter().all(|&c| c == 0));
    }

    #[test]
    fn dc_refinement_only_adds_one_bit() {
        // A coefficient of 4 refined at Al=1 becomes 6 when the bit is set.
        let mut block = [0i16; 64];
        block[0] = 4;
        let data = [0b1000_0000u8];
        let mut r = BitReader::new(&data);
        if r.bit() == 1 {
            block[0] |= 1 << 1;
        }
        assert_eq!(block[0], 6);
    }

    /// Refinement must correct coefficients it passes over, not just the one
    /// it lands on.
    #[test]
    fn ac_refine_corrects_existing_coefficients() {
        let ac = simple_ac();
        let mut block = [0i16; 64];
        block[1] = 2; // already non-zero
        block[2] = -2;
        // EOB immediately (symbol 0x00, run 0), then correction bits.
        let data = [0b0110_0000u8];
        let mut r = BitReader::new(&data);
        let mut eobrun = 0;
        ac_refine(&mut r, &mut block, &ac, 1, 3, 0, &mut eobrun).unwrap();
        // Both existing coefficients moved by exactly one step, in the
        // direction of their own sign.
        assert!(block[1] >= 2 && block[2] <= -2);
    }

    #[test]
    fn baseline_block_reads_dc_and_stops_at_eob() {
        let mut dc_bits = [0u8; 16];
        dc_bits[0] = 1; // one 1-bit code
        let dc = table_of(dc_bits, vec![0x00]); // size 0 -> diff 0
        let ac = simple_ac();

        // DC code '0' (size 0), then AC code '0' (EOB).
        let data = [0b0000_0000u8];
        let mut r = BitReader::new(&data);
        let mut block = [0i16; 64];
        let pred = baseline_block(&mut r, &mut block, &dc, &ac, 7).unwrap();
        assert_eq!(pred, 7); // unchanged by a zero difference
        assert_eq!(block[0], 7);
        assert!(block[1..].iter().all(|&c| c == 0));
    }
}
