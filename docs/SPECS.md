# Specs to read, and test data to stage

## 1. DEFLATE — RFC 1951, and zlib container RFC 1950

Everything else depends on this; PNG cannot be written without it.

**What to extract while reading:** the fixed Huffman code tables (memorise the ranges), the
length/distance code tables with their extra-bit counts, the dynamic block header layout including
the code-length alphabet's strange ordering, and the stored-block escape hatch.

**Build order note:** implement *inflate* first even though darkroom mainly needs *deflate*.
Inflate is simpler, and once it works it becomes the test harness for your compressor.

## 2. PNG — RFC 2083

**Extract:** chunk layout and which chunks are critical, the five scanline filter types and the
Paeth predictor, colour type / bit depth combinations, and Adam7 interlacing.

**Shortcut worth knowing:** you only need to *write* 8-bit RGB non-interlaced. Read support needs
more. Scope accordingly.

## 3. JPEG baseline — ITU-T T.81

The long one, and the one you least want to be meeting at hour 20 of the build.

**Extract:** marker structure (SOI/APPn/DQT/SOF0/DHT/SOS/EOI), Huffman table construction from
BITS and HUFFVAL, the DC differential coding and AC run-length/size coding, the zig-zag order,
dequantisation, the IDCT, chroma subsampling patterns (4:4:4, 4:2:2, 4:2:0), and restart markers.

**Where implementations die:** Huffman table construction, and the MCU interleaving order for
subsampled components. Read those two sections twice.

## 4. EXIF / TIFF IFD — CIPA DC-008

**Extract:** the TIFF header and both byte orders, IFD entry layout, the twelve field types and
their sizes, the value-vs-offset rule for fields over four bytes, the EXIF sub-IFD pointer, and
the GPS IFD with its rational-to-degrees conversion.

**Only a handful of tags matter:** DateTimeOriginal, Make, Model, LensModel, ISO, ExposureTime,
FNumber, Orientation, PixelXDimension/PixelYDimension, and the GPS block. Do not try to table the
whole standard.

## 5. Progressive JPEG — T.81 Annex G

Stretch scope, first on the cut list. Read it only after baseline is understood.

**Extract:** spectral selection, successive approximation, and the EOB run mechanism.

## 6. QR + Reed–Solomon — ISO/IEC 18004

The standard is paywalled; the format is thoroughly documented in public references. Thonky's
tutorial covers the whole encoding pipeline and is sufficient.

**Extract:** GF(256) arithmetic with the QR primitive polynomial, generator polynomial
construction, version and error-correction-level capacity tables, byte-mode segmentation, block
interleaving, the eight mask patterns and their penalty rules, and the format/version information
bit strings.

**Scope:** byte mode only. It only ever encodes a URL.

## 7. ISOBMFF — for the HEIC catalog fallback

Small. Enough to walk the box tree and pull the `meta` box's Exif item, so HEIC files still appear
in the timeline with real dates even though the pixels never decode.

---

## Test corpus to stage

Test **data** is not code. Collect all of this before kickoff — hour one should be implementation,
not scavenging.

| Corpus | Purpose |
| --- | --- |
| **PngSuite** (schaik.com/pngsuite) | PNG conformance, including deliberately corrupt files. The single best oracle in the project. |
| **Curated demo photo folder, JPEG only** | Large enough to be convincing on video. No HEIC. |
| **Near-duplicate set** | One photo at three resolutions, one crop, one re-encode. This is what the pHash fixtures assert against, and what the demo shows. |
| **Progressive JPEGs** | Common on the web; proves the stretch decoder if it lands. |
| **Multi-vendor EXIF samples** | Canon, Nikon, Sony, iPhone, Android. Byte order and tag layout vary more than you expect. |
| **A few HEIC files** | To prove the catalog-without-pixels fallback works. |
| **Subsampling variants** | 4:4:4, 4:2:2, 4:2:0 of the same source image. The MCU interleaving bug hides here. |
| **Pathological files** | Truncated JPEG, zero-byte file, a `.jpg` that is actually a PNG, a 20,000px image. Graceful failure is a Code Quality signal. |

---

## Verification oracles — write these into the README

Both are commands a judge can run themselves. That converts a soft "look, it renders" proof into a
hard pass/fail one, which is the gap I flagged in the metrics review.

1. **DEFLATE round-trip:** darkroom's compressed output decompresses correctly under real `gunzip`.
2. **PNG conformance:** every valid PngSuite file decodes, every corrupt one is rejected without
   panicking.
