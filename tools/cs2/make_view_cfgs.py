"""Write CS2 cfg files for the views of a map, so CS2 reference screenshots can be taken at the same spots as Unreal's.

    python tools/cs2/make_view_cfgs.py de_nuke 1.0 [bsite,bsite2,...]

Reads tools/compare/views/<map>.json and writes into tools/cs2/cfg/:
- ofast_v_<short>_<view>.cfg for every view (setpos / setang; <short> is the map name without "de_");
- ofast_tour_<short>.cfg, a tour of the listed views (default: all) at a locked exposure
  (`mat_tonemap_force_scale <exposure>`; use the exposure OFAST's import uses, the manifest's `post.min_exposure`).
  After `exec ofast_setup` and `exec ofast_tour_<short>`, F8 goes to the next view and F9 re-applies the current one
  (the view drifts once the console closes): per view press F8, wait for the picture to settle, then F9 and Steam's
  F12. The console echoes "OFAST view n/N <name>". `unbind F8; unbind F9` afterwards.

Copy the files into the game's `game/csgo/cfg/` folder before use (see docs/development.md, "Comparing with CS2").
"""

import json
import os
import sys

REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")


def number(value):
    return "%g" % value


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    map_name, exposure = sys.argv[1], float(sys.argv[2])
    short = map_name.split("_", 1)[1] if "_" in map_name else map_name
    with open(os.path.join(REPO, "tools", "compare", "views", map_name + ".json"), encoding="utf-8") as f:
        views = json.load(f)
    by_name = {view["name"]: view for view in views}
    tour = sys.argv[3].split(",") if len(sys.argv) > 3 else [view["name"] for view in views]
    unknown = [name for name in tour if name not in by_name]
    if unknown:
        sys.exit("no such view in %s.json: %s" % (map_name, ", ".join(unknown)))
    cfg_dir = os.path.join(REPO, "tools", "cs2", "cfg")

    def position(view):
        return "setpos %s; setang %s" % (" ".join(map(number, view["setpos"])), " ".join(map(number, view["setang"])))

    for view in views:
        path = os.path.join(cfg_dir, "ofast_v_%s_%s.cfg" % (short, view["name"]))
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write("setpos %s\nsetang %s\n" % (" ".join(map(number, view["setpos"])), " ".join(map(number, view["setang"]))))

    lines = [
        "// OFAST: tour of the %s views in tools/compare/views/%s.json (local -insecure session only)." % (map_name, map_name),
        "// Written by tools/cs2/make_view_cfgs.py. exec ofast_setup first. F8: next view, F9: re-apply the current one",
        "// (the view drifts once the console closes). Per view: F8, wait, F9, Steam's F12. Afterwards: unbind F8; unbind F9",
        "mat_tonemap_force_scale %s" % number(exposure),
    ]
    # Aliases cannot nest quotes, so each view is two: ofast_p_<view> moves there, ofast_v_<view> also points
    # ofast_cur at it and ofast_next at the following view.
    for index, name in enumerate(tour):
        following = tour[index + 1] if index + 1 < len(tour) else "done"
        lines.append('alias ofast_p_%s "%s"' % (name, position(by_name[name])))
        lines.append(
            'alias ofast_v_%s "ofast_p_%s; alias ofast_cur ofast_p_%s; alias ofast_next ofast_v_%s; echo OFAST view %d/%d %s"'
            % (name, name, name, following, index + 1, len(tour), name)
        )
    lines += [
        'alias ofast_v_done "echo OFAST tour of %s done"' % map_name,
        "alias ofast_next ofast_v_%s" % tour[0],
        "alias ofast_cur ofast_next",
        "bind F8 ofast_next",
        "bind F9 ofast_cur",
    ]
    with open(os.path.join(cfg_dir, "ofast_tour_%s.cfg" % short), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print("%d view cfgs, a tour of %d views at exposure %s" % (len(views), len(tour), number(exposure)))


if __name__ == "__main__":
    main()
