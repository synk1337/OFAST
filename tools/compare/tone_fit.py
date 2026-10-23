"""Fit and check CS2's tone rule (exposure, tone curve, colour-correction table) on screenshots.

CS2 shows a map as (docs/formats/lighting.md, "Exposure and tone mapping"):

    shown sRGB = LUT( sRGB( curve(exposure x scene) ) )
    curve(x)   = H(k x) / H(white x W)       Hable maps (measured: k = 2.85, white = 4.2)
    curve(x)   = clamp(x, 0, 1)              linear maps (a .vpost shoulder of 0)

That is CS2's default picture. Its brightness setting (`r_fullscreen_gamma`, default 2.2, kept in Steam's
`userdata/<id>/730/local/cfg/cs2_machine_convars.vcfg`) then raises the shown picture to gamma / 2.2: take CS2 shots
at 2.2, or pass the value they were taken with as --cs2-gamma (this machine's: 2.013074).

H is the .vpost's Hable curve and W its white point (the manifest's `post.tonemap`), LUT its 32^3 colour-correction
table (`<map>_lut.png`, the manifest's `post.color_correction`). OFAST's Unreal tone mapper (import_map.py,
M_OFAST_Tonemap_v3) draws the same rule; its HABLE_INPUT_SCALE and HABLE_WHITE_SCALE are k and white here.

Every subcommand inverts screenshots through a rule (the table in 3D by Newton's method, then sRGB, then the curve)
back to `exposure x scene` per pixel and channel, and compares that with what it should be:

    pair      a CS2 exposure pair of one view -> white (k cancels): every pixel must scale by the exposure ratio
    sky       CS2 sky pixels, whose radiance the exported sky gives -> k (with --fit: k and white together)
    lit       lit surfaces, CS2 against Unreal -> k x CS2 scene / Unreal scene per cell (= k if OFAST's light is right)
    light     light = lit / albedo in each engine (CS2 mat_fullbright 1, Unreal ShowFlag.Lighting 0 + Specular 0)
    simulate  re-tone an Unreal shot to another rule: what a rule change gives, without importing again
    table     the colour table's grey diagonal and its largest change

A rule is OFAST's (`ofast`, the default; the colour table on Hable maps only, like import_map.py) with comma-separated
changes: `table=0` (no colour table), `table=1` (the table, also on a linear map), `k=3.4,white=6`, `curve=linear` or
`curve=hable`. `--rule` is CS2's (the hypothesis under test), `--ue-rule` the one Unreal's shots
were rendered with, `--unlit-rule` that of Unreal's ShowFlag.Lighting 0 render (default: `--ue-rule`). Linear curves
take k = 1 unless the rule sets it. `ofast` is whatever import_map.py draws now: Unreal shots made before a rule
change need the rule they were made with (2026-10-16 night: `k=3.4,white=6`; before that `k=2.5,white=1.25,table=0`).

Examples, de_anubis CT spawn (CS2 locked at 0.7 and 0.35 with mat_tonemap_force_scale; shots in test-data/shots,
taken at brightness 2.013074, so add --cs2-gamma 2.013074 to each):

    python tools/compare/tone_fit.py pair -m test-data/exports/de_anubis cs2_de_anubis_ct0.jpg 0.7 \
        cs2_de_anubis_ct0_e035.jpg 0.35
    python tools/compare/tone_fit.py sky -m test-data/exports/de_anubis --view tools/cs2/cfg/ofast_v_anubis_ct.cfg \
        --unreal de_anubis_ct0.png cs2_de_anubis_ct0.jpg 0.7 cs2_de_anubis_ct0_e035.jpg 0.35 --fit
    python tools/compare/tone_fit.py lit -m test-data/exports/de_anubis cs2_de_anubis_ct0.jpg 0.7 de_anubis_ct0.png
    python tools/compare/tone_fit.py light -m test-data/exports/de_anubis cs2_de_anubis_ct0.jpg 0.7 \
        cs2_de_anubis_ct0_albedo.jpg de_anubis_ct0.png de_anubis_ct0_unlit.png --unlit-rule k=2.5,white=1.25,table=0
    python tools/compare/tone_fit.py simulate -m test-data/exports/de_nuke de_nuke_ct0.png out.png --rule table=1 \
        --compare cs2_de_nuke_ct0.jpg

The shots of one command must show the same view: CS2's `setpos`/`setang` = the Unreal camera, CS2's default field of
view (90 degrees at 4:3). Unreal's shots are scaled to CS2's size. CS2 exposures are the `mat_tonemap_force_scale`
values the shots were taken at; Unreal's is the manifest's (its minimum exposure) unless --ue-exposure says otherwise,
and its base-colour render (ShowFlag.Lighting 0 with ShowFlag.Specular 0: Lighting 0 alone adds 0.45 x the specular
colour) runs at exposure 1. CS2's mat_fullbright shows the albedo directly (no exposure, curve or table; measured on
de_anubis), unless --fb-table says the table applies.
"""

