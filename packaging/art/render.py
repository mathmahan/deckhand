#!/usr/bin/env python3
"""Draw Deckhand's art for Steam from the icon's pixels: the ship, the wordmark and the wake.

Writes an SVG per piece into packaging/art/steam/, and a PNG of each beside it when rsvg-convert is
on the PATH. The PNGs are committed, so a build needs neither Python nor rsvg; run this again after
changing the drawing.

    python3 packaging/art/render.py

Every piece is drawn on whole pixels of its own grid, with hard edges, so it stays as sharp as the
icon at any size Steam asks for:

- wordmark.svg                 the wordmark, over its wake
- logo.png                     the wordmark without the wake, transparent: Steam's logo over the hero
- capsule.png                  600x900, the portrait tile in the library
- wide.png                     920x430, the wide capsule
- hero.png                     1920x620, the backdrop across the top of the page, left clear for the logo
- icon.png                     256x256, the icon beside the name in lists
"""

import os
import random
import shutil
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "steam")

PINK, MAGENTA, VIOLET = "#FF4FD8", "#CF00FB", "#7B2FF7"
WAKE_NEAR, WAKE_FAR = "#3F1A7B", "#2A1350"
WINDOW = "#000000"
# The icon's background, lighter at the top left than at the bottom right.
BG_LIGHT, BG_DARK = "#180F22", "#0B0810"

# The ship as packaging/deckhand.png draws it, one character per pixel of its 19 px grid.
SHIP = [
    "..mm..................",
    "..pp..................",
    "..mm..................",
    ".mmmmm.ppp............",
    ".mkkkm.ppp............",
    ".mmmmm.mmm.ppp.mmm....",
    ".mmmmm.mmm.ppp.mmm....",
    ".mmmmm.vvv.mmm.vvv.pp.",
    ".mmmmm.vvv.mmm.vvv.pp.",
    "pppppppppppppppppppppp",
    "mmmmmmmmmmmmmmmmmmmmmm",
    ".mmmmmmmmmmmmmmmmmmmmm",
    "..vvvvvvvvvvvvvvvvvvv.",
    "....vvvvvvvvvvvvvvv...",
    ".www...wwww...wwww.www",
    "....www...wwww...www..",
]
SHIP_INK = {"p": PINK, "m": MAGENTA, "v": VIOLET, "k": WINDOW}
# The ship's own wake, its last two rows: near, then far.
SHIP_WAKE = [WAKE_NEAR, WAKE_FAR]

# The wordmark's letters, seven pixels tall with two-pixel stems, like the ship's blocks.
GLYPHS = {
    "D": ["#####.", "##..##", "##..##", "##..##", "##..##", "##..##", "#####."],
    "E": ["#####", "##...", "##...", "####.", "##...", "##...", "#####"],
    "C": [".#####", "##....", "##....", "##....", "##....", "##....", ".#####"],
    "K": ["##..##", "##.##.", "####..", "###...", "####..", "##.##.", "##..##"],
    "H": ["##..##", "##..##", "##..##", "######", "##..##", "##..##", "##..##"],
    "A": [".####.", "##..##", "##..##", "######", "##..##", "##..##", "##..##"],
    "N": ["##...##", "###..##", "####.##", "##.####", "##..###", "##...##", "##...##"],
}
# Banded top to bottom as the hull is.
LETTER_BANDS = [PINK, PINK, MAGENTA, MAGENTA, MAGENTA, VIOLET, VIOLET]
# The wake under the word: two rows of dashes, each its own uneven rhythm, as (start, length).
WORD_WAKE = [
    (WAKE_NEAR, [(1, 6), (10, 4), (17, 7), (28, 5), (36, 3), (43, 6), (52, 3)]),
    (WAKE_FAR, [(5, 4), (13, 6), (23, 3), (31, 5), (39, 4), (47, 6)]),
]


def ship_cells():
    cells = []
    for y, line in enumerate(SHIP):
        for x, ch in enumerate(line):
            if ch == "w":
                cells.append((x, y, SHIP_WAKE[y - (len(SHIP) - 2)]))
            elif ch in SHIP_INK:
                cells.append((x, y, SHIP_INK[ch]))
    return cells, len(SHIP[0]), len(SHIP)


def wordmark_cells(wake=True):
    cells, x = [], 0
    for ch in "DECKHAND":
        glyph = GLYPHS[ch]
        for y, line in enumerate(glyph):
            for dx, v in enumerate(line):
                if v == "#":
                    cells.append((x + dx, y, LETTER_BANDS[y]))
        x += len(glyph[0]) + 1
    width, height = x - 1, len(LETTER_BANDS)
    if wake:
        for i, (colour, dashes) in enumerate(WORD_WAKE):
            for start, length in dashes:
                for dx in range(start, min(start + length, width)):
                    cells.append((dx, height + 1 + i, colour))
        height += 1 + len(WORD_WAKE)
    return cells, width, height


def rects(cells, ox, oy, unit):
    """`cells` as SVG rects at `unit` px a pixel from (`ox`, `oy`), runs along a row merged."""
    out = []
    rows = {}
    for x, y, colour in cells:
        rows.setdefault((y, colour), []).append(x)
    for (y, colour), xs in sorted(rows.items()):
        xs.sort()
        start = prev = xs[0]
        for x in xs[1:] + [None]:
            if x is None or x != prev + 1:
                out.append(
                    f'<rect x="{ox + start * unit}" y="{oy + y * unit}" width="{(prev - start + 1) * unit}" '
                    f'height="{unit}" fill="{colour}"/>'
                )
                start = x
            if x is not None:
                prev = x
    return "\n".join(out)


