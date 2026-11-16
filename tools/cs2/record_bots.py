"""Record a demo of bots playing on the installed CS2 build: what a demo of today's CS2 looks like.

    python tools/cs2/record_bots.py [NAME] [SECONDS]

A CS2 patch can change how demos are written (1.41.9.7, 2026-11-11, stored one-byte fields with a new encoder,
`fixed8`, and stamped the pose recipes with a new version): demos recorded before the patch keep reading, so only a
demo of the new build shows it. This records one, by SourceTV as match demos are: ten players who run, shoot, die
and throw grenades for SECONDS (default 200: about two rounds of de_dust2), as `game/csgo/NAME.dem` (default
`ofast_bots`). Then `ofast demo-rounds`, `ofast export-demo --round 1 --players` and `examples/demo_inspect.rs`
(`encoders`) on it (docs/development.md, "After a CS2 update").

CS2 must run offline with its console on a port, in a map (nobody playing on it: the map is loaded again):

    steam.exe -applaunch 730 -insecure -novid -console -netconport 2121 +map de_dust2

SourceTV only starts with a map loaded while `tv_enable` is 1, hence the second load; `tv_delay 0` again after it (it
comes back as 105). `quit` over the console (tools/cs2/netcon.py) closes CS2 afterwards.
"""

import socket
import sys
import time

PORT = 2121


def main():
    name = sys.argv[1] if len(sys.argv) > 1 else "ofast_bots"
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 200.0
    console = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    console.settimeout(0.2)

    def run(command, wait):
        """Runs `command` and returns what the console printed within `wait` seconds."""
        print(">", command, flush=True)
        console.sendall((command + "\n").encode())
        end, out = time.time() + wait, b""
        while time.time() < end:
            try:
                out += console.recv(65536)
            except OSError:
                time.sleep(0.05)
        return out.decode("utf-8", "replace")

    run("sv_cheats 1; tv_enable 1; tv_delay 0; tv_autorecord 0", 1)
    run("map de_dust2", 45)
    run("tv_delay 0", 1)
    run("jointeam 3", 3)
    run("mp_autokick 0; mp_warmup_end; mp_freezetime 2; mp_roundtime 1.5; mp_roundtime_defuse 1.5;"
        " bot_difficulty 3; mp_autoteambalance 0; mp_limitteams 0", 2)
    run("tv_record " + name, 2)
    run("mp_restartgame 1", 4)
    run("god", seconds)
    # "Completed SourceTV demo …: Recording time …, Size …".
    print(run("tv_stoprecord", 2).strip())
    console.close()


if __name__ == "__main__":
    main()
