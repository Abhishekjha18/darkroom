# darkroom — documentation

> **"Point it at a folder of photos. Browse them from your phone. It finds the duplicates."**
>
> A photo library that decodes JPEG, writes PNG, compresses with DEFLATE, parses EXIF,
> perceptually hashes, serves HTTP and generates a QR code — all with an
> **empty `[dependencies]`**.

> 🥈 **Status: runner-up.** `zql` is the pick, by a composite of **4.655 to 4.505**. This
> folder exists so darkroom is a *decision* away from being buildable, not a week away —
> and because the reasoning behind that 0.15 is contestable in two specific places
> (`OVERVIEW.md` §6).

---

## The documents

Read in this order.

| # | File | What it is |
| --- | --- | --- |
| 1 | **`OVERVIEW.md`** | The idea: pitch, scoring, why this shape, why it lost to `zql`, and what would bring it back |
| 2 | **`FEATURES.md`** | Everything planned, marked Core / Planned / Stretch, with the binding cut order and the README-ready limitations |
| 3 | **`ARCHITECTURE.md`** | The system design: module tree, contracts, concurrency, the index format, costing, and the rung-by-rung build order |
| 4 | **`RULES.md`** | Where darkroom's answer differs from `zql`'s — track, bonus, deliverables. The full rules live in `../../zql/docs/RULES.md` |
| 5 | **`BUILD.md`** | Toolchain, the reproducible-build recipe, oracles, networking, demo-day hazards |

### Low-level design — one document per subsystem

| File | Module | Planned | Built | Verified against |
| --- | --- | --- | --- | --- |
| **`LLD-JPEG.md`** | JPEG decode, baseline + progressive | 1,000 | ✅ | `jpeg-js`, 19 files, max Δ 3/255 |
| **`LLD-DEFLATE-PNG.md`** | DEFLATE, zlib, CRC32, PNG encode/decode | 850 | ✅ | real `gunzip` 1.13, Python zlib, PngSuite ×176, Pillow |
| **`LLD-EXIF.md`** | EXIF / TIFF IFD | 400 | ✅ | synthetic IFDs, both byte orders |
| **`LLD-PHASH.md`** | perceptual hashing, clustering, resampling | 550 | ✅ | the 8-file near-duplicate fixture |
| **`LLD-QR.md`** | GF(256), Reed–Solomon, masking | 350 | ✅ | published RS generators; format info decoded back |
| **`LLD-HTTP.md`** | HTTP server, thread pool, index, web client | 1,700 | ✅ | real browsers, `curl`, byte-identical serving |

### Evidence and execution

| File | What it is |
| --- | --- |
| **`SPIKE-JPEG.md`** | The decoder spike — 588 lines, verified against `jpeg-js`, zero panics on 23 adversarial files. **It falsified my own central argument against darkroom** |
| **`CORPUS.md`** | 215 staged test files mapped to the module and the assertion each one makes |
| **`SPECS.md`** | The specifications to read, in dependency order, and what to extract from each |
| **`STDLIB.md`** | The 14 substitutions, each with what it cost. Draft of a required deliverable |
| **`DEMO-SCRIPT.md`** | The 5-minute video, beat by beat, with the pre-flight checklist |
| **`README-SKELETON.md`** | The repo README, written now so it isn't written at hour 70 |
| **`blueprint.html`** | The buildability audit — every subsystem, its spec, its oracle, its risk |
| **`analysis.html`** | The earlier scoring analysis |
| **`qr-contrast-probe.py`** | Terminal QR contrast measurement. Not darkroom source |

Still in `../../planning/`, because they compare projects rather than describe one:
`FINALISTS.md`, `VERDICT.md`, `IDEAS*.md`, `PRIOR-ART.md`, `OPTIONS.md`, `PREFLIGHT.md`, `JOURNAL.md`.

---

## The short version

**What it is.** One binary that walks a photo folder, decodes every image itself, reads the
date the photo was actually taken, clusters near-duplicates by perceptual hash, and serves
the result to a phone over the LAN from an HTTP server it also wrote.

**Why it scores.** Fourteen non-trivial stdlib substitutions with published specifications and
external oracles carry the 30% Craft criterion; a photo folder everyone already has carries
the 35% Functionality criterion; sixteen modules that each own one format carry the 25%
Code Quality criterion.

