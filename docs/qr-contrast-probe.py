"""QR contrast probe — preflight only, not darkroom source.

Renders the same payload four ways so you can photograph the screen and find out
which style a phone camera actually decodes. The demo's opening beat depends on
this working, and terminal QR is where it usually fails: too small, wrong aspect
ratio, or dark-on-light inverted.

Run it, then scan each block with your phone camera from normal sitting distance.
Record which ones decode in PREFLIGHT.md.
"""

import sys

import segno

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

URL = "http://192.168.0.105:8080"
QUIET = 4          # modules of quiet zone; the spec requires 4, and scanners enforce it

RESET = "\x1b[0m"
WHITE_BG, BLACK_BG = "\x1b[48;2;255;255;255m", "\x1b[48;2;0;0;0m"
WHITE_FG, BLACK_FG = "\x1b[38;2;255;255;255m", "\x1b[38;2;0;0;0m"


def matrix(url):
    """Module grid as list of rows of bool, dark=True, with the quiet zone added."""
    rows = [[bool(m) for m in row] for row in segno.make(url, error="m").matrix]
    w = len(rows[0]) + 2 * QUIET
    pad = [[False] * w for _ in range(QUIET)]
    return pad + [[False] * QUIET + r + [False] * QUIET for r in rows] + pad


def full_blocks(m, cell="  ", invert=False):
    """One module = `cell` wide, one terminal row tall. Widest and most reliable."""
    dark, light = (WHITE_BG, BLACK_BG) if invert else (BLACK_BG, WHITE_BG)
    return "\n".join("".join((dark if v else light) + cell for v in row) + RESET
                     for row in m)


def half_blocks(m, cell=2, invert=False):
    """Two module rows per terminal row via the upper-half block. Halves the height."""
    d_fg, l_fg = (WHITE_FG, BLACK_FG) if invert else (BLACK_FG, WHITE_FG)
    d_bg, l_bg = (WHITE_BG, BLACK_BG) if invert else (BLACK_BG, WHITE_BG)
    out = []
    for y in range(0, len(m), 2):
        top = m[y]
        bot = m[y + 1] if y + 1 < len(m) else [False] * len(top)
        line = "".join((d_fg if t else l_fg) + (d_bg if b else l_bg) + "▀" * cell
                       for t, b in zip(top, bot))
        out.append(line + RESET)
    return "\n".join(out)


m = matrix(URL)
side = len(m)

print(f"\npayload : {URL}")
print(f"grid    : {side}x{side} modules including a {QUIET}-module quiet zone")
print("Photograph each block below with your phone camera, from normal sitting distance.\n")

for title, art, note in [
    ("A. FULL BLOCKS, dark-on-light",
     full_blocks(m),
     f"{side * 2} cols x {side} rows. Widest, squarest, most likely to scan. The safe default."),
    ("B. HALF BLOCKS, 2 cells wide, dark-on-light",
     half_blocks(m, 2),
     f"{side * 2} cols x {side // 2 + side % 2} rows. Same width, half the height - "
     "fits an 80x24 terminal. This is the one worth having work."),
    ("C. HALF BLOCKS, 1 cell wide, dark-on-light",
     half_blocks(m, 1),
     f"{side} cols x {side // 2 + side % 2} rows. Smallest, but modules are 1:2 - "
     "expect this to fail. Included to prove the aspect ratio matters."),
    ("D. FULL BLOCKS, INVERTED (light-on-dark)",
     full_blocks(m, invert=True),
     "Most scanners handle inversion, some don't. If A scans and D doesn't, "
     "never auto-match the terminal theme - always render dark-on-light."),
]:
    print(f"\n\x1b[1m{title}\x1b[0m")
    print(f"  {note}\n")
    print(art)

print(f"\n{RESET}Record in PREFLIGHT.md which of A-D decoded, and from what distance.")
print("If B works, use B. If only A works, budget the extra terminal height in the demo.\n")
