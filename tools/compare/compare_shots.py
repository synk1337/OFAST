"""Compare a CS2 screenshot with the matching Unreal screenshot, region by region.

    python tools/compare/compare_shots.py test-data/shots/cs2_de_mirage_ct0.jpg test-data/shots/de_mirage_ct0.png \
        test-data/shots/compare_de_mirage_ct0.png

Both images are scaled to the same size and cut into a grid (default 4 x 3). For every cell the script prints the
mean sRGB colour of each image and the ratio of their mean *linear* luminance (Unreal / CS2): 1.00 means the same
brightness, 0.80 means Unreal is 20 % darker. The output image shows CS2 on top, Unreal below, with the grid and the
ratios drawn on the Unreal half, so lighting differences can be located at a glance.

This is a calibration aid, not a pixel diff: the two renderers never line up to the pixel, and cells are big enough
that small framing differences do not matter.

OFAST reproduces CS2's default picture. CS2's brightness setting (`r_fullscreen_gamma`, default 2.2, in
`cs2_machine_convars.vcfg`) raises CS2's whole picture to the power gamma / 2.2: take references at 2.2, or pass the
value they were taken with as `--cs2-gamma` and the CS2 shot is brought back to the default picture first
(docs/formats/lighting.md).
"""

import argparse

import numpy as np
from PIL import Image, ImageDraw


def srgb_to_linear(c):
    c = c / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def luminance(linear_rgb):
    return linear_rgb[..., 0] * 0.2126 + linear_rgb[..., 1] * 0.7152 + linear_rgb[..., 2] * 0.0722


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("cs2")
    parser.add_argument("unreal")
    parser.add_argument("output")
    parser.add_argument("--cols", type=int, default=4)
    parser.add_argument("--rows", type=int, default=3)
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--cs2-gamma", type=float, default=2.2,
                        help="r_fullscreen_gamma the CS2 shot was taken with (default 2.2, CS2's own)")
    args = parser.parse_args()

    cs2 = Image.open(args.cs2).convert("RGB")
    height = round(args.width * cs2.height / cs2.width)
    cs2 = cs2.resize((args.width, height), Image.LANCZOS)
    if args.cs2_gamma != 2.2:
        default = (np.asarray(cs2, dtype=np.float64) / 255.0) ** (2.2 / args.cs2_gamma)
        cs2 = Image.fromarray((default * 255.0 + 0.5).astype(np.uint8))
    ue = Image.open(args.unreal).convert("RGB").resize((args.width, height), Image.LANCZOS)
    a = np.asarray(cs2, dtype=np.float64)
    b = np.asarray(ue, dtype=np.float64)
    la, lb = luminance(srgb_to_linear(a)), luminance(srgb_to_linear(b))

    out = Image.new("RGB", (args.width, height * 2))
    out.paste(cs2, (0, 0))
    out.paste(ue, (0, height))
    draw = ImageDraw.Draw(out)

    print("cell      CS2 sRGB            Unreal sRGB         luminance ratio (UE/CS2)")
    ratios = []
    for r in range(args.rows):
        for c in range(args.cols):
            x0, x1 = c * args.width // args.cols, (c + 1) * args.width // args.cols
            y0, y1 = r * height // args.rows, (r + 1) * height // args.rows
            ma = a[y0:y1, x0:x1].reshape(-1, 3).mean(axis=0)
            mb = b[y0:y1, x0:x1].reshape(-1, 3).mean(axis=0)
            ratio = lb[y0:y1, x0:x1].mean() / max(la[y0:y1, x0:x1].mean(), 1e-6)
            ratios.append(ratio)
            print(
                "r%d c%d   (%3.0f, %3.0f, %3.0f)     (%3.0f, %3.0f, %3.0f)     %.2f"
                % (r, c, *ma, *mb, ratio)
            )
            draw.rectangle([x0, y0 + height, x1 - 1, y1 - 1 + height], outline=(255, 0, 255))
            draw.text((x0 + 6, y0 + height + 6), "%.2f" % ratio, fill=(255, 0, 255))
    whole = lb.mean() / max(la.mean(), 1e-6)
    print("whole image luminance ratio %.3f (median cell %.3f)" % (whole, float(np.median(ratios))))
    out.save(args.output)


if __name__ == "__main__":
    main()