**Where the risk is.** 5,000 itemised lines against a 4,000 target and ~42 building hours —
and every judge on the panel is holding an iPhone, whose photos darkroom deliberately does
not decode.

**Why it is still credible.** Every rung ships independently. Rung 1 — browse a real photo
folder from your phone — is submittable three hours in, before a single line of codec
exists, because the browser can decode JPEG itself.

---

## Current status

| | |
| --- | --- |
| **All six rungs** | ✅ **built** — R0 through R5, `src/` |
| **Run against a real 414-file library** | ✅ **0 failures**, 8.1 s cold / 6 ms warm, 20 MB RSS, 1679 req/s under 12 concurrent clients |
| JPEG decode, **baseline + progressive** | ✅ `jpeg-js` pixel diff over **19 files**, **max Δ = 3/255** (one byte at 4 in 3.7 M) |
| DEFLATE / zlib / CRC32 | ✅ **6 streams accepted by real `gunzip` 1.13**, incl. `-t`; Python zlib agrees |
| PNG encode | ✅ read back pixel-identical by Pillow; **1.15× libpng at level 9** |
| PNG decode *(was Stretch)* | ✅ **PngSuite 162 valid decoded, 14 corrupt rejected, 0 false either way**; 149 files pixel-identical to Pillow |
| EXIF / TIFF | ✅ both byte orders, the value-vs-offset rule, GPS rationals |
| Perceptual hashing | ✅ **fixture passes**: 7 variants cluster **including `rotated-90`**; `unrelated` does not |
| QR + Reed–Solomon | ✅ published generator polynomials; both format-info copies decode back |
| GIF decode | ✅ **pixel-identical to Pillow** across 8 fixtures (LZW, interlace, transparency, animation) |
| Never panic | ✅ **237 corpus files through every decoder, 0 panics** |
| Test suite | ✅ **322 tests**, zero warnings |
| Reproducible build | ✅ byte-identical across two full rebuilds — `build-repro.ps1` |
| Test corpus | ✅ 237 files — 215 staged 2026-08-16, plus `exif-vendors/`, `progressive/` and `gif/` |
| Oracles installed | ✅ `gunzip` 1.13, Node 22.14 + `jpeg-js`, Python 3.11 + Pillow, PngSuite |

---

## Open decisions

The three that had to be settled before darkroom could be built are now settled by
building it. What remains is judgement, not unknowns.

- [x] ~~**Run the DEFLATE/PNG spike.**~~ **Done, and it passed.** Real `gunzip` 1.13 accepts
      darkroom's compressed output across six stream shapes including empty, incompressible
      and all-byte-values, `-t` included. The thumbnail path is no longer an unknown.
- [x] ~~**QR: keep or cut.**~~ **Kept and built** — 350 lines as costed, and the format
      information decodes back out of both copies. It stays cuttable if the clock demands it.
- [x] ~~**Close the 1,000-line gap.**~~ Resolved by measurement rather than by cutting:
      **~5,870 lines of production Rust plus 493 of web client**, against a 5,000 itemised
      estimate — and that includes PNG *decode*, which was a Stretch item and is what
      unlocks PngSuite as a 176-file oracle.
- [x] ~~**Track F or Track B.**~~ **Track B, Parsers & Data Formats.** The built code
      settles it: six published specifications, one module per format, and no README
      paragraph needed to justify the category. See `RULES.md` §1.
- [~] **Stage multi-vendor EXIF samples.** **Narrowed, not closed.** `exif-vendors/` adds
      nine hand-built files covering **both byte orders**, the left-justified big-endian
      SHORT, awkward GPS denominators, every rung of the date chain, and an ISOBMFF
      container — all cross-checked field by field against Pillow (38 fields, 2 big-endian).
      What they cannot cover is MakerNotes and real firmware quirks: that still needs a
      handful of real photos straight off a camera. `CORPUS.md` §4.
- [x] ~~**Decide about thumbnail weight.**~~ **Fixed by adding a palette path.** PNG
      thumbnails measured ~60 KB against the 3–8 KB `LLD-PHASH.md` §8 assumed. Rather than
      cut the thumbnail size or revive the JPEG encoder, `png/quantise.rs` adds median-cut
      8-bit palette output and keeps whichever encoding is smaller: **4.1× smaller overall**,
      lossless for anything under 256 colours, and now **smaller and more accurate than
      Pillow's own quantiser**. A 200-tile grid is ~3.2 MB cold, free thereafter.
