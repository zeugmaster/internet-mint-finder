"""The IMF sticker teaser: the old oval tag peels itself off a near-black
wall and drifts down like a leaf. 8 s, 1920 by 1080, 30 fps, Cycles.

Builds the whole scene from nothing in Blender 5.2, from the tag's parts as
export_parts.py writes them, and renders it. Every art-direction value is a
constant below. Run export_parts.py first, then (B is the Blender binary,
/Applications/Blender.app/Contents/MacOS/Blender):

    B -b --factory-startup --python-exit-code 1 --python scripts/brand/teaser/teaser.py -- --preview --check
    B -b ... -- --preview --stills 0,33,60,90,120,160,210,239
    B -b ... -- --preview --anim
    scripts/brand/teaser/encode.sh preview
    B -b ... -- --final --anim             (--samples 512 for the cleanest, slower)
    scripts/brand/teaser/encode.sh final

--check walks every frame and checks the sticker clears the wall once it
lets go and is out of frame by the end; --save-blend also writes
renders/teaser/teaser.blend to open by hand. Output goes to
renders/teaser/<preview|final>/ (ignored by git).

Beats: 0-2 s still, the light sweeping across the foil as the site's
sticker does on arrival; 2-4 s a top corner lifts and the sticker peels;
4-7 s it lets go and flutters down out of frame; 7-8 s the empty wall. The
peel rolls back over itself in a tight curl, showing the paper back, as a
real sticker does, and springs open into a loose curl as it lets go. The
camera eases in the whole time and never chases it.
"""

import json
import math
import sys
import time
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Quaternion, Vector

ROOT = Path(__file__).resolve().parents[3]
ASSETS = ROOT / "renders" / "teaser" / "assets"
OUT = ROOT / "renders" / "teaser"

# ---------------------------------------------------------------------------
# Art direction
# ---------------------------------------------------------------------------

FPS, LAST = 30, 239                     # frames 0..239, 8 s
SW, SH = 0.09, 0.0648                   # the sticker, 90 by 64.8 mm (the tag's 0.72)
STICKER_Z = 0.015                       # a touch above the centre of frame
GRID = 201                              # vertices across the sheet (0.45 mm apart)
THICK = 0.00022                         # vinyl and paper together
PEEL_DIR = math.radians(40)             # from the top right corner

# The timeline, in frames.
SWEEP = (9, 57)                         # the site's 0.3 s delay and 1.6 s sweep
PINCH = (62, 72)                        # the corner lifts and rolls back
HOLD = 78                               # a beat of tension
PEELED = 120                            # the peel reaches the far edge
RELEASE = 128                           # it springs open and lets go
GONE = 205                              # out of frame by here
REFOCUS = 216

# How much colour rides on the foil. The site's sticker carries 0.2; the
# film adds its own shift as the foil bends.
HOLO_TINT = 0.18
FILM_NM, FILM_IOR = 300.0, 1.25

# Camera: straight on to the wall, easing in. 16:9 for X; the sensor is
# fitted across the width.
RES = (1920, 1080)
LENS, SENSOR = 85.0, 36.0
CAM_Y = (-0.54, -0.47)                  # the sticker about 40% of the width
CARD_Y = -1.15                          # the reflection card and foil lights, behind the camera
CAM_Z = 0.0
FSTOP = 8.0                             # close in, so the lifting flap stays sharp

WALL = (0.011, 0.011, 0.012)            # near-black, linear
WALL_WASH = 30.0                        # the wall's own light, W
STUDIO = 0.9                            # the studio the foil mirrors
EXPOSURE = 0.0

TEX_W = 4096                            # height maps across the tag's 1000 units

# ---------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------


