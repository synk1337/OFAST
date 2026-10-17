"""How strong is a coloured overlay (road paint, a sign) against the surface under it, in CS2 and Unreal shots?

    python tools/compare/overlay_contrast.py --box 0.18,0.62,0.45,1.0 test-data/shots/cs2_de_nuke_ct0.jpg \
        test-data/shots/de_nuke_ct0.png [--hue 20,75] [--crop out.png]

For each image the box (fractions of the width and height: left, top, right, bottom; the same in every image, so the
shots must share their framing, as CS2's and import_map.py's spawn views do) is split into
- paint: the most saturated 5 % of its pixels whose hue is in --hue (degrees; default 20-75, yellow),
- ground: the median colour of its least saturated half,
and the script prints both mean sRGB colours, the paint's hue and HSV saturation, and the ratio of their linear
luminance (paint / ground). When paint and ground get the same light, the light cancels out of that ratio, so it
compares the blend itself between two renderers whose exposure differs a little.

--albedo PAINT_SRGB (e.g. 237,179,63) treats the images as base-colour renders (CS2 mat_fullbright 1, Unreal
ShowFlag.Lighting 0) and also solves blend = k * a * paint + (1 - a) * ground for the effective opacity a, by least
squares over the three channels (linear values). k is the render's scale of the base colour (Unreal's view keeps the
scene's exposure; CS2's fullbright should give 1); the residual says how well a plain alpha blend explains the
colour (sRGB steps).

--crop writes the boxes side by side, labelled, to look at.

Used for decision 009 (docs/decisions/009-overlays-as-mesh-decals.md).
"""

import argparse
import colorsys

import numpy as np
from PIL import Image, ImageDraw


def srgb_to_linear(c):
    c = np.asarray(c, dtype=np.float64) / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def luminance(srgb):
    linear = srgb_to_linear(srgb)
    return linear[..., 0] * 0.2126 + linear[..., 1] * 0.7152 + linear[..., 2] * 0.0722


def measure(image, box, hue_range):
    """(paint sRGB, paint hue in degrees, paint saturation, ground sRGB) of one box of an image."""
    width, height = image.size
    x0, y0, x1, y1 = box
    pixels = np.asarray(image.crop((int(x0 * width), int(y0 * height), int(x1 * width), int(y1 * height))),
                        dtype=np.float64).reshape(-1, 3)
    high, low = pixels.max(axis=1), pixels.min(axis=1)
    saturation = np.where(high > 0, (high - low) / np.maximum(high, 1), 0)
    r, g, b = pixels[:, 0], pixels[:, 1], pixels[:, 2]
    hue = (np.degrees(np.arctan2(np.sqrt(3) * (g - b), 2 * r - g - b)) + 360) % 360
    candidates = np.where((hue > hue_range[0]) & (hue < hue_range[1]))[0]
    if len(candidates) == 0:
        raise SystemExit("no pixel with a hue in %s in the box" % (hue_range,))
    top = candidates[np.argsort(saturation[candidates])[-max(1, len(pixels) // 20):]]
    paint = pixels[top].mean(axis=0)
    ground = np.median(pixels[np.argsort(saturation)[: len(pixels) // 2]], axis=0)
    h, s, _ = colorsys.rgb_to_hsv(*(paint / 255.0))
    return paint, h * 360, s, ground


def floats(text, count):
    values = [float(v) for v in text.split(",")]
    if len(values) != count:
        raise argparse.ArgumentTypeError("expected %d comma-separated numbers" % count)
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("images", nargs="+")
    parser.add_argument("--box", required=True, type=lambda t: floats(t, 4), help="left,top,right,bottom (0-1)")
    parser.add_argument("--hue", default=[20.0, 75.0], type=lambda t: floats(t, 2), help="paint hue range, degrees")
    parser.add_argument("--albedo", type=lambda t: floats(t, 3), help="paint colour (sRGB bytes) for albedo shots")
    parser.add_argument("--crop", help="write the boxes side by side to this image")
    args = parser.parse_args()

    crops = []
    for path in args.images:
        image = Image.open(path).convert("RGB")
        paint, hue, saturation, ground = measure(image, args.box, args.hue)
        line = "%-40s paint (%3d, %3d, %3d) hue %3.0f sat %.2f | ground (%3d, %3d, %3d) | luminance ratio %.2f" % (
            path.replace("\\", "/").split("/")[-1][-40:], *paint, hue, saturation, *ground,
            luminance(paint) / luminance(ground))
        if args.albedo:
            # blend = u * paint + v * ground with u = k * a, v = 1 - a (the observed ground is k * the true one).
            blend, g = srgb_to_linear(paint), srgb_to_linear(ground)
            basis = np.stack([srgb_to_linear(args.albedo), g], axis=1)
            (u, v), *_ = np.linalg.lstsq(basis, blend, rcond=None)
            alpha = 1.0 - v
            fitted = np.clip(basis @ np.array([u, v]), 0, 1)
            fitted_srgb = np.where(fitted <= 0.0031308, fitted * 12.92, 1.055 * fitted ** (1 / 2.4) - 0.055) * 255
            line += " | opacity %.2f, scale %.2f, residual %.1f" % (
                alpha, u / alpha if alpha > 1e-6 else float("nan"), np.abs(fitted_srgb - paint).max())
        print(line)
        if args.crop:
            width, height = image.size
            x0, y0, x1, y1 = args.box
            crop = image.crop((int(x0 * width), int(y0 * height), int(x1 * width), int(y1 * height)))
            crop = crop.resize((640, round(640 * crop.height / crop.width)), Image.LANCZOS)
            ImageDraw.Draw(crop).text((4, 4), path.replace("\\", "/").split("/")[-1], fill=(255, 0, 255))
            crops.append(crop)
    if crops:
        out = Image.new("RGB", (sum(c.width + 8 for c in crops) - 8, max(c.height for c in crops)))
        x = 0
        for crop in crops:
            out.paste(crop, (x, 0))
            x += crop.width + 8
        out.save(args.crop)


if __name__ == "__main__":
    main()
