# LLD — EXIF / TIFF IFD parsing

> CIPA DC-008 (Exif 2.32), TIFF 6.0 · **400 lines** · risk **low** ·
> oracle **your operating system's photo info panel**
>
> Pre-kickoff planning. **Not code.**

---

## 1. What this buys

The timeline is sorted by **the date the photo was taken**, not the date the file was
written. That is the difference between a photo gallery and a directory listing, and it is
one sentence in the pitch that costs 400 lines.

Filesystem mtime is wrong in the two cases that matter most: photos copied off a camera or
phone (mtime = copy time, all identical) and photos restored from a backup (mtime = restore
time). Both are exactly the folder someone points darkroom at.

---

## 2. Where the bytes live

| Container | Location |
|---|---|
| JPEG | `APP1` segment whose payload begins `Exif\0\0`, then a complete TIFF file |
| PNG | `eXIf` chunk (rare), or `tEXt`/`iTXt` — **out of scope**, PNGs get mtime |
| HEIC | ISOBMFF `meta` box → `iinf`/`iloc` → the Exif item. **This is why HEIC still gets real dates** |
| GIF | none |

The `Exif\0\0` prefix is six bytes and then the TIFF header begins. **All offsets inside
are relative to the start of the TIFF header**, not to the segment, not to the file. Getting
that origin wrong is the single commonest EXIF bug and it produces plausible-looking
garbage rather than an error.

---

## 3. Structures

```rust
struct Tiff<'a> {
    buf:    &'a [u8],    // starts at the TIFF header — the offset origin
    little: bool,
}

struct IfdEntry {
    tag:    u16,
    typ:    u16,       // 1..=12
    count:  u32,
    /// The raw 4 bytes. Whether this is the value or an offset depends on
    /// count * sizeof(typ) — see §5.
    raw:    [u8; 4],
}
```

Deliberately not modelled: a generic `Value` enum covering all twelve types. Only a dozen
tags are ever read, each with a known type, so each accessor asserts the type it expects and
returns `None` otherwise. That is fewer lines and it fails loudly on vendor weirdness
instead of silently coercing.

---

## 4. Header and traversal

```
byte order   "II" (little) or "MM" (big)     2 bytes
magic        42                              2 bytes, in the declared order
ifd0_offset  u32                             4 bytes, from the start of this header
```

**Both byte orders are real and both are common.** Canon writes `II`, many Nikons write
`MM`. Every multi-byte read in this module goes through the endian-aware accessor; there is
no `u32::from_le_bytes` anywhere outside it.

Traversal:

```
IFD0  ──► the image itself: Make, Model, Orientation, and two pointers
      ├─ tag 0x8769  ExifIFD  ──► DateTimeOriginal, ISO, ExposureTime, FNumber, LensModel
      └─ tag 0x8825  GpsIFD   ──► the GPS block
IFD1  ──► the embedded thumbnail. Present, and deliberately ignored — see §8
```

Each IFD is `u16 count`, then `count × 12-byte` entries, then a `u32` offset to the next
IFD (0 = end).

**Two guards that are not optional:**

- **Cap the IFD chain.** A malformed file can point IFD1 back at IFD0. Track visited
  offsets, or simply cap at 8 IFDs and 512 entries.
- **Bounds-check every offset against the buffer** before reading. Offsets are
  file-supplied and this is the module most likely to be handed a hostile one.

---

## 5. The value-vs-offset rule

Each entry's last 4 bytes are **the value itself** when `count × sizeof(type) <= 4`, and an
**offset to the value** otherwise.

| Type | Name | Bytes |
|---|---|---|
| 1 | BYTE | 1 |
| 2 | ASCII | 1, NUL-terminated |
| 3 | SHORT | 2 |
| 4 | LONG | 4 |
| 5 | RATIONAL | 8 (two LONGs) |
| 7 | UNDEFINED | 1 |
| 9 | SLONG | 4 |
| 10 | SRATIONAL | 8 |