def srgb(hex_colour, alpha=1.0):
    h = hex_colour.lstrip("#")
    c = [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    lin = [x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4 for x in c]
    return (*lin, alpha)


def smoothstep(x):
    x = min(max(x, 0.0), 1.0)
    return x * x * (3 - 2 * x)


def smootherstep(x):
    x = min(max(x, 0.0), 1.0)
    return x * x * x * (x * (x * 6 - 15) + 10)


def ease_out(x):
    x = min(max(x, 0.0), 1.0)
    return 1 - (1 - x) ** 3


def cubic_bezier(x1, y1, x2, y2):
    """CSS cubic-bezier() as a function of time 0..1."""
    def f(x):
        lo, hi = 0.0, 1.0
        for _ in range(48):
            t = (lo + hi) / 2
            bx = 3 * (1 - t) ** 2 * t * x1 + 3 * (1 - t) * t * t * x2 + t ** 3
            lo, hi = (t, hi) if bx < x else (lo, t)
        t = (lo + hi) / 2
        return 3 * (1 - t) ** 2 * t * y1 + 3 * (1 - t) * t * t * y2 + t ** 3
    return f


SWEEP_EASE = cubic_bezier(0.2, 0.7, 0.2, 1)


def sock(sockets, name, kind=None):
    """A socket by name, and by type where a node has several of one name."""
    for s in sockets:
        if s.name == name and (kind is None or s.type == kind):
            return s
    raise KeyError(f"no socket {name!r} ({kind})")


class Nodes:
    """Builds a node tree: nodes, links, and maths that accepts numbers or
    sockets as inputs."""

    def __init__(self, tree, clear=True):
        self.t = tree
        if clear:
            tree.nodes.clear()

    def new(self, kind, **props):
        node = self.t.nodes.new(kind)
        for k, v in props.items():
            setattr(node, k, v)
        return node

    def feed(self, target, value):
        if hasattr(value, "is_output"):
            self.t.links.new(value, target)
        else:
            target.default_value = value

    def math(self, op, a, b=None, c=None):
        node = self.new("ShaderNodeMath", operation=op)
        for i, v in enumerate((a, b, c)):
            if v is not None:
                self.feed(node.inputs[i], v)
        return node.outputs[0]

    def vmath(self, op, a, b=None, scale=None):
        node = self.new("ShaderNodeVectorMath", operation=op)
        self.feed(node.inputs[0], a)
        if b is not None:
            self.feed(node.inputs[1], b)
        if scale is not None:
            self.feed(sock(node.inputs, "Scale"), scale)
        return node.outputs["Value" if op == "DOT_PRODUCT" else "Vector"]

    def mix(self, a, b, fac, blend="MIX", kind="RGBA"):
        node = self.new("ShaderNodeMix", data_type=kind, blend_type=blend)
        self.feed(sock(node.inputs, "Factor", "VALUE"), fac)
        t = "RGBA" if kind == "RGBA" else "VALUE"
        self.feed(sock(node.inputs, "A", t), a)
        self.feed(sock(node.inputs, "B", t), b)
        return sock(node.outputs, "Result", t)

    def map_range(self, value, lo, hi, to_lo=0.0, to_hi=1.0, smooth=False):
        node = self.new("ShaderNodeMapRange", clamp=True,
                        interpolation_type="SMOOTHSTEP" if smooth else "LINEAR")
        self.feed(sock(node.inputs, "Value", "VALUE"), value)
        for name, v in (("From Min", lo), ("From Max", hi), ("To Min", to_lo), ("To Max", to_hi)):
            self.feed(sock(node.inputs, name, "VALUE"), v)
        return sock(node.outputs, "Result", "VALUE")

    def ramp(self, factor, stops):
        """A colour ramp from the tag's (hex, percent) stops."""
        node = self.new("ShaderNodeValToRGB")
        cr = node.color_ramp
        cr.interpolation = "LINEAR"
        els = cr.elements
        # Elements stay sorted by position, so keep one and add the rest.
        while len(els) > 1:
            els.remove(els[-1])
        els[0].position, els[0].color = stops[0][1] / 100, srgb(stops[0][0])
        for hex_colour, pct in stops[1:]:
            els.new(pct / 100).color = srgb(hex_colour)
        self.feed(node.inputs["Fac"], factor)
        return node.outputs["Color"]

    def noise(self, vector, scale, detail=2.0):
        node = self.new("ShaderNodeTexNoise")
        self.feed(node.inputs["Vector"], vector)
        node.inputs["Scale"].default_value = scale
        node.inputs["Detail"].default_value = detail
        return node.outputs["Fac"]


def material(name):
    mat = bpy.data.materials.new(name)
    if mat.node_tree is None:               # before 5.0 a material starts without nodes
        mat.use_nodes = True
    g = Nodes(mat.node_tree)
    out = g.new("ShaderNodeOutputMaterial")
    bsdf = g.new("ShaderNodeBsdfPrincipled")
    g.t.links.new(bsdf.outputs["BSDF"], out.inputs["Surface"])
    return mat, g, bsdf


def key_frames(owner, path, frames, values):
    """Key a property on every given frame."""
    for f, v in zip(frames, values):
        setattr(owner, path, v)
        owner.keyframe_insert(path, frame=f)


# ---------------------------------------------------------------------------
# The tag's artwork: height maps rasterised from the exported outlines
# ---------------------------------------------------------------------------


def load_parts():
    path = ASSETS / "parts.json"
    if not path.exists():
        sys.exit(f"{path} is missing: run scripts/brand/teaser/export_parts.py first")
    return json.loads(path.read_text()), path.stat().st_mtime


def fill(polys, w, h, k):
    """Polygons (tag units, y down) filled by nonzero winding at pixel
    centres, as a w by h mask, top row first."""
    acc = np.zeros((h, w + 1), np.int32)
    for poly in polys:
        p = np.asarray(poly, np.float64) * k
        q = np.roll(p, -1, axis=0)
        x0, y0, x1, y1 = p[:, 0], p[:, 1], q[:, 0], q[:, 1]
        keep = y0 != y1
        x0, y0, x1, y1 = x0[keep], y0[keep], x1[keep], y1[keep]
        sign = np.where(y1 > y0, 1, -1)
        r0 = np.ceil(np.minimum(y0, y1) - 0.5).astype(np.int64)
        r1 = np.ceil(np.maximum(y0, y1) - 0.5).astype(np.int64)
        counts = np.clip(r1 - r0, 0, None)
        total = int(counts.sum())
        if not total:
            continue
        edge = np.repeat(np.arange(len(counts)), counts)
        rows = r0[edge] + (np.arange(total) - np.repeat(np.cumsum(counts) - counts, counts))
        yc = rows + 0.5
        xc = x0[edge] + (yc - y0[edge]) * (x1[edge] - x0[edge]) / (y1[edge] - y0[edge])
        cols = np.clip(np.ceil(xc - 0.5).astype(np.int64), 0, w)
        ok = (rows >= 0) & (rows < h)
        np.add.at(acc, (rows[ok], cols[ok]), sign[edge][ok])
    return (np.cumsum(acc[:, :w], axis=1) != 0).astype(np.float32)


def box(a, r, axis):
    """A moving average 2r+1 wide along one axis, edges held."""
    pad = [(0, 0)] * a.ndim
    pad[axis] = (r + 1, r)
    c = np.cumsum(np.pad(a, pad, mode="edge"), axis=axis, dtype=np.float64)
    n = a.shape[axis]
    hi = np.take(c, np.arange(2 * r + 1, 2 * r + 1 + n), axis=axis)
    lo = np.take(c, np.arange(n), axis=axis)
    return ((hi - lo) / (2 * r + 1)).astype(np.float32)


def blur(a, sigma):
    """Three box passes each way, close to a Gaussian of `sigma` px."""
    r = max(1, round((math.sqrt(12 * sigma * sigma / 3 + 1) - 1) / 2))
    for axis in (0, 1):
        for _ in range(3):
            a = box(a, r, axis)
    return a


def height_maps(parts, stamp):
    """IMF raised with rounded shoulders and the name pressed in crisply, as
    0..1 heights. Cached beside parts.json."""
    w = TEX_W
    k = w / parts["size"][0]
    h = round(parts["size"][1] * k)
    px_per_mm = w / (SW * 1000)
    maps = {}
    for name, sigma_mm in (("imf", 0.3), ("arc", 0.06)):
        path = ASSETS / f"{name}_height.png"
        if not path.exists() or path.stat().st_size == 0 or path.stat().st_mtime < stamp:
            t0 = time.time()
            write_png16(path, blur(fill(parts[name], w, h, k), sigma_mm * px_per_mm))
            print(f"{path.name} {w}x{h} in {time.time() - t0:.1f}s")
        img = bpy.data.images.load(str(path), check_existing=True)
        img.colorspace_settings.name = "Non-Color"
        maps[name] = img
    return maps


def write_png16(path, a):
    """A 0..1 map as a 16-bit greyscale PNG, top row first, so the relief
    has no 8-bit steps for the chrome to show."""
    import OpenImageIO as oiio
    h, w = a.shape
    tmp = path.with_suffix(".tmp.png")
    out = oiio.ImageOutput.create(str(tmp))
    if out is None or not out.open(str(tmp), oiio.ImageSpec(w, h, 1, "uint16")):
        sys.exit(f"couldn't write {tmp}: {oiio.geterror()}")
    # OIIO wants an explicit channel axis; a bare 2D array comes out scrambled.
    pixels = np.ascontiguousarray((np.clip(a, 0, 1) * 65535 + 0.5).astype(np.uint16)[:, :, None])
    ok = out.write_image(pixels)
    out.close()
    if not ok:
        sys.exit(f"couldn't write {tmp}: {out.geterror()}")
    back = oiio.ImageInput.open(str(tmp))
    read = back.read_image("uint16")
    back.close()
    if read is None or read.reshape(h, w).tolist()[h // 2][::97] != pixels[h // 2, ::97, 0].tolist():
        sys.exit(f"{tmp} didn't read back as written")
    tmp.replace(path)                       # whole files only, never a half-written one


# ---------------------------------------------------------------------------
# The sticker: a thin oval sheet that peels and flexes
# ---------------------------------------------------------------------------


def sticker_mesh():
    """An all-quad grid mapped from a square onto the oval, so it bends
    evenly everywhere; UVs from the rest shape, matching the tag's units."""
    a = np.linspace(-1.0, 1.0, GRID)
    A, B = np.meshgrid(a, a)
    X = A * np.sqrt(1 - B * B / 2) * SW / 2
    Y = B * np.sqrt(1 - A * A / 2) * SH / 2
    verts = np.stack([X.ravel(), Y.ravel(), np.full(X.size, THICK + 0.00005)], axis=1)
    i, j = np.meshgrid(np.arange(GRID - 1), np.arange(GRID - 1), indexing="ij")
    v = (i * GRID + j).ravel()
    faces = np.stack([v, v + 1, v + GRID + 1, v + GRID], axis=1)

    me = bpy.data.meshes.new("Sticker")
    me.vertices.add(len(verts))
    me.vertices.foreach_set("co", verts.astype(np.float32).ravel())
    me.loops.add(faces.size)
    me.loops.foreach_set("vertex_index", faces.astype(np.int32).ravel())
    me.polygons.add(len(faces))
    me.polygons.foreach_set("loop_start", (np.arange(len(faces)) * 4).astype(np.int32))
    me.update()
    me.validate()

    loop_v = np.empty(len(me.loops), np.int32)
    me.loops.foreach_get("vertex_index", loop_v)
    uv = np.stack([verts[loop_v, 0] / SW + 0.5, verts[loop_v, 1] / SH + 0.5], axis=1).astype(np.float32)
    layer = me.uv_layers.new(name="UVMap")
    try:
        layer.uv.foreach_set("vector", uv.ravel())
    except AttributeError:
        layer.data.foreach_set("uv", uv.ravel())
    me.shade_smooth()
    return me


def peel_tree():
    """The page-curl that doesn't stretch the sheet: everything past the
    Front line (along the peel direction u) rolls up off the wall round a
    cylinder of the given radius, up to Max Angle, and runs on straight
    beyond it."""
    ng = bpy.data.node_groups.new("IMF Peel", "GeometryNodeTree")
    io = ng.interface
    io.new_socket("Geometry", in_out="INPUT", socket_type="NodeSocketGeometry")
    io.new_socket("Geometry", in_out="OUTPUT", socket_type="NodeSocketGeometry")
    for name in ("Front", "Radius", "Max Angle"):
        io.new_socket(name, in_out="INPUT", socket_type="NodeSocketFloat")

    g = Nodes(ng)
    gi, go = g.new("NodeGroupInput"), g.new("NodeGroupOutput")
    u = (math.cos(PEEL_DIR), math.sin(PEEL_DIR), 0.0)
    pos = g.new("GeometryNodeInputPosition").outputs["Position"]
    front, radius, theta_max = gi.outputs["Front"], gi.outputs["Radius"], gi.outputs["Max Angle"]

    d = g.math("MAXIMUM", g.math("SUBTRACT", g.vmath("DOT_PRODUCT", pos, u), front), 0.0)
    r = g.math("MAXIMUM", radius, 0.001)
    th = g.math("MINIMUM", g.math("DIVIDE", d, r), theta_max)
    rest = g.math("MAXIMUM", g.math("SUBTRACT", d, g.math("MULTIPLY", r, theta_max)), 0.0)
    along = g.math("ADD", g.math("MULTIPLY", r, g.math("SINE", th)),
                   g.math("MULTIPLY", rest, g.math("COSINE", theta_max)))
    up = g.math("ADD", g.math("MULTIPLY", r, g.math("SUBTRACT", 1.0, g.math("COSINE", th))),
                g.math("MULTIPLY", rest, g.math("SINE", theta_max)))
    lift = g.new("ShaderNodeCombineXYZ")
    g.feed(lift.inputs["Z"], up)
    offset = g.vmath("ADD", g.vmath("SCALE", u, scale=g.math("SUBTRACT", along, d)), lift.outputs["Vector"])

    setpos = g.new("GeometryNodeSetPosition")
    g.feed(setpos.inputs["Geometry"], gi.outputs["Geometry"])
    g.feed(setpos.inputs["Offset"], offset)
    g.feed(go.inputs["Geometry"], setpos.outputs["Geometry"])
    return ng


def gn_input(mod, name):
    """The modifier's input socket for a group input, in Blender 5.2's
    modifier.properties.inputs, or the older ID property."""
    ident = next(i.identifier for i in mod.node_group.interface.items_tree
                 if getattr(i, "in_out", None) == "INPUT" and i.name == name)
    props = getattr(mod, "properties", None)
    if props is not None and hasattr(props, "inputs"):
        return getattr(props.inputs, ident), None
    return mod, f'["{ident}"]'


def key_gn(mod, name, frames, values):
    owner, path = gn_input(mod, name)
    for f, v in zip(frames, values):
        if path is None:
            owner.value = v
            owner.keyframe_insert("value", frame=f)
        else:
            owner[path[2:-2]] = v
            owner.keyframe_insert(path, frame=f)


def peel_params(f):
    """Front (m along u), radius (m) and the curl's angle at frame f. Past
    90 degrees the peeled part rolls back over the part still stuck, paper
    side out, as a sticker does when it's peeled."""
    u = (math.cos(PEEL_DIR), math.sin(PEEL_DIR))
    s = math.hypot(SW / 2 * u[0], SH / 2 * u[1])    # the farthest point along u
    deg = math.radians
    if f < PINCH[0]:
        return s + 0.001, 0.003, 0.0
    if f <= PINCH[1]:
        # The corner lifts and rolls back on itself.
        k = ease_out((f - PINCH[0]) / (PINCH[1] - PINCH[0]))
        return s - 0.007 * k, 0.003, deg(120) * k
    if f <= HOLD:
        k = (f - PINCH[1]) / (HOLD - PINCH[1])
        return s - 0.007 - 0.0005 * k, 0.003, deg(120) + deg(5) * smoothstep(k)
    if f <= PEELED:
        # The fold runs across the sticker, steady, a touch quicker at first.
        # As the flap lengthens it stands up more, so it doesn't lie back over
        # the foil still stuck and hide it.
        p = (f - HOLD) / (PEELED - HOLD)
        e = (p + smoothstep(p)) / 2
        start, end = s - 0.0075, -s - 0.0005
        return start + (end - start) * e, 0.003 + 0.001 * e, deg(125) - deg(15) * smoothstep(p)
    # The last of it lets go and the sticker springs open into a loose curl
    # that faces the camera (edge on it reads as a dark sliver), still
    # settling as it starts to fall, then breathing as it goes.
    shut = math.exp(-(f - PEELED) / FPS / 0.12)
    theta, radius = deg(30) + deg(80) * shut, 0.03 - 0.026 * shut
    if f > RELEASE:
        t = (f - RELEASE) / FPS
        env = 1 - math.exp(-t / 0.35)
        phase = 2 * math.pi * t / 1.25
        radius += 0.005 * env * math.sin(phase)
        theta += deg(8) * env * math.cos(phase)
    return -s - 0.0005, radius, theta


def build_sticker(maps, parts):
    me = sticker_mesh()
    ob = bpy.data.objects.new("Sticker", me)
    bpy.context.scene.collection.objects.link(ob)
    # Local +Z (the face) toward the camera, local +Y up.
    ob.rotation_euler = (math.radians(90), 0.0, 0.0)
    ob.location = (0.0, 0.0, STICKER_Z)
    for mat in (foil_material(maps, parts), paper_material(), edge_material()):
        me.materials.append(mat)

    mod = ob.modifiers.new("Peel", "NODES")
    mod.node_group = peel_tree()
    frames = list(range(0, LAST + 1))
    params = [peel_params(f) for f in frames]
    key_gn(mod, "Front", frames, [p[0] for p in params])
    key_gn(mod, "Radius", frames, [p[1] for p in params])
    key_gn(mod, "Max Angle", frames, [p[2] for p in params])

    solid = ob.modifiers.new("Thickness", "SOLIDIFY")
    solid.thickness = THICK
    solid.offset = -1.0                     # grows back toward the wall
    solid.use_even_offset = True
    solid.use_quality_normals = True
    solid.use_rim = True
    solid.material_offset = 1               # the paper back
    solid.material_offset_rim = 2           # the die-cut edge
    split = ob.modifiers.new("Crisp edge", "EDGE_SPLIT")
    split.use_edge_angle = True
    split.use_edge_sharp = False
    split.split_angle = math.radians(30)
    if hasattr(ob, "cycles"):
        ob.cycles.motion_steps = 3
    return ob, mod


# ---------------------------------------------------------------------------
# Materials
# ---------------------------------------------------------------------------


def gradient(g, uvx, uvy, ends):
    """The factor of an SVG linear gradient (end points in tag units, y
    down) at the sheet's UV."""
    x1, y1, x2, y2 = ends
    dx, dy = x2 - x1, y2 - y1
    l2 = dx * dx + dy * dy
    a, b = 1000 * dx / l2, -720 * dy / l2
    c = (720 * dy - x1 * dx - y1 * dy) / l2
    return g.math("MULTIPLY_ADD", uvy, b, g.math("MULTIPLY_ADD", uvx, a, c))


def foil_material(maps, parts):
    """Holographic chrome: the tag's chrome bands, its rainbow laid in as
    colour and sliding as the foil bends, a thin film over it, a clear coat;
    IMF raised in steel and the name pressed in, both from height maps; a
    bevel at the die-cut edge."""
    mat, g, bsdf = material("Foil")
    grads, stops = parts["gradients"], parts["stops"]
    uv = g.new("ShaderNodeTexCoord").outputs["UV"]
    sep = g.new("ShaderNodeSeparateXYZ")
    g.feed(sep.inputs[0], uv)
    u, v = sep.outputs["X"], sep.outputs["Y"]

    def height(name):
        tex = g.new("ShaderNodeTexImage", image=maps[name], interpolation="Cubic", extension="EXTEND")
        g.feed(tex.inputs["Vector"], uv)
        return tex.outputs["Color"]

    imf_h, arc_h = g.math("ADD", height("imf"), 0.0), g.math("ADD", height("arc"), 0.0)
    m_imf = g.map_range(imf_h, 0.45, 0.55)
    m_arc = g.map_range(arc_h, 0.45, 0.55)

    facing = g.new("ShaderNodeLayerWeight")
    facing.inputs["Blend"].default_value = 0.35
    slide = g.math("MULTIPLY", g.math("SUBTRACT", 1.0, facing.outputs["Facing"]), 0.5)

    chrome = g.ramp(gradient(g, u, v, grads["metal"]), stops["chrome"])
    base = g.mix(chrome, (0.92, 0.92, 0.92, 1.0), 0.2)
    rainbow = g.ramp(g.math("ADD", gradient(g, u, v, grads["holo"]), slide), stops["rainbow"])
    base = g.mix(base, rainbow, HOLO_TINT, blend="COLOR")

    steel = g.ramp(gradient(g, u, v, grads["steel"]), stops["steel"])
    tint = g.ramp(g.math("ADD", gradient(g, u, v, grads["tint"]), slide), stops["rainbow_cool"])
    steel = g.mix(steel, tint, 0.75 * HOLO_TINT, blend="COLOR")
    base = g.mix(base, steel, m_imf)
    base = g.mix(base, (0.35, 0.35, 0.35, 1.0), g.math("MULTIPLY", m_arc, 1.0), blend="MULTIPLY")
    g.feed(bsdf.inputs["Base Color"], base)

    grain = g.noise(uv, 400.0, 4.0)
    rough = g.math("MULTIPLY_ADD", grain, 0.06, 0.11)
    rough = g.mix(rough, 0.30, m_imf, kind="FLOAT")
    rough = g.mix(rough, 0.42, m_arc, kind="FLOAT")
    g.feed(bsdf.inputs["Metallic"], 1.0)
    g.feed(bsdf.inputs["Roughness"], rough)

    tangent = g.new("ShaderNodeTangent", direction_type="UV_MAP")
    tangent.uv_map = "UVMap"
    g.feed(bsdf.inputs["Anisotropic"], g.math("MULTIPLY", m_imf, 0.5))
    g.feed(bsdf.inputs["Tangent"], tangent.outputs["Tangent"])

    film = g.math("MULTIPLY", g.math("MULTIPLY_ADD", g.noise(uv, 3.0, 1.0), 0.3, 0.85), FILM_NM)
    film = g.math("MULTIPLY", film, g.math("MULTIPLY_ADD", m_imf, -0.5, 1.0))
    g.feed(bsdf.inputs["Thin Film Thickness"], film)
    g.feed(bsdf.inputs["Thin Film IOR"], FILM_IOR)

    # The die-cut bevel: the height falls away over the last 1.5% of the oval.
    cu = g.math("MULTIPLY", g.math("SUBTRACT", u, 0.5), 2.0)
    cv = g.math("MULTIPLY", g.math("SUBTRACT", v, 0.5), 2.0)
    r = g.math("SQRT", g.math("ADD", g.math("MULTIPLY", cu, cu), g.math("MULTIPLY", cv, cv)))
    rim = g.map_range(r, 0.985, 1.0, 1.0, 0.0, smooth=True)
    bump_h = g.math("MULTIPLY", rim, 0.00012)
    bump_h = g.math("MULTIPLY_ADD", imf_h, 0.00012, bump_h)
    bump_h = g.math("MULTIPLY_ADD", arc_h, -0.00006, bump_h)
    bump_h = g.math("MULTIPLY_ADD", g.noise(uv, 900.0, 2.0), 0.000002, bump_h)
    bump = g.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 1.0
    bump.inputs["Distance"].default_value = 1.0
    g.feed(bump.inputs["Height"], bump_h)
    g.feed(bsdf.inputs["Normal"], bump.outputs["Normal"])

    g.feed(bsdf.inputs["Coat Weight"], 0.5)
    g.feed(bsdf.inputs["Coat Roughness"], 0.04)
    g.feed(bsdf.inputs["Coat IOR"], 1.5)
    g.feed(bsdf.inputs["Coat Normal"], bump.outputs["Normal"])
    return mat


def paper_material():
    """The back: matte white paper with a faint sheen of adhesive."""
    mat, g, bsdf = material("Paper back")
    g.feed(bsdf.inputs["Base Color"], (0.82, 0.82, 0.80, 1.0))
    g.feed(bsdf.inputs["Roughness"], 0.6)
    g.feed(bsdf.inputs["Coat Weight"], 0.2)
    g.feed(bsdf.inputs["Coat Roughness"], 0.35)
    uv = g.new("ShaderNodeTexCoord").outputs["UV"]
    bump = g.new("ShaderNodeBump")
    bump.inputs["Distance"].default_value = 1.0
    g.feed(bump.inputs["Height"], g.math("MULTIPLY", g.noise(uv, 900.0, 3.0), 0.000003))
    g.feed(bsdf.inputs["Normal"], bump.outputs["Normal"])
    return mat


def edge_material():
    mat, g, bsdf = material("Die-cut edge")
    g.feed(bsdf.inputs["Base Color"], (0.7, 0.7, 0.7, 1.0))
    g.feed(bsdf.inputs["Roughness"], 0.5)
    g.feed(bsdf.inputs["Metallic"], 0.0)
    return mat


def wall_material():
    """Near-black matte, a little tooth, and a soft contact shadow round the
    sticker that fades on its own as it lifts."""
    mat, g, bsdf = material("Wall")
    obj = g.new("ShaderNodeTexCoord").outputs["Object"]
    ao = g.new("ShaderNodeAmbientOcclusion", samples=16, only_local=False)
    ao.inputs["Distance"].default_value = 0.004
    contact = g.map_range(ao.outputs["AO"], 0.5, 1.0, 0.55, 1.0)
    base = g.mix((*WALL, 1.0), (0.0, 0.0, 0.0, 1.0), g.math("SUBTRACT", 1.0, contact))
    g.feed(bsdf.inputs["Base Color"], base)
    g.feed(bsdf.inputs["Roughness"], g.math("MULTIPLY_ADD", g.noise(obj, 25.0, 3.0), 0.12, 0.56))
    # Matte: nothing the foil is lit by may show on it.
    g.feed(bsdf.inputs["Specular IOR Level"], 0.0)
    bump = g.new("ShaderNodeBump")
    bump.inputs["Distance"].default_value = 1.0
    g.feed(bump.inputs["Height"], g.math("MULTIPLY", g.noise(obj, 600.0, 3.0), 0.00001))
    g.feed(bsdf.inputs["Normal"], bump.outputs["Normal"])
    return mat


# ---------------------------------------------------------------------------
# The set: wall, the reflection card, lights, camera
# ---------------------------------------------------------------------------


def plane(name, w, h, location, rotation, mat):
    me = bpy.data.meshes.new(name)
    me.from_pydata([(-w / 2, -h / 2, 0), (w / 2, -h / 2, 0), (w / 2, h / 2, 0), (-w / 2, h / 2, 0)], [], [(0, 1, 2, 3)])
    uv = me.uv_layers.new(name="UVMap")
    ob = bpy.data.objects.new(name, me)
    ob.location, ob.rotation_euler = location, rotation
    me.materials.append(mat)
    bpy.context.scene.collection.objects.link(ob)
    return ob


def build_wall():
    # Facing the camera (-Y).
    return plane("Wall", 3.0, 3.0, (0.0, 0.0, 0.0), (math.radians(90), 0.0, 0.0), wall_material())


def build_card(foil):
    """An unseen card behind the camera for the foil to mirror: soft bands of
    light like the tag's chrome. Without it a mirror facing the camera shows
    only the dark behind the camera."""
    mat, g, bsdf = material("Reflection card")
    g.t.nodes.remove(bsdf)
    out = next(n for n in g.t.nodes if n.type == "OUTPUT_MATERIAL")
    obj = g.new("ShaderNodeTexCoord").outputs["Object"]
    sep = g.new("ShaderNodeSeparateXYZ")
    g.feed(sep.inputs[0], obj)
    a = math.radians(158)
    s = g.math("MULTIPLY_ADD", sep.outputs["Y"], math.sin(a), g.math("MULTIPLY", sep.outputs["X"], math.cos(a)))
    wave = g.math("SINE", g.math("MULTIPLY", s, 2 * math.pi / 0.05))
    level = g.math("MULTIPLY_ADD", wave, 0.42, 0.58)
    emit = g.new("ShaderNodeEmission")
    g.feed(emit.inputs["Color"], (1.0, 1.0, 1.0, 1.0))
    g.feed(emit.inputs["Strength"], g.math("MULTIPLY", level, 0.8))
    g.t.links.new(emit.outputs["Emission"], out.inputs["Surface"])
    if hasattr(mat, "cycles"):
        mat.cycles.emission_sampling = "FRONT"
    card = plane("Reflection card", 1.2, 1.4, (0.06, CARD_Y, CAM_Z), (math.radians(-90), 0.0, 0.0), mat)
    card.visible_camera = card.visible_diffuse = card.visible_shadow = False
    light_only(card, foil)
    # The bands drift a little with the sweep, as the CSS ones do.
    frames = list(range(SWEEP[0], SWEEP[1] + 1))
    key_frames(card, "location", frames,
               [(0.06 * (1 - SWEEP_EASE((f - SWEEP[0]) / (SWEEP[1] - SWEEP[0]))), CARD_Y, CAM_Z) for f in frames])
    return card


def area_light(name, location, energy, size, size_y=None, shape="RECTANGLE", target=None, rotation=None):
    data = bpy.data.lights.new(name, "AREA")
    data.shape = shape
    data.size = size
    if size_y is not None:
        data.size_y = size_y
    data.energy = energy
    ob = bpy.data.objects.new(name, data)
    ob.location = location
    if target is not None:
        direction = Vector(target) - Vector(location)
        ob.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
    elif rotation is not None:
        ob.rotation_euler = rotation
    bpy.context.scene.collection.objects.link(ob)
    return ob


def light_only(ob, collection):
    """Light only what's in `collection` (Cycles light linking): what lights
    the foil never shows on the wall."""
    ob.light_linking.receiver_collection = collection


def build_lights(foil, wall):
    aim = (0.0, 0.0, STICKER_Z)
    # The wall's own faint light, from up and to the left: a near-black
    # gradient, and the sticker's shadow on it as it lifts.
    wash = area_light("Wall wash", (-0.8, -1.0, 0.9), WALL_WASH, 1.0, 1.0, target=(0.0, 0.0, 0.0))
    light_only(wash, wall)
    key = area_light("Key", (-0.45, -0.55, 0.50), 12.0, 0.5, 0.5, target=aim)
    key.data.spread = math.radians(55)
    key.data.color = (0.95, 0.97, 1.0)

    # A tall thin strip behind the camera, leaning like the CSS streak,
    # crossing right to left with the site's easing: seen only in the foil.
    sweep = area_light("Sweep", (0.36, CARD_Y, CAM_Z), 0.0, 0.025, 1.2,
                       rotation=(math.radians(90), math.radians(22), 0.0))
    sweep.data.diffuse_factor = 0.0
    frames = list(range(SWEEP[0] - 1, SWEEP[1] + 2))
    for f in frames:
        p = min(max((f - SWEEP[0]) / (SWEEP[1] - SWEEP[0]), 0.0), 1.0)
        sweep.location = (0.36 - 0.72 * SWEEP_EASE(p), CARD_Y, CAM_Z)
        sweep.keyframe_insert("location", frame=f)
        sweep.data.energy = 6.0 * math.sin(math.pi * p) ** 0.8
        sweep.data.keyframe_insert("energy", frame=f)

    # The glint settles up and to the left, where the site's light rests.
    glint = area_light("Glint", (-0.05, CARD_Y, 0.09), 0.0, 0.12, shape="DISK",
                       rotation=(math.radians(90), 0.0, 0.0))
    glint.data.diffuse_factor = 0.0
    for f in range(28, 60):
        glint.data.energy = 1.2 * (0.4 + 0.6 * smoothstep((f - 30) / 27))
        glint.data.keyframe_insert("energy", frame=f)

    kicker = area_light("Kicker", (-0.3, -0.7, -0.25), 6.0, 0.3, 0.3, target=aim)
    kicker.data.diffuse_factor = 0.2
    for ob in (key, sweep, glint, kicker):
        light_only(ob, foil)
    return key, sweep, glint, kicker


def build_camera():
    data = bpy.data.cameras.new("Camera")
    data.lens = LENS
    data.sensor_fit = "HORIZONTAL"
    data.sensor_width = SENSOR
    data.clip_start = 0.01
    data.dof.use_dof = True
    data.dof.aperture_fstop = FSTOP
    data.dof.aperture_blades = 7
    cam = bpy.data.objects.new("Camera", data)
    cam.rotation_euler = (math.radians(90), 0.0, 0.0)
    bpy.context.scene.collection.objects.link(cam)
    bpy.context.scene.camera = cam
    frames = list(range(0, LAST + 1))
    key_frames(cam, "location", frames,
               [(0.0, CAM_Y[0] + (CAM_Y[1] - CAM_Y[0]) * f / LAST, CAM_Z) for f in frames])
    return cam


def tan_half_v():
    """Half the vertical field of view, as a tangent (the sensor is fitted
    across the width)."""
    return (SENSOR / 2) / LENS * RES[1] / RES[0]


def cam_y(f):
    return CAM_Y[0] + (CAM_Y[1] - CAM_Y[0]) * f / LAST


# ---------------------------------------------------------------------------
# The flight
# ---------------------------------------------------------------------------


def evaluated_points(ob):
    dg = bpy.context.evaluated_depsgraph_get()
    ev = ob.evaluated_get(dg)
    me = ev.to_mesh()
    co = np.empty(len(me.vertices) * 3, np.float64)
    me.vertices.foreach_get("co", co)
    ev.to_mesh_clear()
    co = co.reshape(-1, 3)
    m = np.array(ev.matrix_world)
    return co @ m[:3, :3].T + m[:3, 3]


def fall_profile(t):
    """The downward speed's shape, slower at the ends of each swing like a
    leaf; integrated numerically. Returns drop / v_t at time t."""
    n = max(1, int(t * 600))
    s = np.linspace(0.0, t, n + 1)
    v = (1 - np.exp(-s / 0.4)) * (1 - 0.35 * np.cos(4 * np.pi * s / 1.25))
    return float(np.trapezoid(v, s)) if hasattr(np, "trapezoid") else float(np.trapz(v, s))


def drift(t):
    """Away from the wall at about the speed the sticker had as it gave way
    (the curl is still touching the wall), a side to side sway that builds
    as it lets go, and a drift on the breeze."""
    env = 1 - math.exp(-t / 0.35)
    # A breeze carries it a little to the right across the wide frame. Each
    # motion starts from rest, so letting go doesn't jolt.
    ease = 1 - math.exp(-t / 0.15)
    return (0.03 * env * math.sin(2 * math.pi * t / 1.25) + 0.035 * (t - 0.35 * env),
            -0.10 * (1 - math.exp(-t / 1.6)) * ease, env)


def build_flight(sticker):
    scene = bpy.context.scene
    scene.frame_set(RELEASE)
    pts = evaluated_points(sticker)
    c0 = Vector(pts.mean(axis=0))
    reach = float(np.linalg.norm(pts - np.array(c0), axis=1).max())

    flight = bpy.data.objects.new("Flight", None)
    flight.empty_display_size = 0.02
    flight.rotation_mode = "QUATERNION"
    flight.location = c0
    scene.collection.objects.link(flight)
    bpy.context.view_layer.update()
    sticker.parent = flight
    sticker.matrix_parent_inverse = flight.matrix_world.inverted()

    # Fall just fast enough that the whole sticker is below the frame at GONE.
    t_gone = (GONE - RELEASE) / FPS
    dx, dy, _ = drift(t_gone)
    depth = (c0.y + dy) - cam_y(GONE)
    bottom = CAM_Z - depth * tan_half_v()
    drop_needed = (c0.z - bottom) + reach + 0.008
    v_t = drop_needed / fall_profile(t_gone)

    second = Vector((math.cos(math.radians(30)), 0.0, math.sin(math.radians(30))))
    prev = Quaternion()
    frames = list(range(RELEASE, LAST + 1))
    for f in frames:
        t = (f - RELEASE) / FPS
        dx, dy, env = drift(t)
        dz = -v_t * fall_profile(t)
        phase = 2 * math.pi * t / 1.25
        roll = Quaternion((0, 1, 0), math.radians(26) * env * math.cos(phase))
        yaw = Quaternion((0, 0, 1), 0.25 * env * math.sin(phase + 0.7))
        flip = Quaternion((1, 0, 0), 2 * math.pi * smootherstep((f - 145) / 22))
        flip = Quaternion(second, math.pi * smootherstep((f - 182) / 18)) @ flip
        q = roll @ yaw @ flip
        if q.dot(prev) < 0:
            q.negate()
        prev = q
        flight.location = c0 + Vector((dx, dy, dz))
        flight.rotation_quaternion = q
        flight.keyframe_insert("location", frame=f)
        flight.keyframe_insert("rotation_quaternion", frame=f)
    # Held still on every frame before it lets go, so the curves can't drift.
    for f in range(0, RELEASE):
        flight.location = c0
        flight.rotation_quaternion = Quaternion()
        flight.keyframe_insert("location", frame=f)
        flight.keyframe_insert("rotation_quaternion", frame=f)
    print(f"release centroid {tuple(round(x, 4) for x in c0)}, reach {reach:.4f} m, v_t {v_t:.4f} m/s")
    return flight, c0, v_t


def build_focus(cam, c0, v_t):
    """Focus on the wall, then pulled to the sticker as it comes off, held on
    it as it falls (a few frames behind), and back to the wall."""
    focus = bpy.data.objects.new("Focus", None)
    bpy.context.scene.collection.objects.link(focus)
    cam.data.dof.focus_object = focus
    wall_y = -THICK
    lifted = -0.012

    def sticker_y(f):
        return c0.y + drift(max(f - 3 - RELEASE, 0) / FPS)[1]

    for f in range(0, LAST + 1):
        if f <= 80:
            y = wall_y
        elif f <= RELEASE:
            y = wall_y + (lifted - wall_y) * smoothstep((f - 80) / (RELEASE - 80))
        elif f <= 200:
            y = lifted + (sticker_y(f) - lifted) * smoothstep((f - RELEASE) / 6)
        else:
            y = sticker_y(200) + (wall_y - sticker_y(200)) * smoothstep((f - 200) / (REFOCUS - 200))
        focus.location = (0.0, y, CAM_Z)
        focus.keyframe_insert("location", frame=f)
    return focus


# ---------------------------------------------------------------------------
# Render settings
# ---------------------------------------------------------------------------


def use_metal():
    prefs = bpy.context.preferences.addons["cycles"].preferences
    prefs.compute_device_type = "METAL"
    if hasattr(prefs, "refresh_devices"):
        prefs.refresh_devices()
    else:
        prefs.get_devices()
    used = []
    for d in prefs.devices:
        d.use = d.type == "METAL"
        if d.use:
            used.append(d.name)
    if hasattr(prefs, "metalrt"):
        prefs.metalrt = "AUTO"
    print("Cycles devices:", ", ".join(used) or "CPU")
    return bool(used)


def render_settings(mode, samples=None):
    scene = bpy.context.scene
    r, c = scene.render, scene.cycles
    r.engine = "CYCLES"
    c.device = "GPU" if use_metal() else "CPU"
    scene.frame_start, scene.frame_end = 0, LAST
    r.fps = FPS
    r.resolution_x, r.resolution_y = RES
    r.resolution_percentage = 50 if mode == "preview" else 100
    r.use_persistent_data = True
    r.use_motion_blur = True
    r.motion_blur_shutter = 0.5
    if hasattr(r, "motion_blur_position"):
        r.motion_blur_position = "CENTER"
    r.dither_intensity = 1.0
    r.use_overwrite = False
    r.use_placeholder = True

    c.use_animated_seed = True
    if hasattr(c, "use_light_tree"):
        c.use_light_tree = True
    c.caustics_reflective = c.caustics_refractive = False
    c.blur_glossy = 0.5
    c.sample_clamp_indirect = 2.0
    c.max_bounces, c.diffuse_bounces, c.glossy_bounces = 8, 2, 4
    c.transmission_bounces, c.transparent_max_bounces = 0, 2
    c.use_denoising = True
    c.denoiser = "OPENIMAGEDENOISE"
    c.denoising_input_passes = "RGB_ALBEDO_NORMAL"
    if hasattr(c, "denoising_use_gpu"):
        c.denoising_use_gpu = True
    if mode == "preview":
        c.samples, c.adaptive_threshold = 24, 0.05
        c.denoising_quality = "FAST"
        depth = "8"
    else:
        # 128 with the denoiser is clean on a phone feed and renders in about
        # 20 minutes on an M1 Max; --samples 512 for the cleanest blacks
        # (about an hour).
        c.samples = samples or 128
        c.adaptive_threshold, c.adaptive_min_samples = 0.01, min(64, c.samples // 2)
        c.denoising_quality = "HIGH"
        c.denoising_prefilter = "ACCURATE"
        depth = "16"

    im = r.image_settings
    if hasattr(im, "media_type"):
        im.media_type = "IMAGE"
    im.file_format = "PNG"
    im.color_mode = "RGB"
    im.color_depth = depth

    vs = scene.view_settings
    vs.view_transform = "AgX"
    try:
        vs.look = "AgX - Medium High Contrast"
    except TypeError:
        pass
    vs.exposure = EXPOSURE
    build_world()


def build_world():
    """A soft studio for the foil to mirror when it tilts away from the
    card: bright above, a soft strip across the horizon, dimmer below. Seen
    only in reflections, so it lights neither the wall nor the paper."""
    scene = bpy.context.scene
    world = bpy.data.worlds.new("Studio")
    scene.world = world
    if world.node_tree is None:
        world.use_nodes = True
    g = Nodes(world.node_tree)
    out = g.new("ShaderNodeOutputWorld")
    bg = g.new("ShaderNodeBackground")
    g.t.links.new(bg.outputs["Background"], out.inputs["Surface"])
    sep = g.new("ShaderNodeSeparateXYZ")
    g.feed(sep.inputs[0], g.new("ShaderNodeTexCoord").outputs["Generated"])
    z = sep.outputs["Z"]
    sky = g.map_range(z, -0.6, 0.9, 0.2, 1.0, smooth=True)
    strip = g.math("POWER", g.math("MAXIMUM", g.math("SUBTRACT", 1.0, g.math("ABSOLUTE", g.math("MULTIPLY", g.math("SUBTRACT", z, 0.25), 6.0))), 0.0), 2.0)
    level = g.math("ADD", g.math("MULTIPLY_ADD", sky, STUDIO, 0.02), g.math("MULTIPLY", strip, STUDIO * 0.8))
    glossy = g.new("ShaderNodeLightPath").outputs["Is Glossy Ray"]
    g.feed(bg.inputs["Color"], (1.0, 1.0, 1.0, 1.0))
    g.feed(bg.inputs["Strength"], g.math("MULTIPLY", level, glossy))


# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------


def check(sticker, cam):
    """Walk every frame: the sticker clears the wall once free, is wholly out
    of frame from GONE, and doesn't jump when it lets go."""
    scene = bpy.context.scene
    tan_v = tan_half_v()
    tan_h = (SENSOR / 2) / LENS
    ok, prev, speeds = True, None, []
    print(" frame  wall_gap_mm  in_frame  max_speed_m_s")
    for f in range(0, LAST + 1):
        scene.frame_set(f)
        pts = evaluated_points(sticker)
        gap = -pts[:, 1].max()
        m = np.array(cam.matrix_world.inverted())
        pc = pts @ m[:3, :3].T + m[:3, 3]
        z = -pc[:, 2]
        inside = (z > 0) & (np.abs(pc[:, 0] / z) <= tan_h) & (np.abs(pc[:, 1] / z) <= tan_v)
        speed = 0.0 if prev is None else float(np.linalg.norm(pts - prev, axis=1).max() * FPS)
        prev = pts
        speeds.append(speed)
        if f % 10 == 0 or f in (RELEASE - 1, RELEASE, RELEASE + 1, GONE):
            print(f" {f:5d}  {gap * 1000:11.2f}  {int(inside.sum()):8d}  {speed:13.3f}")
        if f > RELEASE and gap < 0.0003:
            print(f"FAIL frame {f}: {gap * 1000:.2f} mm from the wall")
            ok = False
        if f >= GONE and inside.any():
            print(f"FAIL frame {f}: {int(inside.sum())} vertices still in frame")
            ok = False
    around = speeds[RELEASE - 3:RELEASE + 4]
    print("speeds around release:", ", ".join(f"{s:.3f}" for s in around))
    return ok


# ---------------------------------------------------------------------------


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    mode = "final" if "--final" in argv else "preview"
    samples = int(argv[argv.index("--samples") + 1]) if "--samples" in argv else None
    stills = []
    if "--stills" in argv:
        stills = [int(x) for x in argv[argv.index("--stills") + 1].split(",")]

    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.fps = FPS
    scene.frame_start, scene.frame_end = 0, LAST
    parts, stamp = load_parts()
    t0 = time.time()
    maps = height_maps(parts, stamp)
    sticker, _ = build_sticker(maps, parts)
    wall = build_wall()
    foil, walls = bpy.data.collections.new("Lit: sticker"), bpy.data.collections.new("Lit: wall")
    foil.objects.link(sticker)
    walls.objects.link(wall)
    build_card(foil)
    build_lights(foil, walls)
    cam = build_camera()
    render_settings(mode, samples)
    _, c0, v_t = build_flight(sticker)
    build_focus(cam, c0, v_t)
    print(f"scene built in {time.time() - t0:.1f}s ({mode})")

    out = OUT / mode
    if "--save-blend" in argv:
        for img in maps.values():
            img.pack()
        bpy.ops.wm.save_as_mainfile(filepath=str(OUT / "teaser.blend"), relative_remap=True)
    if "--check" in argv and not check(sticker, cam):
        sys.exit(1)
    for f in stills:
        scene.frame_set(f)
        scene.render.filepath = str(out / "stills" / f"f{f:04d}.png")
        t1 = time.time()
        bpy.ops.render.render(write_still=True)
        print(f"still {f} in {time.time() - t1:.1f}s")
    if "--anim" in argv:
        (out / "frames").mkdir(parents=True, exist_ok=True)
        scene.render.filepath = str(out / "frames") + "/"
        bpy.ops.render.render(animation=True)


main()
