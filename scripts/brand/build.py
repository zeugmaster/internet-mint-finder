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


# The seal, after the IMF's, on a 240 box: a legend round a ring, a star at
# each side, and a shield with IMF and a sprig of mint (where the IMF has
# olive) cut through. The files are cropped to the ink, INK from the centre.
S, SC = 240, 120
INK = 102
# The legend's band: INTERNET stands out from radius 84, MINT FINDER stands
# in from 101, so both fill 84 to 101.5. Spacing is measured along the middle
# of the band, so the two arcs are set alike.
TOP_R, BOTTOM_R, MID = 84, 101, 92.75
LEGEND = 24
TRACK, WORD = 10.5, 23
SHIELD = ("M71 70 H169 Q173 70 173 74 V126 C173 158 152 177 120 190 "
          "C88 177 67 158 67 126 V74 Q67 70 71 70 Z")


def ring_text(face, text, size, radius, top):
    """Text round the seal, centred on the top or the bottom, with TRACK
    between letters and WORD between words, both measured along the middle
    of the band: over the top reading clockwise with the letters standing
    out from the centre, or under the bottom reading left to right with them
    standing in toward it. Returns the outlines and the arc's span in
    degrees."""
    s = size / face.upm
    advs = [WORD - 2 * TRACK if ch == " " else face.advance(ch) * s for ch in text]
    span = (sum(advs) + TRACK * (len(text) - 1)) / MID
    start = -math.pi / 2 - span / 2 if top else math.pi / 2 + span / 2
    parts, d = [], 0.0
    for ch, adv in zip(text, advs):
        if ch != " ":
            t = start + (d + adv / 2) / MID * (1 if top else -1)
            # Along the reading direction, and the letters' up.
            tx, ty = (-math.sin(t), math.cos(t)) if top else (math.sin(t), -math.cos(t))
            ux, uy = (math.cos(t), math.sin(t)) if top else (-math.cos(t), -math.sin(t))
            px, py = SC + radius * math.cos(t), SC + radius * math.sin(t)
            parts.append(face.path(ch, (s * tx, s * ty, s * ux, s * uy,
                                        px - tx * adv / 2, py - ty * adv / 2)))
        d += adv + TRACK
    return " ".join(parts), math.degrees(span)


def line_text(face, text, size, track, centre_x, baseline):
    """A line of text centred on `centre_x`, the trailing tracking removed."""
    s = size / face.upm
    advs = [face.advance(ch) * s for ch in text]
    x = centre_x - (sum(advs) + track * (len(text) - 1)) / 2
    parts = []
    for ch, adv in zip(text, advs):
        parts.append(face.path(ch, (s, 0, 0, -s, x, baseline)))
        x += adv + track
    return " ".join(parts)


def star(x, y, r):
    pts = []
    for i in range(10):
        rr = r * 0.48 if i % 2 else r
        a = -math.pi / 2 + i * math.pi / 5
        pts.append(f"{x + rr * math.cos(a):.2f} {y + rr * math.sin(a):.2f}")
    return "M" + " L".join(pts) + "Z"


def leaf(x, y, angle, length, width):
    """A leaf from its base along `angle`."""
    a = math.radians(angle)
    ux, uy = math.cos(a), math.sin(a)

    def at(k, side):
        return f"{x + ux * length * k - uy * width * side:.2f} {y + uy * length * k + ux * width * side:.2f}"

    tip = f"{x + ux * length:.2f} {y + uy * length:.2f}"
    return (f"M{x:.2f} {y:.2f} C{at(0.25, 1.05)} {at(0.7, 0.85)} {tip} "
            f"C{at(0.7, -0.85)} {at(0.25, -1.05)} {x:.2f} {y:.2f}Z")


