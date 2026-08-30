# darkroom — the complete feature list

> Everything planned, in one place, marked **Core** (ships or the project has failed),
> **Planned** (ships unless the clock says otherwise), or **Stretch** (only if a rung closes
> early). The cut order at the bottom is binding.
>
> Scope here is the **halved** darkroom — 5,000 itemised lines against a 4,000 target. The
> full 7,750-line version is off the table: it scored *worse* than the halved one and was
> twice as likely to fail.

---

## 1. The product, in one line

> **Point it at a folder of photos. Browse them from your phone. It finds the duplicates.**

Nothing is uploaded anywhere. That is a product claim and an architectural one at the same
time.

Two audiences, and the demo serves them in this order:

1. **The one nobody can do by hand** — a burst of eight near-identical shots collapsing into
   one cluster, with a reclaimed-space number. This is the cold open.
2. **The one that needs no explanation** — photos appearing on a phone that scanned a QR
   code off the terminal. Legible to every judge on the panel without a word of setup.

*(The order was swapped deliberately. `Traverse` — a Rust P2P file-transfer CLI — won
100 Lines, so a QR-to-phone cold open reads as a past winner's territory rather than as
novelty. See `../../planning/PRIOR-ART.md`.)*

---

## 2. Image formats

| # | Feature | Status | Notes |
|---|---|---|---|
| 2.1 | **Baseline JPEG decode** | **Core** | Spiked ✅ — 588 lines, verified against `jpeg-js`, all three subsampling modes right first try. See `SPIKE-JPEG.md` |
| 2.2 | Chroma subsampling 4:4:4 / 4:2:2 / 4:2:0 | **Core** | The MCU interleaving is where decoders die. Already proven |
| 2.3 | Restart interval / `RSTn` resync | **Core** | What turns a truncated scan into a partial image instead of an error |
| 2.4 | **PNG encode** (8-bit RGB, non-interlaced) | **Core** | The thumbnail path. **Not yet spiked** |
| 2.5 | **DEFLATE + zlib + CRC32** | **Core** | Nothing renders without it. **Not yet spiked — first spike to run** |
| 2.6 | PNG decode (all colour types, bit depths, Adam7) | **Stretch** | Unlocks PngSuite as a 178-file oracle. First on the cut list |
| 2.7 | **GIF decode (LZW)** | **Core** | ⚠️ **Was cut, then reinstated on evidence — see §7.2.** LZW, interlacing, transparency, first frame of an animation. **Pixel-identical to Pillow** across 8 fixtures |
| 2.8 | JPEG encode | ❌ **Cut** | PNG thumbnails are sufficient at 256 px |
| 2.9 | **Progressive JPEG (SOF2)** | **Core** | ⚠️ **Was cut, then reinstated on evidence — see §7.1.** Spectral selection, successive approximation, EOBRUN. Verified against `jpeg-js` at max Δ 3/255 across 8 fixtures |
| 2.10 | **HEIC / HEVC pixel decode** | ❌ **Refused, permanently** | Several thousand lines of bit-exact code that fails silently. See §8 |

---

## 3. Metadata

| # | Feature | Status | Notes |
|---|---|---|---|
| 3.1 | **EXIF / TIFF IFD parse, both byte orders** | **Core** | The timeline is sorted by date *taken*, not file mtime — the whole difference between a gallery and a directory listing |
| 3.2 | Date fallback chain | **Core** | `DateTimeOriginal` → `DateTimeDigitized` → `DateTime` → mtime, and the UI marks which |
| 3.3 | **Orientation, applied before thumbnail and hash** | **Core** | Skip it and you get sideways thumbnails *and* broken clustering — one bug presenting as two |
| 3.4 | Camera, lens, ISO, exposure, f-number | **Planned** | Exposure kept as an exact rational — "1/250", not 0.004 |
| 3.5 | GPS with rational→degrees conversion | **Planned** | Vendor denominators vary wildly; a `0` denominator is real |
| 3.6 | **HEIC catalogued from the ISOBMFF container** | **Planned** | Real dates, camera and GPS for files whose pixels never decode |
| 3.7 | Calendar arithmetic (Hinnant, ~40 lines) | **Core** | `std` gives seconds since epoch and nothing else |
| 3.8 | MakerNotes | ❌ **Out of scope** | Every vendor invented their own undocumented format |
| 3.9 | Using the camera's embedded IFD1 thumbnail | ❌ **Refused** | It is right there and it would skip the decode path — which is exactly why. The headline claim is that darkroom decodes the image itself |

---

## 4. The payload — duplicates

