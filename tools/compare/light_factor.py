"""Is a translucent surface (a decal, a sign, a fence card) lit like the opaque surface around it?

    python tools/compare/light_factor.py --albedo albedo.png --mask-from before.png after.png \
        [--box 0.55,0,1,0.88] [--mask-out mask.png] before=before.png perpixel=perpixel.png decal=decal.png \
        [--cs2 cs2.jpg]

Inputs are screenshots of one view with the same framing (shoot_views.py renders of a level, CS2 shots taken from
the same setpos / setang), scaled to 1920 x 1080 here:
- --mask-from A B: two renders that differ only in how the surface is drawn (e.g. before and after a what-if that
  changes its material). The surface's pixels are those whose linear luminance differs by more than 8 % (and 0.004)
  between A and B, inside the box (fractions of the frame: left, top, right, bottom; default the whole frame),
  opened by one pixel to drop noise. The rest of the box is the "ground", the opaque surface around it: choose the
  box so that it holds little else.
- --albedo: the base-colour render of the same view (shoot_views.py -Try with "console": ["ShowFlag.Lighting 0"]).
- label=render: the lit renders to measure.

For every lit render the script prints the light factor of the surface and of the ground (mean linear luminance
lit / base colour) and their ratio: 1.00 when the surface gets the same light as the ground, which is what CS2 does
for a decal or an overlay (they share the lightmap texel of the surface under them). --cs2 adds the luminance ratio
surface / ground in CS2's shot next to the same ratio in each render (CS2 has no base-colour render of these views,
so that comparison keeps the albedos in it).

Used for decision 010 (docs/decisions/010-translucent-decals-and-per-pixel-lighting.md).
"""

import argparse

import numpy as np
from PIL import Image, ImageFilter

SIZE = (1920, 1080)


def luminance(path):
    """Linear luminance of an sRGB image, scaled to SIZE."""
    c = np.asarray(Image.open(path).convert("RGB").resize(SIZE, Image.LANCZOS), dtype=np.float64) / 255.0
    c = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return c[..., 0] * 0.2126 + c[..., 1] * 0.7152 + c[..., 2] * 0.0722


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--albedo", required=True)
    parser.add_argument("--mask-from", nargs=2, required=True, metavar=("A", "B"))
    parser.add_argument("--box", default="0,0,1,1")
    parser.add_argument("--mask-out")
    parser.add_argument("--cs2")
    parser.add_argument("renders", nargs="+", metavar="label=render")
    args = parser.parse_args()

    a, b = (luminance(p) for p in args.mask_from)
    diff = np.abs(a - b)
    surface = (diff > 0.08 * np.maximum(a, b)) & (diff > 0.004)
    x0, y0, x1, y1 = (float(v) for v in args.box.split(","))
    box = np.zeros(surface.shape, bool)
    box[int(y0 * SIZE[1]):int(y1 * SIZE[1]), int(x0 * SIZE[0]):int(x1 * SIZE[0])] = True
    surface &= box
    opened = Image.fromarray((surface * 255).astype(np.uint8)).filter(ImageFilter.MinFilter(3))
    surface = np.asarray(opened.filter(ImageFilter.MaxFilter(3))) > 0
    ground = box & ~surface
    if args.mask_out:
        Image.fromarray((surface * 255).astype(np.uint8)).save(args.mask_out)
    if not surface.any() or not ground.any():
        raise SystemExit("no surface or no ground pixels in the box")
    albedo = luminance(args.albedo)
    print("surface %d px, ground %d px; base colour surface %.3f, ground %.3f"
          % (surface.sum(), ground.sum(), albedo[surface].mean(), albedo[ground].mean()))
    if args.cs2:
        cs2 = luminance(args.cs2)
        print("  %-10s surface / ground luminance %.2f" % ("cs2", cs2[surface].mean() / cs2[ground].mean()))
    for item in args.renders:
        label, path = item.split("=", 1)
        lit = luminance(path)
        surface_factor = lit[surface].mean() / albedo[surface].mean()
        ground_factor = lit[ground].mean() / albedo[ground].mean()
        print("  %-10s light factor surface %.3f, ground %.3f, ratio %.2f; surface / ground luminance %.2f"
              % (label, surface_factor, ground_factor, surface_factor / ground_factor,
                 lit[surface].mean() / lit[ground].mean()))


if __name__ == "__main__":
    main()
