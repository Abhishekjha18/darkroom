//! GIF decode (GIF89a), including LZW. Replaces the GIF half of `image`.
//!
//! **First frame only.** darkroom is a photo gallery: an animated GIF gets
//! the frame a viewer would see first, which is the same thing a contact
//! sheet would show. Decoding every frame would buy nothing a thumbnail can
//! display.
//!
//! The format was cut in `FEATURES.md` §2.7 as "nice, not needed" — and then
//! a real 414-file library turned out to contain two of them, which were the
//! only files in it darkroom could not read. Two files is not many; being
//! the *only* failure is what made it worth the ~250 lines.

use crate::image::{Image, ImageError, check_dimensions};

/// LZW codes are capped at 12 bits by the specification.
const MAX_CODE_BITS: u32 = 12;
const MAX_CODES: usize = 1 << MAX_CODE_BITS;

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let mut r = Cursor::new(bytes);

    // Header: "GIF87a" or "GIF89a".
    let sig = r.take(6)?;
    if &sig[..3] != b"GIF" {
        return Err(ImageError::NotThisFormat);
    }

    // Logical screen descriptor.
    let screen_w = r.u16()? as u32;
    let screen_h = r.u16()? as u32;
    let flags = r.u8()?;
    let _bg = r.u8()?;
    let _aspect = r.u8()?;

    check_dimensions(screen_w.max(1), screen_h.max(1))?;

    let global: Option<Vec<[u8; 3]>> = if flags & 0x80 != 0 {
        let n = 2usize << (flags & 0x07);
        Some(palette(&mut r, n)?)
    } else {
        None
    };

    // Walk blocks until the first image descriptor.
    let mut transparent: Option<u8> = None;
    loop {
        match r.u8()? {
            // Extension introducer.
            0x21 => {
                let label = r.u8()?;
                if label == 0xF9 {
                    // Graphic control extension: it carries the transparent
                    // colour index, which has to be known before the frame
                    // is painted.
                    let size = r.u8()? as usize;
                    let body = r.take(size)?;
                    if size >= 4 && body[0] & 0x01 != 0 {
                        transparent = Some(body[3]);
                    }
                    skip_sub_blocks(&mut r)?;
                } else {
                    // Comment, plain text, application: all skippable.
                    skip_sub_blocks(&mut r)?;
                }
            }
            // Image separator: the frame we want.
            0x2C => break,
            // Trailer before any image.
            0x3B => {
                return Err(ImageError::Truncated { at: r.pos, expected: "GIF image block" });
            }
            other => {
                return Err(ImageError::BadField {
                    at: r.pos,
                    field: "GIF block introducer",
                    value: other as u32,
                });
            }
        }
    }

    // Image descriptor.
    let left = r.u16()? as u32;
    let top = r.u16()? as u32;
    let w = r.u16()? as u32;
    let h = r.u16()? as u32;
    let local_flags = r.u8()?;

    let local: Option<Vec<[u8; 3]>> = if local_flags & 0x80 != 0 {
        let n = 2usize << (local_flags & 0x07);
        Some(palette(&mut r, n)?)
    } else {
        None
    };
    // A local table takes precedence over the global one.
    let table = local
        .or(global)
        .ok_or(ImageError::Truncated { at: r.pos, expected: "GIF colour table" })?;

    let interlaced = local_flags & 0x40 != 0;
    if w == 0 || h == 0 {
        return Err(ImageError::BadField { at: r.pos, field: "GIF frame size", value: 0 });
    }
    check_dimensions(w, h)?;

    let indices = lzw(&mut r, (w as usize) * (h as usize))?;

    // The frame may be smaller than the logical screen and offset within it.
    // Anything the frame does not cover stays white, which is what
    // compositing transparency onto white does anyway.
    let out_w = screen_w.max(left + w);
    let out_h = screen_h.max(top + h);
    check_dimensions(out_w, out_h)?;

    let mut img = Image::new(out_w, out_h);
    img.px.fill(255);

    for row in 0..h as usize {
        // Interlaced GIFs store rows in four passes.
        let dst_row = if interlaced { deinterlace(row, h as usize) } else { row };
        for col in 0..w as usize {
            let Some(&idx) = indices.get(row * w as usize + col) else { continue };
            if Some(idx) == transparent {
                continue; // leave the white background showing
            }
            let c = table.get(idx as usize).copied().unwrap_or([0, 0, 0]);
            let x = left as usize + col;
            let y = top as usize + dst_row;
            if x >= out_w as usize || y >= out_h as usize {
                continue;
            }
            let at = (y * out_w as usize + x) * 3;
            img.px[at..at + 3].copy_from_slice(&c);
        }
    }
    Ok(img)
}