| # | Feature | Status | Notes |
|---|---|---|---|
| 4.1 | **DCT perceptual hash (pHash)** | **Core** | 32×32 grey → 2-D DCT → top-left 8×8 excluding DC → median threshold |
| 4.2 | **dHash** | **Core** | ~30 lines, nearly free, a fast pre-filter and a second opinion |
| 4.3 | Hamming distance + threshold clustering | **Core** | Union-find, single-linkage |
| 4.4 | Cluster diameter cap | **Planned** | ~15 lines, and it prevents the demo's worst moment: one cluster containing everything |
| 4.5 | Keeper selection + reclaimable bytes | **Core** | **The number in the demo** |
| 4.6 | **Rotation-invariant matching** | **Core** | ⚠️ **Was out of scope — see §7.3.** Three extra DCTs on a 32×32 grid, not 4× the pipeline. `rotated-90.jpg` now clusters, and the fixture asserts it |
| 4.7 | Deleting anything | ❌ **Refused** | A heuristic with false positives must never be wired to an irreversible action |

---

## 5. Thumbnails and resampling

| # | Feature | Status | Notes |
|---|---|---|---|
| 5.1 | Box + bilinear, separable two-pass | **Core** | Box to within 2× of target, then bilinear to exact — the naive single-step downscale aliases badly |
| 5.2 | 256 px long edge, stored **in the index** | **Core** | One `open()` per tile across a 200-tile grid is the difference between instant and sluggish on a phone |
| 5.3 | Lanczos-3 | ❌ **Cut** | Indistinguishable at 256 px |
| 5.4 | Linear-light resampling | ❌ **Out of scope** | Gamma-encoded sRGB is technically wrong and visually imperceptible here. Disclosed rather than discovered |

---

## 6. Server and interface

| # | Feature | Status | Notes |
|---|---|---|---|
| 6.1 | **HTTP/1.1 server, keep-alive** | **Core** | Replaces `tokio` + `hyper` + `axum` |
| 6.2 | Thread pool, capped at 8 workers | **Core** | Replaces `rayon`. No work stealing — the honest cost |
| 6.3 | Connection cap + read/write timeouts | **Core** | A phone opening six parallel connections for a thumbnail grid is normal traffic, not an attack |
| 6.4 | **Crash-safe on-disk index** | **Core** | temp → `sync_all()` → `rename`. FNV-1a, **never `DefaultHasher`** |
| 6.5 | Incremental re-index on `(path, bytes, mtime)` | **Core** | The second run does no decoding at all |
| 6.6 | ETag / conditional GET on thumbnails | **Planned** | What makes the grid instant on a second load |
| 6.7 | **Live indexing progress over SSE** | **Planned** | Motion is what past winners have in common, and a live counter is the cheapest motion available |
| 6.8 | Range requests on originals | **Stretch** | Only matters for large files |
| 6.9 | **Embedded web client** — timeline, detail, clusters | **Core** | Three files, `include_str!`, no framework, no build step |
| 6.10 | **QR code in the terminal** (v3–4, byte mode, ECC-L) | **Planned, contested** | The most decisive oracle in the project and cut candidate #1. See §7 |
| 6.11 | `qr.png` written to disk | **Planned** | Free, and the fallback when a terminal renders glyphs badly on camera |
| 6.12 | Local IP via the UDP default-route trick | **Core** | This machine has 7 non-loopback interfaces and **6 are link-local junk** |
| 6.13 | mDNS / `darkroom.local` | ❌ **Impossible** | `std` has no `SO_REUSEADDR`, so port 5353 cannot be shared with a running Bonjour/avahi |
| 6.14 | HTTPS | ❌ **Impossible** | No TLS in `std`. LAN-only plain HTTP, by construction |
| 6.15 | CLI: `--port`, `--host`, `--no-index`, `--invert`, `--help` | **Core** | |

---

## 7. The features that are easy to get wrong about

### 7.1 ⚠️ Progressive JPEG was cut on a wrong assumption

This document said progressive was *"common on the web and rarer from cameras"* and cut it
as *"not load-bearing"*. **Both halves of that were wrong**, and a single folder of real
phone files proved it:

> Four JPEGs pulled from a real phone gallery on 2026-08-19. **All four were SOF2
> progressive.** They were WhatsApp images — and **WhatsApp re-encodes everything it sends
> as progressive.**

That reframes the feature entirely. It is not a web-only edge case; it is *the single most
common JPEG variant in a real phone gallery*, because messaging apps produce it. A photo
gallery that shows *preview unavailable* on every image a user was sent is not missing a
nicety, it is broken for the most common content it will meet.

**Now implemented and verified**: spectral selection, successive approximation (`Ah`/`Al`),
the `EOBRUN` mechanism, DC and AC refinement, and non-interleaved scans. Measured against
`jpeg-js` across 8 progressive fixtures — every subsampling mode, greyscale, odd dimensions,
restart intervals, q30 and q95 — at **max Δ 3/255**, the same tolerance as baseline.

**The lesson worth keeping:** the cut was justified by an assumption about what real files
look like, and nobody had looked at any real files. One folder settled it.

### 7.2 ⚠️ GIF was cut as "nice, not needed" — until it was the only failure

GIF was cut because it seemed like a format nobody keeps photos in. Then darkroom ran
against a **real 414-file library**: 396 files decoded, and the **only two failures in the
whole folder were GIFs**. Two files is not many; being the *entire* failure list is what
made it worth ~250 lines.

