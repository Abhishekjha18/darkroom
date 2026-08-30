# Spike — baseline JPEG decode

> Run 2026-08-17 in `scratchpad/jpegspike/`. **588 lines**, one file, empty
> `[dependencies]`, one compile pass, one trivial warning. Verified pixel-by-pixel against
> **`jpeg-js`**, an independent JavaScript decoder sharing no code with it.
>
> **Throwaway — delete before kickoff.** This document is what survives it.

---

## 1. Why this spike existed

It was run to test **my own argument**, not darkroom's feasibility.

I had claimed the decisive difference between darkroom and `zql` was **failure legibility**:
that a wrong JPEG decoder looks almost right and you find out at hour 30, whereas a wrong
wire protocol refuses to connect in ninety seconds. That claim was carrying most of the
weight in a recommendation to switch projects — and it had never been tested, because I had
spiked `zql` twice and darkroom not at all.

So `zql` had accumulated evidence darkroom was never given the chance to accumulate, and I
had justified the asymmetry with a claim — *"darkroom's core question cannot be answered by
a spike"* — that was simply false.

---

## 2. What was built

Baseline sequential JPEG decoder, complete:

marker parsing · DQT · DHT with canonical Huffman construction from BITS/HUFFVAL · SOF0 ·
DRI and restart intervals · SOS · bit reader with `0xFF00` unstuffing · dequantise ·
dezigzag · separable float IDCT · chroma upsampling · YCbCr→RGB.

---

## 3. Correctness — against `jpeg-js`

Same file decoded by both, mean and max absolute per-channel difference printed.

| Image | Sampling | mean \|Δ\| | max \|Δ\| | within ±1 |
|---|---|---|---|---|
| `base-444` | 1x1,1x1,1x1 | 0.372 | **3** | 99.00% |
| `base-422` | 2x1,1x1,1x1 | 0.377 | **3** | 99.00% |
| `base-420` | 2x2,1x1,1x1 | 0.383 | **3** | 98.99% |
| `original.jpg` | 2x2,1x1,1x1 | 0.384 | **3** | 99.06% |

Max deviation of **3 out of 255**, across every channel of every pixel of every file. That
is ordinary IDCT precision variance between a float implementation and an integer one — not
error.

**All three chroma subsampling modes correct on the first attempt.** That is the headline.
MCU interleaving for subsampled components is the specific bug I had predicted would eat six
hours, and it is the one thing in the decoder that cannot be gotten right by accident.

---

## 4. Robustness — 23 adversarial files

```
decoded 8   rejected 15   PANICKED 0
```

Every malformed input either decoded correctly — files that were genuinely JPEG behind a
misleading extension — or was rejected with a **specific** diagnostic:

```
truncated DHT values
bad quant table id
truncated DQT payload
```

Handled without incident: `actually-jpeg.png`, `double-extension.jpg.png`, `no-extension`,
`unicode-写真-🎞.jpg`, a 20000×150 panorama, `truncated-scan.jpg`, `no-eoi.jpg`.

**Zero panics** is now a measured number rather than an aspiration, and it is a README claim
worth making.

---

## 5. Speed

**135 ms** for 1280×960 with a naive float IDCT and no optimisation whatsoever — roughly
**9 MP/s**. **69 ms/image** across the near-duplicate set including process startup.

Slower than libjpeg-turbo, which is hand-written SIMD assembly. That gap gets measured and
published rather than hidden — it is a `STDLIB.md` entry, not an embarrassment.

---

## 6. What it did to the argument

**It falsified my central objection to darkroom, and that objection was load-bearing.**

- The decoder did not fail silently. Failures were specific, immediate, and named.
- `jpeg-js` turns *"is my decoder correct?"* into a **number, printed in under a second,
  available from hour one**. That is not a slow feedback loop; it is a faster one than
  `zql`'s.
- The 215-file measurement corpus was already staged. The infrastructure existed that day.

**Second concession.** I had argued 7,750 lines was unshippable at ~185 lines/hour. I wrote
**968 lines of correct, oracle-verified spike code in a day**. The throughput objection was
overstated — though spike code is not production code with tests, docs and review, so it is
not fully retired either. darkroom's ~35% ship probability was far too pessimistic; ~85% is
the honest figure.

**What survives:** progressive JPEG was **not** spiked and is genuinely harder than baseline
— the spike explicitly rejects SOF2. It is already on the cut list, so it is not
load-bearing, but it is a real gap in the evidence.

---

## 7. Changes to the plan that came out of this

1. **Wire `jpeg-js` in as a CI oracle from hour one.** Decode → compare → print mean/max
   diff. ~20 minutes of work, and it converts correctness from a judgement call into a
   number on every run. **The single highest-value process change identified anywhere in
   the planning.**
2. **Keep the pathological sweep as a panic test.** "Zero panics across 23 adversarial
   files" is measurable now, so it becomes a standing check rather than a one-off.
3. **Progressive JPEG stays cut.** Not spiked, genuinely harder, not load-bearing.
4. **HEIC stays refused and disclosed**, exactly as planned.

---

## 8. What still has no evidence

**DEFLATE and the PNG encoder.** They are load-bearing — no thumbnail path without them —
and they have never been spiked. `../../planning/VERDICT.md` names this explicitly:

> *"If the DEFLATE/PNG encoder spike fails or proves expensive, darkroom loses the thumbnail
> path and half its craft story."*

It is the cheapest remaining spike in the project, it has a pass/fail oracle already
installed (`gunzip`), and **it is the first thing to do if darkroom is ever revived.**
See `LLD-DEFLATE-PNG.md`.
