"""Frames of a demo through players' eyes at given ticks, from a running CS2: the references to compare a round's
first-person views (and what they look at: effects, skins) with Unreal's `shoot_sequence.py -PovShots`.

    python tools/cs2/pov_shots.py <out folder> 152448:FalleN 152828:soulfly …

CS2 started with `-insecure -netconport 2121` and playing the demo (`+playdemo replays/<demo>`): see netcon.py. For each
`tick:name`, the demo is paused, sought to 64 ticks before and stepped to the tick (`demo_gototick`, `demo_step_tick`;
the console's "demo tick" line is read back and the step repeated until it is the tick), the player spectated in first
person (`spec_player "<name>"`, `spec_mode 1`), and CS2's own picture grabbed (grab_window.ps1) as
`<out>/cs2_<tick>_<name>.png`. CS2 draws the frame of one or two ticks before the console's tick (the demo bar shows
which: of 14 frames on 2026-10-30, some − 1, some − 2); read the bar on the picture before comparing a moment that
lasts a tick or two (a muzzle flash), and shoot Unreal at that tick.

Capture at CS2's default brightness (`r_fullscreen_gamma 2.2`, set back afterwards) or pass the setting to the
comparison tools as `--cs2-gamma`.
"""

import os
import re
import socket
import subprocess
import sys
import time

PORT = 2121
GRAB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "grab_window.ps1")


class Console:
    def __init__(self):
        self.socket = socket.create_connection(("127.0.0.1", PORT), timeout=5)
        self.socket.settimeout(0.3)

    def run(self, command, wait=1.5):
        """Runs `command` and returns what the console printed within `wait` seconds."""
        self.socket.sendall((command + "\n").encode())
        end, out = time.time() + wait, b""
        while time.time() < end:
            try:
                data = self.socket.recv(65536)
                if data:
                    out += data
            except socket.timeout:
                pass
        return out.decode("utf-8", "replace")


def demo_tick(text):
    found = re.findall(r"demo tick (\d+)", text)
    return int(found[-1]) if found else None


def seek(console, tick):
    """Pauses the demo at `tick` exactly; returns the tick the console reports."""
    console.run("demo_pause", 1.0)
    console.run("demo_gototick %d" % (tick - 64), 10.0)
    got = demo_tick(console.run("demo_step_tick 64", 6.0))
    for _ in range(4):
        if got is None or got == tick:
            break
        if abs(got - tick) > 64:
            # The jump had not finished (a long one): jump again, waiting longer.
            console.run("demo_gototick %d" % (tick - 64), 20.0)
            got = demo_tick(console.run("demo_step_tick 64", 6.0))
        elif got < tick:
            got = demo_tick(console.run("demo_step_tick %d" % (tick - got), 6.0)) or got
        else:
            console.run("demo_gototick %d" % (tick - 128), 10.0)
            got = demo_tick(console.run("demo_step_tick 128", 6.0))
    return got


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    console = Console()
    for item in sys.argv[2:]:
        tick, name = item.split(":", 1)
        got = seek(console, int(tick))
        console.run('spec_player "%s"' % name, 1.0)
        console.run("spec_mode 1", 2.0)
        path = os.path.join(out, "cs2_%s_%s.png" % (tick, name))
        subprocess.run(["powershell", "-NoProfile", "-File", GRAB, "-Out", path], check=True,
                       stdout=subprocess.DEVNULL)
        print("%s: demo tick %s -> %s" % (item, got, path))


main()
