# Animation (AnimGraph 2: `.vnmskel`, `.vnmclip`)

CS2 animates players with AnimGraph 2 only. A clip stores bone poses for one skeleton; a graph blends clips from
gameplay parameters at run time. OFAST reads the skeletons and clips (`crates/ofast-core/src/anim/`), puts their poses
on an agent model's own skeleton and writes them as glTF animations, which Unreal imports as AnimSequences. The
graph itself is not read: [decision 013](../decisions/013-replay-rig-baked-in-rust.md) rebuilds the blending in Rust
from demo data (M4.4).

Knowledge from ValveResourceFormat (MIT; `ResourceTypes/ModelAnimation2/AnimationClip.cs`,
`IO/SkeletonRetargeter.cs`, `IO/Gltf/GltfModelExporter.Anim.cs`), checked on local CS2 files; OFAST's code is its own.

## Where things are

```text
animation/skeletons/characters/worldmodel.vnmskel   75 bones: every third-person player clip is made for it
animation/skeletons/characters/viewmodel.vnmskel    first-person arms (M5)
animation/anims/world/<class>/<set>/*.vnmclip       third-person clips (rifle/_default_rifle/run_n_rifle, …)
animation/graphs/worldmodel/worldmodel.vnmgraph     the graph that blends them (not read)
agents/models/<agent>/<agent>.vmdl                  an agent: its own skeleton (86 – 94 bones) and skinned meshes
```

The main archive holds 2735 `.vnmclip_c` files for four skeletons: `worldmodel` (1960), `viewmodel` (656),
`chicken` (104) and `pristine_egg_world` (15). Both kinds are resources with a single KV3 `DATA` block.

## Clip skeleton (`.vnmskel_c`)

| Key | Value |
|---|---|
| `m_boneIDs` | bone names in track order (`root_motion`, `pelvis`, …, `wpnPivot`, `attachWorld`, `camera`) |
| `m_parentIndices` | parent index, −1 for the root; parents come first |
| `m_parentSpaceReferencePose` | per bone, 8 floats `[tx, ty, tz, scale, qx, qy, qz, qw]` relative to the parent |
| `m_modelSpaceReferencePose` | the same in model space |
| `m_maskDefinitions` | named bone weight masks (UpperBody, Flinch, …) the graph uses |

`worldmodel.vnmskel`: `root_motion` at the identity, `pelvis` at (−2.85, 0, 42.82) inches with rotation (0.5, 0.5,
0.5, −0.5). Its model-space reference pose is an agent's bind pose: every bone both share lands within 0.003 inches
of the agent's (checked on `ctm_sas` with `examples/anim_inspect.rs`), except `wpn`, 59.9 inches apart, a weapon
placement bone the agent leaves at its bind pose.

## Clip (`.vnmclip_c`)