Now implemented: LZW with the growing code width, interlacing, transparency composited onto
white, and the first frame of an animation (a gallery shows one frame, not a movie).
**Pixel-identical to Pillow** across 8 fixtures — GIF is lossless and a palette entry is a
palette entry, so the bar is exact equality rather than a tolerance.

**The lesson is the one progressive JPEG already taught**: the cut was justified by a guess
about what real folders contain, and one real folder settled it.

### 7.3 Rotation-invariant matching turned out to be cheap

This was refused as *"4× the work for a rare case"*. **That costing was wrong.** The 32×32
grey reduction is computed **once**; rotating it is an index permutation, so the real cost
is three extra DCTs over 1024 samples — invisible next to decoding the JPEG that produced
them. Measured on a 414-file library: **20.3 ms per image against 19.4 ms before.**

`corpus/near-duplicates/rotated-90.jpg` moved from the "must NOT cluster" list to the "must
cluster" list, and the fixture asserts it. `unrelated.jpg` still must not — that negative
control is the line that actually matters.

### 7.4 The two features that are easy to get wrong about

**QR is contested, and the tension is real.** It is 350 lines of genuine coding theory —
GF(256), Reed–Solomon generator polynomials, eight masks with penalty scoring — which is the
highest craft-per-line in the project, on the 30% criterion. It is also pure demo: the tool
works identically with a printed URL, and the demo's cold open already moved off it.
**Unresolved. Settle it before darkroom is revived, not at hour 40.** See `LLD-QR.md` §9.

**Mask penalty scoring is not optional.** Implementing the eight masks but skipping the
scoring "because mask 0 works on my test string" produces a code that scans today and fails
when the URL changes by one character. ~60 lines, and it is the difference between reliable
and lucky.

---

## 8. What is refused, and why that is a feature

**HEIC / HEVC pixel decoding.** The container is ordinary ISOBMFF. The image inside is HEVC
intra-coded: CABAC arithmetic decoding, quadtree CTU partitioning, thirty-five intra
prediction modes, deblocking, SAO. Several thousand lines of bit-exact code that **fails
silently when it is subtly wrong** — the worst failure mode available inside 72 hours.

**What ships instead:** HEIC files are catalogued from the container. Date, camera,
dimensions and GPS all correct, the file appears in the timeline in the right place, and the
tile reads *preview unavailable — HEVC*.

**This is darkroom's biggest product risk and it should be named as one:** every judge on
the panel is holding an iPhone, and iPhone photos are HEIC. The mitigation is disclosure at
4:30 in the demo, on our terms — and a demo folder that is JPEG only, so a judge does not
find it at 1:15 instead.

---

## 9. Budget and cut order

**5,000 lines itemised. 4,000 target. ~42 building hours.** The 1,000-line gap is real and
unclosed — see `ARCHITECTURE.md` §10.2.

Cut in this order, from the bottom:

| Order | Cut | Saves | Cost of cutting |
|---|---|---|---|
| 1 | PNG decode (Stretch) | ~350 | Loses PngSuite as an oracle. Painful for the Craft criterion, invisible to a user |
| 2 | Thread pool → fixed threads + chunked ranges | ~150 | Slower on uneven batches |
| 3 | Web UI 500 → 300 | ~200 | Drops the cluster animation and the EXIF panel — real demo cost |
| 4 | Index thumbnail store | ~150 | Regenerate on demand; slower grid |
| 5 | **QR** | ~350 | Print the URL. **Contested — see §7** |
| 6 | Range requests | ~80 | Only matters for large originals |
| 7 | GPS | ~80 | One line of the detail panel |

**Never cut upward into a rung already shipped** (`ARCHITECTURE.md` §11). If a cut into
rung 4 — the duplicate finder — is ever being considered, cut the Monday polish block
instead. Rung 4 *is* the product.

---

## 10. Stated limitations — these go in the README verbatim

Written down as pitch assets, not embarrassments. An honest limitations section reads as
confidence; a missing one reads as a project nobody tested.

- **HEIC photos do not display.** They are catalogued with correct dates, camera and GPS,
  and show a *preview unavailable — HEVC* placeholder. The reason is one sentence and it is
  said out loud.
- **No HTTPS.** `std` has no TLS, so darkroom is plain HTTP and LAN-only by construction.
  Do not expose it to the internet.
- **Thread-per-connection, not async.** Fine for a handful of clients on a home network. It
  would not survive the open web and is not meant to.
- **Slower than libjpeg-turbo**, which is hand-written SIMD assembly. Measured: ~9 MP/s from
  a naive float IDCT. Published, not hidden.
- **Duplicate detection is heuristic.** Perceptual hashing has false positives — screenshots
  and flat colour blocks are the real source. darkroom never deletes anything.
- **Resampling happens in gamma-encoded sRGB**, not linear light.
- **No time zone database.** A photo taken abroad sorts by the camera's clock, not yours.
- **The ETag salt is not cryptographically random.** Nothing security-critical depends on it.
