//! PNG encode and decode (RFC 2083). Replaces `png` / `image`.

pub mod chunk;
pub mod crc;
pub mod filter;
pub mod quantise;

use crate::deflate;
use crate::image::{Image, ImageError, check_dimensions};
use filter::Filter;

// ---------------------------------------------------------------- encode

/// Writes 8-bit RGB, colour type 2, non-interlaced.
///
/// **Only one configuration is ever written.** No palette, no alpha, no
/// 16-bit, no Adam7 — thumbnails have no use for any of it.
pub fn encode(img: &Image) -> Vec<u8> {
    debug_assert!(img.is_consistent());
    let w = img.width as usize;
    let bpp = 3;
    let row_bytes = w * bpp;

    let mut raw = Vec::with_capacity((row_bytes + 1) * img.height as usize);
    let mut prev = vec![0u8; row_bytes];
    for y in 0..img.height as usize {
        let row = &img.px[y * row_bytes..(y + 1) * row_bytes];
        let (f, filtered) = filter::choose(row, &prev, bpp);
        raw.push(f as u8);
        raw.extend_from_slice(&filtered);
        prev.copy_from_slice(row);
    }

    let mut out = Vec::with_capacity(raw.len() / 2 + 128);
    out.extend_from_slice(&chunk::SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // colour type: truecolour
    ihdr.push(0); // compression: deflate
    ihdr.push(0); // filter method: adaptive
    ihdr.push(0); // interlace: none
    chunk::write(&mut out, b"IHDR", &ihdr);
    chunk::write(&mut out, b"IDAT", &deflate::zlib_compress(&raw));
    chunk::write(&mut out, b"IEND", &[]);
    out
}

/// Writes 8-bit indexed colour, colour type 3.
pub fn encode_palette(width: u32, height: u32, palette: &[[u8; 3]], indices: &[u8]) -> Vec<u8> {
    debug_assert_eq!(indices.len(), width as usize * height as usize);
    let w = width as usize;

    // **Filtering is left off for indexed images.** The five filters predict
    // a byte from its neighbours, which is meaningful for samples and
    // meaningless for palette indices — index 7 sitting next to index 8 says
    // nothing about the colours. Filtering them reliably makes the file
    // bigger, which is why libpng does the same.
    let mut raw = Vec::with_capacity((w + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(Filter::None as u8);
        raw.extend_from_slice(&indices[y * w..(y + 1) * w]);
    }

    let mut out = Vec::with_capacity(raw.len() / 2 + 1024);
    out.extend_from_slice(&chunk::SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(3); // colour type: indexed
    ihdr.push(0);
    ihdr.push(0);
    ihdr.push(0);
    chunk::write(&mut out, b"IHDR", &ihdr);

    let mut plte = Vec::with_capacity(palette.len() * 3);
    for c in palette {
        plte.extend_from_slice(c);
    }
    chunk::write(&mut out, b"PLTE", &plte);
    chunk::write(&mut out, b"IDAT", &deflate::zlib_compress(&raw));
    chunk::write(&mut out, b"IEND", &[]);
    out
}

/// Encodes a thumbnail the smallest way available.
///
/// Truecolour PNG is lossless but poor at photographs: a 256x192 tile
/// measures ~60 KB, and our encoder is already within 1.15x of libpng, so
/// that is the format rather than the implementation. An 8-bit palette
/// stores a third of the bytes before compression.
///
/// **Both are encoded and the smaller wins**, so this can never make a file
/// bigger. When the image has 256 or fewer colours the palette is exact and
/// the result is still lossless — which is the common case for screenshots.
pub fn encode_thumbnail(img: &Image) -> Vec<u8> {
    let truecolour = encode(img);
    let q = quantise::quantise(img);
    let paletted = encode_palette(img.width, img.height, &q.palette, &q.indices);

    if q.exact {
        // 256 colours or fewer: both encodings are lossless, so this is a
        // free choice and the smaller one wins outright.
        return if paletted.len() < truecolour.len() { paletted } else { truecolour };
    }

    // Otherwise the palette is lossy, and quality is only worth trading for
    // a saving that actually matters. A few percent off a thumbnail is not
    // worth throwing colours away for; a third off is.
    const WORTH_IT: f64 = 0.15;
    let saving = 1.0 - paletted.len() as f64 / truecolour.len() as f64;
    if saving >= WORTH_IT { paletted } else { truecolour }
}

// ---------------------------------------------------------------- decode

struct Header {
    width: u32,
    height: u32,
    depth: u8,
    colour: u8,
    interlace: u8,
}

impl Header {
    fn channels(&self) -> usize {
        match self.colour {
            0 => 1, // greyscale
            2 => 3, // truecolour
            3 => 1, // palette index
            4 => 2, // greyscale + alpha
            6 => 4, // truecolour + alpha
            _ => 0,
        }
    }

    fn bits_per_pixel(&self) -> usize {
        self.channels() * self.depth as usize
    }

    /// Filtering distance, in bytes, rounded up and never below 1.
    fn bpp(&self) -> usize {
        self.bits_per_pixel().div_ceil(8).max(1)
    }

    fn row_bytes(&self, width: usize) -> usize {
        (width * self.bits_per_pixel()).div_ceil(8)
    }
}

/// Adam7: starting row, starting column, row step, column step.
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (0, 4, 8, 8),
    (4, 0, 8, 4),
    (0, 2, 4, 4),
    (2, 0, 4, 2),
    (0, 1, 2, 2),
    (1, 0, 2, 1),
];

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let mut r = chunk::Reader::new(bytes)?;
    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut trns_colour: Option<[u16; 3]> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut seen_end = false;

    while let Some(c) = r.next()? {
        match &c.ctype {
            b"IHDR" => {
                if header.is_some() {
                    return Err(ImageError::BadField { at: c.at, field: "duplicate IHDR", value: 0 });
                }
                if c.payload.len() != 13 {
                    return Err(ImageError::Truncated { at: c.at, expected: "IHDR" });
                }
                let p = c.payload;
                let h = Header {
                    width: u32::from_be_bytes([p[0], p[1], p[2], p[3]]),
                    height: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
                    depth: p[8],
                    colour: p[9],
                    interlace: p[12],
                };
                if p[10] != 0 {
                    return Err(ImageError::BadField { at: c.at, field: "compression", value: p[10] as u32 });
                }
                if p[11] != 0 {
                    return Err(ImageError::BadField { at: c.at, field: "filter method", value: p[11] as u32 });
                }
                if h.interlace > 1 {
                    return Err(ImageError::BadField { at: c.at, field: "interlace", value: h.interlace as u32 });
                }
                if h.channels() == 0 {
                    return Err(ImageError::BadField { at: c.at, field: "colour type", value: h.colour as u32 });
                }
                let ok_depth = match h.colour {
                    0 => [1, 2, 4, 8, 16].contains(&h.depth),
                    3 => [1, 2, 4, 8].contains(&h.depth),
                    _ => [8, 16].contains(&h.depth),
                };
                if !ok_depth {
                    return Err(ImageError::BadField { at: c.at, field: "bit depth", value: h.depth as u32 });
                }
                check_dimensions(h.width, h.height)?;
                header = Some(h);
            }
            b"PLTE" => {
                if c.payload.len() % 3 != 0 {
                    return Err(ImageError::BadField { at: c.at, field: "PLTE length", value: c.payload.len() as u32 });
                }
                palette = c.payload.chunks_exact(3).map(|e| [e[0], e[1], e[2]]).collect();
            }
            b"tRNS" => {
                let Some(h) = &header else {
                    return Err(ImageError::BadField { at: c.at, field: "tRNS before IHDR", value: 0 });
                };
                match h.colour {
                    3 => trns = c.payload.to_vec(),
                    0 if c.payload.len() >= 2 => {
                        let g = u16::from_be_bytes([c.payload[0], c.payload[1]]);
                        trns_colour = Some([g, g, g]);
                    }
                    2 if c.payload.len() >= 6 => {
                        trns_colour = Some([
                            u16::from_be_bytes([c.payload[0], c.payload[1]]),
                            u16::from_be_bytes([c.payload[2], c.payload[3]]),
                            u16::from_be_bytes([c.payload[4], c.payload[5]]),
                        ]);
                    }
                    _ => {}
                }
            }
            b"IDAT" => idat.extend_from_slice(c.payload),
            b"IEND" => {
                seen_end = true;
                break;
            }
            // Unknown critical chunks must be refused; ancillary ones skipped
            // silently, which is exactly what the case bit is for.
            _ if c.is_critical() => {
                return Err(ImageError::Unsupported { feature: "unknown critical PNG chunk" });
            }
            _ => {}
        }
    }

    let Some(h) = header else {
        return Err(ImageError::Truncated { at: 0, expected: "IHDR" });
    };
    if !seen_end {
        return Err(ImageError::Truncated { at: bytes.len(), expected: "IEND" });
    }
    if idat.is_empty() {
        return Err(ImageError::Truncated { at: bytes.len(), expected: "IDAT" });
    }
    if h.colour == 3 && palette.is_empty() {
        return Err(ImageError::Truncated { at: 0, expected: "PLTE for palette image" });
    }

    // The exact number of raw bytes the stream must produce. Using it as the
    // inflate cap is what makes `decompression-bomb.png` a rejection rather
    // than 192 MB of allocation.
    let expected = expected_raw_len(&h);
    let raw = deflate::zlib_decompress(&idat, expected)
        .map_err(|_| ImageError::BadField { at: 0, field: "IDAT stream", value: 0 })?;
    if raw.len() != expected {
        return Err(ImageError::Truncated { at: raw.len(), expected: "IDAT rows" });
    }

    let mut img = Image::new(h.width, h.height);
    let mut cursor = 0usize;

    if h.interlace == 0 {
        let rb = h.row_bytes(h.width as usize);
        let mut prev = vec![0u8; rb];
        let mut row = vec![0u8; rb];
        for y in 0..h.height {
            let f = read_filter(&raw, &mut cursor)?;
            row.copy_from_slice(&raw[cursor..cursor + rb]);
            cursor += rb;
            filter::undo(f, &mut row, &prev, h.bpp());
            for x in 0..h.width {
                let px = pixel_at(&h, &row, x as usize, &palette, &trns, trns_colour)?;
                put(&mut img, x, y, px);
            }
            prev.copy_from_slice(&row);
        }
    } else {
        for &(row0, col0, rstep, cstep) in ADAM7.iter() {
            let pw = pass_extent(h.width as usize, col0, cstep);
            let ph = pass_extent(h.height as usize, row0, rstep);
            if pw == 0 || ph == 0 {
                continue;
            }
            let rb = h.row_bytes(pw);
            // Each pass has its own filter bytes and its own row padding —
            // the second-commonest PngSuite failure after Paeth.
            let mut prev = vec![0u8; rb];
            let mut row = vec![0u8; rb];
            for py in 0..ph {
                let f = read_filter(&raw, &mut cursor)?;
                row.copy_from_slice(&raw[cursor..cursor + rb]);
                cursor += rb;
                filter::undo(f, &mut row, &prev, h.bpp());
                for px_i in 0..pw {
                    let px = pixel_at(&h, &row, px_i, &palette, &trns, trns_colour)?;
                    let x = (col0 + px_i * cstep) as u32;
                    let y = (row0 + py * rstep) as u32;
                    put(&mut img, x, y, px);
                }
                prev.copy_from_slice(&row);
            }
        }
    }

    Ok(img)
}

fn expected_raw_len(h: &Header) -> usize {
    if h.interlace == 0 {
        h.height as usize * (1 + h.row_bytes(h.width as usize))
    } else {
        ADAM7
            .iter()
            .map(|&(row0, col0, rstep, cstep)| {
                let pw = pass_extent(h.width as usize, col0, cstep);
                let ph = pass_extent(h.height as usize, row0, rstep);
                if pw == 0 || ph == 0 { 0 } else { ph * (1 + h.row_bytes(pw)) }
            })
            .sum()
    }
}

fn pass_extent(total: usize, start: usize, step: usize) -> usize {
    if total > start { (total - start).div_ceil(step) } else { 0 }
}

fn read_filter(raw: &[u8], cursor: &mut usize) -> Result<Filter, ImageError> {
    let b = *raw
        .get(*cursor)
        .ok_or(ImageError::Truncated { at: *cursor, expected: "filter byte" })?;
    *cursor += 1;
    Filter::from_byte(b).ok_or(ImageError::BadField { at: *cursor, field: "filter type", value: b as u32 })
}

fn put(img: &mut Image, x: u32, y: u32, px: [u8; 3]) {
    if x >= img.width || y >= img.height {
        return;
    }
    let i = (y as usize * img.width as usize + x as usize) * 3;
    img.px[i] = px[0];
    img.px[i + 1] = px[1];
    img.px[i + 2] = px[2];
}

/// Reads one sample. **Bit depths below 8 unpack from within a byte, MSB
/// first.**
fn sample(row: &[u8], index: usize, depth: u8) -> u16 {
    match depth {
        16 => {
            let i = index * 2;
            u16::from_be_bytes([row[i], row[i + 1]])
        }
        8 => row[index] as u16,
        d => {
            let per_byte = 8 / d as usize;
            let byte = row[index / per_byte];
            let shift = 8 - d as usize * (index % per_byte + 1);
            ((byte >> shift) & ((1u8 << d) - 1)) as u16
        }
    }
}

/// Scales a sample of `depth` bits to 8 bits.
fn to8(v: u16, depth: u8) -> u8 {
    match depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        4 => (v * 17) as u8,
        2 => (v * 85) as u8,
        1 => if v != 0 { 255 } else { 0 },
        _ => v as u8,
    }
}

