# darkroom — the idea, and where it stands

> The pitch, how it scores, why it lost to `zql`, and what would bring it back.
>
> **Status: runner-up.** Composite **4.505** against `zql`'s **4.655**. Kept alive and fully
> documented because the gap is 0.15 and the reasoning behind it is contestable in two
> specific places — §6.

---

## 1. The pitch

> **"A photo gallery you run on your own laptop. It finds your duplicate photos, and you
> browse them from your phone by scanning a QR code."**

| Test | Result |
|---|---|
| Repeatable after hearing it once | ✅ |
| Needs no definition of terms | ✅ |
| Wanted within five seconds | ✅ |
| The name self-describes | ⚠️ "darkroom" does not say what it does |
| **Pitch score** | **4.8** |

Slightly ahead of `zql` on pitch — universal rather than engineer-only. Everyone has a
photo folder; not everyone has a Postgres client.

---

## 2. What it is

Point it at a folder of photos. It decodes them itself, builds thumbnails itself, reads
EXIF itself, computes a perceptual hash to cluster near-duplicates, serves a web gallery
over the LAN, and prints a QR code so a phone can open it. Nothing is uploaded anywhere.

One binary, empty `[dependencies]`. The JPEG decoder, the PNG encoder, DEFLATE, CRC32, the
EXIF parser, the resampler, the perceptual hash, the HTTP server, the thread pool, the JSON
writer, the calendar arithmetic, and the Reed–Solomon QR encoder are all in the repository.

---

## 3. How it scores

| Criterion | Weight | Score | Note |
|---|---|---|---|
| Functionality & Usefulness | 35% | 4.3 | Universal appeal — but Google Photos is free and better at the core task |
| **Zero-Dependency Craft** | 30% | **4.8** | Huffman, the DCT, chroma subsampling, DEFLATE, PNG filtering, CRC32, EXIF/TIFF, perceptual hashing, Reed–Solomon over GF(256) |
| Code Quality & Idiom | 25% | **4.6** | Decomposes cleanly by format — sixteen modules, each with one specification and one oracle |
| Innovation | 10% | 4.1 | A photo gallery is not surprising. The lowest score of any finalist |
| **Composite** | | **4.505** | |
| Demo | — | 4.4 | Photos on a phone, a live counter, clusters collapsing. Legible to all 30+ judges |
| Ship probability | — | ~85% | Was ~35% before the spike |

**Calibration:** winners land 4.08–4.48; the highest published score anywhere is 4.88.
Margins are thin — 0.162 separated four places at Vanilla Web Warriors. A 0.15 gap is not
noise, but it is one revised assumption wide.

---

## 4. Why this shape

**The Craft criterion is 30% and darkroom is built to maximise exactly it.** Fourteen
non-trivial substitutions, each replacing a package a working engineer has actually
installed, each with a published specification and an external oracle. `STDLIB.md` has all
fourteen and none of them are padding.

**The demo has native motion.** The prior-art corpus is unambiguous that winners have
motion — and darkroom has three sources of it without inventing any: an indexing counter
climbing, clusters collapsing, and a phone lighting up. `zql` needed a whole dashboard
module to get what darkroom gets for free.

**Every rung is independently submittable.** Rung 1 — browse a real photo folder from your
phone — ships before a single line of codec exists, because the browser can decode JPEG
itself. Three hours in there is already an artifact that satisfies every deliverable.
Everything after is upside. That structure is the ship-probability argument.

---

## 5. Prior art, honestly

Google Photos is free, better at the core task, and already on everyone's phone. The
differentiator is **local-only, no upload** — which is a real answer for a real audience,
and it is a smaller one than "nothing like this exists".

Nothing about darkroom is technically novel. Every algorithm in it is decades old and every
one has a well-maintained crate. **The novelty is entirely in the constraint** — which is
what the event is about, and also why the Innovation score is 4.1.

---

## 6. Why it lost, and where that reasoning is contestable

Head to head:

| Axis | Weight | **zql** | **darkroom, halved** |
|---|---|---|---|
| Functionality & Usefulness | 35% | **4.6** | 4.3 |
| Zero-Dependency Craft | 30% | **4.8** | **4.8** |
| Code Quality & Idiom | 25% | 4.5 | **4.6** |
| Innovation | 10% | **4.8** | 4.1 |
| **Composite** | | **4.655** | **4.505** |
| Lines | — | 4,600 | 5,000 *(needs trimming to 4,000)* |
| Ship probability | — | ~85% | ~85% |
| Spikes passed | — | ✅ ×4 | ✅ ×1 |
| Test corpus staged | — | ❌ | ✅ **215 files** |
| Interop oracles | — | `psql`, node-postgres, Python `sqlite3` | `jpeg-js`, `gunzip`, PngSuite |

