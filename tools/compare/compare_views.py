"""Compare every view of a map that has a CS2 reference, with compare_shots.py, and print one line per view.

    python tools/compare/compare_views.py [--cs2-gamma 2.013074] <cs2 dir> <unreal dir> <out dir> de_nuke [de_dust2 ...]

For each view in tools/compare/views/<map>.json it looks for `<cs2 dir>/cs2_<map>_<view>.jpg` (Steam F12 captures,
renamed) and `<unreal dir>/<map>_<view>.png` (import_map.py or shoot_views.py screenshots), writes
`<out dir>/compare_<map>_<view>.png` and prints the whole-image luminance ratio (Unreal / CS2), the median cell
and the darkest and brightest cells of compare_shots.py's 4 × 3 grid. Views without both pictures are listed as
missing. The same lines are written to `<out dir>/results.json` for tables (docs/checks.md).

--cs2-gamma: CS2's brightness setting (`r_fullscreen_gamma`, default 2.2) the references were taken at. CS2 shows its
picture raised to gamma / 2.2 (both sRGB-encoded, docs/formats/lighting.md); OFAST reproduces the default picture,
so the references are turned back into it first (shown ^ (2.2 / gamma), written as `<out dir>/cs2/…png`).
"""

import json
import os
import re
import subprocess
import sys

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))


def default_picture(path, gamma, out_dir):
    """The reference as CS2 shows it at its default brightness (r_fullscreen_gamma 2.2)."""
    if gamma == 2.2:
        return path
    shown = np.asarray(Image.open(path).convert("RGB"), dtype=np.float64) / 255.0
    corrected = np.clip(shown ** (2.2 / gamma), 0.0, 1.0)
    os.makedirs(os.path.join(out_dir, "cs2"), exist_ok=True)
    out = os.path.join(out_dir, "cs2", os.path.splitext(os.path.basename(path))[0] + ".png")
    Image.fromarray(np.round(corrected * 255.0).astype(np.uint8)).save(out)
    return out


def main():
    args = sys.argv[1:]
    gamma = 2.2
    if args[:1] == ["--cs2-gamma"]:
        gamma, args = float(args[1]), args[2:]
    if len(args) < 4:
        sys.exit(__doc__)
    cs2_dir, ue_dir, out_dir, maps = args[0], args[1], args[2], args[3:]
    os.makedirs(out_dir, exist_ok=True)
    rows = []
    for map_name in maps:
        with open(os.path.join(HERE, "views", map_name + ".json"), encoding="utf-8") as f:
            views = json.load(f)
        for view in views:
            cs2 = os.path.join(cs2_dir, "cs2_%s_%s.jpg" % (map_name, view["name"]))
            ue = os.path.join(ue_dir, "%s_%s.png" % (map_name, view["name"]))
            if not (os.path.exists(cs2) and os.path.exists(ue)):
                print("%-10s %-13s missing %s" % (map_name, view["name"], cs2 if not os.path.exists(cs2) else ue))
                continue
            out = os.path.join(out_dir, "compare_%s_%s.png" % (map_name, view["name"]))
            cs2 = default_picture(cs2, gamma, out_dir)
            text = subprocess.run(
                [sys.executable, os.path.join(HERE, "compare_shots.py"), cs2, ue, out],
                capture_output=True, text=True, check=True,
            ).stdout
            cells = [float(v) for v in re.findall(r"\)\s+([\d.]+)\s*$", text, re.M)]
            whole = re.search(r"whole image luminance ratio ([\d.]+) \(median cell ([\d.]+)\)", text)
            row = {
                "map": map_name, "view": view["name"], "whole": float(whole.group(1)),
                "median_cell": float(whole.group(2)), "min_cell": min(cells), "max_cell": max(cells),
            }
            rows.append(row)
            print("%-10s %-13s whole %.2f  median cell %.2f  cells %.2f - %.2f"
                  % (map_name, view["name"], row["whole"], row["median_cell"], row["min_cell"], row["max_cell"]))
    with open(os.path.join(out_dir, "results.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(rows, f, indent=1)


if __name__ == "__main__":
    main()
