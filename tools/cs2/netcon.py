"""Send console commands to a running CS2 and print what its console answers meanwhile.

    python tools/cs2/netcon.py [--wait=SECONDS] "command" ["command" ...]

CS2 started with `-netconport 2121` (`steam.exe -applaunch 730 -insecure -novid -console -netconport 2121 …`) opens a
plain-text console on that local TCP port: each line sent runs as if typed, and the console's output streams back. No
keyboard focus is needed, so nothing (an input panel, another window) can swallow the keys, unlike driving the console
by key presses. Waits 1.5 s for the output by default; a demo skip (`demo_gototick`) needs about 10 s, a
`demo_step_tick 64` about 5.
"""

import socket
import sys
import time

PORT = 2121


def main():
    wait = 1.5
    commands = sys.argv[1:]
    if commands and commands[0].startswith("--wait="):
        wait = float(commands[0][len("--wait="):])
        commands = commands[1:]
    s = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    s.settimeout(0.3)
    for command in commands:
        s.sendall((command + "\n").encode())
    end = time.time() + wait
    out = b""
    while time.time() < end:
        try:
            chunk = s.recv(65536)
            if not chunk:
                break
            out += chunk
        except socket.timeout:
            pass
    s.close()
    print(out.decode("utf-8", "replace"))


if __name__ == "__main__":
    main()