import argparse
import ast
import glob
import json
import math
import os
import re
import sys

import numpy as np
from PIL import Image

from sky_exposure import read_hdr

IMPORT_MAP = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "unreal", "import_map.py")
LUMA = np.array([0.2126, 0.7152, 0.0722])
# Hable's own parameters (most official maps' .vpost), for `curve=hable` on a map whose curve is linear.
STOCK_HABLE = {"shoulder_strength": 0.15, "linear_strength": 0.5, "linear_angle": 0.1, "toe_strength": 0.2,
               "toe_numerator": 0.02, "toe_denominator": 0.3, "white_point": 4.0}
# Shown levels (of 255) closer than this to what a channel's darkest / brightest input gives are not inverted: the
# picture is clipped there and JPEG noise alone moves them by a level or two.
FLOOR_LEVELS, CEILING_LEVELS = 3, 2
# CS2's default r_fullscreen_gamma (the in-game brightness setting); see load_cs2.
DEFAULT_GAMMA = 2.2


def ofast_scales():
    """import_map.py's HABLE_INPUT_SCALE and HABLE_WHITE_SCALE, read from its source (it only runs inside Unreal), so
    that `ofast` is always the rule the current import draws."""
    values = {"HABLE_INPUT_SCALE": 2.85, "HABLE_WHITE_SCALE": 4.2}
    try:
        tree = ast.parse(open(IMPORT_MAP, encoding="utf-8").read())
    except OSError:
        return values
    for node in tree.body:
        if (isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)
                and node.targets[0].id in values):
            values[node.targets[0].id] = float(ast.literal_eval(node.value))
    return values


def srgb_dec(v):
    return np.where(v <= 0.04045, v / 12.92, ((np.maximum(v, 0.0) + 0.055) / 1.055) ** 2.4)


