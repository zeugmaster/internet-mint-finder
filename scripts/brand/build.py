"""Build the Internet Mint Finder tag as static SVG files.

The live sticker in the site header is CSS (style/main.css, .brand-tag).
These files are the same sticker held at one light angle, with every letter
outlined so no font is needed to open them: a master with the name on the
arc, the mark without it, a solid-lettered mark for 48px and under, and a
favicon drawn on the 16px pixel grid. render.sh turns them into PNGs.

Needs fonttools and brotli, and the TX-02 files in public/fonts (they are
kept out of git):

    python3 -m venv /tmp/imf-brand
    /tmp/imf-brand/bin/pip install fonttools brotli
    /tmp/imf-brand/bin/python scripts/brand/build.py
    scripts/brand/render.sh
"""

import math
from pathlib import Path

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = Path(__file__).resolve().parents[2]
FONTS = ROOT / "public" / "fonts"
OUT = ROOT / "public" / "brand"

# The sticker, in units of its width: 1000 wide, 720 tall (the CSS 0.72).
W, H = 1000, 720
CX, CY, RX, RY = W / 2, H / 2, W / 2, H / 2

# The locked look: how much colour rides on the metal, and the hero light,
# up and to the left, as in the reference.
COLOUR = 0.38
LIGHT = {"px": 0.28, "py": 0.18, "bx": 0.22, "by": 0.30}

CHROME = [
    ("#fbfcfd", 0), ("#d3d8df", 8), ("#98a1ad", 16), ("#e6e9ed", 24), ("#ffffff", 31),
    ("#bcc3cc", 40), ("#7f8896", 49), ("#cfd4db", 57), ("#f7f8fa", 65), ("#a6aeb9", 74),
    ("#eef0f3", 82), ("#8d96a3", 91), ("#e4e7eb", 100),
]
# Cool-led, with one soft peach: warm hues turn olive on the dark bands.
RAINBOW = [
    ("#7a95ff", 0), ("#b48cff", 12), ("#f08ad8", 23), ("#ffc4a8", 31), ("#7fd8ff", 42),
    ("#6fe0c0", 52), ("#8aa6ff", 63), ("#c79bff", 74), ("#ff9fcf", 84), ("#8fe6d4", 93),
    ("#7a95ff", 100),
]
RAINBOW_COOL = [
    ("#6f8fff", 0), ("#a685ff", 18), ("#52b0ff", 36), ("#5fdca6", 52), ("#ef6fd0", 70),
    ("#6f8fff", 86), ("#52b0ff", 100),
]
STEEL = [("#a7afbb", 0), ("#6b7483", 28), ("#b9c0ca", 50), ("#545c6b", 72), ("#8f98a6", 100)]


def light_dir(px, py):
    """Unit vector from the centre toward the light; overhead reads as from above."""
    dx, dy = px - 0.5, py - 0.5
    n = math.hypot(dx, dy)
    return (0.0, -1.0) if n < 0.08 else (dx / n, dy / n)


def css_linear(angle, box_w, box_h, size, pos, origin=(0, 0)):
    """End points of a CSS linear-gradient(angle) drawn in a background tile of
    `size` (fractions of the box) at `pos` (background-position fractions)."""
    tw, th = box_w * size[0], box_h * size[1]
    ox = origin[0] + (box_w - tw) * pos[0]
    oy = origin[1] + (box_h - th) * pos[1]
    a = math.radians(angle)
    dx, dy = math.sin(a), -math.cos(a)
    length = abs(tw * dx) + abs(th * dy)
    cx, cy = ox + tw / 2, oy + th / 2
    return (cx - dx * length / 2, cy - dy * length / 2, cx + dx * length / 2, cy + dy * length / 2)


def gradient(gid, ends, stops):
    """Stops are (colour, offset %) or (colour, offset %, opacity)."""
    x1, y1, x2, y2 = ends
    body = "".join(
        f'<stop offset="{stop[1]}%" stop-color="{stop[0]}"'
        + (f' stop-opacity="{stop[2]}"' if len(stop) > 2 else "")
        + "/>"
        for stop in stops
    )
    return (f'<linearGradient id="{gid}" gradientUnits="userSpaceOnUse" '
            f'x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}">{body}</linearGradient>')