| Key | Value |
|---|---|
| `m_skeleton` | the `.vnmskel` whose bone order the tracks follow |
| `m_nNumFrames`, `m_flDuration` | frames and seconds; moving clips are 30 fps (`duration = (frames − 1) / 30`), a still clip can store 2 frames over its whole length (`idle_rifle`: 0.73 s) |
| `m_trackCompressionSettings` | one entry per skeleton bone (below) |
| `m_compressedPoseData` | blob of little-endian `u16`, every frame's dynamic values |
| `m_compressedPoseOffsets` | start of each frame in that stream, in `u16` units |
| `m_bIsAdditive` | the poses are deltas over another pose (252 clips: per-weapon idles, shoot, flinch, breathing) |
| `m_secondaryAnimations` | nested clips for other skeletons, same layout (a weapon's magazine and bolt during a reload) |
| `m_events`, `m_syncTrack`, `m_rootMotion` | timed events (`CNmIDEvent` such as `WPN_IK_ACTION_ENDING` on a reload), sync markers, root motion (identity transforms on the player clips checked) |

A track's settings: `m_translationRangeX/Y/Z` and `m_scaleRange` (`m_flRangeStart`, `m_flRangeLength`),
`m_constantRotation` (a quaternion), and `m_bIsRotationStatic`, `m_bIsTranslationStatic`, `m_bIsScaleStatic` (no
data in the stream for that part).

### Decoding a frame

Start each track from its static values: the constant rotation, translation = each range's start, scale = the scale
range's start. Then read the frame from its offset, track after track in bone order:

- **rotation** (unless static), 3 × `u16`, "smallest three": `a = d0 & 0x7FFF`, `b = d1 & 0x7FFF`, `c = d2`, each
  mapped to `n × (√2 / 32767) − 1/√2`. The largest component, left out, is `√(1 − a² − b² − c²)`, at index
  `((d0 >> 14) & 2) | (d1 >> 15)` (0 = x … 3 = w); a, b, c fill the other slots in order;
- **translation** (unless static), 3 × `u16`: `start + u / 65535 × length` per axis (inches, parent frame);
- **scale** (unless static), 1 × `u16`, the same way.

So a frame takes 3 values per dynamic rotation, 3 per dynamic translation and 1 per dynamic scale, and the frames
follow each other with no gap. A time between two frames interpolates them (translations linearly, rotations along
the shorter way, renormalised: `Clip::sample`).

## Retargeting onto an agent

A clip's tracks follow its `.vnmskel`, not the agent: the agent adds twist, jiggle and aim bones (94 bones on
`ctm_sas`, 64 of them driven by a `worldmodel` clip). `Retargeter` follows Source 2 Viewer's rule: an agent bone
named like a clip bone (exact, then ignoring case) takes that bone's **model-space** pose; any other bone keeps its
bind-pose offset from its parent; each bone's local transform is then taken back from those world poses. Matching
world poses makes the result independent of how each skeleton splits a transform between a bone and its parent (the
agent puts the A-pose's turn on `root_motion`, the clip skeleton on `pelvis`).

## glTF export

`ofast export-model <agent> --clip <clip> [--clip …]` (`export_model_with_clips`) adds one glTF animation per clip,
named after the clip file: a translation and a rotation channel per bone, linear, in the same conventions as the
skin's bone nodes (a root bone converted from Source to glTF axes, the others in metres). Keys are written on a 30 fps
grid, sampled from the clip: Unreal's glTF import guesses an animation's frame rate from its key times, and the 2
frames of a still clip would read as 1.4 fps. Additive clips are skipped with a warning.

```bash
ofast export-model agents/models/ctm_sas/ctm_sas.vmdl_c --out exports/ctm_sas \
    --clip animation/anims/world/rifle/_default_rifle/run_n_rifle.vnmclip
```

A channel that never changes (a bone the clips do not move) is written with two keys, at the start and the end,
instead of one per frame; rotations are kept in one hemisphere from key to key, so interpolating takes the short way.

## Locomotion graph (`worldmodel_locomotion.vnmgraph`)

A compiled graph is KV3 in the `DATA` block: `m_variationID`, `m_controlParameterIDs` (the graph's inputs, by
index), `m_nodePaths` (editor names), `m_resources` (the clips, by data slot) and `m_nodes`, each with `_class`
(`CNmClipNode::CDefinition`, `CNmBlend2DNode::CDefinition`, `CNmStateMachineNode::CDefinition`…) and indices of the
nodes it reads. Third-person locomotion has three variations, `+rifle`, `+pistol` and `+knife`, with the same nodes
and different clips (`animation/anims/world/<set>/_default_<set>/`); their legs move identically, only the upper
body differs. Its control parameters: `move_speed_horizontal`, `move_crouch_amount`, `move_type`,
`air_height_above_ground`, ladder angles, `ground_action`, `move_speed_x`, `move_speed_y`,
`ground_turn_angle_or_velocity`, `move_is_walking`, `move_crouch_amount_eased`…

**Moving on the ground** (`crates/ofast-core/src/anim/locomotion.rs`) is a `CNmBlend1DNode` on `move_crouch_amount`
(0 – 1) between two `CNmBlend2DNode`s on `move_speed_x` (forward) and `move_speed_y` (left), in units/s:

| 2D blend | Points (`m_values`) |
|---|---|
| standing | `idle` at (0, 0); `run_n, ne, e, se, s, sw, w, nw` at 225 (diagonals ±159, ±159); `walk_*` at 136 (±96) |
| crouched | `idle_crouch` at (0, 0); `crouch_*` at 96 (±67) |

Each 2D blend stores its triangles (`m_indices`, 24 for standing) and outline (`m_hullIndices`, the 8 outer points).
A velocity inside a triangle blends its three clips with barycentric weights; outside the outline it takes the
nearest point of the outline (a knife runs at 250: the run clips alone). The idles sit under a
`CNmDurationScaleNode` (scale 1) and play at `m_flSpeedMultiplier` 0.167; every other clip at 1. The clips play in
step: one normalised phase for all, advancing by `dt / D` with `D` the weighted average of the sources' durations at
their speeds (run 0.733 s, walk 1.0 s, crouch 0.8 s, idle 0.73 / 0.167 = 4.4 s, so mixing in the idle slows the
stride at low speed). This is AnimGraph 2's synchronised blending (Esoterica's design) simplified to one phase; the
clips' foot sync tracks are not read.