fn pixel_at(
    h: &Header,
    row: &[u8],
    x: usize,
    palette: &[[u8; 3]],
    trns: &[u8],
    trns_colour: Option<[u16; 3]>,
) -> Result<[u8; 3], ImageError> {
    let ch = h.channels();
    let base = x * ch;

    let (rgb, alpha) = match h.colour {
        0 => {
            let v = sample(row, base, h.depth);
            let g = to8(v, h.depth);
            let a = match trns_colour {
                Some(t) if t[0] == v => 0u8,
                _ => 255,
            };
            ([g, g, g], a)
        }
        2 => {
            let r = sample(row, base, h.depth);
            let g = sample(row, base + 1, h.depth);
            let b = sample(row, base + 2, h.depth);
            let a = match trns_colour {
                Some(t) if t == [r, g, b] => 0u8,
                _ => 255,
            };
            ([to8(r, h.depth), to8(g, h.depth), to8(b, h.depth)], a)
        }
        3 => {
            let idx = sample(row, base, h.depth) as usize;
            let c = *palette.get(idx).ok_or(ImageError::BadField {
                at: 0,
                field: "palette index",
                value: idx as u32,
            })?;
            (c, trns.get(idx).copied().unwrap_or(255))
        }
        4 => {
            let g = to8(sample(row, base, h.depth), h.depth);
            let a = to8(sample(row, base + 1, h.depth), h.depth);
            ([g, g, g], a)
        }
        _ => {
            let r = to8(sample(row, base, h.depth), h.depth);
            let g = to8(sample(row, base + 1, h.depth), h.depth);
            let b = to8(sample(row, base + 2, h.depth), h.depth);
            let a = to8(sample(row, base + 3, h.depth), h.depth);
            ([r, g, b], a)
        }
    };

    // Composite onto white. darkroom has one pixel format and it has no
    // alpha channel; a checkerboard would be a lie about what was stored.
    Ok(if alpha == 255 {
        rgb
    } else {
        let a = alpha as u32;
        // `+ 127` rounds to nearest. Truncating here is off by one on about a
        // third of partially-transparent pixels, which is exactly the gap
        // that showed up against Pillow.
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        [mix(rgb[0]), mix(rgb[1]), mix(rgb[2])]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 3;
                img.px[i] = (x * 255 / w.max(1)) as u8;
                img.px[i + 1] = (y * 255 / h.max(1)) as u8;
                img.px[i + 2] = ((x + y) % 256) as u8;
            }
        }
        img
    }

    #[test]
    fn encode_decode_round_trips() {
        for (w, h) in [(1u32, 1u32), (2, 3), (17, 5), (64, 64), (200, 7)] {
            let img = gradient(w, h);
            let png = encode(&img);
            let back = decode(&png).expect("own decoder must accept own encoder");
            assert_eq!(back.width, w);
            assert_eq!(back.height, h);
            assert_eq!(back.px, img.px, "pixels differ at {w}x{h}");
        }
    }

    #[test]
    fn encodes_a_valid_signature_and_chunk_order() {
        let png = encode(&gradient(4, 4));
        assert_eq!(&png[..8], &chunk::SIGNATURE);
        let mut r = chunk::Reader::new(&png).unwrap();
        assert_eq!(&r.next().unwrap().unwrap().ctype, b"IHDR");
        assert_eq!(&r.next().unwrap().unwrap().ctype, b"IDAT");
        assert_eq!(&r.next().unwrap().unwrap().ctype, b"IEND");
        assert!(r.next().unwrap().is_none());
    }

    #[test]
    fn rejects_a_truncated_file() {
        let mut png = encode(&gradient(8, 8));
        png.truncate(png.len() / 2);
        assert!(decode(&png).is_err());
    }

    #[test]
    fn rejects_a_corrupt_idat_crc() {
        let mut png = encode(&gradient(8, 8));
        let n = png.len();
        png[n - 10] ^= 0xFF;
        assert!(decode(&png).is_err());
    }

    #[test]
    fn rejects_absurd_dimensions_before_allocating() {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut png = chunk::SIGNATURE.to_vec();
        chunk::write(&mut png, b"IHDR", &ihdr);
        chunk::write(&mut png, b"IDAT", &[0; 4]);
        chunk::write(&mut png, b"IEND", &[]);
        assert!(matches!(decode(&png), Err(ImageError::TooLarge { .. })));
    }

    #[test]
    fn rejects_a_non_png() {
        assert!(matches!(decode(b"not a png at all"), Err(ImageError::NotThisFormat)));
        assert!(decode(b"").is_err());
    }

    #[test]
    fn sub_byte_samples_unpack_msb_first() {
        // 0b1101_0010 at depth 2 is samples 3, 1, 0, 2.
        let row = [0b1101_0010u8];
        assert_eq!(sample(&row, 0, 2), 3);
        assert_eq!(sample(&row, 1, 2), 1);
        assert_eq!(sample(&row, 2, 2), 0);
        assert_eq!(sample(&row, 3, 2), 2);
    }

    #[test]
    fn scales_low_depths_to_full_range() {
        assert_eq!(to8(1, 1), 255);
        assert_eq!(to8(3, 2), 255);
        assert_eq!(to8(15, 4), 255);
        assert_eq!(to8(0xFF00, 16), 0xFF);
    }

    #[test]
    fn adam7_extents_cover_the_image() {
        // Every pixel of an 8x8 image belongs to exactly one pass.
        let mut covered = vec![0u32; 64];
        for &(row0, col0, rstep, cstep) in ADAM7.iter() {
            for py in 0..pass_extent(8, row0, rstep) {
                for px in 0..pass_extent(8, col0, cstep) {
                    covered[(row0 + py * rstep) * 8 + col0 + px * cstep] += 1;
                }
            }
        }
        assert!(covered.iter().all(|&c| c == 1), "{covered:?}");
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;

    fn photo(w: u32, h: u32) -> Image {
        // Smooth, many-coloured content: the case palette encoding targets.
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 3;
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                img.px[i] = (180.0 * fx + 40.0 * fy) as u8;
                img.px[i + 1] = (120.0 * fy + 90.0 * (fx * 6.0).sin().abs()) as u8;
                img.px[i + 2] = (200.0 * (1.0 - fx) * (1.0 - fy) + 30.0) as u8;
            }
        }
        img
    }

    fn screenshot(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 3;
                let c: [u8; 3] = match (x / 17 + y / 23) % 4 {
                    0 => [250, 250, 250],
                    1 => [30, 30, 34],
                    2 => [217, 164, 65],
                    _ => [70, 130, 180],
                };
                img.px[i..i + 3].copy_from_slice(&c);
            }
        }
        img
    }

    /// A palette PNG we write must come back through our own decoder with
    /// exactly the colours the palette named.
    #[test]
    fn palette_encoding_round_trips() {
        for (w, h) in [(1u32, 1u32), (8, 5), (64, 64), (256, 192)] {
            let img = photo(w, h);
            let q = quantise::quantise(&img);
            let png = encode_palette(w, h, &q.palette, &q.indices);
            let back = decode(&png).expect("own decoder must accept own palette PNG");

            assert_eq!((back.width, back.height), (w, h));
            for (i, &idx) in q.indices.iter().enumerate() {
                let want = q.palette[idx as usize];
                assert_eq!(&back.px[i * 3..i * 3 + 3], &want, "pixel {i} at {w}x{h}");
            }
        }
    }

    /// Few-colour images stay bit-exact, so a screenshot loses nothing.
    #[test]
    fn a_screenshot_survives_losslessly_and_shrinks() {
        let img = screenshot(256, 192);
        let small = encode_thumbnail(&img);
        let back = decode(&small).unwrap();
        assert_eq!(back.px, img.px, "few-colour images must be lossless");
        assert!(
            small.len() < encode(&img).len(),
            "palette should beat truecolour on a 4-colour image"
        );
    }

    #[test]
    fn a_photograph_gets_smaller() {
        let img = photo(256, 192);
        let truecolour = encode(&img);
        let chosen = encode_thumbnail(&img);
        assert!(
            chosen.len() < truecolour.len(),
            "expected a saving, got {} vs {}",
            chosen.len(),
            truecolour.len()
        );
        // And it must still be a PNG our decoder accepts.
        let back = decode(&chosen).unwrap();
        assert_eq!((back.width, back.height), (img.width, img.height));
    }

    /// The chooser must never make a file bigger than plain truecolour.
    #[test]
    fn choosing_never_loses() {
        for img in [photo(64, 64), screenshot(64, 64), Image::new(32, 32)] {
            assert!(encode_thumbnail(&img).len() <= encode(&img).len());
        }
    }

    /// Quality is only traded for a saving worth having. A palette that
    /// barely beats truecolour must lose, because truecolour is lossless.
    #[test]
    fn a_marginal_saving_does_not_justify_losing_colours() {
        // Noise defeats both encoders, so the palette cannot win by much.
        let mut img = Image::new(64, 64);
        for (i, px) in img.px.chunks_exact_mut(3).enumerate() {
            let v = (i as u32).wrapping_mul(2654435761);
            px.copy_from_slice(&[(v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8]);
        }
        let q = quantise::quantise(&img);
        assert!(!q.exact, "premise: this image needs more than 256 colours");

        let truecolour = encode(&img);
        let paletted = encode_palette(64, 64, &q.palette, &q.indices);
        let chosen = encode_thumbnail(&img);

        let saving = 1.0 - paletted.len() as f64 / truecolour.len() as f64;
        if saving < 0.15 {
            assert_eq!(chosen.len(), truecolour.len(), "should have kept the lossless one");
        }
    }

    #[test]
    fn palette_pngs_declare_colour_type_3() {
        let img = screenshot(32, 32);
        let q = quantise::quantise(&img);
        let png = encode_palette(32, 32, &q.palette, &q.indices);
        let mut r = chunk::Reader::new(&png).unwrap();
        let ihdr = r.next().unwrap().unwrap();
        assert_eq!(&ihdr.ctype, b"IHDR");
        assert_eq!(ihdr.payload[9], 3, "colour type must be 3");
        assert_eq!(&r.next().unwrap().unwrap().ctype, b"PLTE");
    }
}
