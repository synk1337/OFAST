"""Place CS2's demo free camera at a point and angle, for a reference view from anywhere during demo playback.

    python tools/cs2/free_cam.py X Y Z PITCH YAW

Needs CS2 started with `-netconport 2121` (see netcon.py) and the free camera on (`spec_mode 4`; `spec_mode 3` is the
chase camera, which follows the spectated player's eye angles). `spec_goto` and `setpos` only turn the free camera,
so it is flown: facing along one axis at a time (+x, -x, +y, -y, up, down), a `+forward` press of up to 0.25 s (it
moves about 1000 units a second), checked with `getpos`, again until within 2 units, each press halved after an
overshoot; then turned to the angle. Prints where it ended up, as `getpos` gives it (Source units and degrees): the
camera to project into, or to hand to `import_demo.py -ViewShots`; exits with 1 (and says so) when it could not get
within 2 units. The camera stays put across demo seeks. Its field of view is the default 90 at 4:3.
"""

import re
import socket
import sys
import time

PORT = 2121
TOLERANCE = 2.0


def main():
    target = [float(v) for v in sys.argv[1:4]]
    pitch, yaw = float(sys.argv[4]), float(sys.argv[5])
    s = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    s.settimeout(0.2)

    def drain(longest=2.0):
        # What the console sent until 0.2 s of quiet (or `longest` seconds, if it keeps talking).
        out, end = b"", time.time() + longest
        while time.time() < end:
            try:
                chunk = s.recv(65536)
                if not chunk:
                    break
                out += chunk
            except socket.timeout:
                break
        return out.decode("utf-8", "replace")

    def send(command):
        s.sendall((command + "\n").encode())

    def where():
        drain()
        send("getpos")
        answer, end = "", time.time() + 5
        while time.time() < end:
            answer += drain()
            found = re.findall(r"setpos (\S+) (\S+) (\S+);setang (\S+) (\S+)", answer)
            if found:
                return [float(v) for v in found[-1]]
        sys.exit("free_cam: no answer to getpos (is CS2 started with -netconport 2121?)")

    scale = {}
    p = where()
    for _ in range(60):
        delta = [target[k] - p[k] for k in range(3)]
        k = max(range(3), key=lambda k: abs(delta[k]))
        if abs(delta[k]) < TOLERANCE:
            break
        if k == 0:
            view = (0, 0 if delta[0] > 0 else 180)
        elif k == 1:
            view = (0, 90 if delta[1] > 0 else -90)
        else:
            view = (-89 if delta[2] > 0 else 89, 0)
        # Halve this axis' presses each time it overshoots (the sign of what is left flips).
        sign = delta[k] > 0
        last_sign, factor = scale.get(k, (sign, 1.0))
        factor = factor / 2 if sign != last_sign else factor
        scale[k] = (sign, factor)
        send(f"spec_goto {p[0]} {p[1]} {p[2]} {view[0]} {view[1]}")
        time.sleep(0.15)
        send("+forward")
        time.sleep(min(0.25, max(0.005, factor * abs(delta[k]) / 1500)))
        send("-forward")
        time.sleep(0.5)
        p = where()
    send(f"spec_goto {p[0]} {p[1]} {p[2]} {pitch} {yaw}")
    time.sleep(0.3)
    p = where()
    print("setpos {} {} {};setang {} {}".format(*p))
    off = max(abs(target[k] - p[k]) for k in range(3))
    if off >= TOLERANCE:
        sys.exit(f"free_cam: stopped {off:.1f} units from the target")


if __name__ == "__main__":
    main()
