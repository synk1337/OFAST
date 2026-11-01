# 025 — Cinematics: a campath-style Cameras panel in Unreal

- **Date:** 2026-11-01
- **Status:** accepted (first pass)

## Context

Fragmovies cut between the players' views and cinematic shots: a camera moving round players as they run, peek or
shoot. The Studio's shot cameras (orbit, tripod, above…, decision 021) are ready-made, and the user's word on them was
that people "like to do their own cinematics", while most do not know Unreal's camera or render system (Sequencer,
keys, camera cuts). CS editors know HLAE's campath: fly the free camera in the demo, `mirv_campath add` at a few
moments, play the smooth path through them. OFAST already imports such paths (HLAE `.xml` and `.cam`).

Options weighed with the user (2026-10-31):

- **A Cameras panel in Unreal** (chosen): campath's way of working inside Unreal's viewport, so the shot is placed in
  the look it renders in (the realistic look, the Look panel's settings), with options a campath has not: keep a
  player in frame, follow one, focus on them.
- A campath tool in CS2 (OFAST's capture DLL instead of HLAE): familiar, but placed in CS2's look, with CS2 running
  `-insecure`; HLAE files already cover it for those who have HLAE.
- A guide to Unreal's own Sequencer: the least work, but beginners would have to learn Sequencer.

## Decision

- **A panel** (`tools/unreal/ofast_cameras.py`) made as the Look panel is: an object of a Python class shown in an
  editor tab, its fields the settings and its `CallInEditor` functions the buttons (they show in alphabetical order:
  hence "Earlier point" and "Later point"); `GetOptions` lists the cinematics and the round's players. It opens with
  "Open in Unreal" and from OFAST > Cameras.
- **Points, not keys**: the user's points (demo tick, place, turn, field of view) are kept in a file per round
  ([cinematics.md](../formats/cinematics.md)) and the camera's keys are made from them whenever they change, a key a
  tick. Following a player needs keys made from the player's path anyway, and the file makes the cinematics survive a
  round sent again (its sequence is made anew), a camera change, and an editor closed without saving (a render makes
  them again from the file). Keys changed by hand in Sequencer are replaced at the next change: the panel is the tool.
- **Flying is Unreal's own piloting**: "New cinematic" and "Add point" pilot the cinematic's camera, so the viewport
  is the camera (its field of view too) and scrubbing shows the move; "Preview" ejects it and locks the viewport to
  the cuts.
- **The round's own cuts kept on the sequence** (`OFAST_cuts`) so a cinematic can be cut in and out again without
  the cameras file (which the editor does not have); the muzzle lights keyed again for the cuts left through each
  shooter's eyes.
- **Messages in the viewport** (Unreal's on-screen messages) rather than in the panel: a panel row of changing height
  moved the buttons under the mouse.

## Checks

Round 17 of aurora-vs-furia-m1-dust2 (de_dust2, CS2's look): a 3-point cinematic round molodoy, kept in frame with
focus, zooming 70° to 50°, cut in between his own view's cuts (POV → cinematic 2804–2965 → POV); preview renders with
him kept in frame and the background blurred; the following version stays with him as he moves (frame-to-frame
change at most 2.4 times the median, the peak an effect leaving the frame, not a jump). In the editor: buttons, the
player lists, flying, adding, jumping between and removing points, a second cinematic made and removed, preview.

## Depth of field over the round (2026-11-01)

The user asked for the camera blur set in Unreal (near and far blur, strength) and changing with the scene, rather
than a depth pass for compositing. Unreal's cinematic depth of field (Diaphragm DOF) is a lens: a focus distance and
an aperture; the near and far blur sizes of its settings are for the mobile renderer only. What it does take apart
is the most a thing in front of the focus and behind it can blur (`r.DOF.Kernel.MaxForegroundRadius`,
`MaxBackgroundRadius`), one value each for the round. So the panel keys the lens, as the points key the camera: focus
keys (what is sharp, how strong the blur), eased from one to the next on whichever camera is cut in; the limits as
two fields. The cameras' own focus (the POV's on what the player aims at, a cinematic's on its player) is kept on
row 0 of their tracks and only switched off while the round has keys ([cinematics.md](../formats/cinematics.md)).

Checked on round 17 of aurora-vs-furia-m2-nuke: keys on the 4 POV cameras cut in and a cinematic, their own focus
switched off then back on with the keys removed (the POV lens back to CS2's look's: off); a preview render racks
from 0.6 m (the hallway blurred behind the gun) to where KSCERATO aims (the far end sharp).

## Later

- A camera's ease in and out at its ends (now straight from the first point and into the last).
- Slow motion inside Unreal (Sequencer's play rate) for shots where the camera moves at full speed while the round
  slows.
- Cinematics written back to HLAE (`export_camera.py` reads any camera of the sequence already).
