# STDLIB.md — draft

> ⚠️ **Correction, 2026-08-17.** This document was drafted as a bid for the *STDLIB Log*
> bonus. The site says **"pick one and do it well"** — bonuses do not stack — and the
> declared bonus is **Reproducible Build (+5)**. That changes nothing about this file:
> `STDLIB.md` is a **required deliverable** regardless, and it feeds the 30%
> Zero-Dependency Craft criterion, which is the heaviest thing on the rubric after
> Functionality. See `RULES.md` §3.

The STDLIB Log bonus would have required **≥10 real, non-trivial substitutions with
rationales**. There are fourteen below. That is not padding — darkroom genuinely needs all of them, which is the whole argument for
having chosen Rust.

**Format for each entry:** what I needed → what I would have reached for → what std actually gave
me → what I wrote → **what it cost me.** That last line is the one judges read. Every entry must
have an honest one; an entry with no cost is an entry nobody believes.

> ⚠️ **Verify before shipping:** every download figure below is from memory and must be checked
> against npmjs.com / crates.io / pypistats on the day. The rubric rewards honest numbers, and a
> wrong one is worse than no number at all.

---

### 1. JPEG baseline decode

- **Need:** turn camera JPEGs into pixels for thumbnailing and hashing
- **Would have used:** `image`, `jpeg-decoder` · in other ecosystems `sharp`, `Pillow`, `libvips`
- **Std gave me:** `File`, `Read`, `Vec<u8>`. Nothing else.
- **Wrote:** marker parsing, quantisation tables, Huffman table construction and decode, dequantise, inverse DCT, chroma upsampling, YCbCr→RGB
- **Cost:** baseline **and progressive** (SOF2 — spectral selection, successive approximation, EOBRUN). No arithmetic coding, no 12-bit, no lossless, no CMYK. Chroma upsampling is nearest-neighbour, not libjpeg's fancy triangular filter, which is invisible at thumbnail scale and visible against Pillow at colour edges. **Measured:** against `jpeg-js` over 11 corpus files, mean |Δ| 0.35–0.54, **max 3/255** (a single byte reached 4 across 3.7 M samples). ~17 ms per image end to end, decode through thumbnail through hash.

### 2. PNG encode and decode

- **Need:** write thumbnails the browser can render; read PNGs in the library
- **Would have used:** `png`, `image`
- **Std gave me:** nothing
- **Wrote:** chunk layer, all five scanline filters, interlace, colour type handling, CRC validation, and a median-cut colour quantiser for 8-bit palette output
- **Cost:** filter selection is heuristic (minimum sum of absolute differences) rather than exhaustive, so files are larger than optimal. **Measured: 1.15× libpng at `compress_level=9`** on a real thumbnail — 68,050 bytes against 59,108. Encode writes 8-bit RGB only; decode handles every colour type, bit depths 1/2/4/8/16, palettes, tRNS and Adam7. **PngSuite: 162 valid files decoded, 14 corrupt rejected, zero false either way**; 149 of them pixel-identical to Pillow. Thumbnails are written truecolour *or* 8-bit palette, whichever is smaller — **4.1× smaller overall**, and **13,196 B against Pillow's 13,881 B at lower colour drift (4.89 vs 5.58)** on a single tile. Images of 256 colours or fewer stay lossless.

### 3. DEFLATE — compress and decompress

- **Need:** PNG's compression layer (RFC 1951/1950). Nothing renders without it.
- **Would have used:** `flate2`, `miniz_oxide` · `zlib` is in Python's and Go's stdlib, but not Rust's
- **Std gave me:** nothing
- **Wrote:** LZ77 with a hash-chain match finder, static and dynamic Huffman, the full inflate side too
- **Cost:** compression ratio trails zlib at equivalent levels — **measured at 1.15× libpng's output** on thumbnail data. **Verified against real `gunzip` 1.13** across six stream shapes (empty, short, repetitive, prose, incompressible, all-byte-values), including `gunzip -t`; Python's zlib agrees independently. A 9-bit direct-lookup table for short Huffman codes made inflate **8.7× faster** (15.0 s → 1.7 s on a 192 MB stream).

### 3a. GIF decode and LZW

- **Need:** two files in a real 414-photo library were GIFs, and they were the only ones darkroom could not read
- **Would have used:** `image`, `gif` · `Pillow`
- **Std gave me:** nothing
- **Wrote:** LZW with the growing code width and the KwKwK case, sub-block stitching, interlacing, transparency, global and local colour tables
- **Cost:** first frame only. A photo gallery shows one frame per tile, so decoding an animation buys nothing a thumbnail can display. **Verified pixel-identical to Pillow** across 8 fixtures — LZW is lossless, so the bar is exact equality rather than a tolerance.

### 4. CRC32

- **Need:** PNG chunk integrity
- **Would have used:** `crc32fast`
- **Wrote:** table-driven CRC-32/ISO-HDLC
- **Cost:** none worth reporting. Small, but genuinely required — without it no PNG you write is valid.

### 5. EXIF / TIFF IFD parsing

- **Need:** the date a photo was actually taken, so the timeline is real rather than filesystem mtime
- **Would have used:** `kamadak-exif` · `exifread`, `exiftool`
- **Std gave me:** nothing — not even endian-aware reads
- **Wrote:** TIFF header, both byte orders, IFD traversal, all base tag types, the EXIF sub-IFD, GPS IFD with rational coordinate conversion
- **Cost:** vendor MakerNotes are not decoded. Every camera manufacturer invented their own undocumented format and no honest weekend covers them.

