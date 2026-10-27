# 021 — Cameras: built in Rust, any camera works in Unreal, HLAE's paths sampled exactly

- **Date:** 2026-10-26
- **Status:** accepted (M7, first pass)

## Context

Until M7 a round's sequence had one kind of camera: each player's eyes, with camera cuts to each killer, kill by
kill. A fragmovie needs more: shots from outside the players (behind a killer, around him, from a fixed point that
sees both players, from above), a calmer first-person view than the raw mouse, and the cameras editors already make
with HLAE in CS2 (`mirv_campath`, `mirv_camio`). And a camera tool is only useful if any camera can be cut to.

Two things stood in the way:

- **Visibility was keyed to the cuts.** A player's body (and their muzzle flashes, the blood on them, their thrown
  cases) was hidden while the cut looked through their eyes, their viewmodel shown only then. A camera added in
  Sequencer, or a cut changed to another camera, showed the inside of a head, or a viewmodel floating in the air.
- **The first-person cameras were computed in Unreal's Python** from the replay and the agents manifest, where they
  could not be smoothed against the shots or tested.

## Decision

- **Hide by owner, not by cut.** Each player's POV camera owns what their own view must not show (their body, their
  third-person flashes, the blood of hits on them, their cases in the world while their view shows the first-person
  ones): those primitives are `bOwnerNoSee`; their viewmodel and what rides on it are `bOnlyOwnerSee`. Unreal then
  hides them in that camera only, whichever camera a cut uses (checked 2026-10-26 on de_dust2 round 17: through
  XANTARES's eyes the view is clear; with the flag off his head fills it; from behind him his body shows).
  Visibility keys keep only life and death. Lights cannot be hidden per view: the two muzzle lights (the
  third-person one, the first-person one on the viewmodel) stay keyed by the cuts, and are keyed again when the
  cameras change.
- **Every camera of a round is computed in Rust** (`camera`, `export::cameras`) and written to the round's cameras
  file (`<demo>_r<round>.cameras.json`): each player's eyes (positions, the eye angles plus the recoil on the view,
  smoothed when asked), the shot cameras, and the cuts. Unreal only keys them (`import_demo.py`, and
  `import_cameras.py` to change the cameras of a round already built, in seconds instead of a full import).
- **Shots are chosen per kill** (the Studio, `ofast export-demo --shots`): the killer's eyes (the default), behind
  the killer, an orbit around him, a fixed point that sees both players, from above, or the victim's eyes. A shot
  covers the last seconds before its kill; the rest of the kill's cut stays on the killer's eyes. Shot cameras keep
  out of walls with the map's collision (the one bullets use).
- **First-person smoothing keeps the shots exact.** The eye angles are smoothed (a Whittaker smoother: the closest
  curve to the angles whose second differences stay small), pinned to the original angles on the ticks around each
  shot: the bullets, tracers and hits come from the original aim, and the crosshair must be on them.
- **HLAE's files are read and written** (`camera::hlae`): `mirv_campath` XML and `mirv_camio` `.cam`. A campath is
  sampled on every tick with HLAE's own interpolation (cubic splines at rest at both ends for position and field of view,
  James McEnnan's quaternion spline, CC0, for rotation; or linear and slerp), so the camera moves as it does in CS2.
  HLAE's times are CS2's game clock (`curtime`): the server's tick, which runs a constant distance ahead of the
  demo's (`RoundTrack::server_tick_offset`).

## Alternatives considered

- **Keep the keyed visibility** and rebuild it with each camera change: every visibility track of a round depends on
  the cuts (bodies, flashes, viewmodels, blood, cases), so changing a camera means touching all of them, and a camera
  the user adds by hand in Sequencer would still be wrong.
- **Shot cameras made in Unreal** (look-at tracking, spline rails, attached cameras): easy to place, but not
  testable, not exportable to HLAE, and a look-at camera in Unreal cannot see the map's collision.
- **Unreal's own cubic keys for a campath's points**: close, but not HLAE's curve (auto tangents differ from HLAE's
  spline, and Euler curves from a quaternion spline).

## Not verified yet

- HLAE was not installed: the formats follow its source (`shared/CamPath.cpp`, `shared/CamIO.cpp`,
  `shared/AfxMath.cpp`, `AfxHookSource2/main.cpp`); a campath made in CS2 has not been imported yet. The time mapping
  was checked in CS2 itself with ofast-capture (2026-10-27, decision 022): `curtime × 64 − demo tick` is the demo's
  server tick offset (134067 on the Aurora vs FURIA de_dust2 demo) plus up to 2 ticks (CS2 draws a little behind
  its clock).
- Movie Render Queue's view of owner visibility: it renders through the player controller's view target, the cut's
  camera, which `bOwnerNoSee` checks; to be checked with M9's renders.
- CS2 draws a player's own shadow in first person or not: with owner visibility Unreal draws it.
