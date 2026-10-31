"""Tick-by-tick frames of a demo from a running CS2: an effect's animation as CS2 draws it (an HE blast from its first
frame to its smoke), to compare Unreal's frame by frame.

    python tools/cs2/tick_shots.py <out folder> <first tick> <frames> ["<player>"] [step]

CS2 started with `-insecure -netconport 2121` and playing the demo (see netcon.py, pov_shots.py). The demo is paused
and stepped forward to `first tick`, or sought back to it when it is past it (pov_shots.seek), the player spectated in first person when named (a demo recorded locally plays
through its recorder's eyes already), then stepped one tick at a time (`demo_step_tick 1`; `step` ticks at a time
when given: a molotov's seven seconds), CS2's own picture grabbed after each step (grab_window.ps1) as
`<out>/tick_<n>.png`, n counting from 0 at `first tick` (frame n is `first tick` + n × step). CS2's particles stand
still while the demo is paused, but each step moves them about 3 ticks further than the demo (2026-10-31): the frames
show an effect's stages in order and how it looks, not when; for its timing use slow_shots.py (a stepped "+N" one
tick at a time is about +4.2 N of the demo's time; 8 at a time, +1.4 N). The demo bar on the picture shows the tick
drawn (one after the tick asked for, in the references of 2026-10-30): read it on the first frame.
"""

import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pov_shots import GRAB, Console, demo_tick, seek  # noqa: E402


def main():
    out, first, frames = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    player = sys.argv[4] if len(sys.argv) > 4 and sys.argv[4] else None
    step = int(sys.argv[5]) if len(sys.argv) > 5 else 1
    os.makedirs(out, exist_ok=True)
    console = Console()
    # Where the paused demo is: ahead of it, step there (a forward demo_gototick while paused does not move);
    # behind it, seek back (pov_shots.seek).
    console.run("demo_pause", 1.0)
    got = demo_tick(console.run("demo_step_tick 1", 2.0))
    if got is not None and got < first:
        got = demo_tick(console.run("demo_step_tick %d" % (first - got), 8.0)) or got
    if got != first:
        got = seek(console, first)
    if player:
        console.run('spec_player "%s"' % player, 1.0)
        console.run("spec_mode 1", 2.0)
    for n in range(frames):
        if n > 0:
            got = demo_tick(console.run("demo_step_tick %d" % step, 1.5 + step * 0.05)) or got
        path = os.path.join(out, "tick_%03d.png" % n)
        subprocess.run(["powershell", "-NoProfile", "-File", GRAB, "-Out", path], check=True,
                       stdout=subprocess.DEVNULL)
        print("frame %d: demo tick %s -> %s" % (n, got, path))


if __name__ == "__main__":
    main()