class Face:
    """One font, ready to turn strings into outlined path data."""

    def __init__(self, font):
        self.font = font
        self.glyphs = font.getGlyphSet()
        self.cmap = font.getBestCmap()
        self.upm = font["head"].unitsPerEm

    def glyph(self, ch):
        return self.cmap[ord(ch)]

    def advance(self, ch):
        return self.font["hmtx"][self.glyph(ch)][0]

    def path(self, ch, transform):
        pen = SVGPathPen(self.glyphs)
        self.glyphs[self.glyph(ch)].draw(TransformPen(pen, transform))
        return pen.getCommands()

    def bounds(self, ch):
        pen = BoundsPen(self.glyphs)
        self.glyphs[self.glyph(ch)].draw(pen)
        return pen.bounds


def load_faces():
    inter = TTFont(FONTS / "InterVariable.woff2")
    # The browser sets opsz from the size: 32 at every size the mark shows large.
    inter = instantiateVariableFont(inter, {"wght": 820, "opsz": 32})
    tx = TTFont(FONTS / "tx-02-regular.otf")
    return Face(inter), Face(tx)


def imf_path(face, size, cap_center_y, nudge=0.0):
    """IMF set like the CSS: tracking -0.035em, centred on its advance with
    the trailing tracking removed, caps centred on `cap_center_y`."""
    s = size / face.upm
    track = -0.035 * size
    advances = [face.advance(ch) * s for ch in "IMF"]
    width = sum(advances) + track * 2
    x = CX - width / 2 + nudge
    cap = face.font["OS/2"].sCapHeight * s
    baseline = cap_center_y + cap / 2
    parts, left = [], x
    for ch, adv in zip("IMF", advances):
        parts.append(face.path(ch, (s, 0, 0, -s, x, baseline)))
        x += adv + track
    return " ".join(parts), (left, cap_center_y - size / 2, width, size)


def arc_path(face, size, tracking, rx, ry, cy, text="INTERNET MINT FINDER"):
    """The name pressed along the bottom of an ellipse, read left to right,
    each glyph upright on the curve and centred on its share of the arc."""
    samples = 2000
    pts = [(CX - rx * math.cos(t), cy + ry * math.sin(t))
           for t in (math.pi * i / samples for i in range(samples + 1))]
    cum = [0.0]
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        cum.append(cum[-1] + math.hypot(x1 - x0, y1 - y0))
    s = size / face.upm
    advs = [face.advance(ch) * s + tracking for ch in text]
    total = sum(advs) - tracking
    pos = cum[-1] / 2 - total / 2

    def at(d):
        i = min(range(len(cum)), key=lambda k: abs(cum[k] - d))
        j = min(i + 1, len(pts) - 1)
        i = j - 1
        (x0, y0), (x1, y1) = pts[i], pts[j]
        return x0, y0, math.atan2(y1 - y0, x1 - x0)

    parts = []
    for ch, adv in zip(text, advs):
        glyph_w = adv - tracking
        if ch != " ":
            x, y, ang = at(pos + glyph_w / 2)
            c, n = math.cos(ang), math.sin(ang)
            # Rotate about the glyph's centre on the baseline; flip font y-up.
            a, b = c * s, n * s
            cc, d = n * s, -c * s
            e = x - c * glyph_w / 2
            f = y - n * glyph_w / 2
            parts.append(face.path(ch, (a, b, cc, d, e, f)))
        pos += adv
    return " ".join(parts)