**The other states** — starting, stopping and planted direction changes (`planted_*`), turning on the spot
(`turn_*`), jumps and air (`jump_*`, `inair_*`, 2D blends on the eased crouch amount), ladders — are not rebuilt yet.

The graph does not scale a clip's playback to the actual speed. `examples/locomotion_clips.rs` estimates each
clip's own speed from its feet (a planted ankle slides back under the body): 130 – 205 units/s for the runs, 37 – 83
for the walks, below the 225 and 136 the blends place them at. Either the feet slide a little in CS2 too, or the
estimate (ankle within 1.5 inches of its lowest) is rough; not checked yet.

## The main graph's upper body (`worldmodel.vnmgraph`)

The root of the main graph is a stack (`crates/ofast-core/src/anim/{graph,layers,weapon,aim}.rs`):

```text
Snap Weapon                          wpn onto hand_R, left hand IK (not rebuilt)
└ Layer Blend (model space)          + flashed (not rebuilt)
  └ Aim IK (CNmAimCSNode)            procedural aim, rebuilt in anim::aim
    └ Foot IK                        (not rebuilt)
      └ Layer Blend
          base: SM0 → Locomotion = Layer Blend(locomotion set, + Idle Poses (additive, UpperBody))
          + Weapons        (model space, UpperBody): the weapon's own graph — draw, reload, silencer, knife attacks
          + Defuse         (model space)
          + BodyAdditives  (additive): breathing, jump start / land
          + Weapon Shoot   (additive, UpperBody): shoot_<weapon>
          + Body / Head / Fire flinches (additive)
```

States are selected by ID parameters: `weapon_category` (`weapon_category_rifle`, `_pistol`, `_knife`, `_smg`,
`_shotgun`, `_sniper`, `_machinegun`, `_grenade`, `_equipment`), `weapon_type` (the item's name in
`scripts/items/items_game.txt`: `weapon_ak47`, `weapon_m4a1` for the M4A4, `weapon_knife` for the CT knife…),
`action` (`action_idle`, `_deploy`, `_attack`…), `attack_type`. `anim::graph::Graph` reads a graph's nodes and
evaluates state entry conditions (ID comparisons with and / or / not) for given values; `anim::weapon::WeaponClips`
walks Idle Poses, Weapon Shoot and Weapons to a weapon's clips, e.g. for `weapon_ak47`: `idle_ak` / `idle_crouch_ak`
(additive), `shoot_ak` (additive), `draw_ak` / `draw_crouch_ak` and `reload_ak` / `reload_crouch_ak` (from
`worldmodel_gun.vnmgraph+ak47`). The locomotion set follows the category: pistols → pistol, knives and grenades →
knife, rifles, SMGs, shotguns, snipers and machine guns → rifle (equipment matches no state).

**Additive clips** store per-bone deltas in the bone's own frame: the delta rotation multiplies the base on the right
(`base × delta`), translations and scales add (scale deltas from 0). Checked: `idle_ak` over the rifle set's
`idle_rifle` gives its shipped `idle_ak.vnmclip+non_additive` within 0.04° (`examples/additive_probe.rs`); the CT
knife's `idle_default_ct` is all identity (the knife set's carry is already right).

