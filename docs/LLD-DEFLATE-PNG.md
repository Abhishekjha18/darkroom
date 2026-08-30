# LLD — DEFLATE, CRC32, and PNG

> RFC 1951 (DEFLATE) · RFC 1950 (zlib) · RFC 2083 (PNG) · **500 + 350 lines** ·
> risk **low** · oracles **real `gunzip`**, **PngSuite**, **any browser**
>
> **Not spiked.** This is the one load-bearing darkroom module with no evidence behind it,
> and `../../planning/VERDICT.md` names it as the spike to run next: *"if the DEFLATE/PNG encoder spike
> fails or proves expensive, darkroom loses the thumbnail path and half its craft story."*

---

## 1. Why this exists before anything else

Nothing renders without it. PNG's entire compression layer is DEFLATE, and `std` has no
compression of any kind — no `zlib`, unlike Python's and Go's standard libraries. Without
this module the thumbnail path does not exist, and without thumbnails darkroom is a
directory listing.

It is also the cheapest craft in the project per line: two short, frozen, forty-year-old
specifications with a pass/fail oracle sitting on every developer machine.

---

## 2. Build inflate first

Counter-intuitive, and it is the right order.

darkroom only *needs* deflate — it writes PNGs, it does not have to read them. But
**inflate is the simpler half**, it is finished in a couple of hours, and the moment it
works it becomes the test harness for the compressor: compress a buffer, decompress it with
your own inflate, compare. That loop closes before `gunzip` is ever involved.

Then close it a second way, which is the one that goes in the README:

> darkroom's compressed output decompresses correctly under real `gunzip`.

A command a judge can run. That converts a soft *"look, it renders"* claim into a hard
pass/fail one.

---

## 3. DEFLATE — structures

```rust
/// Bit-level, LSB-first — the opposite of JPEG's MSB-first reader.
/// Two bit orders in one binary is a genuine trap; the types must not be interchangeable.
struct BitWriter { out: Vec<u8>, acc: u32, n: u32 }
struct BitReader<'a> { src: &'a [u8], pos: usize, acc: u32, n: u32 }

struct Huff {
    /// Canonical code per symbol, plus its length. Length 0 = symbol unused.
    lengths: Vec<u8>,
    codes:   Vec<u16>,
}

/// Hash-chain match finder over a 32 KiB sliding window.
struct MatchFinder {
    window: Vec<u8>,
    head:   [i32; 1 << 15],   // hash of 3 bytes -> most recent position
    prev:   Vec<i32>,         // position -> previous position with the same hash
}
```

**LSB-first here, MSB-first in JPEG.** Both readers are ~40 lines and look almost
identical. Sharing them is the single most tempting wrong refactor in the project — the
bit orders are incompatible and the failure is a garbage stream two thousand bytes in.
Keep them in separate modules with different type names.

---

## 4. DEFLATE — block types

| Type | Bits | Use |
| --- | --- | --- |
| `00` stored | — | Incompressible data, and the **escape hatch**: a valid DEFLATE stream can be all stored blocks. Implement this first — it makes a valid PNG *today* |
| `01` static Huffman | fixed table | The default. Fixed literal/length and distance tables, no header cost. Good enough for thumbnails |
| `10` dynamic Huffman | table in header | Better ratio, and the header encoding is the fiddly part |

**Ship order: stored → static → dynamic.** Each is independently correct and each produces
a stream real `gunzip` accepts. Dynamic is the only one that can be cut under time pressure,
and the cost is a few percent of file size on thumbnails nobody measures.

### 4.1 The length/distance alphabets

- Literals `0..=255`, `256` = end of block, `257..=285` = match lengths 3–258 with 0–5
  extra bits.
- Distances `0..=29` = 1–32768 with 0–13 extra bits.
- **Length codes 284/285 are irregular** — 285 encodes exactly 258 with no extra bits.
  Table-drive both alphabets; deriving them arithmetically gets this wrong.

### 4.2 The dynamic header's strange ordering

Code lengths for the code-length alphabet are written in the permuted order
`16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15`. It exists so trailing zeros can be
truncated. **Copy the table verbatim from the RFC.** Every implementation that types it
from memory gets it wrong, and the symptom is a stream that inflates to garbage only for
certain inputs.

### 4.3 The compressor

- Hash three bytes → chain of prior positions. Walk the chain up to a bounded depth
  (128 is ample), keep the longest match ≥ 3 bytes.
- **Lazy matching:** if position `i+1` yields a longer match than `i`, emit a literal at
  `i` and take the longer one. Worth roughly 5% for about fifteen lines.
- Emit `end-of-block` and flush. **A missing final-block bit** is the classic bug and
  presents as `gunzip: unexpected end of file`.

