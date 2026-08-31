# darkroom

[![Playground](https://github.com/Abhishekjha18/darkroom/actions/workflows/pages.yml/badge.svg)](https://github.com/Abhishekjha18/darkroom/actions/workflows/pages.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-informational)](LICENSE)

**Point it at a folder of photos. Browse them from your phone. It finds the duplicates.**

**Zero dependencies.** `[dependencies]` in `Cargo.toml` is empty. The JPEG decoder, the PNG
codec, DEFLATE, EXIF parsing, perceptual hashing, the QR generator, and the HTTP server that
serves it all — every byte of it is in this repository, not pulled from crates.io.

```
cargo run --release -- ./demo-photos --port 8080
```

That's the whole quick start — a small sample folder (with deliberate near-duplicates baked
in) ships in this repo specifically so this runs without needing your own photos. It opens a
pairing page in your browser automatically — a real QR code and **"scan this with your
phone"** — and the terminal prints the same URL as a fallback. Point it at a real photo folder
the same way:

```
cargo run --release -- ~/Pictures --port 8080
```

**Try the QR encoder live in your browser, no install:**
[abhishekjha18.github.io/darkroom](https://abhishekjha18.github.io/darkroom/) — darkroom's own
`src/qr/` code, compiled to WebAssembly, running client-side. Type anything; what renders is a
real call across the WASM boundary into the same encoder the CLI uses.

---

## At a glance

| | |
|---|---|
| **Dependencies** | 0 — `cargo tree` is one node |
| **Tests** | 456, zero warnings |
| **Language** | Rust, ~13,500 lines |
| **Formats decoded** | JPEG (baseline + progressive), PNG, GIF, EXIF, HEIC containers |
| **Formats encoded** | PNG, QR |
| **Network** | Plain HTTP over your own LAN, nothing leaves it |

## What it does

- Indexes a photo folder — decoding JPEG (baseline **and progressive**), PNG, and GIF from
  scratch, plus reading EXIF (both TIFF byte orders) and HEIC container metadata
- Serves a browsable timeline over your LAN, grouped by the date the photo was *taken* (from
  EXIF), not the date the file happens to sit at on disk
- Finds near-duplicates with a from-scratch perceptual hash — the same shot at a different
  resolution, crop, or re-encode clusters together; unrelated photos don't. Nothing is ever
  deleted — darkroom marks a keeper per cluster and reports the bytes you'd reclaim
- Opens a pairing page with a real QR code, so a phone gets to the timeline without anyone
  typing an IP address — `--no-open` skips the auto-open for headless/SSH use
- Lets you switch which folder is being browsed from that same pairing page, live, without
  restarting the process — guarded so only the machine darkroom is running on can do it (a
  phone on the same Wi-Fi can browse the timeline, never redirect it)

## Install / build

One command, no build script, no external toolchain step beyond Rust itself:

```
cargo build --release
```

The toolchain version is pinned in `rust-toolchain.toml` by version only, not host triple, so
`rustup` resolves it the same way on macOS, Linux, or Windows.

```
cargo test --release
```

**456 tests, zero warnings**, across the library and binary crates, with no network access or
external tools required for that number. A handful of additional oracle checks (PngSuite
conformance, a `jpeg-js` pixel diff, EXIF cross-checks against Pillow) opt in automatically
when their corpus/tooling is present, and can be forced to fail loudly instead of skipping
quietly with `DARKROOM_REQUIRE_ORACLES=1 cargo test --release`.

## Verify the zero-dependency claim yourself

```
cat Cargo.toml    # [dependencies] is empty
cargo tree        # one node: darkroom itself
```

## Verified, not just claimed

The QR encoder is a good example of what "verify it yourself" is worth taking seriously here.
Every unit test for it passed from the start — and it was still silently unscannable by every
real QR reader, on every input, because the tests checked the encoder against its own (wrong)
convention rather than an independent one. It took two outside decoders (OpenCV, `jsQR`) and a
byte-level trace to find: `write_format()` placed the format-info bits MSB/LSB-reversed relative
to the actual ISO/IEC 18004 spec. Fixed in one function, confirmed against real scanners
afterward, including a live phone camera. That fix, and the reasoning behind it, is commit
[`4ae1591`](https://github.com/Abhishekjha18/darkroom/commit/4ae1591e9728d686519aa6f3fdcebeb5eba89ea3).
The honest version of "well-tested" is that this happened *despite* a passing test suite, not
that a passing suite ruled it out.

## Limits — read this part

- **HEIC photos do not display.** The container parses fine — HEIC files are catalogued with a
  correct date, camera, and GPS — but the image data inside is HEVC-encoded, which needs CABAC
  arithmetic decoding and a real intra-prediction pipeline. That's out of scope here, and those
  files show a "preview unavailable" placeholder rather than pretending otherwise.
- **No video.** Photos only, by design — a video codec is a different project.
- **No HTTPS.** Rust's standard library has no TLS, so darkroom is plain HTTP and LAN-only by
  construction. Don't expose it past your own network.
- **Thread-per-connection, not async.** Fine for a phone or two on a home network; not meant for
  the open web.
- **Duplicate detection is heuristic**, like every perceptual hash. False positives exist —
  darkroom never deletes anything on its own, only reports and lets you decide.

## Further reading

`docs/` has the full architecture, a low-level design document per subsystem, and the complete
numbers behind every claim above — start at `docs/README.md`.

## Licence

MIT — see `LICENSE`.