def sprig(x0, y0, x1, y1):
    """A sprig of mint: a gently rising stem with broad leaves in opposite
    pairs, shrinking toward a pair at the tip."""
    def at(t):
        return x0 + (x1 - x0) * t, y0 + (y1 - y0) * t - 14 * t * (1 - t)

    def heading(t):
        (ax, ay), (bx, by) = at(max(0, t - 0.01)), at(min(1, t + 0.01))
        return math.degrees(math.atan2(by - ay, bx - ax))

    stem = "M" + " L".join(f"{x:.2f} {y:.2f}" for x, y in (at(i / 20) for i in range(21)))
    leaves = []
    for t, spread, length, width in ((0.22, 55, 13, 5.2), (0.5, 55, 12, 4.8),
                                     (0.76, 52, 10, 4.2), (1, 30, 8.5, 3.6)):
        x, y = at(t)
        leaves += [leaf(x, y, heading(t) - spread, length, width),
                   leaf(x, y, heading(t) + spread, length, width)]
    return stem, " ".join(leaves)


def shield(inter, small=False):
    """The shield with IMF and the sprig cut through it: the mask that cuts
    them, and the shield drawn through it. `small` is the header's: no
    sprig, which is a smudge at that size, and IMF opened up and centred."""
    if small:
        imf = line_text(inter, "IMF", 48, 1.5, SC + 0.5, 135.5)
        sprig_cut = ""
    else:
        imf = line_text(inter, "IMF", 46, 1, SC + 0.5, 133)
        stem, leaves = sprig(90, 162, 150, 153)
        sprig_cut = (f'<path d="{stem}" fill="none" stroke="#000" stroke-width="2.5"/>'
                     f'<path d="{leaves}" fill="#000"/>')
    cut = (f'<mask id="cut" maskUnits="userSpaceOnUse" x="0" y="0" width="{S}" height="{S}">'
           f'<rect width="{S}" height="{S}" fill="#fff"/>'
           f'<path d="{imf}" fill="#000"/>{sprig_cut}</mask>')
    return cut, f'<path d="{SHIELD}" mask="url(#cut)"/>'


def seal_mask_svg(inter):
    """The seal's shape alone, for the live seal: the site lays the tag's
    metal into it with a CSS mask (.brand-seal in style/main.css). Each star
    sits in the middle of the gap between the two arcs, and the shield is
    set a size down inside the ring so it clears it all round."""
    top, top_span = ring_text(inter, "INTERNET", LEGEND, TOP_R, top=True)
    bottom, bottom_span = ring_text(inter, "MINT FINDER", LEGEND, BOTTOM_R, top=False)
    lift = math.radians((bottom_span - top_span) / 4)
    stars = " ".join(star(SC + side * MID * math.cos(lift), SC - MID * math.sin(lift), 8.5)
                     for side in (-1, 1))
    cut, body = shield(inter)
    lo = SC - INK
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{lo} {lo} {2 * INK} {2 * INK}" '
        f'width="{2 * INK}" height="{2 * INK}">'
        f'<defs>{cut}</defs>'
        f'<path d="{top} {bottom} {stars}"/>'
        f'<circle cx="{SC}" cy="{SC}" r="78" fill="none" stroke="#000" stroke-width="3.2"/>'
        f'<g transform="translate({SC} {SC + 3}) scale(.9) translate({-SC} {-SC})">{body}</g>'
        '</svg>'
    )


# The shield alone, cropped to its bounds, for the header.
SHIELD_BOX = (66, 69, 108, 122)


def shield_mask_svg(inter):
    """The header's shield alone, for the live header mark (.brand-shield)."""
    x, y, w, h = SHIELD_BOX
    cut, body = shield(inter, small=True)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {w} {h}" width="{w}" height="{h}">'
        f'<defs>{cut}</defs>{body}</svg>'
    )


def main():
    inter, tx = load_faces()
    OUT.mkdir(exist_ok=True)
    files = {
        OUT / "imf-seal-mask.svg": seal_mask_svg(inter),
        OUT / "imf-shield-mask.svg": shield_mask_svg(inter),
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
