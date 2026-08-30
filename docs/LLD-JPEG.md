# LLD — baseline JPEG decode

> ITU-T T.81 + JFIF · **1,000 lines** · risk **medium** · oracle **`jpeg-js`**
>
> **Spiked and passed**, 588 lines, first attempt, all three subsampling modes correct —
> see `SPIKE-JPEG.md`. This document is the design that spike proved out.
>
> Pre-kickoff planning. **Not code.** Structures below carry no function bodies.

---

## 1. Why this module is the project

It is the largest single module, the one the whole thumbnail path hangs off, and the one
that carries most of the 30% Zero-Dependency Craft criterion. Huffman decoding and the IDCT
are the two things a senior reviewer will actually open the file to look at.

It is also the module that was **claimed to be unshippable and then shipped in a day**.
The spike falsified my own central objection to darkroom — that a wrong decoder fails
silently at hour 30. It does not: failures are specific, immediate, and named.

---

## 2. Scope

| In | Out |
|---|---|
| Baseline sequential DCT, 8-bit (SOF0) | Arithmetic coding — nobody produces it |
| **Progressive (SOF2)** — spectral selection, successive approximation, EOBRUN | 12-bit, lossless, hierarchical modes |
| Grayscale (1 component) and YCbCr (3) | — |
| 4:4:4, 4:2:2, 4:2:0 chroma subsampling | CMYK / YCCK (4 components, Adobe APP14) |
| Restart intervals (DRI/RSTn) | — |

Everything in the right column returns `ImageError::Unsupported { feature }` with the
feature named. **The entry still lands in the catalog** with `EntryState::NoPreview`.

---

## 3. Pipeline

```
bytes
 └─ marker walk ────────────► segments
      ├─ DQT  ─► quant tables      [4][64]  u16
      ├─ SOF0 ─► frame header      dims, components, sampling factors
      ├─ DHT  ─► Huffman tables    [2][4]   canonical, built from BITS/HUFFVAL
      ├─ DRI  ─► restart interval  MCUs between RSTn
      └─ SOS  ─► scan header       component → table mapping, then entropy data
 └─ bit reader ─────────────► MCU loop
      ├─ per block: DC differential  ─► AC run/size pairs ─► coef[64] in zigzag order
      ├─ dezigzag                    ─► natural order
      ├─ dequantise                  ─► coef × quant table
      └─ IDCT                        ─► 8×8 spatial samples, level-shifted +128
 └─ per component plane ────► chroma upsample to full resolution
 └─ YCbCr → RGB ────────────► Image
```

---

## 4. Data structures

```rust
struct Component {
    id:        u8,
    h_samp:    u8,     // 1..=4
    v_samp:    u8,
    quant_tbl: u8,     // index into [4]
    dc_tbl:    u8,     // set by SOS, not SOF
    ac_tbl:    u8,
    plane:     Vec<u8>,        // decoded, at this component's own resolution
    plane_w:   usize,          // ceil to a whole MCU — see §7.2
    plane_h:   usize,
}

struct Frame {
    width:     u16,
    height:    u16,
    comps:     Vec<Component>,  // 1 or 3
    h_max:     u8,              // max h_samp across components
    v_max:     u8,
    mcu_w:     usize,           // 8 * h_max
    mcu_h:     usize,
    mcus_x:    usize,           // ceil(width  / mcu_w)
    mcus_y:    usize,
}

/// Canonical Huffman, decoded by the length-first method — no tree nodes,
/// no pointer chasing, and it is the form the spec itself describes.
struct HuffTable {
    /// min_code[l] / max_code[l] for code length l = 1..=16; -1 means "no codes"
    min_code:  [i32; 17],
    max_code:  [i32; 17],
    val_ptr:   [i32; 17],   // index into `values` of the first code of length l
    values:    Vec<u8>,     // HUFFVAL, in order
}

struct BitReader<'a> {
    bytes: &'a [u8],
    pos:   usize,
    bits:  u32,      // accumulator, MSB-aligned
    n:     u32,      // valid bits in the accumulator
    marker_hit: Option<u8>,
}
```