def sticker_svg(inter, tx, *, with_arc=True, size=None, grain=True, small=False):
    """The tag at the hero light. `with_arc` False gives the mark: no name,
    IMF centred and a touch larger. `small` is for 48px and under, as the
    live sticker does it: solid engraving instead of shaded steel, which
    melts into the silver at that size."""
    px, py, bx, by = LIGHT["px"], LIGHT["py"], LIGHT["bx"], LIGHT["by"]
    lx, ly = light_dir(px, py)
    e = 0.006 * W

    defs = [
        f'<clipPath id="oval"><ellipse cx="{CX}" cy="{CY}" rx="{RX}" ry="{RY}"/></clipPath>',
        gradient("metal", css_linear(158, W, H, (2, 2), (bx, by)), CHROME),
        gradient("holo", css_linear(118, W, H, (3, 3), (1 - bx, by)), RAINBOW),
        gradient("streak", css_linear(112, W, H, (2.6, 1), (1 - px, 0.5)),
                 [("#ffffff", 38, 0), ("#ffffff", 47, 0.7), ("#ffffff", 56, 0)]),
    ]
    far = max(math.hypot(px * W - x, py * H - y) for x in (0, W) for y in (0, H))
    defs.append(
        f'<radialGradient id="glint" gradientUnits="userSpaceOnUse" cx="{px * W:.1f}" cy="{py * H:.1f}" r="{far:.1f}">'
        '<stop offset="0" stop-color="#fff" stop-opacity=".9"/>'
        '<stop offset=".14" stop-color="#fff" stop-opacity=".35"/>'
        '<stop offset=".38" stop-color="#fff" stop-opacity="0"/></radialGradient>'
    )
    defs.append(
        '<filter id="grain" x="0" y="0" width="100%" height="100%">'
        '<feTurbulence type="fractalNoise" baseFrequency=".36" numOctaves="3" stitchTiles="stitch"/>'
        '<feColorMatrix type="saturate" values="0"/></filter>'
    )
    lip = max(2.0, 0.008 * W)
    for mid, dy in (("lipTop", lip), ("lipBottom", -lip), ("soft", -0.04 * W)):
        defs.append(
            f'<mask id="{mid}"><ellipse cx="{CX}" cy="{CY}" rx="{RX}" ry="{RY}" fill="#fff"/>'
            f'<ellipse cx="{CX}" cy="{CY + dy:.1f}" rx="{RX}" ry="{RY}" fill="#000"/></mask>'
        )
    defs.append('<filter id="softBlur"><feGaussianBlur stdDeviation="40"/></filter>')
    defs.append(f'<filter id="loBlur" x="-10%" y="-10%" width="120%" height="120%"><feGaussianBlur stdDeviation="{0.8 * e:.1f}"/></filter>')

    # IMF: raised steel, lit from the hero light.
    imf_size = 0.25 * W if with_arc else 0.27 * W
    cap_y = CY - 0.045 * W if with_arc else CY
    letters, (bx0, by0, bw, bh) = imf_path(inter, imf_size, cap_y, nudge=0.004 * W)
    defs.append(gradient("steel", css_linear(172, bw, bh, (1, 2.2), (0, by), (bx0, by0)), STEEL))
    # The CSS tint tile is 2.6 sticker-widths wide and the letters' height tall.
    defs.append(gradient("tint", css_linear(118, bw, bh, (2.6 * W / bw, 1), (bx, 0), (bx0, by0)), RAINBOW_COOL))

    body = [
        f'<g clip-path="url(#oval)" style="isolation:isolate">',
        f'<rect width="{W}" height="{H}" fill="url(#metal)"/>',
        f'<rect width="{W}" height="{H}" fill="url(#holo)" style="mix-blend-mode:color" opacity="{COLOUR}"/>',
        f'<rect width="{W}" height="{H}" fill="url(#glint)" style="mix-blend-mode:screen"/>',
        f'<rect width="{W}" height="{H}" fill="url(#streak)" style="mix-blend-mode:screen"/>',
    ]
    if grain:
        body.append(f'<rect width="{W}" height="{H}" filter="url(#grain)" style="mix-blend-mode:overlay" opacity=".38"/>')
    body += [
        f'<rect width="{W}" height="{H}" fill="#ffffff" fill-opacity=".9" mask="url(#lipTop)"/>',
        f'<rect width="{W}" height="{H}" fill="#3a4252" fill-opacity=".25" mask="url(#lipBottom)"/>',
        f'<g filter="url(#softBlur)"><rect width="{W}" height="{H}" fill="#2a3243" fill-opacity=".08" mask="url(#soft)"/></g>',
        f'<ellipse cx="{CX}" cy="{CY}" rx="{RX}" ry="{RY}" fill="none" stroke="#ffffff" stroke-opacity=".45" stroke-width="{2 * e:.1f}"/>',
    ]
    if with_arc:
        arc = arc_path(tx, 50, 6, 400, 280, 370)
        dxa, dya = -lx * 3.5, -ly * 3.5
        body += [
            f'<path d="{arc}" fill="#ffffff" fill-opacity=".95" transform="translate({dxa:.2f} {dya:.2f})"/>',
            f'<path d="{arc}" fill="#1c222f" fill-opacity=".72"/>',
        ]
    if small:
        body.append(f'<path d="{letters}" fill="#3a4352" fill-opacity=".92"/>')
    else:
        body += [
            f'<path d="{letters}" fill="#1c222f" fill-opacity=".55" filter="url(#loBlur)" transform="translate({-lx * 1.4 * e:.2f} {-ly * 1.4 * e:.2f})"/>',
            f'<path d="{letters}" fill="#ffffff" fill-opacity=".94" transform="translate({lx * 1.1 * e:.2f} {ly * 1.1 * e:.2f})"/>',
            f'<path d="{letters}" fill="url(#steel)"/>',
            f'<path d="{letters}" fill="url(#tint)" style="mix-blend-mode:color" opacity="{COLOUR * 0.75:.3f}"/>',
        ]
    body += [
        '</g>',
        # The die-cut edge, outside the clip: a hairline that holds on light grounds.
        f'<ellipse cx="{CX}" cy="{CY}" rx="{RX - 1.5}" ry="{RY - 1.5}" fill="none" stroke="#3a4252" stroke-opacity=".32" stroke-width="3"/>',
    ]
    dims = f' width="{size}" height="{round(size * H / W)}"' if size else ""
    label = "Internet Mint Finder" if with_arc else "IMF"
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}"{dims} role="img" aria-label="{label}">'
            f'<title>{label}</title><defs>{"".join(defs)}</defs>{"".join(body)}</svg>')


