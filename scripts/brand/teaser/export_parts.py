"""Export the old oval IMF tag's parts for the Blender teaser (teaser.py).

Blender can't import build.py (it needs fonttools), so this runs in the
brand venv and hands the tag over as plain data: the outlines of IMF and of
the name on the arc, flattened to polygons in the tag's 1000 by 720 units
(y down, as in the SVG), the oval, and the gradients and colour stops the
tag is painted with. Same parameters as sticker_svg(), so the sticker in
the video is the tag in public/brand/imf-tag.svg.

    /tmp/imf-brand/bin/python scripts/brand/teaser/export_parts.py

Writes renders/teaser/assets/parts.json (ignored by git).
"""

import json
import math
import sys
from pathlib import Path

from fontTools.pens.basePen import BasePen
from fontTools.svgLib.path import parse_path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build as b  # noqa: E402

OUT = b.ROOT / "renders" / "teaser" / "assets"

# Curves are cut into segments no longer than this, in tag units (a tag
# unit is 0.09 mm on the 90 mm sticker).
STEP = 0.6


class Flatten(BasePen):
    """Collects closed polygons from pen calls, curves sampled by length."""

    def __init__(self):
        super().__init__(None)
        self.contours = []
        self.points = []

    def _moveTo(self, pt):
        self.points = [pt]

    def _lineTo(self, pt):
        self.points.append(pt)

    def _sample(self, curve):
        p0 = self.points[-1]
        rough = sum(math.dist(a, c) for a, c in zip((p0, *curve), curve))
        n = max(2, math.ceil(rough / STEP))
        for i in range(1, n + 1):
            self.points.append(curve_at((p0, *curve), i / n))

    def _curveToOne(self, p1, p2, p3):
        self._sample((p1, p2, p3))

    def _qCurveToOne(self, p1, p2):
        self._sample((p1, p2))

    def _closePath(self):
        if len(self.points) > 2:
            if self.points[0] == self.points[-1]:
                self.points.pop()
            self.contours.append([[round(x, 3), round(y, 3)] for x, y in self.points])
        self.points = []

    _endPath = _closePath


def curve_at(pts, t):
    """A point on a Bezier of any order, by de Casteljau."""
    pts = list(pts)
    while len(pts) > 1:
        pts = [((1 - t) * a[0] + t * c[0], (1 - t) * a[1] + t * c[1]) for a, c in zip(pts, pts[1:])]
    return pts[0]


def contours(path_data):
    pen = Flatten()
    parse_path(path_data, pen)
    return pen.contours


def main():
    inter, tx = b.load_faces()
    W, H, CX, CY = b.W, b.H, b.CX, b.CY
    px, py, bx, by = (b.LIGHT[k] for k in ("px", "py", "bx", "by"))

    # As sticker_svg(with_arc=True) sets them.
    imf, (bx0, by0, bw, bh) = b.imf_path(inter, 0.25 * W, CY - 0.045 * W, nudge=0.004 * W)
    arc = b.arc_path(tx, 50, 6, 400, 280, 370)

    data = {
        "size": [W, H],
        "oval": {"cx": CX, "cy": CY, "rx": b.RX, "ry": b.RY},
        "imf": contours(imf),
        "arc": contours(arc),
        "imf_box": [bx0, by0, bw, bh],
        # Gradient end points (x1, y1, x2, y2) in tag units, as the SVG has them.
        "gradients": {
            "metal": b.css_linear(158, W, H, (2, 2), (bx, by)),
            "holo": b.css_linear(118, W, H, (3, 3), (1 - bx, by)),
            "steel": b.css_linear(172, bw, bh, (1, 2.2), (0, by), (bx0, by0)),
            "tint": b.css_linear(118, bw, bh, (2.6 * W / bw, 1), (bx, 0), (bx0, by0)),
        },
        "stops": {
            "chrome": b.CHROME,
            "rainbow": b.RAINBOW,
            "rainbow_cool": b.RAINBOW_COOL,
            "steel": b.STEEL,
        },
        "colour": b.COLOUR,
        "light": b.LIGHT,
    }
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / "parts.json"
    path.write_text(json.dumps(data))
    points = sum(len(c) for c in data["imf"] + data["arc"])
    print(f"{path.relative_to(b.ROOT)}: {len(data['imf'])} IMF and {len(data['arc'])} arc contours, {points} points")


if __name__ == "__main__":
    main()