/// Maps a row of an interlaced frame to its real position.
fn deinterlace(row: usize, height: usize) -> usize {
    let p1 = height.div_ceil(8);
    let p2 = p1 + (height.saturating_sub(4)).div_ceil(8);
    let p3 = p2 + (height.saturating_sub(2)).div_ceil(4);
    if row < p1 {
        row * 8
    } else if row < p2 {
        (row - p1) * 8 + 4
    } else if row < p3 {
        (row - p2) * 4 + 2
    } else {
        (row - p3) * 2 + 1
    }
}

fn palette(r: &mut Cursor, n: usize) -> Result<Vec<[u8; 3]>, ImageError> {
    let raw = r.take(n * 3)?;
    Ok(raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

fn skip_sub_blocks(r: &mut Cursor) -> Result<(), ImageError> {
    loop {
        let n = r.u8()? as usize;
        if n == 0 {
            return Ok(());
        }
        r.take(n)?;
    }
}

/// LZW decompression (GIF variant).
///
/// **The GIF variant is LSB-first and its code width grows as the dictionary
/// fills**, unlike the fixed-width LZW some other formats use. The two
/// control codes — clear and end-of-information — sit immediately above the
/// palette, which is why the dictionary starts at `clear + 2`.
fn lzw(r: &mut Cursor, expected: usize) -> Result<Vec<u8>, ImageError> {
    let min_code_size = r.u8()? as u32;
    if !(2..=11).contains(&min_code_size) {
        return Err(ImageError::BadField {
            at: r.pos,
            field: "LZW code size",
            value: min_code_size,
        });
    }

    // Sub-blocks are length-prefixed; stitch them into one stream.
    let mut data = Vec::new();
    loop {
        let n = r.u8()? as usize;
        if n == 0 {
            break;
        }
        data.extend_from_slice(r.take(n)?);
    }

    let clear = 1u16 << min_code_size;
    let end = clear + 1;

    // The dictionary. Each entry is a prefix plus one byte, so an entry
    // expands without recursion by walking the prefix chain backwards.
    let mut prefix = vec![0u16; MAX_CODES];
    let mut suffix = vec![0u8; MAX_CODES];
    for i in 0..clear as usize {
        suffix[i] = i as u8;
    }

    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut stack: Vec<u8> = Vec::with_capacity(MAX_CODES);

    let mut code_size = min_code_size + 1;
    let mut next = end + 1;
    let mut prev: Option<u16> = None;

    let mut bit = 0usize;
    let total_bits = data.len() * 8;

    while bit + code_size as usize <= total_bits {
        // LSB-first, spanning byte boundaries.
        let mut code = 0u32;
        for k in 0..code_size {
            let b = bit + k as usize;
            let v = (data[b >> 3] >> (b & 7)) & 1;
            code |= (v as u32) << k;
        }
        bit += code_size as usize;
        let code = code as u16;

        if code == clear {
            code_size = min_code_size + 1;
            next = end + 1;
            prev = None;
            continue;
        }
        if code == end {
            break;
        }

        let first_out = out.len();
        stack.clear();

        let mut walk = if (code as usize) < next as usize {
            code
        } else if let Some(p) = prev {
            // The KwKwK case: a code that refers to the entry being defined
            // right now. Its expansion is the previous string plus that
            // string's own first byte.
            stack.push(first_byte(&prefix, &suffix, p, clear));
            p
        } else {
            return Err(ImageError::BadField { at: r.pos, field: "LZW code", value: code as u32 });
        };

        // Walk the prefix chain, then unwind it.
        let mut guard = 0;
        while walk >= clear {
            if guard > MAX_CODES {
                return Err(ImageError::BadField { at: r.pos, field: "LZW chain", value: walk as u32 });
            }
            stack.push(suffix[walk as usize]);
            walk = prefix[walk as usize];
            guard += 1;
        }
        stack.push(suffix[walk as usize]);
        out.extend(stack.iter().rev());

        if let Some(p) = prev
            && (next as usize) < MAX_CODES
        {
            prefix[next as usize] = p;
            suffix[next as usize] = out[first_out];
            next += 1;
            // **The code width grows the moment the dictionary reaches
            // `1 << code_size`**, and stops at 12 bits. Encoder and decoder
            // must widen at exactly the same code or the stream
            // desynchronises a few symbols later.
            if next == (1 << code_size) && code_size < MAX_CODE_BITS {
                code_size += 1;
            }
        }
        prev = Some(code);

        // A stream that produces more pixels than the frame declares is
        // malformed; stopping is better than growing without bound.
        if out.len() > expected {
            break;
        }
    }

    out.resize(expected, 0);
    Ok(out)
}

fn first_byte(prefix: &[u16], suffix: &[u8], mut code: u16, clear: u16) -> u8 {
    let mut guard = 0;
    while code >= clear && guard <= MAX_CODES {
        code = prefix[code as usize];
        guard += 1;
    }
    suffix[code as usize]
}

/// Bounds-checked reader. Every read can fail; none can panic.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ImageError> {
        let end = self.pos.checked_add(n).ok_or(ImageError::Truncated {
            at: self.pos,
            expected: "GIF data",
        })?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or(ImageError::Truncated { at: self.pos, expected: "GIF data" })?;
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, ImageError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ImageError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 GIF: red, green / blue, white, built by hand.
    fn tiny_gif() -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&2u16.to_le_bytes()); // width
        g.extend_from_slice(&2u16.to_le_bytes()); // height
        g.push(0x80 | 0x01); // global table, 4 entries
        g.push(0); // background
        g.push(0); // aspect
        g.extend_from_slice(&[255, 0, 0]); // 0 red
        g.extend_from_slice(&[0, 255, 0]); // 1 green
        g.extend_from_slice(&[0, 0, 255]); // 2 blue
        g.extend_from_slice(&[255, 255, 255]); // 3 white

        g.push(0x2C); // image separator
        g.extend_from_slice(&0u16.to_le_bytes()); // left
        g.extend_from_slice(&0u16.to_le_bytes()); // top
        g.extend_from_slice(&2u16.to_le_bytes()); // width
        g.extend_from_slice(&2u16.to_le_bytes()); // height
        g.push(0); // no local table, not interlaced

        // LZW: clear(4), the four literals, end(5).
        //
        // **The encoder has to widen its codes exactly where the decoder
        // does** — when the dictionary reaches `1 << code_size`. Writing
        // every code at the starting width is the obvious way to build this
        // fixture and it desynchronises after the third literal, which is
        // what makes this a worthwhile thing to get right here rather than
        // discover against a real file.
        let codes: [u16; 6] = [4, 0, 1, 2, 3, 5];
        let min_code_size = 2u32;
        let clear = 1u16 << min_code_size;
        let end = clear + 1;

        let mut bits: Vec<u8> = Vec::new();
        let mut acc = 0u32;
        let mut n = 0u32;
        let mut code_size = min_code_size + 1;
        let mut next = end + 1;
        let mut prev_was_code = false;

        for c in codes {
            acc |= (c as u32) << n;
            n += code_size;
            while n >= 8 {
                bits.push((acc & 0xFF) as u8);
                acc >>= 8;
                n -= 8;
            }
            if c == clear {
                code_size = min_code_size + 1;
                next = end + 1;
                prev_was_code = false;
            } else if c != end {
                if prev_was_code {
                    next += 1;
                    if next == (1 << code_size) && code_size < MAX_CODE_BITS {
                        code_size += 1;
                    }
                }
                prev_was_code = true;
            }
        }
        if n > 0 {
            bits.push((acc & 0xFF) as u8);
        }

        g.push(2); // min code size
        g.push(bits.len() as u8);
        g.extend_from_slice(&bits);
        g.push(0); // block terminator
        g.push(0x3B); // trailer
        g
    }

    #[test]
    fn decodes_a_hand_built_gif() {
        let img = decode(&tiny_gif()).expect("should decode");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.get(0, 0), [255, 0, 0]);
        assert_eq!(img.get(1, 0), [0, 255, 0]);
        assert_eq!(img.get(0, 1), [0, 0, 255]);
        assert_eq!(img.get(1, 1), [255, 255, 255]);
    }

    #[test]
    fn rejects_a_non_gif() {
        assert!(matches!(decode(b"not a gif"), Err(ImageError::NotThisFormat)));
        assert!(decode(b"").is_err());
        assert!(decode(b"GIF").is_err());
    }

    /// Truncation at every length must be an error, never a panic.
    #[test]
    fn truncation_never_panics() {
        let g = tiny_gif();
        for cut in 0..g.len() {
            let _ = decode(&g[..cut]);
        }
    }

    /// Corrupting any single byte must not panic either.
    #[test]
    fn corruption_never_panics() {
        let base = tiny_gif();
        for i in 0..base.len() {
            for mask in [0xFF, 0x01, 0x80] {
                let mut g = base.clone();
                g[i] ^= mask;
                let _ = decode(&g);
            }
        }
    }

    #[test]
    fn rejects_an_absurd_screen_size() {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&65535u16.to_le_bytes());
        g.extend_from_slice(&65535u16.to_le_bytes());
        g.extend_from_slice(&[0, 0, 0]);
        assert!(matches!(decode(&g), Err(ImageError::TooLarge { .. })));
    }

    #[test]
    fn deinterlace_covers_every_row_once() {
        for height in [1usize, 2, 5, 8, 16, 17, 100] {
            let mut seen = vec![0u32; height];
            for row in 0..height {
                let d = deinterlace(row, height);
                assert!(d < height, "height {height} row {row} -> {d}");
                seen[d] += 1;
            }
            assert!(seen.iter().all(|&c| c == 1), "height {height}: {seen:?}");
        }
    }

    #[test]
    fn a_gif_with_no_image_block_is_an_error() {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&1u16.to_le_bytes());
        g.extend_from_slice(&1u16.to_le_bytes());
        g.extend_from_slice(&[0, 0, 0]);
        g.push(0x3B); // straight to trailer
        assert!(decode(&g).is_err());
    }
}
