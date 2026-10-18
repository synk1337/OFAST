"""Oracle: reference values from demoparser2, to check OFAST's demo reader against.

demoparser2 (https://github.com/LaihoE/demoparser, MIT) is an established CS2 demo parser. OFAST does not use it as a
library; the `game-data` test `crates/ofast-core/tests/cs2_demo.rs` runs this script on the same demo and checks
that OFAST reads the same numbers.

Usage:
    python -I demoparser2_oracle.py positions <demo.dem> <first_tick> <last_tick>
        CSV, one row per player per tick in [first_tick, last_tick]: tick,steamid,x,y,z,pitch,yaw,name
        (name last, unquoted: split each line into at most 8 fields)
    python -I demoparser2_oracle.py round-starts <demo.dem>
        One tick per line: the tick of every round_start game event.
    python -I demoparser2_oracle.py deaths <demo.dem>
        CSV, one row per player_death event: tick,headshot,weapon,victim,attacker
        (names with their commas replaced by ';', so every line splits into exactly 5 fields; an empty attacker
        means nobody, e.g. fall damage)
    python -I demoparser2_oracle.py rig <demo.dem> <first_tick> <last_tick>
        CSV, one row per player per tick: the inputs of OFAST's animation rig as demoparser2 reads them:
        tick,steamid,flags,duck_amount,walking,scoped,defusing,move_type,item,anim_state,reloading,last_jump_raw,
        body_yaw,name (name last; empty item / anim_state / reloading without a weapon in hand; item and anim_state
        may print as floats; body_yaw is the yaw of the pawn's `CBodyComponent` rotation). last_jump_raw is `m_nLastJumpTick` as demoparser2 decodes it: unsigned, i.e. the zigzag code,
        twice the game tick. Not the velocity: demoparser2 names fields without their send node, so
        `CCSPlayerPawn.m_vecX/Y/Z` mixes `m_vecVelocity` and `m_vecViewOffset` (z came out as the eye height).

Setup (any folder outside the repository; on Linux/macOS use oracle-venv/bin/python instead of Scripts):
    python -m venv oracle-venv
    oracle-venv/Scripts/python -m pip install demoparser2 pandas
then point the test at that interpreter: set OFAST_ORACLE_PYTHON=<full path to oracle-venv/Scripts/python>.
"""

import sys

from demoparser2 import DemoParser


def positions(demo: str, first: int, last: int) -> None:
    frame = DemoParser(demo).parse_ticks(["X", "Y", "Z", "pitch", "yaw"], ticks=list(range(first, last + 1)))
    out = sys.stdout
    out.write("tick,steamid,x,y,z,pitch,yaw,name\n")
    for row in frame.itertuples(index=False):
        # repr() prints floats exactly, so the comparison is not blurred by rounding. The name is needed to tell
        # bots apart: they all have Steam id 0.
        name = str(row.name).replace("\n", " ")
        out.write(f"{row.tick},{row.steamid},{row.X!r},{row.Y!r},{row.Z!r},{row.pitch!r},{row.yaw!r},{name}\n")


def round_starts(demo: str) -> None:
    for tick in DemoParser(demo).parse_event("round_start")["tick"]:
        print(int(tick))


def deaths(demo: str) -> None:
    frame = DemoParser(demo).parse_event("player_death")
    print("tick,headshot,weapon,victim,attacker")

    def name(value) -> str:
        return "" if value is None or value != value else str(value).replace(",", ";").replace("\n", " ")

    for row in frame.itertuples(index=False):
        print(f"{int(row.tick)},{bool(row.headshot)},{row.weapon},{name(row.user_name)},{name(row.attacker_name)}")


RIG_PROPS = {
    "flags": "CCSPlayerPawn.m_fFlags",
    "duck_amount": "CCSPlayerPawn.CCSPlayer_MovementServices.m_flDuckAmount",
    "walking": "is_walking",
    "scoped": "is_scoped",
    "defusing": "is_defusing",
    "move_type": "move_type",
    "item": "item_def_idx",
    "anim_state": "m_iWeaponGameplayAnimState",
    "reloading": "m_bInReload",
    "last_jump_raw": "CCSPlayerPawn.CCSPlayer_MovementServices.m_nLastJumpTick",
    # A (pitch, yaw, roll) triple: only the yaw is written.
    "body_yaw": "CCSPlayerPawn.CBodyComponentBaseAnimGraph.m_angRotation",
}


def rig(demo: str, first: int, last: int) -> None:
    frame = DemoParser(demo).parse_ticks(list(RIG_PROPS.values()), ticks=list(range(first, last + 1)))

    def text(value) -> str:
        # Missing values (no weapon in hand) come as NaN or None; floats are printed exactly with repr(); of an
        # angle triple, the yaw.
        if value is None:
            return ""
        if hasattr(value, "__len__") and not isinstance(value, str):
            return repr(float(value[1])) if len(value) == 3 else ""
        if value != value:
            return ""
        if isinstance(value, float):
            return repr(value)
        return str(value)

    out = sys.stdout
    out.write("tick,steamid," + ",".join(RIG_PROPS) + ",name\n")
    columns = list(RIG_PROPS.values())
    for row in frame.to_dict("records"):
        values = ",".join(text(row.get(c)) for c in columns)
        name = str(row["name"]).replace("\n", " ")
        out.write(f"{row['tick']},{row['steamid']},{values},{name}\n")


def main() -> None:
    # Player names can contain any Unicode; Windows consoles default to a legacy code page.
    sys.stdout.reconfigure(encoding="utf-8")
    command = sys.argv[1]
    if command == "positions":
        positions(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
    elif command == "round-starts":
        round_starts(sys.argv[2])
    elif command == "deaths":
        deaths(sys.argv[2])
    elif command == "rig":
        rig(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
    else:
        sys.exit(f"unknown command {command!r}")


if __name__ == "__main__":
    main()
