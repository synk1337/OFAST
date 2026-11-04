"""Side views of one player's legs over the ground, frame by frame, from an export's OFAST_FEET_DUMP file
(crates/ofast-core/src/anim/feet.rs): to see the feet on stairs and slopes without Unreal.

    OFAST_FEET_DUMP=feet.txt ofast export-demo match.dem -r 14 --players -o out
    python tools/compare/feet_views.py feet.txt X Y FIRST LAST out_prefix [step]

Picks the player whose origin is nearest (X, Y) on frame FIRST (frames count from the round's first tick), draws
frames FIRST..LAST (every `step`): the ground along the way the player goes (grey), the legs before the feet were
placed (pale) and after (left blue, right red), the pelvis (dot) and the player's origin (black cross), 5 pixels an
inch, the grid lines 8 inches apart. Writes <out>_grid.png (all panels) and <out>.gif (animated, 16 frames a second).
Needs PIL (Pillow).
"""
import math
import sys

from PIL import Image, ImageDraw

path, OUT = sys.argv[1], sys.argv[6]
X, Y = float(sys.argv[2]), float(sys.argv[3])
FIRST, LAST = int(sys.argv[4]), int(sys.argv[5])
STEP = int(sys.argv[7]) if len(sys.argv) > 7 else 1

players = []
last = None
for line in open(path):
    parts = line.split("|")
    head = parts[0].split()
    f = int(head[0])
    if last is None or f <= last:
        players.append({})
    last = f
    after = [float(v) for v in head[1:]]
    before = [float(v) for v in parts[1].split()]
    origin = [float(v) for v in parts[2].split()]
    profile = [float(v) for v in parts[3].split()]
    direction = [float(v) for v in parts[4].split()]
    players[-1][f] = (after, before, origin, profile, direction)


def nearest(p):
    frame = p.get(FIRST)
    if frame is None:
        return math.inf
    o = frame[2]
    return math.hypot(o[0] - X, o[1] - Y)


player = min(players, key=nearest)
frames = [f for f in range(FIRST, LAST + 1, STEP) if f in player]
o0 = player[frames[0]][2]
o1 = player[frames[-1]][2]
dx, dy = o1[0] - o0[0], o1[1] - o0[1]
length = math.hypot(dx, dy) or 1.0
ref = (dx / length, dy / length)

SCALE = 5.0
W, H = 360, 540


def s_of(p):
    return (p[0] - o0[0]) * ref[0] + (p[1] - o0[1]) * ref[1]


panels = []
for f in frames:
    after, before, origin, profile, direction = player[f]
    img = Image.new("RGB", (W, H), (255, 255, 255))
    d = ImageDraw.Draw(img)
    cs, cz = s_of(origin), origin[2] + 12.0

    def px(s, z):
        return (W / 2 + (s - cs) * SCALE, H / 2 - (z - cz) * SCALE)

    # Grid lines every 8 inches of height, for the steps.
    for k in range(-10, 11):
        z = math.floor(cz / 8.0) * 8.0 + 8.0 * k
        y = px(0, z)[1]
        d.line([(0, y), (W, y)], fill=(238, 238, 238))
    # The ground along the way.
    pts = []
    for k, z in enumerate(profile):
        s = s_of([origin[0] + direction[0] * (k - 20) * 2.0, origin[1] + direction[1] * (k - 20) * 2.0])
        if not math.isnan(z):
            pts.append(px(s, z))
        elif len(pts) > 1:
            d.line(pts, fill=(120, 120, 120), width=3)
            pts = []
    if len(pts) > 1:
        d.line(pts, fill=(120, 120, 120), width=3)

    def legs(values, colours, width):
        p = [values[i:i + 3] for i in range(0, len(values), 3)]
        pelvis = p[0]
        for side, colour in zip(((1, 2, 3, 4), (5, 6, 7, 8)), colours):
            chain = [pelvis] + [p[i] for i in side]
            d.line([px(s_of(q), q[2]) for q in chain], fill=colour, width=width)
        x, y = px(s_of(pelvis), pelvis[2])
        d.ellipse([x - 4, y - 4, x + 4, y + 4], fill=colours[0])

    legs(before, ((170, 190, 235), (240, 175, 175)), 2)
    legs(after, ((30, 70, 200), (200, 30, 30)), 3)
    x, y = px(cs, origin[2])
    d.line([(x - 6, y), (x + 6, y)], fill=(0, 0, 0), width=2)
    d.line([(x, y - 6), (x, y + 6)], fill=(0, 0, 0), width=2)
    d.text((6, 6), "frame %d  z %.1f" % (f, origin[2]), fill=(0, 0, 0))
    panels.append(img)

cols = 8
rows = math.ceil(len(panels) / cols)
grid = Image.new("RGB", (cols * W, rows * H), (255, 255, 255))
for i, p in enumerate(panels):
    grid.paste(p, ((i % cols) * W, (i // cols) * H))
grid.save(OUT + "_grid.png")
panels[0].save(OUT + ".gif", save_all=True, append_images=panels[1:], duration=62, loop=0)
print("drew", len(panels), "frames of the player at", o0)
