# LLD — perceptual hashing, clustering, and resampling

> Published perceptual-hash literature · **350 + 200 lines** · risk **low** ·
> oracle **the 8-file near-duplicate fixture set**

---

## 1. This is the payload

Everything else in darkroom is infrastructure for one moment: a folder of thousands of
photos collapsing into clusters, and a number for how much space the duplicates are wasting.

It is also the only feature the user genuinely cannot do by hand. Byte-identical duplicates
are unremarkable — every file manager finds those. **Near-duplicates are the thing**: the
same shot at three resolutions, a crop, a re-encode, a burst of eight frames.

The demo opens here, not on the QR code. See `DEMO-SCRIPT.md` §1:40 and the reasoning in
`OVERVIEW.md` §7.

---

## 2. Two hashes, on purpose

| | dHash | pHash |
| --- | --- | --- |
| Cost | ~30 lines | ~150 lines (needs a forward DCT) |
| Input | 9×8 grey | 32×32 grey |
| Signal | gradient direction between adjacent pixels | low-frequency DCT coefficients |
| Robust to | scaling, mild compression | scaling, compression, brightness, small crops, gamma |
| Weak to | brightness and contrast shifts | fine detail; over-eager on flat images |

**Both are computed and both are stored.** dHash is nearly free and is a fast pre-filter;
pHash is the one that actually decides. Disagreement between them is itself signal — a pair
close in dHash and far in pHash is usually two different photos of the same flat scene.

```rust
pub struct Sig { pub dhash: u64, pub phash: u64 }
```

Both are 64-bit, both live in the `Entry`, both cost 16 bytes on disk.

---

## 3. dHash

1. Resize to **9×8 grey** (box filter — precision is pointless here).
2. For each of the 8 rows, compare each pixel to its right neighbour: 8 comparisons × 8 rows
   = 64 bits.
3. Bit set when left > right.

That is the whole algorithm. Its power is that it encodes *relative* gradients, so it
survives resizing and JPEG re-encoding almost perfectly — which is exactly the
`scaled-160` / `scaled-320` / `reencoded-q40` axis of the fixture set.

---

## 4. pHash

1. Resize to **32×32 grey**.
2. **2-D DCT-II** of the 32×32 block — separable, rows then columns, ~40 lines and the
   forward twin of the JPEG IDCT (different module, deliberately: different size, different
   normalisation, and sharing them would couple the thumbnail path to the dedupe path).
3. Keep the **top-left 8×8 low-frequency block, excluding `[0][0]`** — the DC term is
   average brightness and including it makes the hash brightness-sensitive, which is the
   whole thing pHash is supposed to fix.
4. Median of those 63 values.
5. Bit set where the coefficient exceeds the median.

**Median, not mean.** The mean is dragged by a single large coefficient and produces
degenerate all-ones or all-zeros hashes on high-contrast images. Median is one sort of 63
elements and it is what makes the hash behave on the fixture set's `cropped-10pct`.

**Grey conversion is luma only:** `0.299R + 0.587G + 0.114B`. Not the average of channels.

---

## 5. Distance and thresholds

Hamming distance — `(a ^ b).count_ones()`, one instruction.

| Distance | Meaning | Action |
| --- | --- | --- |
| 0 | identical hash | same photo, near-certainly |
| 1–5 | very close | **cluster** |
| 6–10 | plausible | cluster, flagged as "possible" in the UI |
| 11+ | unrelated | no |

**The threshold is tuned against `../../corpus/near-duplicates/`, not chosen from a blog
post.** That directory holds one original plus `cropped-10pct`, `rotated-90`,
`scaled-160`, `scaled-320`, `scaled-1280`, `reencoded-q40`, and one deliberately
**unrelated** image. The correct threshold is the one that clusters the first six with the
original and leaves the eighth out.

⚠️ **`rotated-90` now clusters — this section used to say it would not.**

The original reasoning was that rotation invariance means "hashing four orientations and
taking the minimum — 4× the work". **That costing was wrong.** The 32×32 grey reduction is
computed once, and rotating a 32×32 grid is an index permutation, so the real cost is three
extra DCTs over 1024 samples. Measured end to end on a 414-file library: **20.3 ms per image
against 19.4 ms before.**

Each signature therefore stores `phash` plus three rotated pHashes, and `distance()` takes
the minimum. Only one side needs turning — rotations form a group, so every orientation of
`a` against `b` upright covers all four relative angles.

`unrelated.jpg` still must not cluster, and that assertion is the one that matters.

---

## 6. Clustering

Single-linkage over the distance threshold, via union-find.

```rust
pub struct Cluster { pub ids: Vec<u64>, pub best: u64, pub wasted_bytes: u64 }
```

- `best` = the largest-resolution member, tie-broken by file size. It is what the UI shows
  as the keeper.
- `wasted_bytes` = total minus `best`. **This is the number in the demo.**

