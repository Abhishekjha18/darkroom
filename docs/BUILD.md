# darkroom — build, toolchain, and machine environment

> The build recipe is shared with `zql` — same machine, same toolchain, same verified
> reproducible-build envelope. What is darkroom-specific is §4 onward: the oracles, the
> network, and the demo-day hazards.

---

## 1. The one-command build

The rubric's highest-weighted criterion says *"builds in one command"*. For a judge with
Rust installed and nothing else:

```
cargo build --release
```

No build script, no fixture generation step, no Python. The web client is `include_str!`'d,
the CRC table is `const`, and the test corpus is data the build never touches.

---

## 2. Toolchain — locked

| | |
| --- | --- |
| Toolchain | **`1.97.1-x86_64-pc-windows-gnu`** |
| `RUSTUP_HOME` | `D:\Aniket\rust\.rustup` |
| `CARGO_HOME` | `D:\Aniket\rust\.cargo` |
| `CARGO_TARGET_DIR` | `D:\Aniket\rust\tmp\target` |

**GNU, not MSVC — locked 2026-08-16, do not revisit.** MSVC needs Visual Studio Build
Tools, which needs gigabytes on a C: drive that had 0.3 GB free. The whole toolchain was
relocated to D: and the build verified to consume **0 MB on C:**. Every piece of
verification below was performed against GNU; switching invalidates the evidence for zero
gain — **including if a linker error tempts you at hour 50.**

---

## 3. Reproducible build — +5, verified working

**It did not work out of the box.** Two clean builds with only `--remap-path-prefix` and
`CARGO_INCREMENTAL=0` produced *different* binaries:

```
build 1: 702BB466…E7272E01
build 2: 2FED7044…38C26261   ✗
```

The cause was a **PE/COFF header timestamp the MinGW linker embeds.** Adding
`-Wl,--no-insert-timestamp` fixed it:

```
build 1: E2644016AE64E78CC1CE469E4420BA30230F645C9897EF67B81289B2866EAA56
build 2: E2644016AE64E78CC1CE469E4420BA30230F645C9897EF67B81289B2866EAA56   ✓
```

Re-verified cold later the same day: two clean builds → identical `B9CE7746…FA6BB5FB`.

### The recipe

```powershell
$env:CARGO_INCREMENTAL = "0"
$env:SOURCE_DATE_EPOCH = "1000000000"
$US = [char]0x1f
$env:CARGO_ENCODED_RUSTFLAGS =
  "--remap-path-prefix=<abs-project-path>=." + $US + "-Clink-arg=-Wl,--no-insert-timestamp"
cargo +1.97.1-x86_64-pc-windows-gnu build --release --target x86_64-pc-windows-gnu
```

Run it with `build-repro.ps1`, which is the above plus the two-build hash comparison.

### ⚠️ Correction, 2026-08-18 — `RUSTFLAGS` cannot carry this project's path

The recipe above previously used `RUSTFLAGS`. **It does not work here**, and the reason is
the project path:

```
D:\Aniket\Zero Dependency\Darkroom
              ^ this space
```

`RUSTFLAGS` is split on whitespace and **has no quoting mechanism at all**, so
`--remap-path-prefix=D:\Aniket\Zero Dependency\Darkroom=.` is torn into
`--remap-path-prefix=D:\Aniket\Zero` and `Dependency\Darkroom=.`, and the build dies with:

```
error: --remap-path-prefix must contain '=' between FROM and TO
```

`CARGO_ENCODED_RUSTFLAGS` separates arguments on `\x1f` instead, so spaces survive. Same
flags, same output, and it is the only form that works from a path containing a space.

**Verified 2026-08-18 with the real rung-1 binary**, two full rebuilds from a deleted
`release/` directory rather than a `cargo clean`:

```
build 1  B7F37D7532A5D68CCDDB8C1C6FC9BE74183F740C1CA231E29C866DC185982D98
build 2  B7F37D7532A5D68CCDDB8C1C6FC9BE74183F740C1CA231E29C866DC185982D98   ✓
```

**A second trap, found the same way:** `cargo clean --release` left the previous binary in
place and the second build finished in 0.03 s without recompiling — so the hashes matched
because nothing had been rebuilt. A comparison like that passes forever and proves nothing.
Delete the `release/` directory outright and assert the binary is *gone* before each build.

**The trap:** `rust-toolchain.toml` resolves against the **working directory**, not
`--manifest-path`. Passing `--manifest-path` from elsewhere silently ignores the pin and
falls back to the MSVC host toolchain. That costs one build cycle now and an hour at hour
68. Always use the explicit `+toolchain` and `--target` flags in the build script.

### The envelope, stated honestly in the README

Same machine, same toolchain version, same target. `C:\MinGW\bin` is on `PATH` twice and
`gcc`/`ld` resolve there — which looked like a hidden dependency, so it was checked: the
`windows-gnu` toolchain is self-contained and does not need the separate MinGW install.
Verified by scrubbing MinGW from `PATH` and rebuilding.

