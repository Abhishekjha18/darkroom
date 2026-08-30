# LLD — QR encoding and Reed–Solomon

> ISO/IEC 18004 · **350 lines** (halved scope) / 600 (full) · risk **low** ·
> oracle **any phone — it scans or it doesn't**

---

## 1. The most decisive oracle in the project

Every other module needs a comparison, a corpus, or a judgement call. This one does not.
**A phone camera either resolves the code or it does not**, in under a second, with no
instrumentation, from hour one of the module existing.

It is also the module with the most craft per line. Reed–Solomon over GF(256) is real
coding theory, it is forty-year-old frozen mathematics, and it is the kind of thing a senior
reviewer opens the file to look at. In a hackathon whose heaviest non-functional criterion
is *"quality of stdlib substitutions"*, that matters.

**And yet it is first on the cut list.** See §9 — that tension is real and is recorded
rather than resolved by assertion.

---

## 2. Scope — deliberately narrow

| In | Out |
| --- | --- |
| **Byte mode only** | numeric, alphanumeric, kanji, ECI |
| Versions **3–4** (29×29, 33×33) | versions 1–2, 5–40 |
| ECC level **L** | M, Q, H |
| All 8 mask patterns with penalty scoring | — |
| Terminal render + `qr.png` | — |

It only ever encodes one thing: `http://192.168.0.105:8080`. That is 25 bytes. Version 3-L
holds 53 bytes in byte mode, version 4-L holds 78. Two versions cover every LAN URL
including a long hostname, and everything else in the standard is dead weight.

**Numeric and alphanumeric modes would compress a URL well** — alphanumeric covers uppercase
and digits at 5.5 bits per character. URLs contain lowercase. Not worth the segmentation
logic.

---

## 3. Pipeline

```
"http://192.168.0.105:8080"
 └─ mode + character count header ────────► bit stream
 └─ pad: terminator, byte-align, EC-EC-11 ► data codewords
 └─ Reed–Solomon over GF(256) ────────────► error-correction codewords
 └─ interleave data and EC blocks ────────► final codeword sequence
 └─ place in the matrix, boustrophedon ───► module grid
 └─ 8 masks, penalty score each ──────────► pick the lowest
 └─ format information (×2, with its own BCH) ──► matrix
 └─ render ──► half-block terminal glyphs  +  qr.png
```

---

## 4. GF(256)

The field is `GF(2^8)` with the QR primitive polynomial **`0x11D`**
(`x^8 + x^4 + x^3 + x^2 + 1`).

```rust
struct Gf {
    exp: [u8; 512],   // doubled, so multiplication needs no modulo on the index
    log: [u8; 256],
}
```

Build both tables once by repeatedly multiplying by the generator `α = 2`, reducing by
`0x11D` on overflow. Then:

- `mul(a, b) = exp[log[a] + log[b]]`, with `0` special-cased — `log[0]` is undefined and
  reading it is the classic first bug here.
- The **doubled `exp` table** is the trick that removes a `% 255` from the inner loop of
  every polynomial multiply. Sixteen extra bytes, meaningfully simpler code.

**`0x11D` is not the same primitive polynomial other Reed–Solomon applications use.**
CD-ROM and DVB use different ones. Taking a generic RS implementation's constant produces
codewords that are self-consistent and unreadable by every scanner on earth.

---

## 5. Reed–Solomon

The generator polynomial for `n` error-correction codewords is
`(x − α^0)(x − α^1)…(x − α^(n−1))`, expanded in GF(256). Build it iteratively; it is about
fifteen lines.

Encoding is **polynomial long division of the message by the generator; the remainder is
the EC codewords.** Systematic — the data appears unchanged and the check bytes follow.

**Only the encoder is needed.** No syndrome calculation, no Berlekamp–Massey, no Chien
search, no Forney — all of that is *decoding*, and darkroom never reads a QR code. That is
what makes this module 350 lines instead of 900, and it is worth saying in `STDLIB.md`:
the substitution replaced `qrcodegen` and the honest cost is that it encodes only.

### Block structure

| Version-ECC | Total codewords | Data | EC per block | Blocks |
| --- | --- | --- | --- | --- |
| 3-L | 70 | 55 | 15 | 1 |
| 4-L | 100 | 80 | 20 | 1 |