**Bone masks** (`.vnmskel` `m_maskDefinitions`): `UpperBody` lists `spine_0` and the weapon bones (`wpn`,
`wpnPivot`, `wpnTip`, `wpnEnd`, `wpnHand_L`, `wpnHand_R`) at 1; every other bone takes its nearest listed ancestor's
weight, so the spine, arms, head and weapon get 1 and the pelvis and legs 0. **Model-space layers** blend each masked
bone's model-space rotation towards the layer's and recompute its local rotation under the already blended parent.

**Aim** (`CNmAimCSNode`, engine code, rebuilt from Source 2 Viewer's reconstruction): the view yaw relative to the
body (−89 … 89°, remapped into −55 … 65°) turns `spine_0 … 3` about the up axis by 0.1 / 0.2 / 0.3 / 0.4 of it; the
pitch (positive down) bends the spine about each bone's local Z by 0.125 / 0.225 / 0.25 / 0.4 of a per-category range
(rifles −25.5 … 25°) and the neck / head by 0.35 / 0.65 of another (rifles −35 … 20°); the weapon pivot moves onto
`spine_3`; knives pitch the pivot, rifles and pistols pitch the weapon, shift it at extreme pitches and turn the
barrel onto the aim direction; rifles turn the head towards the muzzle; a two-bone IK puts the hands on the weapon's
hand bones (not the left hand for knives and grenades). Draws and reloads pull the pitch towards level over 0.4 s and
release the hands (CS2 releases them by clip events, not read here).

## Each player's animation (`anim::bake`, `export::players`)

For every tick a player is alive: the weapon in hand (item index → name → category, from `items_game.txt` and the
main graph) picks the locomotion set, the velocity is turned into the body's frame by the body yaw, and the set's
blend gives the clips, weights and cycle length; the phase advances by the tick. Over that: the weapon's additive
idle pose (blended by crouch), its draw or reload clip in model space while the weapon's gameplay state is 11 or
800 / 801 (from the state's first tick, fading in over 0.15 s and out over 0.25 s), its additive shot from each tick
the state enters 100 or 101, and the aim from the eye angles and body yaw. Poses are sampled at 30 fps from the
round's first tick, retargeted onto the player's own agent (the pawn's `m_hModel`, [demo.md](demo.md#models)) and
written as one glTF per look — agent, mesh groups, glove skin — with every weapon the look's players carry
(below):

```bash
ofast export-demo match.dem --round 5 --out exports/demos --players
# → <demo>_r5.ofast-replay.json, <demo>_r5.agents.json (the files and their players),
#   <demo>_r5_ctm_sas_5.gltf (3 players, 8 weapons), <demo>_r5_ctm_sas_6.gltf (2 players with defuse kits,
#   12 weapons), <demo>_r5_tm_phoenix_1.gltf (5 players, 12 weapons)
```

A 76-second round: 10 players × 2278 frames and 32 weapon bones, 19 – 27 MB per look, in 13 s.
`tools/unreal/import_demo.py` imports them and plays each player's animation on an agent actor moved along the
replay's positions and turned along its body yaw ([replay.md](replay.md#in-unreal-import_demopy)).

## Weapons in hand and holstered (`export::loadout`)

**In hand.** A weapon's world model has a root bone `weapon`; CS2 puts it on the agent's `weapon` attachment, which
is the `wpn` bone itself (no offset) — the bone the clips and the aim move. The weapon models say so
(`examples/weapon_grip.rs`): their right-hand guide bone `ag1_hand_r` sits at one place relative to `weapon` per
class — (−3.41, 0.19, −4.51) inches on every rifle checked (AK-47, M4A4, AWP), (−2.80, 0, −3.05) on every pistol
(Glock-18, Desert Eagle) — close to where `idle_*` clips hold the hand relative to `wpn` ((−5.7, −1.6, −5.1) for the
AK-47: the rest is the wrist-to-grip distance), while the models' origins are anywhere relative to `weapon` (the
M4A4's is 22 inches away), so nothing else could be the reference.

**Holstered.** Carried weapons not in hand hang on the agent's attachments ([model.md](model.md#attachments)),
their `weapon_holster_center` onto the attachment: rifles, snipers, shotguns and machine guns on `primary`
(`items_game.txt`'s `addon_location` `primary_rifle` / `_sniper` / `_shotgun` / `_mg`), SMGs on `primary_smg`,
pistols on `pistol` (right thigh), knives on `knife` (left thigh), flashbangs on `grenade0` and `grenade1`, the HE on
`grenade2`, the smoke on `grenade3`, the decoy on `grenade4` (CS:GO's order), the C4 on `c4`, the Zeus on
`eholster`. Molotovs, incendiaries and the Healthshot are not shown (CS:GO showed none; not checked in CS2). Two
flashbangs are one entity: the pawn's ammo count for flashbangs (`m_iAmmo[14]`, [demo.md](demo.md#models)) says 2,
and the second copy goes on the next grenade attachment (`grenade0` beside one in hand, `grenade1` beside one
holstered).

**Baked.** Each look's file gives every weapon its players carry (a weapon model and mesh-group mask, and which
copy: a second flashbang is a second bone) a bone of its own, `ofast_weapon_<n>`, child of the agent's root, with
the weapon rigidly on it. Every frame of a player's animation puts that bone on `wpn`, on the holster, or scales
it to 0 (step keys) while the player does not carry the weapon. A weapon moving between two places (holster to
hand) hides on the last frame at the old place and the first at the new one, and a hidden frame is keyed at the
place of its neighbour that shows the weapon (`loadout::hide_changes`, `key_places`): Unreal blends the scale
between keys, so the weapon shrinks and grows where it is and only travels while hidden — it never sweeps through
the body. In Unreal the weapons are part of the agent's skeletal mesh: no attached actors, and scrubbing shows the
same weapons as playback.

## Verified

- `crates/ofast-core/tests/cs2_anim.rs` (2026-10-18): **all 2735 clip files, 4309 clips with the nested ones, decode;
  each frame starts exactly where the previous one ends and the last one ends the stream; every rotation is a unit
  quaternion to 1.2 × 10⁻⁷** (226,333 frames).
- Retargeting `worldmodel`'s reference pose onto `ctm_sas` gives its bind pose (above).
- **In Unreal** (2026-10-18): `ctm_sas` with `run_n_rifle`, `idle_rifle` and `crouch_n_rifle` imports as one
  skeletal mesh and three AnimSequences at 30 fps (22, 22 and 24 frames). Posed with `run_n_rifle` at 0, 0.2 and
  0.4 s (`tools/unreal/run_shot.ps1 -Anim run_n_rifle -AnimTimes 0,0.2,0.4`), the pelvis, head, both hands and the
  right ankle sit where `examples/anim_inspect.rs` computes them from the clip, to 0.1 cm; the right ankle swings
  from 3.4 cm ahead of the root to 65.8 cm behind and back. Screenshots in `test-data/shots/agents/ctm_sas_run_*`.
- **Against CS2** (2026-10-18): in the local match demo (de_nuke, round 5, ticks 26508 – 26528) CT "vg", an SAS
  agent, runs straight at 250 units/s with his knife out (rifle slung), seen almost side-on by a teammate 4.7 m
  away. The demo's rig inputs (M4.3) identify him: his weapon in hand is the CT knife (item 42); P1 next to him,
  hidden behind another teammate, holds an M4A4. Unreal renders `ctm_sas` with `run_n_knife` from that teammate's
  eye, placed relative to vg from the demo's positions and view angles, with CS2's field of view
  (`run_shot.ps1 -Camera`): the agent lands where CS2 draws vg, within 10 pixels of a 1920-wide frame. The legs go
  through the same stride as in CS2 — the rear foot kicked up behind, the long reach of the front leg, the leg
  nearer the camera in front at the same moments — with clip times 0.40 – 0.70 s against ticks 26508 – 26528. The
  upper body does not match: CS2 reaches the left arm forward and leans the torso further. Those come from the
  graph's weapon and aim layers ([decision 013](../decisions/013-replay-rig-baked-in-rust.md), M4.4), not from the
  base clip. Side by side: `test-data/shots/agents/cs2_run/compare_knife.png`. (The first version of this check
  named P1 as the runner, from his apparent distance; the rig inputs corrected it.)
- **A whole round in Unreal** (2026-10-18, M4.4): round 5 of the match demo exported with `--players` and imported
  with `import_demo.py`: 10 animated agents in the de_nuke level (sequence built in 9 s, 70,991 keys). Through the
  eyes of the same teammate at ticks 26508 – 26528 (`-PovShots`), the three CTs running along the wall stand where
  CS2 draws them, face the same way and run through the same stride cycle; the phase within the stride can differ
  (CS2's carries the history before the round, OFAST's starts at the round's first tick), and the arms differ: CS2's
  weapon layers (knife and grenade in hand, the reaching arm) are not baked yet. In the killers' views the
  Terrorists show as Phoenix agents. Side by side: `test-data/shots/replay_r5/compare_locomotion_r5.png`.
- **Weapon layers and aim against CS2** (2026-10-18, M4.4b), through players' eyes in the same round
  (`test-data/shots/replay_r5b/compare_rifles_r5.png`): P1 drawing his M4A4 at tick 25824, seen from the front by a
  teammate, holds his hands together at the chest as in CS2, and so does the CT beside him; bad_klan firing his
  M4A4 at tick 27664, seen from the front by XANTARES, stands in the same rifle hold with both arms raised forward,
  but CS2's hands sit higher, at the sights, and its head drops onto the gun while OFAST's stays upright. The knife
  runners of ticks 26508 – 26528 barely change (`compare_layers_r5.png`): the CT knife's idle pose is the identity,
  and CS2's more upright knife run is not explained yet.
- **Weapons against CS2** (2026-10-22, M4.5), the same views (`test-data/shots/replay_r5c/compare_weapons_r5.png`,
  close-ups `zoom_26508_*.png`, `zoom_25824*.png`): at tick 26508 the runner with the CT knife has it in his right
  hand, blade up, and his M4A4 slung from the right shoulder down to the left hip, the pistol in the right thigh
  holster and the HE grenade on the belly — all as CS2 draws them; the runner in front carries his AK-47 on the same
  diagonal, a grenade on the left hip, the pistol on the right thigh, as in CS2. At tick 25824 P1, drawing, holds
  his M4A4 at the chest pointing down towards the viewer, as in CS2. Differences: vg switched to his rifle on that
  very tick and CS2 still shows it slung (CS2's draw takes the weapon from the holster partway through the clip,
  timed by clip events; OFAST puts it in hand at once); CS2's knife hand is raised (the knife pose, M4.4c); the
  weapons render too metallic (CS2's weapon shader is not converted yet, like the agents' character shader).
- `tests/cs2_anim.rs` `weapon_clips_found_in_the_graphs` finds every tested weapon's clips and category through the
  graphs; `item_names_read` reads 2059 item names from `items_game.txt`.
- `tests/cs2_anim.rs` `locomotion_graphs_read`: the three locomotion graphs give the blends above (17 standing and
  9 crouched points, `run_n` at 225, all clips decoded); `tests/cs2_demo.rs` `players_export_one_animation_per_player`:
  round 1 exports one animation per player, 30 fps over the whole round, one file per look with a bone per weapon
  listed (every player of the pistol round carries a knife and a pistol); `glove_skins_merge_onto_the_agent`: glove
  skins replace the agent's own gloves in the look's file.

## Not done yet

- **Additive clips**: composing a delta onto a base pose (for the weapon idle, shoot and flinch layers).
- **Secondary clips** (weapon parts) are decoded but not exported.
- **Events and sync tracks** are not read (the locomotion blends share one phase instead).
- **The rest of the graphs**: knife attacks, grenade throws, planting and defusing, the silencer, breathing,
  flinches, the flashed layer, foot IK, the weapon snap; the locomotion graph's start, stop, turn, jump, air and
  ladder states. The aim's weapon drop, C4-planting fade, and tilt and roll off to the side.
- **Clip events** (`WPN_DISABLE_HAND_IK`, `WPN_IK_ACTION_ENDING`…) that time CS2's hand IK and action blends: OFAST
  releases the hands for a whole draw or reload instead, and puts a drawn weapon in hand on the first tick instead
  of taking it from the holster when the hand reaches it.
- **Glove skins against CS2**: merged and tested on the export side, never seen in the game (no player of the local
  demo wears glove skins).
