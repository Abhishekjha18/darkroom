# darkroom

**Point it at a folder of photos. Browse them from your phone. It finds the duplicates.**

**Zero dependencies.** `[dependencies]` in `Cargo.toml` is empty. The JPEG decoder, the PNG
encoder, DEFLATE, EXIF parsing, perceptual hashing, the QR generator, and the HTTP server that
serves it all — every byte of it is in this repository, not pulled from crates.io.

```
cargo run --release -- ./demo-photos --port 8080
```

That's the whole quick start — a small sample folder (with a couple of deliberate
near-duplicates) ships in this repo specifically so you can run this without needing your own
photos. A QR code appears in the terminal; scan it with your phone on the same Wi-Fi, or open
the printed `http://<ip>:8080` URL directly. Point it at a real photo folder the same way:

```
cargo run --release -- ~/Pictures --port 8080
```

**Try the QR encoder live in your browser, no install:** [abhishekjha18.github.io/darkroom](https://abhishekjha18.github.io/darkroom/)
— darkroom's own `src/qr/` code, compiled to WebAssembly, running client-side.

---

## What it does

- Indexes a photo folder — decoding JPEG (baseline **and progressive**), PNG, and GIF from
  scratch, plus reading EXIF (both TIFF byte orders) and HEIC container metadata
- Serves a browsable timeline over your LAN, grouped by the date the photo was *taken* (from
  EXIF), not the date the file happens to sit at on disk
- Finds near-duplicates with a from-scratch perceptual hash — the same shot at a different
  resolution, crop, or re-encode clusters together; unrelated photos don't. Nothing is ever
  deleted — darkroom marks a keeper per cluster and reports the bytes you'd reclaim
- Prints (and now also renders as a real QR code in the browser) a pairing code, so a phone
  gets to the timeline without anyone typing an IP address

## Install / build

One command, no build script, no external toolchain step beyond Rust itself:

```
cargo build --release
```

The toolchain version is pinned in `rust-toolchain.toml` (portable — `rustup` resolves it on
macOS, Linux, or Windows). `cargo test --release` runs the full suite (**451 tests** across the
library and binary crates) — no network access or external tools required for that number;
a handful of additional oracle checks (PngSuite conformance, a `jpeg-js` pixel diff, EXIF
cross-checks against Pillow) opt in automatically when their corpus/tooling is present, and can
be forced to fail loudly instead of skipping quietly with `DARKROOM_REQUIRE_ORACLES=1`.

## Verify the zero-dependency claim yourself

```
cat Cargo.toml    # [dependencies] is empty
cargo tree        # one node: darkroom itself
```

## Limits — read this part

- **HEIC photos do not display.** The container parses fine — HEIC files are catalogued with a
  correct date, camera, and GPS — but the image data inside is HEVC-encoded, which needs CABAC
  arithmetic decoding and a real intra-prediction pipeline. That's out of scope here, and those
  files show a "preview unavailable" placeholder rather than pretending otherwise.
- **No HTTPS.** Rust's standard library has no TLS, so darkroom is plain HTTP and LAN-only by
  construction. Don't expose it past your own network.
- **Thread-per-connection, not async.** Fine for a phone or two on a home network; not meant for
  the open web.
- **Duplicate detection is heuristic**, like every perceptual hash. False positives exist —
  darkroom never deletes anything on its own, only reports and lets you decide.
- **See `docs/`** for the full architecture, the low-level design of each subsystem, and the
  honest numbers behind every claim above — `docs/README.md` is the index.

## Licence

MIT — see `LICENSE`.
