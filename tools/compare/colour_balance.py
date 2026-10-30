"""Colour of Unreal's views against CS2's, view by view: balance (R/G, B/G) and brightness in the sun and in the
shade, and saturation (chroma over luminance).

    python tools/compare/colour_balance.py test-data/shots/view_checks/cs2 <unreal dir> de_dust2 --cs2-gamma 2.013074

The CS2 references are `cs2_<map>_<view>.jpg`, the Unreal shots `<map>_<view>.png` (shoot_views.py). Each pair is cut
into 16 × 9 cells; a cell is "sun" when CS2's linear luminance is above 0.25, "shade" below 0.12. Per cell, Unreal
over CS2: (R/G) / (R/G), (B/G) / (B/G) — a warmer light in Unreal reads R/G > 1, B/G < 1 —, luminance, and the
saturation ratio (|rgb − luminance| / luminance, on cells CS2 shows with some colour). Medians per view and over all.
The references' brightness setting is undone first (`--cs2-gamma`, docs/formats/lighting.md).

First run (2026-10-30, docs/checks.md "Colour against CS2"): de_dust2's sun 1.01 / 0.99, shade 1.02 / 0.97,
saturation 1.05.
"""

import argparse
import glob
import os

import numpy as np
from PIL import Image

LUMA = np.array([0.2126, 0.7152, 0.0722])
SUN, SHADE = 0.25, 0.12


def linear(srgb):
    c = srgb / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def load(path, gamma=None):
    """The picture's linear RGB at 1920 × 1080; with `gamma`, a CS2 shot brought back to the default picture."""
    a = np.asarray(Image.open(path).convert("RGB").resize((1920, 1080), Image.LANCZOS), dtype=np.float64)
    if gamma:
        a = 255.0 * (a / 255.0) ** (2.2 / gamma)
    return linear(a)


def cells(image, cols=16, rows=9):
    h, w, _ = image.shape
    return [image[r * h // rows:(r + 1) * h // rows, c * w // cols:(c + 1) * w // cols].reshape(-1, 3).mean(0)
            for r in range(rows) for c in range(cols)]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("cs2")
    parser.add_argument("unreal")
    parser.add_argument("map")
    parser.add_argument("--cs2-gamma", type=float, default=2.2)
    args = parser.parse_args()
    totals = {"sun": [], "shade": [], "saturation": []}
    for ref in sorted(glob.glob(os.path.join(args.cs2, "cs2_%s_*.jpg" % args.map))):
        view = os.path.basename(ref)[len("cs2_%s_" % args.map):-4]
        shot = os.path.join(args.unreal, "%s_%s.png" % (args.map, view))
        if "albedo" in view or not os.path.exists(shot):
            continue
        per = {"sun": [], "shade": [], "saturation": []}
        for a, b in zip(cells(load(ref, args.cs2_gamma)), cells(load(shot))):
            if min(a.min(), b.min()) < 0.004:
                continue
            la, lb = a @ LUMA, b @ LUMA
            key = "sun" if la > SUN else "shade" if la < SHADE else None
            if key:
                per[key].append(((b[0] / b[1]) / (a[0] / a[1]), (b[2] / b[1]) / (a[2] / a[1]), lb / la))
            sa, sb = np.linalg.norm(a - la) / la, np.linalg.norm(b - lb) / lb
            if sa > 0.08:
                per["saturation"].append(sb / sa)
        line = view.ljust(14)
        for key in ("sun", "shade"):
            if per[key]:
                line += "  %s: R/G %.2f B/G %.2f lum %.2f (%d)" % (key, *np.median(per[key], axis=0), len(per[key]))
        if per["saturation"]:
            line += "  saturation %.2f" % np.median(per["saturation"])
        print(line)
        for key in totals:
            totals[key] += per[key]
    for key in ("sun", "shade"):
        if totals[key]:
            print("ALL %s: R/G %.3f  B/G %.3f  lum %.3f  (%d cells)"
                  % (key, *np.median(totals[key], axis=0), len(totals[key])))
    if totals["saturation"]:
        print("ALL saturation: %.3f (%d cells)" % (np.median(totals["saturation"]), len(totals["saturation"])))


main()
