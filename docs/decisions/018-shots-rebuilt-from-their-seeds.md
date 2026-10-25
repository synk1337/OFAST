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
  glass or clips), with a grid-walking ray cast on the same `WorldCollision` the ragdolls use. Each world hit takes
  the surface property of the triangle it met, and from it the impact system and decal group CS2 would use.
- **Through walls and players** (added 2026-10-25): the ray's every crossing of the world's surfaces, in and out
  (CS2's world meshes face outward: 99.3 – 100 % of the bullets from the eyes meet a front first), makes the walls on
  a bullet's line; each costs it damage by CS:GO's rule as its players documented it, with CS2's own values
  (`m_flPenetration`, `m_flRangeModifier`, `m_nDamage` in `weapons.vdata`; each surface's
  `bulletPenetrationDistanceModifier` in `surfaceproperties_game.txt`), and a player as flesh, until it stops. The
  rule is checked, not assumed: a hit after a wall starts where the bullet came out of it (the demo's effect
  dispatch), and on 13 demos of 6 maps 456 of 545 such hits start within half an inch of OFAST's traced way out,
  while the damage CS2 recorded for 289 of 314 non-lethal hits through walls is the rule's exactly. The demo's hits
  stay the truth where the trace cannot see (props, a build whose plaster differed): a wall before a hit is gone
  through, a wall between a hit's start and its point is not there. The rotating doors are walls too: each door
  model's own collision, the bullet's line turned into its space as the demo has the door standing on that tick
  (no bullet holes on them: a hole is fixed in the world and a door moves).
- **A tracer on every shot, from the moment it was fired**: CS2 draws one on each shot of a gun with a tracer system
  (YEKINDAR's view: four in a row; soulfly's: molodoy's AK on shots one in three would skip), whatever
  `m_nTracerFrequency` says (3 on the AK; 0 still means none: the silenced modes). The shot is fired before its
  message's tick: the attack tick − 1 + the sub-tick fraction the fire-bullets message carries on local demos, the
  start of that tick on tournament demos (CS2 shows YEKINDAR's tracer in flight on his shot's own tick). In the
  shooter's own view it leaves where their viewmodel's barrel shows: the viewmodel muzzle on the camera, its
  sideways and up offsets scaled by tan(world FOV / 2) / tan(viewmodel FOV / 2) so it lands on the same place of the
  screen (Source's `FormatViewModelAttachment`), on an actor shown with the viewmodel; the world's tracers, from the
  barrel the body holds, are hidden with the body there.
- **Brass from the gun's own casing system**: the weapon model's `shoot` sequence names it (an event on the
  `shell_eject` attachment); its numbers (speed out of the port, gravity, drag, tumble, bounce, stop speed, life)
  fly each case in the core, in the world against the map's collision and, for the shooter's own view, in the
  viewmodel's space; Unreal keys one actor per case as it keys a dropped weapon.
- **Place the muzzle** where the exported agent shows the gun at the shot's tick: the weapon model's `muzzle_flash`
  attachment on the hand's weapon bone, in the frame Unreal plays; and where the shooter's viewmodel shows it, on
  the weapon's own bones in the viewmodel rig's space, for the flash CS2 draws in first person. That one rides on the
  viewmodel actor in Unreal (moved, mirrored, shown and hidden with it, drawn with Unreal's first-person rendering):
  the sprite material's slots can be in their component's space (`local`), turned into the world's by the vertex
  shader. CS2's first-person system is the third-person one made smaller (`*_fps.vpcf`, a global scale of 0.35 for the
  AK); its size here is fitted to CS2's pictures (what that scale scales is not in the files). It has its own light
  too, at the viewmodel's barrel: a light component on the viewmodel actor, shown with it.
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
- **Penetration from the demo's hits alone** (a wall before a hit is gone through, nothing else): no rule to get
  wrong, but no bullet would go on through a wall where no player stood behind it, and every exit hole and impact on
  a wall's far side would be missing. The rule decides; the hits overrule it where they show more.
- **A tracer on one shot in `m_nTracerFrequency`**, counted on the magazine (until 2026-10-25): the files say 3 for
  the AK, but CS2 drew one on shots that count would skip, in first and third person.
- **The shot on its message's tick**: one tick after a shot, CS2's tracer was further along than OFAST's, and on
  the message's own tick CS2's is already in flight.

## Consequences

- Impacts and holes land where CS2's do, for every shot the demo has; the checks run on any demo with glass or hits
  (`examples/shot_spread.rs`, `examples/shot_traces.rs`), and with the damage of its hits
  (`examples/shot_penetration.rs`).
- A bullet going through a wall leaves an impact and a hole on both sides; one going through a player goes on.
- Not yet: what else has a model of its own (`prop_dynamic`: de_nuke's silo doors; brush entities: a retake blocker
  of de_nuke's B site stopped bullets in a tournament demo; a railing tagged `passbullets` a hit started behind,
  though making grates, railings and glass walls changed no count on the 13 demos), each gun's own muzzle flash
  (every gun draws the AK's two flames: CS2's `ak47_primaryflash` and `_alt`, one per shot as its chooser picks,
  without the compensator's side flashes and sparks), a tracer's trail (one tick after molodoy's shot at 40813 CS2's
  still reaches back to his gun where OFAST's has gone), a player's depth from their hitboxes (8 inches meanwhile),
  bullet holes on doors (moving with them), shotguns' pellets after the first, the R8's and Negev's own spread rules, the limit on how
  many bullet holes CS2 keeps and which (four round-3 holes on de_nuke's open A-site door, whose collision is a static
  hull, do not show in CS2 in round 5), hiding the blood of a hit on the player the camera looks through (the hits
  do not name the player hit; CS2 shows none in that view), holes on props and signs (not in the world's collision).
  Older demos may have played on other values: one de_inferno demo's plaster walls cost bullets as if plaster's
  distance modifier were 0.5, the current files' 0.7.
- The sprite material's per-instance data grew from 8 to 12 floats; levels imported before read the defaults
  (straight up, no reach), so the grenades' effects are unchanged.