So a `SHORT` sits inline; a `RATIONAL` never does. **And when a value sits inline in a
big-endian file, it is left-justified in those 4 bytes** — a single `SHORT` is in bytes
0–1, not 2–3. This is the second commonest EXIF bug and it produces values that are wrong
by a factor of 65536.

**ASCII strings are NUL-terminated and often NUL-padded, and vendors pad with spaces too.**
Trim both. `"Canon\0\0\0"` and `"Canon      "` are the same camera and must group as one in
the UI.

---

## 6. The tags that matter

Twelve. Not the standard.

| Tag | IFD | Type | Use |
|---|---|---|---|
| `0x0110` Make | 0 | ASCII | camera |
| `0x0110` Model | 0 | ASCII | camera |
| `0x0112` Orientation | 0 | SHORT | **applied before thumbnail and hash** |
| `0x9003` DateTimeOriginal | Exif | ASCII | **the timeline** |
| `0x9004` DateTimeDigitized | Exif | ASCII | fallback |
| `0x0132` DateTime | 0 | ASCII | second fallback |
| `0x829A` ExposureTime | Exif | RATIONAL | kept as a rational — "1/250" |
| `0x829D` FNumber | Exif | RATIONAL | |
| `0x8827` ISO | Exif | SHORT | |
| `0xA434` LensModel | Exif | ASCII | |
| `0xA002/3` PixelXDimension/Y | Exif | LONG/SHORT | cheap dimensions without decoding |
| `0x0001`–`0x0004` GPS | GPS | ASCII + RATIONAL×3 | see §7 |

**Date format is `"YYYY:MM:DD HH:MM:SS"` — colons in the date, not dashes.** Twenty bytes
including the NUL. It is **local wall-clock time with no zone**, which is why the calendar
layer (`ARCHITECTURE.md` §8) needs no time zone database at all.

**Fallback chain:** `DateTimeOriginal` → `DateTimeDigitized` → `DateTime` → file mtime.
The UI marks entries that fell through to mtime, because "this date is a guess" is
information the user wants.

### Orientation

Values 1–8, encoding the eight combinations of rotation and mirroring. Only 1, 3, 6, 8 are
common (0°, 180°, 90° CW, 90° CCW) but all eight are ten lines and the mirrored ones do
occur on front cameras.

**Applied in the indexer, before thumbnailing and before hashing.** See `ARCHITECTURE.md`
§6.1 — skipping it produces sideways thumbnails *and* broken duplicate clustering, which
present as two unrelated bugs.

---

## 7. GPS

Latitude and longitude arrive as **three RATIONALs — degrees, minutes, seconds — plus a
separate ASCII reference tag** (`"N"`/`"S"`, `"E"`/`"W"`).

`deg + min/60 + sec/3600`, negated for `S` and `W`.

**Denominators vary wildly by vendor.** Some write seconds as `4230/100`; some write
degrees as `37/1` and push all precision into minutes as `2537/100` with seconds `0/1`.
Any implementation that assumes `/1` denominators produces coordinates in the wrong ocean.
And a `0` denominator is a real thing in the wild — guard the division.

---

## 8. Deliberately not done

- **MakerNotes.** Every manufacturer invented their own undocumented format inside tag
  `0x927C`. There is no honest weekend in it. Stated in `STDLIB.md`.
- **The embedded thumbnail in IFD1.** It is *right there*, 160×120, and using it would skip
  the whole decode path. That is exactly why it is refused: darkroom's claim is that it
  decodes the image itself. Using the camera's thumbnail would make the headline claim
  quietly false, and it would be lower quality besides.
- **Writing EXIF.** darkroom never modifies a file. Read-only is a product boundary.

---

## 9. Oracle

**The operating system's photo info panel**, compared field by field. It is on every
machine, it needs no setup, and it disagrees loudly when the offset origin is wrong.

Corpus requirement: **multi-vendor samples** — Canon, Nikon, Sony, iPhone, Android — because
byte order and layout vary more between vendors than the specification suggests. This is a
known gap in the staged corpus; see `CORPUS.md` §4.
