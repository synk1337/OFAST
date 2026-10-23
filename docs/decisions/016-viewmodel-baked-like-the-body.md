# 016 — First-person view: CS2's viewmodel baked in Rust like the body, drawn by Unreal's first-person rendering

- **Date:** 2026-10-23
- **Status:** accepted (M5)

## Context

A POV shot in the Unreal replay is a camera at the player's eye with nothing in front of it: no arms, no gun. In CS2
the first-person view draws the **viewmodel**, and POV kills are the core of a fragmovie. What CS2 has (checked in
its files, its demos and its published dumps, 2026-10-23):

- **The rig.** `weapons/models/shared/arms/weapon_arms.vmdl` holds the viewmodel skeleton (`root_motion`, `wpn` where the
  weapon sits, `wpnHand_L/R` the hands' targets, the arms and fingers; no meshes). The clips
  (`animation/anims/viewmodel/<set>/<weapon>/{draw,idle,shoot1,reload,lookat01,…}_<weapon>`) are on
  `animation/skeletons/characters/viewmodel.vnmskel`, with a **secondary animation** on the weapon's own skeleton
  (`animation/skeletons/weapons/ak47.vnmskel`: magazine, bolt, slide). `root_motion` stays at the origin: the clips
  are authored relative to the camera (`wpn` at 14.8 inches ahead, 5.2 right, 2.5 down in the AK's idle).
- **The meshes.** The agent model itself carries the first-person sleeves (mesh groups `firstperson_default`,
  `firstperson_hide_default_gloves` beside the third-person ones), each glove model a `firstperson_default` group;
  both bone-merge onto the rig by bone name, the weapon's world model sits on `wpn`.
- **The graphs.** `animation/graphs/viewmodel/viewmodel.vnmgraph` picks a per-weapon graph
  (`viewmodel_gun.vnmgraph+ak47`, `viewmodel_grenade+…`, knives, inspects) through a state machine, and ends in two
  two-bone IK nodes putting `hand_R` / `hand_L` on `wpnHand_R` / `wpnHand_L` (the left one off during
  `WPN_DISABLE_LEFT_HAND_IK` events). Their control parameters are the third-person weapons layer's: `weapon_type`,
  `weapon_category`, `action`, `attack_type`, ammo, silencer, the variations. No movement parameter: walking bob,
  landing and the lag behind fast turns are hard-coded in CS2's client (no cvars left since 2024).
- **The demo.** The viewmodel is client-only since July 2025 (`C_CS2HudModelArms`), so demos carry no viewmodel
  pose, but they carry what drives it: each weapon's gameplay state and timestamp (what the third-person bake
  already reads, decision 013), the inspect fields, and on each pawn the player's own viewmodel settings
  (`m_flViewmodelFOV` 60 – 68, `m_flViewmodelOffsetX/Y/Z`, `m_bLeftHanded`) and the view punch
  (`m_vecCsViewPunchAngle`). On the Aurora vs FURIA demo the ten players use FOV 60, 65 or 68 and five different
  offsets.
- **The projection.** Like the world's, the viewmodel FOV is horizontal at 4:3 (60 → 75.18° at 16:9); CS2 draws
  the viewmodel in its own pass over the world (Source 1's rule, kept in CS2: HLAE's notes).
- **Unreal 5.8** draws primitives tagged `FirstPerson` with the camera's `FirstPersonFieldOfView` (in the vertex
  shader: view-space x and y scaled by tan(world FOV / 2) / tan(first-person FOV / 2)), optionally scaled towards
  the camera by `FirstPersonScale` so that they reach less far into walls, and without shadows: CS2's separate
  viewmodel projection, built in.

## Decision

The viewmodel is baked in OFAST's core per player and tick, like the body (decision 013), and Unreal only plays it:

- **Inputs:** the demo's weapon states, cut into action segments by the rules of the third-person weapons layer
  (the same state → action tables, `anim::states`, and the same start ticks; the viewmodel's own segment type,
  since every state change is one there, idle included: draws, reloads, attacks, inspects, silencer, grenade
  throws, knife swings), each shot from the gameplay state's timestamp; the player's viewmodel FOV, offsets and
  hand.
- **Evaluation:** CS2's viewmodel graphs through `anim::eval`, extended with the nodes they use (bone masks on layers,
  scale nodes, float maths, constant and switched IDs, transition duration overrides), onto the arms rig; the clips'
  secondary animations onto the weapon's bones; the two hand IK nodes applied by the bake.
- **Export:** one glTF per look (agent, gloves) like the third-person export: the rig, the agent's and gloves'
  first-person meshes merged by bone name, and every weapon the look's players hold, each on its own copy of its
  model's bones under `wpn` (shown only while in hand), one animation per player.
- **Unreal:** the viewmodel is attached to each player's POV camera at the player's offsets (view space, inches),
  mirrored from every tick the player holds it in the left hand, tagged `FirstPerson`; the camera's first-person
  FOV is the player's viewmodel FOV converted to 16:9; shown only while the camera looks through that player's
  eyes.

## Alternatives considered

- **An Unreal AnimBP that rebuilds the viewmodel graph.** Rejected for the reasons of decision 013: CS2's graphs and
  clips are read in Rust already, the result would live only in Unreal and could not be tested in `cargo test`.
- **Record CS2's own viewmodel** (HLAE's streams, or `ofast-capture`, M8). Exact, and a later option for a
  pixel-perfect pass; but it needs CS2 running for every shot and gives a picture, not a scene Unreal can relight.
- **Draw the viewmodel with the world's FOV, scaled to look right.** Unreal's first-person rendering does what CS2
  does (own FOV, drawn over the world) without distorting the model.

## Consequences

- The evaluator grows the viewmodel graphs' node types; the body and the viewmodel share the state tables and the
  blend constants, each with its own segment logic.
- Walking bob, landing and turn lag are not in CS2's data: a first version draws none (a still viewmodel while
  running), to be modelled and checked against CS2 later.
- Weapon skins are M10: the viewmodel shows default finishes, like the agents' weapons.
- Not in the first version: the view punch on the POV camera (the pawn networks it with the tick it was set; the
  client decays it), ironsights, the shotguns' staged reloads (they need a state's current sync event), and
  `FirstPersonScale` (with Unreal's 10 cm near plane, scaling the viewmodel towards the camera clips its near parts:
  to be tuned when a shot pokes the weapon into a wall).
- Older demos may carry a viewmodel FOV CS2 did not apply to spectators before 2026-05-07; OFAST uses the
  recorded value.

## Checked

Against CS2's first-person spectating of the Aurora vs FURIA de_nuke demo (round 5): four kills (right- and
left-handed, FOV 60, 65 and 68, three different offsets), two reloads and two inspects — the weapon's size, angle and
place and the hands match ([animation.md](../formats/animation.md#first-person-view-animviewmodel-exportviewmodel)).
