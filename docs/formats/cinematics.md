# Cinematics (OFAST's own)

What the Cameras panel in Unreal (`tools/unreal/ofast_cameras.py`, [decision 025](../decisions/025-cinematics-panel.md))
keeps for a round: the cinematic shots the user made with points, from which their cameras are made.

## File

One JSON file per round, in the Unreal project: `Saved/OFAST/Cinematics/<demo>_r<round>.json` (the round's sequence's
name).

```json
{
  "format": "ofast-cinematics",
  "version": 1,
  "sequence": "/Game/OFAST/Replays/<demo>_r<round>",
  "cinematics": [
    {
      "name": "Cinematic 1",
      "keep_in_frame": "CT molodoy",
      "follow": "",
      "focus": true,
      "depth_of_field": 0.7,
      "points": [
        {"tick": 153200, "location": [-669.6, -2830.5, -77.6], "rotation": [-12.9, -140.0, 0.0], "fov": 70.0}
      ]
    }
  ]
}
```

| Field | What |
|---|---|
| `name` | the camera's label in the level and its name in Sequencer ("Cinematic N") |
| `keep_in_frame`, `follow` | a player's body label in the level (`T <name>`, `CT <name>`), or empty |
| `focus` | depth of field on the player kept in frame (else the one followed) |
| `depth_of_field` | 0 to 1, as the Look panel's: f-stop = 22 ^ (1 − amount) (0.5: about f/4.7) |
| `points[].tick` | the demo tick (the sequence's frame plus its `OFAST_start_tick`), so a round sent again keeps it |
| `points[].location` | where the camera is, Unreal centimetres in the map's level |
| `points[].rotation` | pitch (up +), yaw, roll, Unreal degrees |
| `points[].fov` | horizontal field of view across the 16:9 picture, degrees (90: Unreal's viewport) |

## The camera made from it

- A `CineCameraActor` labelled with its name, tagged `OFAST_replay` (removed with its round's other actors when
  another round of the map is sent) and `OFAST_cinematic`; a 16:9 sensor (36 × 20.25 mm).
- Its transform keyed at every tick from its first point to its last (and on each point), cubic (auto) keys, so it
  plays smoothly at any frame rate. Between points a cubic Hermite curve: each point's slope that of the parabola
  through it and its neighbours (Catmull-Rom for unequal spacing), the end points' that of their one neighbour; each
  angle turned the short way from the point before. Before the first point and after the last it holds.
- Following a player: each point is kept as an offset from the player's path averaged over ±24 ticks; the camera is
  that path plus the curve through the offsets. Keeping a player in frame: pitch and yaw aim at the player's path
  averaged over ±6 ticks, 115 cm above an agent's feet (25 cm above a stand-in's middle); roll from the points.
- Its focal length keyed from the field of view; with focus, `FocusSettings.ManualFocusDistance` keyed from the
  distance to the player, its aperture from `depth_of_field`.

## Cuts

The round's own cuts (from its cameras file) are kept on the sequence as the metadata tag `OFAST_cuts`
(`{"end_frame", "cuts": [{"start", "end", "camera", "eyes"}]}`, frames; `replay_cameras.add_cuts`). The camera cut
track is that list with each cinematic of two points or more cut in from its first point's frame to its last's (its
last frame included), in the order they start: a later one over an earlier one. The muzzle lights' visibility is keyed
again for the cuts through each shooter's eyes that remain (`replay_cameras.key_lights`). A round built before
`OFAST_cuts` (2026-11-01) takes its cut track as it is the first time, without knowing whose eyes each cut is: its
muzzle lights are left as they were.

Made again from the file by `ofast_cameras.restore`: after a round is sent (`import_demo.py`), after its cameras
change (`import_cameras.py`), before a render (`render_round.py`), and when the panel finds a cinematic's camera
missing (Unreal closed without saving the level).
