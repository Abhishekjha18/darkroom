# The hackathon — darkroom's reading of the rules

> **The full rules, rubric, deadlines and deliverables live in one place:
> [`../../zql/docs/RULES.md`](../../zql/docs/RULES.md).** They are project-independent and
> were re-verified against the live site on 2026-08-17. Duplicating them here would only
> create two copies to drift apart.
>
> This document records the parts where **darkroom's answer differs from `zql`'s**, plus
> the one rule that governs whether any of this may be written yet.

---

## 1. Track — darkroom's answer differs

The entry form requires exactly one of six tracks.

| Track | | darkroom | `zql` |
| --- | --- | --- | --- |
| A | Developer Tools & CLI | no | plausible |
| B | **Parsers & Data Formats** | ✅ **the pick** | plausible |
| C | Web & Network | no | no |
| D | Data & Storage | no | ✅ **the pick** |
| E | Security & Crypto | no | no |
| F | Wildcard | ~~the blueprint's pick~~ — **overturned, see below** | no |

**Track B — decided 2026-08-19, overturning the blueprint.**

`blueprint.html` chose Wildcard. That was the wrong call and the built code settles it:
darkroom is, structurally, **six format implementations in a trenchcoat** — JPEG (ITU-T
T.81), PNG (RFC 2083), DEFLATE and zlib (RFC 1951/1950), EXIF/TIFF (CIPA DC-008), QR
(ISO/IEC 18004), and the index's own on-disk format. Every one of them is a published
specification parsed or emitted from scratch, and `src/` is laid out one module per format.

**Track B (Parsers & Data Formats) describes that exactly, and needs no justification.**
Wildcard would have cost a README paragraph defending the category choice and earned
nothing for it.

**The counter-argument, for the record:** darkroom is also a photo gallery and an HTTP
server, so a judge might read "Parsers & Data Formats" as underselling the product. That is
a real cost, and it is the smaller one — the parsers are where the 30% Craft criterion is
won, and the track should point at them.

---

## 2. Bonuses — ⚠️ "Pick one and do it well"

The site says pick one. Earlier planning — including `../../planning/VERDICT.md`'s "11/16, identical for
both" line — assumed all four stack. **That was wrong, and it applies to darkroom exactly
as it applies to `zql`: the bonus is +5, not +11.**

| Bonus | Difficulty | Points | darkroom's position |
| --- | --- | --- | --- |
| **Reproducible Build** | Hard | +5 | ✅ **This is the one.** Already verified on this machine — two clean builds, identical SHA-256. See `BUILD.md` §3 |
| Single File | Hard | +5 | ❌ Declined. 5,000 lines in one file is a stunt that costs more inside the 25% Code Quality criterion than the +5 buys |
| Package Killer | Medium | +3 | ➖ Not the declared bonus, but **it has its own $100 prize** and darkroom's kill list is the strongest asset it has: `sharp` · `Pillow` · `libvips` · `image` · `jpeg-decoder` · `png` · `flate2` · `crc32fast` · `kamadak-exif` · `img_hash` · `qrcode` · `tokio` · `hyper` · `axum` · `rayon` · `serde_json` · `chrono` · `walkdir` · `clap` · `rand` |
| STDLIB Log | Medium | +3 | ➖ Not the declared bonus — **`STDLIB.md` is a required deliverable anyway**, and darkroom's has **14 substitutions**, which feeds the 30% criterion directly |

darkroom's Package Killer list is longer and heavier than `zql`'s. It is not the declared
bonus, but it is a separate prize and it gets claimed visually in the demo at 4:00.

---

## 3. Deliverables — darkroom-specific notes

The full list is in `../../zql/docs/RULES.md` §6. Three of them land differently here:

- **The demo video must show the tool *and* the empty manifest.** darkroom's script holds
  the manifest shot in one unbroken take at 0:20 and closes on it at 4:50 — see
  `DEMO-SCRIPT.md`. **Forgetting the manifest shot is the commonest way to lose points**,
  and it is a required deliverable, not a flourish.
- **Dependency proof** — command output or a CI log. Budget ~20 minutes. Not previously
  recorded anywhere in darkroom's planning.
- **One-command build.** darkroom passes trivially: no build script, no fixture generation,
  no Python at build time. The web client is `include_str!`'d. Worth protecting — a
  `build.rs` that shells out to anything would break the highest-weighted criterion.

**Out of scope, per the site — darkroom checked against all seven:** not a trivial toy; does
not shell out to a separately installed tool *(it decodes JPEG itself — using the OS or
`sqlite3` or ImageMagick would be exactly this failure)*; no undisclosed vendoring; no
homemade ciphers; not an LLM dump with no docs; no custom hardware; **no GUI framework —
the interface is a browser rendering HTML this binary serves**; no running third-party
service. **Clears all seven.**

---

## 4. Structural constraints `std` imposes

Full table with the answers in `ARCHITECTURE.md` §12. The three that shaped the product
rather than the code:

- **No TLS** → LAN-only plain HTTP, by construction, stated in the README under limits.
- **No `SO_REUSEADDR`** → mDNS on port 5353 is not reliably possible beside a running
  Bonjour or avahi, so `darkroom.local` is out and **the QR code is the discovery
  mechanism**. That constraint is why the QR module exists at all.
- **No RNG** → ETag salts come from a freshly constructed `RandomState`. Nothing
  security-critical depends on it and the README says so plainly.
