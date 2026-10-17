"""How dark (or bright) are grey marks on a surface — Mod2x grime, floor arrows, tyre marks — in CS2 and Unreal shots?

    python tools/compare/mark_contrast.py --box 0.30,0.62,0.60,0.80 test-data/shots/view_checks/cs2/cs2_de_nuke_asite.jpg \
        test-data/shots/overlay_blend/de_nuke_asite.png [--share 0.05] [--bright] [--crop out.png]

For each image the box (fractions of the width and height: left, top, right, bottom; the same in every image, so the
shots must share their framing, as CS2's setpos views and import_map.py's -Views cameras do) is split by linear
luminance into
- mark: the darkest --share of its pixels (default 5 %; --bright: the brightest, for marks that lighten),
- ground: the median pixel of the box (the surface the marks lie on, when they cover less than half of it),
and the script prints both mean sRGB colours and the ratio of their linear luminance (mark / ground). Marks and
ground lie on one surface and get the same light, so the light cancels out of the ratio: it measures what the
overlay does to the surface (a Mod2x overlay multiplies it by 2 x its colour), comparable between two renderers whose
exposure differs a little. A plain surface without marks gives the spread of its own texture (often 0.6-0.8 for the
darkest 5 %): compare a box against the same box of the other image, not against 1.

overlay_contrast.py is the counterpart for coloured overlays (road paint), picked by hue.

--crop writes the boxes side by side, labelled, to look at.

Used for decision 010 (docs/decisions/010-static-overlay-blend-modes.md).
"""

import argparse

import numpy as np
from PIL import Image, ImageDraw


def srgb_to_linear(c):
    c = np.asarray(c, dtype=np.float64) / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def luminance(srgb):
    linear = srgb_to_linear(srgb)
    return linear[..., 0] * 0.2126 + linear[..., 1] * 0.7152 + linear[..., 2] * 0.0722


def crop(image, box):
    width, height = image.size
    x0, y0, x1, y1 = box
    return image.crop((int(x0 * width), int(y0 * height), int(x1 * width), int(y1 * height)))


def measure(image, box, share, bright):
    """(mark sRGB, ground sRGB, mark / ground linear luminance) of one box of an image."""
    pixels = np.asarray(crop(image, box), dtype=np.float64).reshape(-1, 3)
    order = np.argsort(luminance(pixels))
    count = max(1, int(len(pixels) * share))
    mark = pixels[order[-count:] if bright else order[:count]].mean(axis=0)
    ground = pixels[order[len(order) // 2]]
    return mark, ground, luminance(mark) / luminance(ground)


def floats(text, count):
    values = [float(v) for v in text.split(",")]
    if len(values) != count:
        raise argparse.ArgumentTypeError("expected %d comma-separated numbers" % count)
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("images", nargs="+")
    parser.add_argument("--box", required=True, type=lambda t: floats(t, 4), help="left,top,right,bottom (0-1)")
    parser.add_argument("--share", type=float, default=0.05, help="share of the box's pixels that are the mark")
    parser.add_argument("--bright", action="store_true", help="the marks lighten: take the brightest pixels")
    parser.add_argument("--crop", help="write the boxes side by side to this image")
    args = parser.parse_args()

    crops = []
    for path in args.images:
        image = Image.open(path).convert("RGB")
        mark, ground, ratio = measure(image, args.box, args.share, args.bright)
        name = path.replace("\\", "/").split("/")[-1]
        print("%-40s mark (%3d, %3d, %3d) | ground (%3d, %3d, %3d) | luminance ratio %.2f"
              % (name[-40:], *mark, *ground, ratio))
        if args.crop:
            part = crop(image, args.box)
            part = part.resize((640, round(640 * part.height / part.width)), Image.LANCZOS)
            ImageDraw.Draw(part).text((4, 4), name, fill=(255, 0, 255))
            crops.append(part)
    if crops:
        out = Image.new("RGB", (sum(c.width + 8 for c in crops) - 8, max(c.height for c in crops)))
        x = 0
        for part in crops:
            out.paste(part, (x, 0))
            x += part.width + 8
        out.save(args.crop)


if __name__ == "__main__":
    main()