**Where `zql` genuinely wins:** Functionality by 0.3 on the heaviest criterion, Innovation
by 0.7, and it has fewer modules to get wrong.

**Where darkroom genuinely wins:** Code Quality by 0.1, a staged 215-file corpus, a complete
environment preflight, and universal rather than engineer-only appeal.

### The two places the reasoning is contestable

**1. The Craft tie is doing a lot of work.** Both are scored 4.8. But darkroom substitutes
more libraries with harder algorithms — a recursive-descent parser is more familiar
territory to a senior reviewer than an IDCT is. `../../planning/VERDICT.md` scored this **darkroom 4.8 /
zql 4.6** and the later head-to-head levelled it. If the earlier number is right, the gap
closes to 0.09.

**2. Craft is the theme of the event.** `zql` wins Innovation (10%) by 0.7; darkroom wins
Craft (30%) by 0.2 under the earlier scoring. Weighted, those nearly cancel — but when a
near-tie must be broken, there is an argument for breaking it toward the thing the
organisers built the hackathon to measure. That argument was made in `../../planning/VERDICT.md`, which
concluded **darkroom**, and then reversed once `sqlite()` replaced `git()` in `zql` and
lifted its Functionality score from 4.4 to 4.6.

**So the decision turns on one substitution in the other project.** That is worth knowing
and it is why this folder exists.

### The bias audit that produced `../../planning/VERDICT.md`

Worth preserving, because it is the most useful thing in the planning record. Between
recommending darkroom and recommending `zql`, three things changed: the user said *"push
harder"* (**contamination**), I found the interop-as-proof idea (legitimate), and I spiked
`zql` and it passed (legitimate).

The fourth thing went unnoticed and was the important one: **I spiked `zql` and never spiked
darkroom** — then justified the asymmetry with a claim (*"darkroom's core question cannot be
answered by a spike"*) that was simply false. Running the spike falsified my own central
argument. See `SPIKE-JPEG.md` §6.

---

## 7. What would bring darkroom back

In order of how much each would move the decision:

1. **The DEFLATE/PNG spike fails for `zql`'s SQLite reader — or succeeds cheaply for
   darkroom.** darkroom's remaining unknown is one cheap spike with an oracle already
   installed (`gunzip`).
2. **The 1,000-line gap gets closed on paper.** `ARCHITECTURE.md` §10.2 names four
   candidate cuts. Any three close it. Until then darkroom's real budget is 25% over.
3. **A judge-panel signal that skews away from database infrastructure.** `zql`'s
   Functionality edge is largely audience fit — Cisco, Reddit, DoorDash, Okta, Red Hat
   engineers would install it. A different panel changes that number, not darkroom's.
4. **`zql`'s wire protocol hits something the spikes did not cover.** The extended protocol
   is out of scope there, and a client that needs it is a visible limitation on camera.

**What would not bring it back:** more planning. darkroom has a schedule, a demo script, a
STDLIB draft, a README skeleton, a staged corpus, a verified reproducible build and now a
full low-level design. It is planned to the point of diminishing returns. The remaining
questions are a spike and a decision.

---

## 8. Honest limitations

The full list, README-ready, is `FEATURES.md` §10. The ones that affect the *decision* rather
than the product:

1. **Every judge is holding an iPhone, and iPhone photos are HEIC.** The single biggest
   product risk. Mitigated by disclosure and a JPEG-only demo folder, not solved.
2. **Progressive JPEG is untested and cut.** The spike explicitly rejects SOF2. Not
   load-bearing, but a real gap in the evidence.
3. **Google Photos is free and better** at the core task. The honest differentiator is
   local-only, no upload.
4. **More modules than `zql`** — sixteen against nine. More surface, more places to be wrong.
5. **The 1,000-line discrepancy is unresolved.** 4,000 was claimed; 5,000 is itemised.
6. **Innovation 4.1 is the lowest of any finalist.** On the lightest criterion, but real.
7. **The full-scope 7,750-line darkroom is off the table** — it scored *worse* than the
   halved version and was twice as likely to fail. Do not revive it under time pressure at
   hour 30, which is precisely when it will look tempting.
