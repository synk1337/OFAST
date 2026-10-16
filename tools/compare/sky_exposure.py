"""Measure a screenshot's effective exposure on pure-sky pixels, whose radiance is known from OFAST's sky export.

    python tools/compare/sky_exposure.py test-data/exports/de_mirage/de_mirage_sky.hdr \
        test-data/shots/cs2_de_mirage_ct0.jpg 331 0 1100,40 1150,70 950,12

Arguments: the exported `<map>_sky.hdr`, a screenshot, the camera yaw and pitch as typed in CS2's `setang`, then
pixel positions (x,y in the screenshot) that show nothing but sky. The camera is assumed to have CS2's default
field of view (90 degrees at 4:3, i.e. 106.26 degrees horizontally at 16:9), like OFAST's Unreal screenshots.

For each pixel the script looks up the sky radiance in that view direction and prints displayed linear value /
radiance. With no tone curve this ratio *is* the exposure the picture was taken with, so running it on a CS2
screenshot and on the matching Unreal one tells whether the two exposures agree, independently of any lighting.
This is how OFAST found that CS2 shows `exposure x scene` (de_mirage, sunlit: 0.80 = minexposure) and not
`exposure x scene / white_point` (docs/formats/lighting.md). The sky's own bloom slightly lifts the darker
channels in CS2 (red more than blue), so read the blue channel first.
"""

import math
import sys

import numpy as np
from PIL import Image


def read_hdr(path):
    """Radiance .hdr reader for the run-length encoded files OFAST writes (export/sky.rs)."""
    data = open(path, "rb").read()
    header_end = data.index(b"\n\n") + 2
    line_end = data.index(b"\n", header_end)
    dims = data[header_end:line_end].split()  # b"-Y", height, b"+X", width
    h, w = int(dims[1]), int(dims[3])
    pos = line_end + 1
    img = np.zeros((h, w, 4), dtype=np.uint8)
    for y in range(h):
        if data[pos] != 2 or data[pos + 1] != 2:
            raise ValueError("only run-length encoded scanlines are supported")
        pos += 4
        for c in range(4):
            x = 0
            while x < w:
                n = data[pos]
                if n > 128:
                    img[y, x : x + n - 128, c] = data[pos + 1]
                    x += n - 128
                    pos += 2
                else:
                    img[y, x : x + n, c] = list(data[pos + 1 : pos + 1 + n])
                    x += n
                    pos += 1 + n
    exponent = img[..., 3].astype(int)
    # value = (mantissa + 0.5) / 256 * 2^(e - 128)
    scale = np.where(exponent > 0, np.ldexp(1.0, exponent - 136), 0.0)
    return (img[..., :3].astype(np.float64) + 0.5) * scale[..., None]


def srgb_to_linear(c):
    c = c / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def main():
    if len(sys.argv) < 6:
        sys.exit(__doc__)
    sky = read_hdr(sys.argv[1])
    shot = np.asarray(Image.open(sys.argv[2]).convert("RGB"), dtype=np.float64)
    yaw, pitch = math.radians(float(sys.argv[3])), math.radians(float(sys.argv[4]))
    height, width = shot.shape[:2]
    tan_x = math.tan(math.radians(106.26 / 2))
    tan_y = tan_x * height / width
    # Source axes: forward from yaw/pitch (positive pitch looks down), right-handed with Z up.
    forward = np.array([math.cos(pitch) * math.cos(yaw), math.cos(pitch) * math.sin(yaw), -math.sin(pitch)])
    right = np.array([math.sin(yaw), -math.cos(yaw), 0.0])
    up = np.cross(right, forward)
    ratios = []
    for arg in sys.argv[5:]:
        px, py = (int(v) for v in arg.split(","))
        xn = 2 * (px + 0.5) / width - 1
        yn = 1 - 2 * (py + 0.5) / height
        d = forward + xn * tan_x * right + yn * tan_y * up
        d /= np.linalg.norm(d)
        # Equirect layout of export/sky.rs, in Unreal axes (Source y flipped).
        ue = np.array([d[0], -d[1], d[2]])
        u = 0.5 + math.atan2(ue[0], -ue[1]) / (2 * math.pi)
        v = math.acos(max(-1.0, min(1.0, ue[2]))) / math.pi
        radiance = sky[min(int(v * sky.shape[0]), sky.shape[0] - 1), int(u * sky.shape[1]) % sky.shape[1]]
        patch = shot[max(py - 2, 0) : py + 3, max(px - 2, 0) : px + 3].reshape(-1, 3).mean(axis=0)
        ratio = srgb_to_linear(patch) / radiance
        ratios.append(ratio)
        print("pixel %4d,%4d  sRGB %s  radiance %s  display/radiance %s" % (px, py, patch.round(), radiance.round(3), ratio.round(3)))
    print("mean display/radiance (R, G, B): %s" % np.mean(ratios, axis=0).round(3))


if __name__ == "__main__":
    main()