---

## 4. Oracles — all installed, re-verified 2026-08-18

darkroom's correctness story rests on software it did not write. All of it is present:

| Oracle | Version / location | Proves |
| --- | --- | --- |
| **`gunzip`** | **gzip 1.13**, `/usr/bin/gunzip` (Git Bash) | DEFLATE output decompresses under a real tool — README claim #1 |
| **Node** | **v22.14.0**, on PATH | Runs `jpeg-js` for the pixel-diff oracle |
| **`jpeg-js`** | to be fetched | Independent baseline decoder. **Wire it in at hour one** — see `SPIKE-JPEG.md` §7 |
| **Python 3.11** | on PATH | `generate_fixtures.py`; Pillow as a second opinion on pathological files |
| **PngSuite** | `../../corpus/pngsuite/`, 178 files | PNG conformance — README claim #2 |
| **Any browser** | | PNG validity, HTTP correctness, the web client |
| **A phone camera** | | The QR code. Scans or it doesn't |
| **The OS photo info panel** | | EXIF, field by field |

**`jpeg-js` is a dev-time oracle, not a dependency.** It never ships, it is not in
`Cargo.toml`, and it is run by `node`, not by the build. Note it in `STDLIB.md` anyway —
disclosure costs nothing and the rubric rewards it.

### Known gotchas on this machine

- **PowerShell `2>&1` on `cargo`** produces a spurious `NativeCommandError` and exit 1 even
  on a successful build. Cosmetic. Do not chase it at hour 40.
- **Python printing non-ASCII** dies with `UnicodeEncodeError` under the default codepage.
  Set `PYTHONIOENCODING=utf-8`. Relevant here — the corpus contains `unicode-写真-🎞.jpg`.
- **`PATH` edits do not reach already-open shells.** At kickoff, open a **fresh** terminal
  first, or the first thing that happens is a phantom "Rust isn't installed" panic.

---

## 5. Networking — darkroom is a server, so this matters more than it does for `zql`

Verified 2026-08-16: `0.0.0.0:8080` reachable from a phone on the same LAN, HTTP 200, at
`192.168.0.105:8080`.

**One real finding:** this machine reports **seven** non-loopback IPv4 interfaces, **six of
them `169.254.x.x` link-local junk**. A naive "enumerate interfaces, take the first
non-loopback" prints a QR code that leads nowhere — and it fails on camera, at the one
moment of the demo that cannot be re-shot casually.

The fix is the UDP default-route trick: bind `0.0.0.0:0`, `connect()` to a routable address,
read `local_addr()`. **Proven in compiled Rust, not just in theory.** Keep a `--host`
override for the no-default-route case.

**Not mDNS.** `std` has no `SO_REUSEADDR`, so port 5353 cannot be shared with a running
Bonjour or avahi, and `darkroom.local` is not reliably possible. The QR carries a raw IP and
port and needs no discovery protocol at all.

---

## 6. Demo-day hazards — none of these are engineering risks, and all of them have ended demos

- **The firewall prompt.** Windows interrupts the first bind to `0.0.0.0` with a dialog.
  Accept it once **before** recording. *(Done in preflight, 2026-08-16.)*
- **AP isolation.** Many guest and hotel networks block client-to-client traffic entirely,
  so the phone cannot reach the laptop at all no matter what the QR says. Verified working
  on the demo network; re-verify on the day, on the actual network.
- **Terminal QR contrast.** On a dark theme the naive rendering is inverted, and **most
  scanners refuse inverted codes**. Probed with `qr-contrast-probe.py`; `--invert` exists;
  `qr.png` is written to disk as the fallback path.
- **Quiet zone.** Four modules on every side, and it is the first thing dropped when the
  code doesn't fit the window. Without it phones fail to detect the code at all, and the
  failure looks like a bad encoder rather than bad framing.

---

## 7. Pre-kickoff checklist

- [x] Toolchain installed and pinned, on D:
- [x] Reproducible build verified byte-identical, twice, including from a cold shell
- [x] MSVC-vs-GNU decided and locked
- [x] LAN reachability proven from a phone
- [x] Firewall prompt accepted
- [x] Test corpus staged — 215 files
- [x] JPEG decode spiked and passed against `jpeg-js`
- [x] `gunzip`, Node 22, Python 3.11 confirmed present
- [ ] **DEFLATE/PNG encoder spike — the last load-bearing unknown**
- [ ] **Multi-vendor EXIF samples** — the one gap in the corpus (`CORPUS.md` §4)
- [ ] Fetch `jpeg-js` and stage the diff harness
- [ ] Curate the demo folder — thousands of JPEGs, **no HEIC**, with a real near-duplicate set
- [ ] Delete every spike before kickoff, `scratchpad/jpegspike/` included
- [ ] Open a **fresh** terminal at kickoff before doing anything else