def sea(width, height, top, unit, ox, seed):
    """Rows of wake dashes from `top` to the bottom edge, on the grid of `unit` px squares that
    starts at `ox`, so the ship's own wake runs into them. Sparse and dim at the horizon, closer and
    brighter toward the viewer."""
    rng = random.Random(seed)
    out = []
    rows = max(1, -(-(height - top) // unit))
    first = -(-ox // unit)
    columns = (width - ox) // unit + 1
    for r in range(rows):
        near = r / max(1, rows - 1)
        colour = WAKE_NEAR if r % 2 else WAKE_FAR
        x = -first + rng.randrange(0, 5)
        while x < columns:
            length = rng.randrange(2, 7)
            out.append(
                f'<rect x="{ox + x * unit}" y="{top + r * unit}" width="{length * unit}" height="{unit}" '
                f'fill="{colour}" opacity="{0.45 + 0.55 * near:.2f}"/>'
            )
            x += length + rng.randrange(2, 9 - round(4 * near))
    return "\n".join(out)


def stars(width, height, unit, count, seed, clear=()):
    """A few single pixels in the sky, faint, none inside the boxes in `clear` (x, y, w, h)."""
    rng = random.Random(seed)
    out = []
    margin = 2 * unit
    while len(out) < count:
        x = rng.randrange(0, width // unit) * unit
        y = rng.randrange(0, height // unit) * unit
        if any(bx - margin <= x < bx + bw + margin and by - margin <= y < by + bh + margin for bx, by, bw, bh in clear):
            continue
        colour = rng.choice([VIOLET, WAKE_NEAR, PINK])
        out.append(f'<rect x="{x}" y="{y}" width="{unit}" height="{unit}" fill="{colour}" opacity="{rng.uniform(0.25, 0.6):.2f}"/>')
    return "\n".join(out)


def svg(width, height, body, background=True):
    bg = ""
    if background:
        bg = (
            '<defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">'
            f'<stop offset="0" stop-color="{BG_LIGHT}"/><stop offset="1" stop-color="{BG_DARK}"/>'
            "</linearGradient></defs>\n"
            f'<rect width="{width}" height="{height}" fill="url(#bg)"/>\n'
        )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" shape-rendering="crispEdges">\n{bg}{body}\n</svg>\n'
    )


def centred(size, total):
    return (total - size) // 2


def wordmark(wake=True):
    cells, w, h = wordmark_cells(wake)
    unit = 20
    return svg(w * unit, h * unit, rects(cells, 0, 0, unit), background=False)


def logo():
    # Steam draws it over the hero's sea, which is wake enough.
    return wordmark(wake=False)


# The row the ship's wake starts on: the sea is drawn from there down.
WAKE_ROW = len(SHIP) - 2


def capsule():
    W, H = 600, 900
    ship, sw, sh = ship_cells()
    word, ww, wh = wordmark_cells(wake=False)
    su, wu = 22, 9
    ship_x = centred(sw * su, W)
    ship_y = H - (sh + 4) * su
    word_x, word_y = centred(ww * wu, W), 300
    body = "\n".join([
        stars(W, ship_y - 2 * wu, wu, 24, seed=11, clear=[(word_x, word_y, ww * wu, wh * wu)]),
        sea(W, H, ship_y + WAKE_ROW * su, su, ship_x, seed=1),
        rects(ship, ship_x, ship_y, su),
        rects(word, word_x, word_y, wu),
    ])
    return svg(W, H, body)


def wide():
    W, H = 920, 430
    ship, sw, sh = ship_cells()
    word, ww, wh = wordmark_cells(wake=False)
    su, wu = 12, 8
    gap = 56
    left = centred(sw * su + gap + ww * wu, W)
    ship_y = H - (sh + 5) * su
    # The word stands on the deck's line, beside the ship.
    word_y = ship_y + 9 * su - wh * wu
    word_x = left + sw * su + gap
    body = "\n".join([
        stars(W, ship_y + 6 * su, wu, 22, seed=12,
              clear=[(left, ship_y, sw * su, sh * su), (word_x, word_y, ww * wu, wh * wu)]),
        sea(W, H, ship_y + WAKE_ROW * su, su, left, seed=2),
        rects(ship, left, ship_y, su),
        rects(word, word_x, word_y, wu),
    ])
    return svg(W, H, body)


def hero():
    W, H = 1920, 620
    ship, sw, sh = ship_cells()
    su = 18
    ship_x = W - sw * su - 216
    ship_y = H - (sh + 4) * su
    body = "\n".join([
        stars(W, ship_y + 6 * su, su // 2, 40, seed=13, clear=[(ship_x, ship_y, sw * su, sh * su)]),
        sea(W, H, ship_y + WAKE_ROW * su, su, ship_x, seed=3),
        rects(ship, ship_x, ship_y, su),
    ])
    return svg(W, H, body)


def icon():
    W = 256
    ship, sw, sh = ship_cells()
    su = 10
    body = rects(ship, centred(sw * su, W), centred(sh * su, W) + 6, su)
    # Rounded as the icon is.
    return svg(W, W, body).replace(
        '<rect width="256" height="256" fill="url(#bg)"/>', '<rect width="256" height="256" rx="48" fill="url(#bg)"/>'
    )


PIECES = {"logo": logo, "capsule": capsule, "wide": wide, "hero": hero, "icon": icon}


def main():
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(HERE, "wordmark.svg"), "w") as f:
        f.write(wordmark())
    rsvg = shutil.which("rsvg-convert")
    for name, draw in PIECES.items():
        path = os.path.join(OUT, f"{name}.svg")
        with open(path, "w") as f:
            f.write(draw())
        if rsvg:
            subprocess.run([rsvg, path, "-o", os.path.join(OUT, f"{name}.png")], check=True)
    if not rsvg:
        print("rsvg-convert is not on the PATH: wrote the SVGs only")


if __name__ == "__main__":
    main()
