# 026 — Scopes and sights through the players' eyes: CS2's zoom, its sight pose, its scope on a card

- **Date:** 2026-11-05
- **Status:** accepted

## Context

Through a player's eyes the POV camera kept CS2's field of view and the gun on screen whatever the player did: an
AWPer scoped in for half a round showed the AWP held at the hip and the map at 90°. In CS2, scoping in zooms the view
(the AWP 40 then 10, the SSG 08 and the auto-snipers 40 then 15, the AUG and SG 553 45), a sniper's scope hides the
gun and draws its black scope over the picture, and the AUG and SG 553 bring their sight up to the eye with the world
around it blurred and a dot in the lens (checked in a local game at 2560 × 1440, 2026-11-05). The user's choice: CS2's
scope on by default, with a switch to leave only the zoom (an editor may draw their own scope).

What the demo has: the pawn's camera services (`m_iFOV`, `m_iFOVStart`, `m_flFOVTime`, `m_flFOVRate`), `m_bIsScoped`,
the AUG's and SG 553's `m_iIronSightMode`. What CS2's files have: the zoom levels and which guns hide the viewmodel
(`scripts/weapons.vdata`), the sight's pose in the AUG's and SG 553's viewmodel graphs (`weapon_ironsight_amount`
blending `idle` and `ironsight_fidget`), and the sniper scope as one shader with no textures
(`materials/effects/legacy_sniper_scope.vmat`: its lens radius, edge, vignette and lines read from the shader).

## Decision

- **The zoom** is the eyes' field of view in the cameras file (a seventh value per row, `demo::players::view_fovs`:
  each change eased from where the view was over its rate, CS2's smooth step), keyed on each POV camera's component
  in Unreal by the Viewmodel panel's keying (`ofast_viewmodel.key_zoom`), which already owns the cameras' field of
  view; CS2's zoom levels are absolute, the unzoomed view the panel's **View field of view**.
- **What the view looks through** is the cameras file's `scopes`: the runs of ticks a player is scoped with a gun that
  zooms, `scope` (the gun hidden: its visibility keyed off) or `sight`.
- **The sight's pose** is baked with the viewmodel: `weapon_ironsight_amount` from `m_iIronSightMode` at the guns'
  pull-up and put-down speeds.
- **The scope over the view** is a card a few centimetres in front of the POV camera, with a translucent unlit material
  computed from the screen's position (and, for the sight, the picture behind it), shown by keyed visibility over the
  player's spans and only through their eyes (OnlyOwnerSee, as the viewmodel); a replay collection parameter turns
  every card off (the Look panel's **Scope overlay**).

## Alternatives considered

- **A post-process material on each POV camera**, as the flashed view is (`M_Flashed`): its instance holds the
  blindings as parameter slots read against the replay tick. A player scopes in tens of times a round (an AWP's
  every shot unzooms it), more than slots allow, and the flashed material already takes the camera's blendables.
- **One post-process material keyed per cut** (a parameter collection track following the camera cuts): wrong as soon
  as the user changes the cuts or adds a camera.
- **Only the zoom**: what the switch leaves, not CS2's look the user asked for by default.

## Consequences

- The sight's card is built but not placed (`scopes.PLACED`): in CS2 the sight comes right up to the eye (its housing
  0.54 of the screen's height in radius), where its clip holds the gun 17 inches out; CS2's client moves the viewmodel
  there (CS:GO's iron sight code did, by an eye position and a field of view), which OFAST does not yet, and a blur
  round the middle of the screen with the gun below it looked wrong. Its lens will be a circle at a fixed size (0.257
  of the screen's height: the AUG's at viewmodel FOV 68), where CS2 masks with the sight model's lens.
- The sniper scope's slight barrel warp and colour fringe at the rim are not drawn, nor its blur with inaccuracy.
- A player's first-person flashes and tracers ride on the viewmodel, so a sniper's are hidden while its scope is up
  (whether CS2 draws one through the scope is not checked).