**Honest cost for `STDLIB.md`:** compression ratio trails zlib at equivalent levels.
Measured, published, and irrelevant at thumbnail sizes.

---

## 5. zlib container — RFC 1950

Two bytes in front, four behind. Trivially small and trivially easy to omit.

```
CMF   0x78   (deflate, 32 KiB window)
FLG   chosen so (CMF<<8 | FLG) % 31 == 0
...deflate stream...
ADLER32  big-endian, over the *uncompressed* bytes
```

**Adler-32 is not CRC-32 and PNG needs both** — Adler inside the zlib stream, CRC on every
chunk. Confusing them produces a file every decoder rejects with a checksum error and no
hint as to which checksum.

---

## 6. CRC-32

Table-driven CRC-32/ISO-HDLC: polynomial `0xEDB88320` reflected, init `0xFFFFFFFF`, final
XOR `0xFFFFFFFF`. A 256-entry table generated once at startup — or a `const` table, which
costs nothing and removes the initialisation ordering question entirely.

~40 lines. Genuinely required: without it no PNG darkroom writes is valid.

---

## 7. PNG — encode

Only one configuration is ever written: **8-bit RGB, colour type 2, non-interlaced.** No
palette, no alpha, no 16-bit, no Adam7. Thumbnails have no use for any of it.

```
89 50 4E 47 0D 0A 1A 0A          signature
IHDR   width, height, bitdepth=8, colour=2, compression=0, filter=0, interlace=0
IDAT   zlib( filtered scanlines )     — may be split across several chunks
IEND
```

Chunk layout: `length (u32 BE, payload only)`, `type (4 ASCII)`,
`payload`, `CRC32 (over type + payload, not over length)`.

**Two traps in one sentence:** the length excludes the type and CRC; the CRC includes the
type. Both are easy to get backwards and both produce "not a PNG" from every reader.

### 7.1 Scanline filters

Each row is prefixed with a filter byte. All five are implemented because the decode side
needs them anyway and the encode side wants the choice:

| # | Filter | Predictor |
| --- | --- | --- |
| 0 | None | — |
| 1 | Sub | `a` (left) |
| 2 | Up | `b` (above) |
| 3 | Average | `(a + b) / 2`, floor |
| 4 | Paeth | nearest of `a`, `b`, `c` to `a + b − c` |

**Filtering operates on bytes at a distance of `bpp`** (3 here), not on pixels, and it
**wraps modulo 256** — that is not a bug to be fixed with saturating arithmetic. The Paeth
predictor's tie-break order is `a`, then `b`, then `c`, and getting it wrong produces
images that are correct except along edges.

**Selection heuristic:** the standard minimum-sum-of-absolute-differences rule — filter each
row all five ways, keep the one with the smallest sum of signed-byte magnitudes. Exhaustive
selection (trial compression per row) is a few percent better and many times slower.

**Honest cost for `STDLIB.md`:** filter selection is heuristic rather than exhaustive, so
files are a few percent larger than optimal.

---

## 8. PNG — decode (Stretch)

Not needed to ship. Needed for two things worth having:

1. **The library contains PNGs**, and screenshots are a real part of a photo folder.
2. **PngSuite is the best oracle in the project** — 178 files including deliberately
   corrupt ones, and "every valid file decodes, every corrupt one is rejected without
   panicking" is a strong, checkable README claim.

Decode needs more than encode: all colour types, bit depths 1/2/4/8/16, palettes, tRNS,
and Adam7 interlacing. That is where the extra ~350 lines go, and it is the first thing to
cut.

**Bit depths below 8 unpack from within a byte, MSB first**, and rows are byte-aligned with
padding at the end. Interlaced sub-images each get their own filter bytes and their own
row padding — the second-commonest PngSuite failure after Paeth.

---

## 9. Failure surface

| Input | Must do |
| --- | --- |
| Truncated IDAT | `Truncated`, no panic |
| Bad chunk CRC | Reject with the chunk type named |
| `IHDR` not first / `IEND` missing | Reject |
| Unknown ancillary chunk (lowercase first letter) | **Skip silently** — that is what the case bit means |
| Unknown critical chunk (uppercase) | Reject |
| Dimensions × bpp overflowing `usize` | `TooLarge` before allocating |
| zlib stream that inflates to the wrong length | Reject; do not pad |

---

## 10. Oracles

| Check | Command | Verdict |
| --- | --- | --- |
| DEFLATE round-trip | darkroom's output through real `gunzip` | pass/fail |
| Self round-trip | own deflate → own inflate → compare | pass/fail, available hour one |
| PNG validity | open in any browser | visual |
| PNG conformance | 178 PngSuite files in `../../corpus/pngsuite/` | pass/fail per file |

The first and last go in the README as commands a judge can run. See `CORPUS.md`.
