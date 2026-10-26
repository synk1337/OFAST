# 019 — Recoil on the first-person view: the aim punch rebuilt, the view punch from the demo

- **Date:** 2026-10-26
- **Status:** accepted (M5)

## Context

In CS2 a spray kicks the player's view: the camera turns off the eye angles by part of the aim punch (the recoil
the bullets follow, twice its angle) and by the view punch (a short kick per shot), and the viewmodel turns a little
further. OFAST's first-person cameras followed the eye angles alone, so every spray framed differently from CS2's:
players pull the mouse down against recoil, and without the punch the camera followed them down, 3 – 4° in five
shots.

The demo sends the view punch (`m_pCameraServices.m_vecCsViewPunchAngle`, with the tick it was set) but not the aim
punch (`m_pAimPunchServices` stays unsent on SourceTV). Every shot shows the aim punch, though: its aim is the eye
angles plus twice the punch.

## Decision

- **Rebuild the aim punch from each player's shots** with Source 2 Viewer's model (`AimPunchServices.cs`,
  `RecoilPattern.cs`): each kick pushes a velocity that decays, and the punch integrates it while decaying back
  (`demo::recoil`). Each kick is the view punch's jump at that shot divided by 0.055 (the share of a kick the view
  punch takes); without one, the gun's recoil table's at the shot's recoil index.
- **Take the view punch from the demo**, decayed at 18 per second from when it was set.
- **Turn the camera** by 0.9 × the aim punch + the view punch, and **the viewmodel** by 0.65 × the aim punch past it,
  keyed per tick from the agents manifest's `punch` (`export::punch`); in Unreal on the POV camera's rotation and on
  the viewmodel's transform track.

## Alternatives considered

- **The recoil table alone** for every kick: exact for full-auto guns' first kick (0.0001°), but CS2's semi-automatic
  pistols take each kick from another entry of their table (the Glock's at recoil index 0 is entry 5 for one player,
  11 for another); their view punch jumps show which. Both sources rebuild the aim punch within the same 0.07 – 0.09°
  median of what the shots show, the eye angles' tick rate being the limit.
- **The aim punch from the shots' own aim only** (aim − eye angles, between shots by the model): exact only at the
  shots, and the eye angles come once a tick, not at the shot's sub-tick moment.

## Consequences

- YEKINDAR's view at ticks 37703 and 37709 of de_nuke round 5 frames as CS2's does: the A sign's centre at 47.45 %
  and 47.35 % of the height, CS2's at 47.6 % and 47.4 % (about 44 % without the recoil); after the spray's first shot
  49.2 % against CS2's 49.0 % (47.9 % without).
- Deep in a full-auto spray the rebuilt aim punch runs about 3 % under what the shots show (the order of decay and
  integration in CS2's step is between Source 2 Viewer's and the exact integral); about 0.06° on the camera.
- Not yet: the viewmodel's place mid-spray (its muzzle 4 % of the frame higher than CS2's, as on a single shot without
  recoil: the firing animation's pose), the aim punch on the third-person body's aim (Source 2 Viewer adds it to the
  body's aim layer), the view punch of being hit (not seen apart from shots yet).
