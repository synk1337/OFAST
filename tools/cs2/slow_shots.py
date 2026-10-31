"""Frames of a demo playing in slow motion, from a running CS2: an effect's animation in CS2's own time, to time
Unreal's against (an HE blast from its flash to its last smoke, a molotov's burst).

    python tools/cs2/slow_shots.py <out folder> <first tick> <demo seconds> [timescale]

CS2 started with `-insecure -netconport 2121` and playing the demo (see netcon.py, pov_shots.py). The demo is sought
to `first tick` (pov_shots.seek), played at `timescale` (`demo_timescale`, 0.25 unless given) for `demo seconds` of
the demo while grab_window's capture runs in a loop (grab_run.ps1: about 30 frames a real second, so at 0.25 about
one every half tick), then paused and set back to full speed. The frames are `<out>/run_NNN.jpg`; `<out>/times.txt`
has each one's time since the first, in real seconds, and the demo tick that makes at `timescale` from the tick the
seek reported (64 ticks a second; about right: the demo bar on each frame shows the tick drawn).

Why not tick_shots.py: each `demo_step_tick` moves CS2's particles about 3 ticks further than the demo (2026-10-31:
round 17's HE at the mid doors and its molotov at the top of mid, stepped one tick at a time, reached in 22 steps what
they reach in about 92 ticks of playback), so stepped frames show an effect's stages in the right order but about 4
times too fast. Played, the particles run in the demo's own time.
"""

import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pov_shots import Console, seek  # noqa: E402

GRAB_RUN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "grab_run.ps1")
TICKS_PER_SECOND = 64.0
FRAMES_PER_SECOND = 30.0


def main():
    out, first, seconds = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
    timescale = float(sys.argv[4]) if len(sys.argv) > 4 else 0.25
    os.makedirs(out, exist_ok=True)
    console = Console()
    got = seek(console, first)
    console.run("demo_timescale %g" % timescale, 1.0)
    console.run("demo_resume", 0.1)
    count = int(seconds / timescale * FRAMES_PER_SECOND) + 1
    try:
        grabbed = subprocess.run(["powershell", "-NoProfile", "-File", GRAB_RUN, "-OutDir", out, "-Count", str(count)],
                                 check=True, capture_output=True, text=True).stdout
    finally:
        console.run("demo_pause", 0.5)
        console.run("demo_timescale 1", 0.5)
    with open(os.path.join(out, "times.txt"), "w", encoding="utf-8") as times:
        for line in grabbed.splitlines():
            n, at = line.split()
            tick = got + float(at) * timescale * TICKS_PER_SECOND
            times.write("run_%03d.jpg %s %.1f\n" % (int(n), at, tick))
    print("%d frames from demo tick %s at %g speed -> %s" % (count, got, timescale, out))


if __name__ == "__main__":
    main()