**Why the length-first Huffman form and not a tree:** it is a fixed-size struct with no
allocation per node, it decodes with one comparison per bit, it is exactly what T.81 Figure
F.16 specifies, and — the practical reason — it makes malformed tables detectable at
*construction* time rather than at a null pointer twenty thousand blocks later.

---

## 5. The marker walk

| Marker | Bytes | Handling |
|---|---|---|
| `SOI` | `FF D8` | must be first two bytes, else `NotThisFormat` |
| `APPn` | `FF E0`–`FF EF` | skip by length; **`APP1` with `Exif\0\0` is handed to `exif`** |
| `DQT` | `FF DB` | one segment may hold several tables; precision nibble picks u8 vs u16 |
| `SOF0` | `FF C0` | the frame. `SOF1` accepted as baseline; `SOF2` → `Unsupported` |
| `DHT` | `FF C4` | class nibble (0 = DC, 1 = AC) + id nibble; several tables per segment |
| `DRI` | `FF DD` | restart interval in MCUs; 0 disables |
| `SOS` | `FF DA` | component→table mapping, then entropy-coded data begins |
| `RSTn` | `FF D0`–`FF D7` | reset DC predictors, byte-align, expect them in order 0–7 |
| `EOI` | `FF D9` | end. **Its absence is tolerated** — truncated files still yield rows |
| `DNL` | `FF DC` | height defined in the scan. Rare. Accept or `Unsupported`, don't crash |

**Segment length includes its own two bytes.** Off-by-two here walks the parser into the
middle of a segment and produces a cascade of nonsense errors far from the real fault.

---

## 6. Huffman table construction — read this section twice

This is where implementations die, and the spec's own pseudo-code is the thing to follow
literally rather than reinvent.

1. `BITS[1..=16]` — how many codes exist of each length. `HUFFVAL` — the symbol values, in
   canonical order.
2. Generate `HUFFSIZE` by repeating each length `BITS[l]` times.
3. Generate `HUFFCODE`: start at code 0, increment per symbol, **shift left by one when the
   length increases**.
4. Derive `min_code`, `max_code`, `val_ptr` per length.

**Validation that must happen at construction, not at use:**

- `sum(BITS) == HUFFVAL.len()` — the spike's `truncated DHT values` diagnostic comes from
  exactly this check.
- `sum(BITS) <= 256`.
- No code may be all-ones for its length (an over-subscribed table); reject it.
- Table id ≤ 3, class ≤ 1.

**Decode is:** read one bit at a time into `code`; at each length `l`, if
`code <= max_code[l]`, the symbol is `values[val_ptr[l] + code - min_code[l]]`. Bail with
`BadField` past length 16 rather than looping.

---

## 7. The entropy scan

### 7.1 Bit reader — the `0xFF00` rule

Entropy-coded data cannot contain a raw `FF`, so encoders stuff a zero after every `FF`
byte. The reader must **swallow the `00`**. If the byte after `FF` is *not* `00`, a marker
has been reached: stop, record it, and let the MCU loop decide (an `RSTn` is expected; an
`EOI` means the scan ended early; anything else is a corrupt file).

Getting this wrong produces images that are perfect for the first few hundred blocks and
then dissolve — which reads like an IDCT bug and is not one.

### 7.2 Block decode

- **DC:** decode a size `s` from the DC table, read `s` additional bits, extend the sign
  (values below `2^(s-1)` are negative and need `+ (-1 << s) + 1`). The result is a
  **difference from the previous block of the same component** — keep one predictor per
  component and reset all of them at every restart marker.
- **AC:** decode a byte per symbol: high nibble = run of zeros, low nibble = size.
  `0x00` = EOB (rest of the block is zero). `0xF0` = ZRL, a run of sixteen zeros.
  Anything that would push the coefficient index past 63 is a corrupt file, not a wrap.

### 7.3 MCU interleaving — the second place implementations die

For subsampled images the blocks are **interleaved per MCU**, not per plane. One MCU
contains `h_samp × v_samp` blocks of each component, in component order, luma first:

| Sampling | Y blocks | Cb | Cr | Per MCU |
|---|---|---|---|---|
| 4:4:4 (`1x1,1x1,1x1`) | 1 | 1 | 1 | 3 |
| 4:2:2 (`2x1,1x1,1x1`) | 2 | 1 | 1 | 4 |
| 4:2:0 (`2x2,1x1,1x1`) | 4 | 1 | 1 | 6 |

**Component planes are allocated to whole-MCU dimensions and cropped at the very end.** An
image 1281 px wide with 4:2:0 has a 16-px MCU and needs a 1296-px luma plane. Allocating to
the declared width and writing the last MCU is the classic overrun; allocating correctly and
forgetting to crop is the classic garbage-stripe-on-the-right.

The spike got all three modes right on the first attempt, which is the single strongest
piece of evidence in this document — this was the bug predicted to eat six hours.

### 7.4 Restart markers

Every `restart_interval` MCUs: byte-align the reader, expect `FF D0+n` with `n` cycling
0–7, reset all DC predictors. Restart markers exist precisely so a corrupt stream can
resync — so on a mismatch, **scan forward to the next valid RSTn and continue** rather than
failing the file. That is what turns `truncated-scan.jpg` into a partial image instead of an
error.

---

## 8. IDCT

**Separable, float, row-column.** Two passes of an 8-point 1-D IDCT: eight on the rows,
eight on the columns, then `+128` level shift and clamp to `0..=255`.

Rejected alternatives, and why:

- **AAN / integer fast IDCT** — faster, and roughly twice the code with sign and scaling
  conventions that are easy to get subtly wrong. Subtly wrong here costs half a day and
  looks like a colour bug.
- **Naive O(64×64) double loop** — correct, trivially, and ~8× slower. Worth keeping as a
  test-only reference implementation to diff the fast path against, at ~20 lines.

**Precision:** float IDCT against `jpeg-js`'s integer one gives max |Δ| = 3 out of 255 —
ordinary implementation variance, not error. That number is measured, published, and worth
stating in the README rather than claiming bit-exactness nobody asked for.

**Optimisation that is worth the ten lines:** if all 63 AC coefficients are zero — very
common in flat regions — the block is a constant `DC/8 + 128`. Short-circuit it.

---

## 9. Upsampling and colour

- **Chroma upsample:** nearest-neighbour replication for 4:2:2 and 4:2:0. Fancy
  (triangular) upsampling is visibly better on hard edges and is not worth the lines at
  256-px thumbnail scale.
- **YCbCr → RGB**, the JFIF equations, in integer fixed point with rounding:
  `R = Y + 1.402(Cr−128)`, `G = Y − 0.344136(Cb−128) − 0.714136(Cr−128)`,
  `B = Y + 1.772(Cb−128)`. Clamp, do not wrap — wrapping produces the psychedelic-pixel
  artifact that looks like a Huffman bug.
- **1-component images** skip all of the above and replicate the luma plane into RGB.

---

## 10. Failure surface

Every one of these was exercised by the spike's 23 adversarial files. **Zero panics** is a
README claim, so it is a design requirement, not an aspiration.

| Input | Must do |
|---|---|
| Missing SOI | `NotThisFormat` — and the file is then re-probed as PNG/GIF |
| Truncated DQT/DHT payload | `Truncated { at, expected }` |
| Quant table id > 3 | `BadField` |
| SOF2 (progressive) | `Unsupported { feature: "progressive JPEG" }` |
| 4 components / Adobe CMYK | `Unsupported` |
| Declared dims 65535×65535 | `TooLarge` **before allocation** |
| Truncated scan, no EOI | Decode what exists, return the partial image |
| `.png` that is a JPEG | Decodes — probe on magic bytes, never on extension |
| 20000×150 panorama | Decodes |
| Unicode / emoji filename | Irrelevant to this module, and it must stay irrelevant |

---

## 11. Oracle

**`jpeg-js`** — a from-scratch JavaScript baseline decoder sharing no code with this one.
Decode the same file in both, print mean and max absolute per-channel difference.

**Wire it in at hour one.** It converts "is my decoder correct?" from a judgement call into
a number available on every run, and it costs about twenty minutes. This is the
highest-value process change identified anywhere in the planning.

Second oracle, free: re-encode to PNG and open it beside the original in any viewer.