def srgb_enc(c):
    c = np.clip(c, 0.0, 1.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


def load_image(path, size=None):
    """RGB in 0..1 as float, scaled (Lanczos) to `size` = (width, height) if given."""
    image = Image.open(path).convert("RGB")
    if size and image.size != tuple(size):
        image = image.resize(tuple(size), Image.LANCZOS)
    return np.asarray(image, dtype=np.float64) / 255.0


def load_cs2(path, gamma, size=None):
    """A CS2 shot as CS2's default picture. CS2's brightness setting (`r_fullscreen_gamma`, default 2.2; this
    machine's 2.013) raises the final, encoded picture to gamma / 2.2 (measured: shots at 2.013 = shots at 2.2 to the
    power 0.916 at every brightness), so a shot taken at `gamma` is brought back with the power 2.2 / gamma."""
    shot = load_image(path, size)
    return shot if gamma == DEFAULT_GAMMA else shot ** (DEFAULT_GAMMA / gamma)


def save_image(path, rgb):
    Image.fromarray((np.clip(rgb, 0.0, 1.0) * 255.0 + 0.5).astype(np.uint8)).save(path)


# --- The colour-correction table -----------------------------------------------------------------------------------


def load_lut(path, n):
    """The exported table (<map>_lut.png: n blue slices side by side, pixel (b n + r, g) = output for input (r, g, b);
    the exporter's lut_strip) as an array [b, g, r, channel] in 0..1."""
    strip = np.asarray(Image.open(path).convert("RGB"), dtype=np.float64) / 255.0
    if strip.shape[:2] != (n, n * n):
        sys.exit("%s is %dx%d, not a %d^3 table strip" % (path, strip.shape[1], strip.shape[0], n))
    return strip.reshape(n, n, n, 3).transpose(1, 0, 2, 3)


def lut_lookup(lut, v, jacobian=False):
    """Trilinear lookup of inputs v (N, 3) in 0..1, like CS2 (and OFAST's Unreal tone mapper). With `jacobian`, also
    the derivatives d output / d input (N, 3 outputs, 3 inputs), exact inside each cell of the table."""
    n = lut.shape[0]
    p = np.clip(v, 0.0, 1.0) * (n - 1)
    i0 = np.minimum(np.floor(p).astype(np.int64), n - 2)
    f = p - i0
    r0, g0, b0 = i0[:, 0], i0[:, 1], i0[:, 2]
    fr, fg, fb = f[:, 0:1], f[:, 1:2], f[:, 2:3]
    corner = {(db, dg, dr): lut[b0 + db, g0 + dg, r0 + dr] for db in (0, 1) for dg in (0, 1) for dr in (0, 1)}
    along_r = {(db, dg): corner[db, dg, 0] + fr * (corner[db, dg, 1] - corner[db, dg, 0]) for db in (0, 1) for dg in (0, 1)}
    along_g = {db: along_r[db, 0] + fg * (along_r[db, 1] - along_r[db, 0]) for db in (0, 1)}
    value = along_g[0] + fb * (along_g[1] - along_g[0])
    if not jacobian:
        return value

    def lerp_gb(d):  # interpolate per-(b, g) quantities along g, then b
        e = {db: d[db, 0] + fg * (d[db, 1] - d[db, 0]) for db in (0, 1)}
        return e[0] + fb * (e[1] - e[0])

    d_r = lerp_gb({(db, dg): corner[db, dg, 1] - corner[db, dg, 0] for db in (0, 1) for dg in (0, 1)})
    d_g_b = {db: along_r[db, 1] - along_r[db, 0] for db in (0, 1)}
    d_g = d_g_b[0] + fb * (d_g_b[1] - d_g_b[0])
    d_b = along_g[1] - along_g[0]
    return value, np.stack([d_r, d_g, d_b], axis=-1) * (n - 1)


def lut_invert(lut, shown):
    """Table input for each shown pixel (..., 3), and per channel whether it is determined (False where the table
    clips: within FLOOR_LEVELS / CEILING_LEVELS of what that channel's darkest / brightest input gives, the other two
    held; or where Newton's method did not reach the pixel to 1.5 levels). Starts from the grey diagonal's inverse."""
    flat = shown.reshape(-1, 3)
    n = lut.shape[0]
    steps = np.arange(n) / (n - 1)
    v_all, ok_all = np.empty_like(flat), np.empty(flat.shape, dtype=bool)
    for start in range(0, len(flat), 1 << 18):
        s = flat[start:start + (1 << 18)]
        v = np.empty_like(s)
        for c in range(3):
            out = lut[np.arange(n), np.arange(n), np.arange(n), c]
            rising = np.concatenate([[True], np.diff(np.maximum.accumulate(out)) > 0])
            v[:, c] = np.interp(s[:, c], out[rising], steps[rising])
        for _ in range(12):
            value, jac = lut_lookup(lut, v, jacobian=True)
            jt = jac.transpose(0, 2, 1)
            normal = jt @ jac + 1e-6 * np.eye(3)
            v = np.clip(v - np.linalg.solve(normal, (jt @ (value - s)[..., None]))[..., 0], 0.0, 1.0)
        ok = np.abs(lut_lookup(lut, v) - s) < 1.5 / 255
        for c in range(3):
            for edge, margin in ((0.0, FLOOR_LEVELS), (1.0, -CEILING_LEVELS)):
                held = v.copy()
                held[:, c] = edge
                bound = lut_lookup(lut, held)[:, c] + margin / 255
                ok[:, c] &= s[:, c] > bound if edge == 0.0 else s[:, c] < bound
        v_all[start:start + len(s)], ok_all[start:start + len(s)] = v, ok
    return v_all.reshape(shown.shape), ok_all.reshape(shown.shape)


# --- Maps and rules ------------------------------------------------------------------------------------------------


class Map:
    """What the tool needs from an export: the manifest's curve, exposure and sky, and the colour table."""

    def __init__(self, path):
        if os.path.isdir(path):
            found = glob.glob(os.path.join(path, "*.ofast.json"))
            if len(found) != 1:
                sys.exit("expected one *.ofast.json in %s, found %d" % (path, len(found)))
            path = found[0]
        folder = os.path.dirname(os.path.abspath(path))
        manifest = json.load(open(path, encoding="utf-8"))
        self.name = manifest.get("map") or os.path.basename(path).split(".")[0]
        post = manifest.get("post") or {}
        self.tonemap = {key: value for key, value in (post.get("tonemap") or {}).items() if value is not None}
        self.hable = self.tonemap.get("shoulder_strength", 0.0) > 0.0
        if self.tonemap.get("exposure_bias"):
            print("note: the .vpost's exposure bias (%g) is not modelled" % self.tonemap["exposure_bias"])
        table = post.get("color_correction") or {}
        self.lut = load_lut(os.path.join(folder, table["lut"]), table.get("size", 32)) if table.get("lut") else None
        # A CS2 scene value of 1 is a luminance of lux_per_unit / pi in Unreal, which shows luminance x 2^-EV100.
        unit = manifest.get("lux_per_unit", math.pi) / math.pi
        self.exposure = unit * 2.0 ** -post.get("ev100_max", 0.0)  # Unreal's fixed exposure (CS2's minexposure)
        self.min_exposure = post.get("min_exposure")
        sky = manifest.get("sky") or {}
        self.sky_hdr = os.path.join(folder, sky["hdr"]) if sky.get("hdr") else None
        # The visible sky is the .hdr x sky_intensity in Unreal luminance; in CS2 scene units, divided by `unit`.
        self.sky_scale = sky.get("sky_intensity", 1.0) / unit


class Rule:
    """shown sRGB = LUT(sRGB(curve(x))), x = exposure x scene: OFAST's rule for a map, changed by `spec` (see the
    module doc)."""

    def __init__(self, m, spec="ofast"):
        scales = ofast_scales()
        # import_map.py's setup_post applies the table on Hable maps only (decided 2026-10-18; CS2 applies it on
        # linear maps too, measured on de_nuke: docs/formats/lighting.md). `table=1` puts it back on.
        self.hable = m.hable
        self.lut = m.lut if m.hable else None
        self.k, self.white = scales["HABLE_INPUT_SCALE"], scales["HABLE_WHITE_SCALE"]
        k_given = False
        for item in filter(None, (part.strip() for part in (spec or "").split(","))):
            key, _, value = item.partition("=")
            if key == "ofast" and not value:
                continue
            if key == "k":
                self.k, k_given = float(value), True
            elif key == "white":
                self.white = float(value)
            elif key == "table":
                self.lut = None if value.lower() in ("0", "off", "no", "false") else m.lut
            elif key == "curve" and value in ("hable", "linear"):
                self.hable = value == "hable"
            else:
                sys.exit("rule %r: unknown change %r (k=, white=, table=0|1, curve=hable|linear)" % (spec, item))
        if not self.hable and not k_given:
            self.k = 1.0
        params = m.tonemap if m.hable else STOCK_HABLE
        self.params = [params.get(key, STOCK_HABLE[key]) for key in
                       ("shoulder_strength", "linear_strength", "linear_angle", "toe_strength", "toe_numerator",
                        "toe_denominator")]
        self.white_point = params.get("white_point", 4.0)

    def describe(self):
        curve = "H(%.3g x) / H(%.3g W), W = %.3g" % (self.k, self.white, self.white_point) if self.hable else \
            "clamp(%.3g x)" % self.k
        return "%s, %s" % (curve, "table" if self.lut is not None else "no table")

    def H(self, u):
        a, b, c, d, e, f = self.params
        return (u * (a * u + c * b) + d * e) / (u * (a * u + b) + d * f) - e / f

    def curve(self, x):
        if not self.hable:
            return np.clip(self.k * x, 0.0, 1.0)
        return np.clip(self.H(self.k * x) / self.H(self.white * self.white_point), 0.0, 1.0)

    def curve_inverse(self, t):
        """Curve output t (0..1) -> x. t = 1 gives the white (x where the curve reaches 1)."""
        if not self.hable:
            return t / self.k
        top = self.white * self.white_point
        u = np.concatenate([[0.0], np.geomspace(1e-6, top, 20000)])
        return np.interp(t * self.H(top), self.H(u), u) / self.k

    def forward(self, x):
        """x = exposure x scene (..., 3) -> shown sRGB (0..1)."""
        shown = srgb_enc(self.curve(x))
        if self.lut is None:
            return shown
        return lut_lookup(self.lut, shown.reshape(-1, 3)).reshape(shown.shape)

    def curve_value(self, shown):
        """Shown sRGB -> curve output t (linear, 0..1) and whether it is determined (table and sRGB clips)."""
        if self.lut is None:
            v = shown
            ok = (shown > FLOOR_LEVELS / 255) & (shown < 1.0 - CEILING_LEVELS / 255)
        else:
            v, ok = lut_invert(self.lut, shown)
        return srgb_dec(v), ok

    def inverse(self, shown, fill=False):
        """Shown sRGB -> x = exposure x scene; NaN where not determined (clipped black or white), unless `fill`, which
        keeps the nearest value the rule can give (for re-toning)."""
        t, ok = self.curve_value(shown)
        x = self.curve_inverse(np.clip(t, 0.0, 1.0))
        if fill:
            return x
        return np.where(ok & (t < 0.995), x, np.nan)


# --- Helpers -------------------------------------------------------------------------------------------------------


def grid_cells(h, w, rows=3, cols=4):
    for r in range(rows):
        for c in range(cols):
            yield r, c, (slice(r * h // rows, (r + 1) * h // rows), slice(c * w // cols, (c + 1) * w // cols))


def print_grid(values, rows=3, cols=4, fmt="%5.2f"):
    for r in range(rows):
        print("    " + " ".join(fmt % values[r, c] if np.isfinite(values[r, c]) else "  -  " for c in range(cols)))


def parse_view(text):
    """CS2 view angles (pitch, yaw): a cfg with a `setang pitch yaw roll` line (tools/cs2/cfg/ofast_v_*.cfg) or
    "pitch,yaw"."""
    if os.path.isfile(text):
        match = re.search(r"^\s*setang\s+(\S+)\s+(\S+)", open(text, encoding="utf-8").read(), re.M)
        if not match:
            sys.exit("no setang line in " + text)
        return float(match.group(1)), float(match.group(2))
    pitch, yaw = (float(v) for v in text.split(","))
    return pitch, yaw


def radiance_map(sky, w, h, pitch, yaw):
    """The exported sky's radiance seen through each pixel of a CS2 view (default field of view: 90 degrees at 4:3,
    the same in OFAST's Unreal shots). Same mapping as sky_exposure.py, for the whole image."""
    pitch, yaw = math.radians(pitch), math.radians(yaw)
    tan_y = 0.75  # tan(45 degrees) x 3/4: the 4:3 field of view's vertical half-angle, kept at any width
    tan_x = tan_y * w / h
    fwd = np.array([math.cos(pitch) * math.cos(yaw), math.cos(pitch) * math.sin(yaw), -math.sin(pitch)])
    right = np.array([math.sin(yaw), -math.cos(yaw), 0.0])
    up = np.cross(right, fwd)
    xs, ys = np.meshgrid(2 * (np.arange(w) + 0.5) / w - 1, 1 - 2 * (np.arange(h) + 0.5) / h)
    d = fwd + xs[..., None] * tan_x * right + ys[..., None] * tan_y * up
    d /= np.linalg.norm(d, axis=2, keepdims=True)
    # export/sky.rs's equirect layout, in Unreal axes (Source y flipped).
    u = 0.5 + np.arctan2(d[..., 0], d[..., 1]) / (2 * math.pi)
    v = np.arccos(np.clip(d[..., 2], -1.0, 1.0)) / math.pi
    iy = np.minimum((v * sky.shape[0]).astype(int), sky.shape[0] - 1)
    ix = (u * sky.shape[1]).astype(int) % sky.shape[1]
    return sky[iy, ix]


def erode(mask, r=3):
    out = mask.copy()
    for dy, dx in ((r, 0), (-r, 0), (0, r), (0, -r), (1, 0), (-1, 0), (0, 1), (0, -1)):
        out &= np.roll(np.roll(mask, dy, 0), dx, 1)
    return out


def shot_pairs(values):
    if len(values) % 2:
        sys.exit("shots come as pairs: <shot> <exposure> ...")
    return [(values[i], float(values[i + 1])) for i in range(0, len(values), 2)]


def summary(values):
    if len(values) == 0:
        return "no pixels"
    return "%.3f (p25 %.3f, p75 %.3f, n %d)" % (np.median(values), np.percentile(values, 25),
                                                np.percentile(values, 75), len(values))


# --- Subcommands ---------------------------------------------------------------------------------------------------


def cmd_pair(args):
    """With the right rule, x = exposure x scene from the darker shot divided by the brighter's is the same at every
    brightness: the exposure ratio. Printed per brightness bin, with its spread (rms log deviation of the bins from
    their median: the shape test) and its level (median / exposure ratio: whether CS2 applied the forced exposures
    exactly). The ratio does not depend on k (it scales both shots), so a pair fixes the white, fitted on the spread:
    too low a white raises the bright end's ratios, too high a white lowers them. A table that CS2 does not apply, or
    one it does apply but the rule leaves out, shows as a slope at the dark end."""
    m = Map(args.manifest)
    hi = load_cs2(args.hi, args.cs2_gamma)
    lo = load_cs2(args.lo, args.cs2_gamma, hi.shape[1::-1])
    expected = args.e_lo / args.e_hi
    print("%s: %s at %g vs %s at %g; expected ratio %.3f" % (m.name, args.hi, args.e_hi, args.lo, args.e_lo, expected))
    rule = Rule(m, args.rule)
    variants = [rule]
    if m.lut is not None:
        other = Rule(m, args.rule)
        other.lut = None if rule.lut is not None else m.lut
        variants.append(other)
    rng = np.random.default_rng(0)
    for r in variants:
        t_hi, ok_hi = r.curve_value(hi)
        t_lo, ok_lo = r.curve_value(lo)
        ok = ok_hi & ok_lo & (t_hi < 0.995) & (t_lo < 0.995) & (t_lo > 0.0)
        t_hi, t_lo = t_hi[ok], t_lo[ok]
        if len(t_hi) > 150000:
            pick = rng.choice(len(t_hi), 150000, replace=False)
            t_hi, t_lo = t_hi[pick], t_lo[pick]
        # Brightness bins of equal pixel count, from the brighter shot's curve output (the same for every white).
        edges = np.quantile(t_hi, np.linspace(0, 1, 11))
        bins = np.clip(np.digitize(t_hi, edges[1:-1]), 0, 9)

        def by_bin(white):
            r.white = white
            ratio = r.curve_inverse(t_lo) / np.maximum(r.curve_inverse(t_hi), 1e-9)
            med = np.array([np.median(ratio[bins == i]) for i in range(10)])
            level = float(np.median(med))
            return med, float(np.sqrt(np.mean(np.log(med / level) ** 2))), level / expected, \
                float(np.sqrt(np.mean(np.log(med / expected) ** 2)))

        def line(med, spread, level, error):
            return "ratio by bin %s   spread %.4f, level %.3f, error %.4f" % (
                " ".join("%.3f" % v for v in med), spread, level, error)

        print("\n%s (%d channel values, brightness bins from dark to bright)" % (r.describe(), len(t_hi)))
        if not r.hable:
            print("  " + line(*by_bin(r.white)))
            continue
        scan = [(by_bin(w), w) for w in np.arange(1.0, 15.001, 0.1)]
        flat = [w for w in (1.25, 2.0, 4.0, 6.0, 8.0, 12.0)]
        for name, index in (("flattest", 1), ("closest to the exposure ratio", 3)):
            score, best = min((fit[index], w) for fit, w in scan)
            near = [w for fit, w in scan if fit[index] <= score * 1.25 + 0.002]
            flat.append(round(best, 2))
            print("  %s: white %.2f x W = %.1f (%.4f; within 25 %%: %.2f - %.2f x W)" % (
                name, best, best * r.white_point, score, min(near), max(near)))
        for white in sorted(set(flat)):
            print("  white %5.2f x W (%5.1f): %s" % (white, white * r.white_point, line(*by_bin(white))))


def cmd_sky(args):
    """Sky pixels (where Unreal's shot of the same view shows its own sky to 2 % in every channel) have a known scene
    value: the exported sky's radiance. Inverting CS2's shot with k = 1 gives k x exposure x radiance there."""
    m = Map(args.manifest)
    if not m.sky_hdr:
        sys.exit("the manifest has no sky")
    pitch, yaw = parse_view(args.view)
    shots = shot_pairs(args.shots)
    first = load_image(shots[0][0])
    h, w = first.shape[:2]
    radiance = radiance_map(read_hdr(m.sky_hdr), w, h, pitch, yaw) * m.sky_scale * args.sky_scale

    ue_rule = Rule(m, args.ue_rule)
    e_ue = args.ue_exposure or m.exposure
    pred = srgb_dec(ue_rule.forward(e_ue * radiance))
    ue = srgb_dec(load_image(args.unreal, (w, h)))
    match = np.all(np.abs(ue - pred) < 0.02 * np.maximum(pred, 0.05), axis=2)
    match &= np.all((pred > 0.01) & (pred < 0.95), axis=2)
    mask = erode(match)
    print("%s sky: %d of %d pixels (Unreal: %s, exposure %.3f)" % (m.name, mask.sum(), w * h, ue_rule.describe(), e_ue))
    if mask.sum() < 1000:
        print("  too few sky pixels: is the Unreal shot the same view, rendered with --ue-rule?")
        if mask.sum() == 0:
            return

    rule = Rule(m, args.rule)
    rule.k = 1.0
    print("CS2: %s (k measured)" % rule.describe())
    xs, ts = [], []
    for path, e in shots:
        shot = load_cs2(path, args.cs2_gamma, (w, h))
        t, ok = rule.curve_value(shot)
        x = rule.curve_inverse(np.clip(t, 0.0, 1.0))
        k = np.where(ok & (t < 0.995), x / (e * radiance), np.nan)
        print("  %s at %g" % (os.path.basename(path), e))
        for c, name in enumerate("RGB"):
            sel = mask & np.isfinite(k[..., c])
            print("    %s: k %s" % (name, summary(k[..., c][sel])))
        cells = np.full((3, 4), np.nan)
        for r, c, cell in grid_cells(h, w):
            sel = mask[cell] & np.all(np.isfinite(k[cell]), axis=2)
            if sel.sum() >= 200:
                cells[r, c] = np.median(k[cell][sel] @ LUMA)
        print("    k per cell (luminance; clouds that only one engine draws stand out):")
        print_grid(cells)
        sel = mask[..., None] & ok & (t > 0.03) & (t < 0.995)
        xs.append((e * radiance)[sel])
        ts.append(t[sel])

    if args.fit:
        x, t = np.concatenate(xs), np.concatenate(ts)
        if len(x) > 30000:
            pick = np.random.default_rng(0).choice(len(x), 30000, replace=False)
            x, t = x[pick], t[pick]
        print("\nfit over %d sky values (x = exposure x radiance %.2f - %.2f):" % (len(x), x.min(), x.max()))
        if not rule.hable:
            print("  linear: k = median(t / x) = %s" % summary(t / x))
            return
        rows = []
        for k in np.arange(1.0, 8.001, 0.05):
            best = (np.inf, 0.0)
            for white in np.arange(1.0, 15.001, 0.05):
                rule.k, rule.white = k, white
                best = min(best, (float(np.sqrt(np.mean(np.log(rule.curve(x) / t) ** 2))), white))
            rows.append((best[0], k, best[1]))
        err, k, white = min(rows)
        for e, kk, ww in rows[::10]:
            print("  k %.2f: best white %.2f x W, rms log error %.4f" % (kk, ww, e))
        print("  best k %.2f, white %.2f x W = %.1f (rms log error %.4f)" % (k, white, white * rule.white_point, err))


def cmd_lit(args):
    """Inverting CS2's shot with k = 1 gives k x exposure x CS2 scene; Unreal's, through its own rule, exposure x
    Unreal scene. Per cell: k x CS2 scene / Unreal scene, which is k where OFAST's light x albedo matches CS2."""
    m = Map(args.manifest)
    cs2 = load_cs2(args.cs2, args.cs2_gamma)
    h, w = cs2.shape[:2]
    rule = Rule(m, args.rule)
    rule.k = 1.0
    ue_rule = Rule(m, args.ue_rule)
    e_ue = args.ue_exposure or m.exposure
    k_scene = rule.inverse(cs2) / args.exposure
    ue_scene = ue_rule.inverse(load_image(args.unreal, (w, h))) / e_ue
    print("%s: CS2 %s at %g; Unreal %s at %.3f" % (m.name, rule.describe(), args.exposure, ue_rule.describe(), e_ue))
    ok = np.all(np.isfinite(k_scene) & np.isfinite(ue_scene) & (ue_scene > 0), axis=2)
    cells = np.full((3, 4), np.nan)
    for r, c, cell in grid_cells(h, w):
        sel = ok[cell]
        if sel.sum() >= 500:
            cells[r, c] = (k_scene[cell][sel] @ LUMA).mean() / (ue_scene[cell][sel] @ LUMA).mean()
    print("  k x CS2 scene / Unreal scene per cell (mean luminance of the pixels both engines resolve):")
    print_grid(cells)
    finite = cells[np.isfinite(cells)]
    whole = (k_scene[ok] @ LUMA).mean() / (ue_scene[ok] @ LUMA).mean()
    print("  median over cells %.3f (%.2f - %.2f), whole image %.3f, %d pixels" % (
        np.median(finite), finite.min(), finite.max(), whole, ok.sum()))


def cmd_light(args):
    """Light = lit scene / albedo in each engine. CS2: its lit shot through --rule, its mat_fullbright shot as the
    albedo (shown directly). Unreal: its lit shot through --ue-rule at its exposure, its base-colour render
    (ShowFlag.Lighting 0 and ShowFlag.Specular 0) through --unlit-rule at exposure 1. Per cell: both lights, their
    ratio, and base colour / albedo."""
    m = Map(args.manifest)
    cs2 = load_cs2(args.cs2, args.cs2_gamma)
    h, w = cs2.shape[:2]
    rule, ue_rule = Rule(m, args.rule), Rule(m, args.ue_rule)
    unlit_rule = Rule(m, args.unlit_rule or args.ue_rule)
    e_ue = args.ue_exposure or m.exposure
    s_c = rule.inverse(cs2) / args.exposure
    fb = load_cs2(args.fullbright, args.cs2_gamma, (w, h))
    if args.fb_table and m.lut is not None:
        v, ok = lut_invert(m.lut, fb)
        albedo = np.where(ok, srgb_dec(v), np.nan)
    else:
        albedo = np.where((fb > FLOOR_LEVELS / 255) & (fb < 0.98), srgb_dec(fb), np.nan)
    s_u = ue_rule.inverse(load_image(args.unreal, (w, h))) / e_ue
    base = unlit_rule.inverse(load_image(args.unlit, (w, h))) / args.unlit_exposure
    print("%s: CS2 %s at %g; Unreal %s at %.3f; unlit %s at %g" % (
        m.name, rule.describe(), args.exposure, ue_rule.describe(), e_ue, unlit_rule.describe(), args.unlit_exposure))
    with np.errstate(divide="ignore", invalid="ignore"):
        light_c, light_u = (s_c / albedo) @ LUMA, (s_u / base) @ LUMA
        base_ratio = (base @ LUMA) / (albedo @ LUMA)
    ok = np.isfinite(light_c) & np.isfinite(light_u) & np.isfinite(base_ratio)
    print("  cell   CS2 light  Unreal light  Unreal/CS2  base colour/albedo")
    ratios, bases = [], []
    for r, c, cell in grid_cells(h, w):
        sel = ok[cell]
        if sel.sum() < 2000:
            continue
        lc, lu, br = np.median(light_c[cell][sel]), np.median(light_u[cell][sel]), np.median(base_ratio[cell][sel])
        ratios.append(lu / lc)
        bases.append(br)
        print("  r%dc%d  %8.3f  %12.3f  %10.2f  %18.2f" % (r, c, lc, lu, lu / lc, br))
    if ratios:
        print("  median light Unreal/CS2 %.3f (%.2f - %.2f); base colour / albedo %.3f (%.2f - %.2f)" % (
            np.median(ratios), min(ratios), max(ratios), np.median(bases), min(bases), max(bases)))


def cmd_simulate(args):
    """Unreal's shot back to exposure x scene through --ue-rule, times --exposure-scale, then forward through --rule."""
    m = Map(args.manifest)
    ue = load_image(args.unreal)
    src, dst = Rule(m, args.ue_rule), Rule(m, args.rule)
    out = dst.forward(src.inverse(ue, fill=True) * args.exposure_scale)
    save_image(args.output, out)
    print("%s: %s -> %s (exposure x %g) -> %s" % (args.unreal, src.describe(), dst.describe(), args.exposure_scale,
                                                    args.output))
    if args.compare:
        cs2 = load_cs2(args.compare, args.cs2_gamma)
        size = cs2.shape[1::-1]
        lum = lambda img: (srgb_dec(img) @ LUMA).mean()  # noqa: E731
        base = lum(cs2)
        print("  whole image / CS2 (%s): as shot %.3f, simulated %.3f" % (
            os.path.basename(args.compare), lum(load_image(args.unreal, size)) / base,
            lum(np.asarray(Image.open(args.output).convert("RGB").resize(size, Image.LANCZOS)) / 255.0) / base))


def cmd_table(args):
    m = Map(args.manifest)
    if m.lut is None:
        print("%s has no colour table" % m.name)
        return
    lut = m.lut
    n = lut.shape[0]
    print("%s colour table, %d^3; grey diagonal, input -> output R G B (levels of 255):" % (m.name, n))
    for i in range(0, n, 2 if n > 16 else 1):
        print("  %5.1f -> %5.1f %5.1f %5.1f" % (i / (n - 1) * 255, *(lut[i, i, i] * 255)))
    steps = np.arange(n) / (n - 1)
    identity = np.stack(np.meshgrid(steps, steps, steps, indexing="ij"), axis=-1)[..., ::-1]  # [b, g, r] -> (r, g, b)
    change = np.abs(lut - identity) * 255
    b, g, r, c = np.unravel_index(np.argmax(change), change.shape)
    print("largest change %.0f levels, at input (%.0f, %.0f, %.0f) channel %s; grey diagonal's %.0f" % (
        change.max(), r / (n - 1) * 255, g / (n - 1) * 255, b / (n - 1) * 255, "RGB"[c],
        max(change[i, i, i].max() for i in range(n))))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                     formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    def command(name, func, help_text):
        p = sub.add_parser(name, help=help_text, description=(func.__doc__ or help_text).strip())
        p.set_defaults(func=func)
        p.add_argument("-m", "--manifest", required=True, help="<map>.ofast.json, or the export folder holding it")
        p.add_argument("--rule", default="ofast", help="CS2's rule (default: OFAST's)")
        p.add_argument("--ue-rule", default="ofast", help="the rule Unreal's shots were rendered with")
        p.add_argument("--ue-exposure", type=float, help="Unreal's exposure (default: the manifest's)")
        p.add_argument("--cs2-gamma", type=float, default=DEFAULT_GAMMA,
                       help="r_fullscreen_gamma the CS2 shots were taken with (default 2.2, CS2's own)")
        return p

    p = command("pair", cmd_pair, "white from a CS2 exposure pair of one view")
    p.add_argument("hi")
    p.add_argument("e_hi", type=float)
    p.add_argument("lo")
    p.add_argument("e_lo", type=float)

    p = command("sky", cmd_sky, "k from CS2 sky pixels of known radiance")
    p.add_argument("--view", required=True, help="cfg with the view's setang (tools/cs2/cfg), or 'pitch,yaw'")
    p.add_argument("--unreal", required=True, help="Unreal shot of the same view (finds the pure-sky pixels)")
    p.add_argument("--sky-scale", type=float, default=1.0, help="multiplies the exported sky (another visible sky)")
    p.add_argument("--fit", action="store_true", help="also fit k and white together")
    p.add_argument("shots", nargs="+", help="<CS2 shot> <exposure> pairs")

    p = command("lit", cmd_lit, "k x CS2 scene / Unreal scene per cell")
    p.add_argument("cs2")
    p.add_argument("exposure", type=float)
    p.add_argument("unreal")

    p = command("light", cmd_light, "light (lit / albedo) in both engines per cell")
    p.add_argument("cs2")
    p.add_argument("exposure", type=float)
    p.add_argument("fullbright", help="CS2 shot with mat_fullbright 1")
    p.add_argument("unreal")
    p.add_argument("unlit", help="Unreal shot with ShowFlag.Lighting 0 and ShowFlag.Specular 0 (shoot_views.py -Try)")
    p.add_argument("--unlit-rule", help="rule of the unlit render (default: --ue-rule)")
    p.add_argument("--unlit-exposure", type=float, default=1.0)
    p.add_argument("--fb-table", action="store_true", help="CS2's fullbright went through the colour table")

    p = command("simulate", cmd_simulate, "re-tone an Unreal shot from --ue-rule to --rule")
    p.add_argument("unreal")
    p.add_argument("output")
    p.add_argument("--exposure-scale", type=float, default=1.0, help="target exposure / Unreal's")
    p.add_argument("--compare", help="CS2 shot of the same view: print whole-image brightness ratios")

    command("table", cmd_table, "the colour table's grey diagonal and largest change")

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