**Single-linkage chains, and that is a real failure mode**: A near B, B near C, A far from
C, all one cluster. At a threshold of 5 over a few thousand photos it is rarely visible; at
10 it is. The mitigation is a cap on cluster diameter — reject a merge that would put two
members more than `2 × threshold` apart. ~15 lines, and it prevents the demo's worst
possible moment, which is one cluster containing everything.

### Doing it without comparing everything to everything

`n²/2` at 5,000 photos is 12.5 M comparisons of a single XOR and popcount — about 50 ms.
**It does not need optimising, and it should not be.**

If the corpus ever justifies it: bucket by the 16 high bits of dHash and compare within
buckets. Written down here so the temptation is answered rather than acted on.

---

## 7. Resampling — 200 lines

Two filters, separable, two passes (horizontal then vertical).

| Filter | Use |
| --- | --- |
| **Box** | any downscale ≥ 2×, and every hash input. Fast and correct for large ratios |
| **Bilinear** | the final step to exact thumbnail dimensions |

**Lanczos is cut** from the halved scope. It is visibly better on hard edges at 1:1 and
essentially indistinguishable at 256 px, which is the only size darkroom produces.

**Downscale in the right order:** box-filter down to within 2× of the target, then bilinear
to the exact size. Bilinear alone from 4000 px to 256 px samples a tiny fraction of the
source pixels and produces aliased, sparkly thumbnails — the classic naive-resize artifact.

**The honest cost, for `STDLIB.md`:** resampling happens in **gamma-encoded sRGB, not
linear light**. Technically wrong; visually imperceptible at thumbnail size. Stated because
it is the kind of thing a reviewer should be told rather than discover.

---

## 8. Thumbnails

- **256 px on the long edge**, aspect preserved. Retina phones show the grid at ~120 px CSS,
  so 256 covers 2× without storing 4× the bytes.
- **PNG, not JPEG.** The JPEG *encoder* is cut from the halved scope; PNG encode exists
  anyway for the DEFLATE story. With the palette path below, PNG is competitive enough that
  the cut no longer costs anything worth measuring.

> ⚠️ **Correction, 2026-08-19 — measured, then fixed.**
>
> This section claimed thumbnails are “3–8 KB either way at this size”. **They are not.**
> A 256×192 truecolour PNG of real photographic content measures **~60 KB**, and that is
> not an implementation problem: libpng at `compress_level=9` produces 59,108 bytes where
> darkroom produces 68,050 — **1.15×**. Lossless PNG is simply poor at photographs, and a
> 200-tile grid would have been **~13 MB** on first load.
>
> **Resolved by adding an 8-bit palette path** (`png/quantise.rs`, median cut) rather than
> by cutting the thumbnail size or reviving the JPEG encoder. Measured across the
> near-duplicate set: **554,801 B truecolour → 135,336 B, a 4.1× reduction**, so the grid is
> ~3.2 MB. On a single thumbnail darkroom now produces **13,196 B against Pillow's 13,881 B
> at a mean colour drift of 4.89 against Pillow's 5.58** — smaller *and* closer to the
> source than the reference quantiser.
>
> Both encodings are produced and the better one is kept, so this can never make a file
> larger. **Images of 256 colours or fewer — screenshots, flat graphics — quantise exactly
> and stay lossless.** Dithering was implemented and dropped: 36% larger for a smoothing
> benefit invisible at the ~112 px a phone actually renders a tile at.

- Stored **in the index**, not as files on disk. One `open()` per thumbnail across a
  200-tile grid on a phone is the difference between instant and sluggish.
- **Orientation is already applied** before this point.

---

## 9. What can go wrong

| Case | Handling |
| --- | --- |
| Image that never decoded (HEIC, corrupt) | `phash = 0`, `dhash = 0`, **excluded from clustering** — a sentinel of 0 that participates would cluster every failure together |
| 1×1 or 2×3 image | Resize handles degenerate sizes; hash is meaningless but harmless |
| Screenshots, flat colour blocks | Genuinely produce near-identical hashes. **This is the real false-positive source**, and it is why the UI never deletes anything without asking |
| Grayscale source | Works unchanged |
| Enormous panorama (20000×150) | Box filter first; no intermediate allocation larger than the source |

**darkroom never deletes anything.** It shows clusters, marks a keeper, and reports
reclaimable bytes. Deletion is the user's, in their own file manager. A heuristic with
false positives must not be wired to an irreversible action, and saying so is a Code Quality
signal as much as an ethical one.

---

## 10. Oracle

`../../corpus/near-duplicates/` — eight files, assertions written as a fixture test:

```
original          ─┐
cropped-10pct      │
scaled-160         ├─ must all land in one cluster
scaled-320         │
scaled-1280        │
reencoded-q40     ─┘
rotated-90        ─┘  (was "must NOT cluster"; now clusters — see §5)
unrelated          ── must NOT cluster (a false positive here fails the build)
```

That last line is the one that matters. A hash that clusters everything scores 100% on the
first six and is useless.