**Single-block for both target versions**, which removes interleaving entirely. Version 5-L
is the first with two blocks, and it is out of scope precisely because of that. If the URL
ever needs it, the interleave rule is: take codeword *i* from every block in turn, data
blocks first, then EC blocks.

---

## 6. Matrix construction

Fixed patterns first, and they are excluded from data placement:

- **Finder patterns** — 7×7, three corners, each with a 1-module separator.
- **Alignment pattern** — 5×5. Version 3 has one at (22,22); version 4 at (26,26).
- **Timing patterns** — alternating row 6 and column 6.
- **Dark module** — always set, at `(4×version + 9, 8)`. Easy to forget; the code then
  fails to scan with no other symptom.
- **Format information area** — reserved before placement, filled after masking.

**Data placement** is two-module-wide columns from the bottom-right, alternating upward and
downward, skipping column 6 (the vertical timing pattern) entirely. Within each column pair
the right module comes first. Every reserved module is skipped, not overwritten.

---

## 7. Masking — the part that is easy to skip and must not be

Eight patterns, each a formula over `(row, col)`. Apply to data modules only (never to
function patterns), score, keep the lowest.

The four penalty rules:

| # | Condition | Points |
| --- | --- | --- |
| 1 | Run of 5+ same-colour modules in a row/column | `3 + (len − 5)` |
| 2 | Each 2×2 block of one colour | 3 |
| 3 | The pattern `1011101` with 4 light modules on one side | 40 |
| 4 | Deviation of dark-module proportion from 50% | `10 × ⌊ | pct − 50 | / 5⌋` |

**Implementing masks but not the penalty scoring "because mask 0 works on my test string"
is the trap.** It does work — until the URL changes by one character and produces a large
solid region that a phone camera cannot lock onto. The scoring is ~60 lines and it is what
makes the module reliable rather than lucky.

**Format information** is 5 bits (ECC level + mask) with a 10-bit BCH(15,5) code, XORed
with `0x5412`, and written **twice** in different places. Both copies, both correct — a
scanner reads whichever it can and a wrong second copy fails intermittently, which is the
worst kind of demo bug.

---

## 8. Rendering

### Terminal

**Half-block glyphs (`▀`), two QR rows per text row.** Terminal cells are roughly 1:2, so
full-block rendering produces a code twice as tall as it is wide, which many scanners
reject. Half-blocks with foreground and background colours give square modules.

**A quiet zone of 4 modules on every side is mandatory**, and it is what an implementation
drops when the code doesn't fit the window. Without it, phones fail to detect the code at
all — and the failure looks like a bad encoder rather than bad framing.

**Contrast:** print dark modules as background-coloured and light as foreground, or the
inverse, depending on terminal theme. On a dark-themed terminal the naive rendering is
inverted and **most scanners refuse inverted codes**. A `--invert` flag, and the contrast
was already probed on this machine — see `qr-contrast-probe.py`.

### `qr.png`

Written to disk unconditionally as the fallback path. It costs nothing (the PNG encoder
exists), and it is the answer to a terminal that renders the glyphs badly during recording.
`DEMO-SCRIPT.md` lists it as a pre-flight item for exactly that reason.

---

## 9. ⚠️ The cut tension, stated honestly

`ARCHITECTURE.md` §10.2 names QR as cut candidate #1 — 350 lines, replaceable by printing
the URL as text.

**The argument for cutting it:** it is pure demo. The tool works identically with a printed
URL. 350 lines is 7% of the halved budget and it buys no functionality.

**The argument against:** it is the highest craft-per-line module in the project, on the
30% criterion, and the demo's opening beat depends on it. There is also a prior-art
consideration — `Traverse`, a Rust P2P file-transfer CLI, won *100 Lines*, so QR-to-phone
reads as a past winner's territory rather than as novelty.

**Where that landed:** the demo already moved its cold open **off** the QR and onto the
duplicate finder (`DEMO-SCRIPT.md`, and the reasoning in `../../planning/VERDICT.md`). That weakens the
demo argument for keeping it — but not the craft argument.

**Unresolved.** It is one of the open decisions in `README.md`, and it must be settled
before darkroom is revived, not at hour 40.
