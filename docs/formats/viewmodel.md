# Viewmodel settings (OFAST's own)

What the Viewmodel panel in Unreal (`tools/unreal/ofast_viewmodel.py`) keeps: how the first-person gun is drawn in
every round of the project, and each round's own data to key its guns again with other settings.

## Settings

One JSON file for the project: `Saved/OFAST/Viewmodel.json` in the Unreal project. Without it, CS2's own default
viewmodel for everyone (a new install's `viewmodel_presetpos 1`).

```json
{
 "format": "ofast-viewmodel",
 "version": 1,
 "whose": "Same for everyone",
 "fov": 68.0,
 "offset": [2.5, 0.0, -1.5],
 "hand": "As played",
 "motion": 1.0
}
```

| Field | What |
|---|---|
| `whose` | `Same for everyone`: `fov` and `offset` for every player; `Each player's own`: the settings each player had in the demo (`m_flViewmodelFOV`, `m_flViewmodelOffsetX/Y/Z`; [demo.md](demo.md#first-person-view)) |
| `fov` | CS2's `viewmodel_fov`: horizontal at 4:3, degrees (CS2 allows 54 – 68) |
| `offset` | CS2's `viewmodel_offset_x`, `_y`, `_z`: inches right, forward, up (CS2 allows −2.5 – 2.5, −2 – 2 for z) |
| `hand` | `As played` (each player's `m_bLeftHanded`, tick by tick), `Right` or `Left` for everyone |
| `motion` | the gun's motion with the player (the agents manifest's `sway`, [animation.md](animation.md#first-person-view-animviewmodel-exportviewmodel)) times this: 0 a still gun, 1 CS2's |

Why a default for everyone (the user's choice, 2026-11-01): CS2 seems to draw a spectated player's gun at the
viewer's own settings, and players' own differ (Nuke round 17 of Aurora vs FURIA: FOV 60, 65 and 68, five sets of
offsets); one viewmodel through a whole movie looks like one game.

## Each round's data

`Saved/OFAST/Viewmodels/<demo>_r<round>.json` (the round's sequence's name), written by `import_demo.py`:

```json
{
 "format": "ofast-viewmodels",
 "version": 1,
 "frames": [0, 1, 2],
 "end_frame": 5002,
 "players": [
  {"name": "XANTARES", "fov": 60.0, "offset": [1.0, 1.0, -1.0], "hands": [[0, false]],
   "punch": [[1520, 0.31, -0.02]], "sway": [[210, 1.85, 0.12]]}
 ]
}
```

| Field | What |
|---|---|
| `frames` | the round's sample frames (demo tick − the round's first) |
| `end_frame` | the sequence's last keyed frame |
| `players[].fov`, `offset` | the viewmodel the player played with |
| `players[].hands` | `[frame, left]` at each change of hand |
| `players[].punch` | `[frame, pitch, yaw]`: the agents manifest's `punch`, its viewmodel part (Source's degrees) |
| `players[].sway` | `[frame, pitch, yaw]`: the agents manifest's `sway` |

## Keyed in Unreal

`ofast_viewmodel.key(sequence)`, by `import_demo.py`, before every render (`render_round.py`), when the panel finds the
open round keyed with other settings (the sequence's `OFAST_viewmodel` metadata tag holds the settings it was keyed
with), and after each change in the panel (once its fields are left alone 0.4 s): each `VM <name>` binding's transform
track made anew (the offsets, the hand mirroring it, the recoil plus the motion: [replay.md](replay.md#in-unreal-import_demopy))
and each `POV <name>` camera's `FirstPersonFieldOfView`. A round sent before the panel (2026-11-01) has no data:
send it again.