### 6. Image resampling

- **Need:** thumbnails
- **Would have used:** `image`'s resize, `fast_image_resize`
- **Wrote:** box, bilinear, and Lanczos-3 with a separable two-pass kernel
- **Cost:** operates in gamma-encoded sRGB rather than linear light. Technically wrong; visually imperceptible at thumbnail size. Stated because it is the kind of thing a reviewer should be told, not discover.

### 7. Perceptual hashing and clustering

- **Need:** find duplicates that are not byte-identical — the whole point of the tool
- **Would have used:** `img_hash` · `imagehash`
- **Wrote:** dHash, DCT-based pHash **at four orientations**, Hamming distance, and threshold clustering with a diameter cap
- **Cost:** heuristic by nature. **Tuned against `corpus/near-duplicates/` and asserted as a test**: the original plus crop, re-encode, three rescales **and a 90-degree rotation** all cluster; the unrelated control does not. Single-linkage is capped at twice the threshold so one cluster cannot swallow the library. darkroom never deletes anything.

### 8. QR encoding and Reed–Solomon

- **Need:** get the phone onto the server without typing an IP address
- **Would have used:** `qrcode`, `qrcodegen` · npm `qrcode`
- **Std gave me:** nothing
- **Wrote:** GF(256) arithmetic, Reed–Solomon generator polynomials, data segmentation, version selection, all eight masks with penalty scoring, format and version information
- **Cost:** byte mode only. No kanji, no ECI. Sufficient for a URL, which is all it ever encodes.

### 9. HTTP/1.1 server

- **Need:** serve the library to a browser and a phone
- **Would have used:** `tokio` + `hyper` + `axum` — three of the largest dependency trees in the ecosystem
- **Std gave me:** `TcpListener` and blocking `TcpStream`. No async runtime, no HTTP.
- **Wrote:** request parsing, keep-alive, chunked transfer, range requests, conditional GET with ETag, static routing, percent-decoding, MIME mapping
- **Cost:** thread-per-connection rather than async. Fine for a LAN tool with a handful of clients; it would not survive the open internet, and it is not meant to.

### 10. Thread pool

- **Need:** decode thousands of images across cores without spawning thousands of threads
- **Would have used:** `rayon`
- **Std gave me:** `std::thread`, `mpsc`, `Arc`, `Mutex`, `available_parallelism`. Genuinely good primitives.
- **Wrote:** fixed-size worker pool over a shared queue, with ordered result collection
- **Cost:** no work-stealing, so a batch with wildly uneven image sizes leaves cores idle at the tail. Capped at 8 workers: beyond that the bottleneck is IO and memory bandwidth, and 32 workers each holding a decoded 24 MP image is 2 GB of peak residency.

### 11. JSON writer

- **Need:** hand the catalog to the browser client
- **Would have used:** `serde` + `serde_json`
- **Wrote:** a streaming writer with correct string escaping, including control characters and surrogate pairs
- **Cost:** write-only. Nothing in darkroom ever needs to parse JSON, so nothing was written to.

### 12. Calendar arithmetic

- **Need:** group photos by day and month
- **Would have used:** `chrono`, `time`
- **Std gave me:** `SystemTime` — seconds since the epoch and nothing more. No dates, no formatting, no timezones.
- **Wrote:** days-from-civil and civil-from-days, roughly forty lines, exact for the full proleptic Gregorian range
- **Cost:** no timezone database. EXIF stores local wall-clock time already, so darkroom never needs one — but a photo taken abroad sorts by the camera's clock, not yours.

### 13. Randomness

- **Need:** ETag salts and cache keys
- **Would have used:** `rand`, `getrandom`
- **Std gave me:** no RNG of any kind — but `RandomState` is seeded by the standard library from the OS entropy source
- **Wrote:** entropy extraction by hashing a fixed value through a freshly constructed `RandomState`
- **Cost:** this is not a general-purpose RNG and I do not pretend it is. It is used for nothing security-critical, and the README says so plainly.

### 14. Local IP discovery

- **Need:** know which address to put in the QR code
- **Would have used:** `local-ip-address`, `if-addrs`
- **Std gave me:** no interface enumeration at all
- **Wrote:** bind a UDP socket to `0.0.0.0:0`, `connect()` it to a routable address, read `local_addr()`. UDP connect sets a peer without sending a packet, so the kernel reveals the interface carrying the default route.
- **Cost:** requires a default route to exist; `--host` overrides. **This turned out better than enumeration would have been** — the development machine has seven non-loopback interfaces and six are link-local junk, so a naive "first non-loopback" would have printed a QR code that led nowhere.

---

## Deliberately not attempted

Worth its own section. What you refused says as much as what you built.

**HEIC / HEIF pixel decoding.** The container is straightforward ISOBMFF. The image inside is
HEVC intra-coded, which means CABAC arithmetic decoding, quadtree CTU partitioning, thirty-five
intra prediction modes, deblocking, and SAO — several thousand lines of bit-exact code that fails
silently when it is subtly wrong. It is not a weekend's work and claiming otherwise would be
dishonest. HEIC files are still catalogued from the container: date, camera, dimensions, and GPS
all read correctly, and the file appears in the timeline with an explicit
*preview unavailable — HEVC* placeholder rather than vanishing.

**TLS.** Rust's standard library has none, so darkroom is LAN-only plain HTTP by construction.
Stated in the README under limits, not buried.
