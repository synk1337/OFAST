# 018 — Shots: each bullet rebuilt from its shot's seed and traced through the map

- **Date:** 2026-10-25
- **Status:** accepted (M6)

## Context

A fragmovie lives on its shots: muzzle flashes, tracers, sparks and dust where bullets land, bullet holes, blood. A
demo records none of where the bullets went on the world: no `bullet_impact` events, no impacts. What it holds
(`docs/formats/demo.md`, "Shots"):

- a **fire-bullets message** per shot (packet message 452): the shooter's eyes, the aim (recoil included), the
  weapon, its inaccuracy and spread, the fire mode, and a **random seed**. CS2's clients trace the bullets from these
  themselves, so every viewer sees the same impacts;
- an **effect dispatch** per bullet that hit a player (message 400): where it met the player;
- a message per pane of **glass** a bullet broke (message 423): where it met the pane.

CS2's files hold the rest: per weapon its bullets per shot, tracer frequency and tracer system
(`scripts/weapons.vdata`), per surface property its impact system and bullet-hole decal group
(`scripts/surfaceproperties_impact_effects.txt`, `surfaceproperties.vsurf`, `scripts/decalgroups.vdata`), per
collision triangle its surface property (`world_physics`), per weapon model its `muzzle_flash` attachment.

## Decision

- **Rebuild each bullet's direction exactly** from the seed, with Valve's random stream (Numerical Recipes' `ran1`)
  and the order of draws the glass panes and player hits pin down for a shot's first bullet (seed + 1; inaccuracy
  fraction and angle, spread angle and fraction; a shotgun's later pellets take seed + 1 + their number, as CS:GO's
  did, not checked yet). Checked, not assumed: 82 of the pro demo's 83 panes (the other is another bullet's) and all
  17 of the local demo's lie on the rebuilt path to a thousandth of a milliradian, and 725 of the two demos' 726 player
  hits on it to a thousandth of an inch.
- **Trace each bullet** from the eyes through the map's collision as bullets see it (`stops_bullets`: not grates,
  glass or clips), with a grid-walking ray cast on the same `WorldCollision` the ragdolls use; a bullet whose line
  passes through a player hit the demo sent ends on that player. Each world hit takes the surface property of the
  triangle it met, and from it the impact system and decal group CS2 would use.
- **Place the muzzle** where the exported agent shows the gun at the shot's tick: the weapon model's `muzzle_flash`
  attachment on the hand's weapon bone, in the frame Unreal plays. Third person for now.
- **Draw it in Unreal with the grenades' "particle system in a material"** (decision 017), extended so particles fly
  along an axis of their own (a barrel, a surface's normal, a bullet's path) and a tracer stops at its bullet's end;
  bullet holes as decals that carry their shot's tick in their decal colour (one material instance per texture).
- Everything goes in the **agents manifest** (`shots`, `earlier_holes`, `decals`): it needs the map's collision and
  the baked agents, as the blasts' ground and the fires' walls do.
- **Keep the earlier rounds' bullet holes**: CS2's clients do (a hole on de_nuke's A site in round 5 is an AWP
  bullet's from round 3), so the export traces every shot before the round too and Unreal shows their holes from the
  round's start.

## Alternatives considered

- **Random directions within the spread** (no seed): simple, but the bullet holes would land where CS2's do not, and
  a fragmovie maker comparing with CS2 would see it. The seed is in the demo; using it costs a few lines.
- **Tracing against the render meshes** (the exported map glTF): finer, but not what CS2 traces against, and it has
  no surface properties; the physics meshes are what the game uses for bullets.
- **Muzzle positions from the eyes** (eye + forward): no bake needed, but a third-person flash would hang in front of
  the face instead of at the barrel, 25 – 55 units ahead and off to the side.
- **One material instance per bullet hole**, its tick a parameter: an instance per hole made the soot of fires minutes
  slower to import (decision 017); the decal colour carries the tick for free.

## Consequences

- Impacts and holes land where CS2's do, for every shot the demo has; the checks run on any demo with glass or hits
  (`examples/shot_spread.rs`, `examples/shot_traces.rs`).
- Not yet: bullets going through walls and players (CS2's penetration: an exit impact and the bullet going on), props
  and doors (not in the world's collision), first-person muzzle flashes (the viewmodel's `muzzle_flash`, drawn with
  the viewmodel's field of view; CS2 draws a large one), brass (CS2 throws casings), the tracer counter (inferred: one
  shot in the frequency, counted on the magazine; in CS2 a tracer showed one tick after molodoy's shot where the rule
  puts one, none after two of YEKINDAR's where it also does), a tracer's timing (one tick after a shot CS2's is
  further along than OFAST's), shotguns' pellets after the first, the R8's and Negev's own spread rules, the limit on
  how many bullet holes CS2 keeps and which (four round-3 holes on de_nuke's open A-site door, whose collision is a
  static hull, do not show in CS2 in round 5), hiding the blood of a hit on the player the camera looks through (the hits do not
  name the player hit; CS2 shows none in that view), holes on props and signs (not in the world's collision).
- The sprite material's per-instance data grew from 8 to 12 floats; levels imported before read the defaults
  (straight up, no reach), so the grenades' effects are unchanged.