# Pixel letters for 16px: one-pixel strokes on a 16 grid, so every edge
# lands on a whole pixel. Rows top to bottom.
PIXELS = {
    "I": ["#"] * 7,
    "M": ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"],
    "F": ["####", "#...", "#...", "###.", "#...", "#...", "#..."],
}


def pixel_letters(x0, y0):
    rects, x = [], x0
    for ch in "IMF":
        rows = PIXELS[ch]
        for r, row in enumerate(rows):
            for c, cell in enumerate(row):
                if cell == "#":
                    rects.append(f'<rect x="{x + c}" y="{y0 + r}" width="1" height="1"/>')
        x += len(rows[0]) + 1
    return "".join(rects)


def favicon16_svg():
    """16px favicon: the tag's metal and colour, IMF in whole pixels."""
    outline = '<ellipse cx="8" cy="8.5" rx="7.6" ry="6.1"/>'
    metal = gradient("m", css_linear(158, 16, 16, (2, 2), (0.22, 0.30)), CHROME)
    holo = gradient("h", css_linear(118, 16, 16, (3, 3), (0.78, 0.30)), RAINBOW)
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">'
        f'<defs>{metal}{holo}<clipPath id="c">{outline}</clipPath></defs>'
        '<g clip-path="url(#c)" style="isolation:isolate">'
        '<rect width="16" height="16" fill="url(#m)"/>'
        f'<rect width="16" height="16" fill="url(#h)" style="mix-blend-mode:color" opacity="{COLOUR + 0.1}"/>'
        '</g>'
        f'<g fill="none" stroke="#3a4252" stroke-opacity=".5" stroke-width=".6">{outline}</g>'
        f'<g fill="#252c3a" shape-rendering="crispEdges">{pixel_letters(2, 5)}</g>'
        '</svg>'
    )


def main():
    inter, tx = load_faces()
    OUT.mkdir(exist_ok=True)
    files = {
        OUT / "imf-tag.svg": sticker_svg(inter, tx, with_arc=True),
        OUT / "imf-mark.svg": sticker_svg(inter, tx, with_arc=False),
        OUT / "imf-mark-small.svg": sticker_svg(inter, tx, with_arc=False, grain=False, small=True),
        ROOT / "public" / "favicon.svg": favicon16_svg(),
    }
    for path, svg in files.items():
        path.write_text(svg + "\n")
        print(f"{path.relative_to(ROOT)}: {len(svg) / 1024:.1f} KB")


if __name__ == "__main__":
    main()
