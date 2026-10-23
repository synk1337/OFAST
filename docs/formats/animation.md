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

## Procedural bones (`anim::constraints`)

Of the agent bones no clip drives, the twist bones (`arm_upper_L_TWIST`, `arm_lower_R_TWIST1`, `leg_upper_L_TWIST`…,
flag `0x400000`), `head_0_TWIST` and the shoulder blades are placed by CS2 after each pose, from constraints in the
model's keyvalues (`DATA → m_modelInfo.m_keyValueText`, KV3 text, `BoneConstraintList`), applied in list order in
model space. OFAST applies them to every exported frame, after retargeting (`export::players`, and
`export_model_with_clips`); glove skins, merged onto the agent's bones by name, follow. The other unmapped bones
(jiggle bones, eyes) stay at their bind offset from their parent.

| Class | On `ctm_sas` | What it does |
|---|---|---|
| `CTiltTwistConstraint` | 13: head, upper and lower arms, thighs | the target's rotation relative to its parent, with the target's `m_qOffset` taken off, is split into a tilt of axis `m_nTargetAxis` and a twist about it; the slave's local rotation becomes that twist × the slave's weight about its axis `m_nSlaveAxis` (`arm_lower_R_TWIST` takes 0.5 of the hand's roll, `arm_lower_R_TWIST1` all of it) |
| `CPointConstraint` | 1: `head_0_TWIST` | the slave moves to the weighted mean of the targets (`neck_0` and `head_0`, 0.5 each) |
| `CAimConstraint` | 2: `scapula_L` / `_R` | the slave's +Y turns towards the weighted mean of the targets but the last (`scap_L_AIMAT` on the spine), its +Z towards the last target (`m_nUpType` 2: `scap_AIMUP`; 0, 1 and 3 take `m_vUpVector` in that target's, the model's or the parent's frame), then by `m_qAimOffset` |
| `CBoneConstraintDotToMorph` | 2: the eyes | a morph from the dot product of two bones' directions: not applied (no morphs exported) |

Slaves and targets name bones by MurmurHash2 of the lower-cased name, seed `0x31415926`, unless `m_sName` is set;
a target can be an attachment (`m_bIsAttachment`, never on CS2's agents: such a constraint is left out). 91 of the
92 agent and glove models carry constraints, 1183 tilt-twists, 182 aims, 91 points and 160 dot-to-morphs in all;
the 11 glove models carry an agent's list over an arm-only skeleton, where the head's constraints find no bone and
the upper arms' twists no parent to measure against (left out; the export applies the agent's list anyway). A slave
carries its children along; none of the 15 slaves of any agent has any. Each frame stands alone: at the weight 0.5
a twist bone covers ±90°, so a hand rolled past ±180° flips its half-twist bone by 180° (Source 2 Viewer's unwrap
from frame to frame changes no rotation either).

Checked (`tests/cs2_anim.rs` `procedural_bones_keep_the_bind_pose`): on all 91 models with constraints, applied to
the bind pose they move no bone but the forearms' twist bones, by at most 0.66° — every agent shares one list,
authored on hands 6.6° from theirs (almost all tilt, which the twist ignores). On `ctm_sas`, the right hand rolled
+60° about its own x turns `arm_lower_R_TWIST` +30.0° and `arm_lower_R_TWIST1` +60.0°. Knowledge from Source 2
Viewer (MIT; `BoneConstraintSolver.cs`, compared line by line in review); independent implementation.

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

**The other states** (`SM` → `Ground` / `Ladder` / `InAir`, by `move_type`) are evaluated by `anim::eval`
([below](#evaluating-the-graphs-animeval-animstates)) from the state the bake decides:

| State | Selected by | Plays |
|---|---|---|
| `Ground/Standing/Starts` | `ground_action_start`, `ground_action_direction_id` (`N`, `NE` … `NW`, an `IDToFloat` index into a selector) | `planted_start_<dir>` (0.53 s), crouched: `idle_crouch` |
| `Ground/Standing/PlantAndTurn` (`…Walks` when walking) | `ground_action_plant_and_turn`, the new direction (`E2W`, `W2E`, `N2S`, `S2N`), `move_speed_horizontal_previous` ≥ 110 (60 walking) for the cross-legged variants | `planted_<e2w…>`, `planted_slow_*`, `planted_walk_*` |
| `Ground/Standing/TurnOnSpot` → `Turn Loop` | `ground_action_turn_on_spot_loop`, `ground_turn_angle_or_velocity` in degrees/s (positive left; blend points ±7, ±75, ±150, ±450) | `turn_<left/right>_loop_<45/90/180>` |
| `Ladder` | `ladder_angle_yaw` (front within ±125°), crouch | `ladder_facing_fast_<angle>` poses (`CNmAnimationPoseNode`) at `ladder_cycle` (0 – 1) |
| `InAir/SM/Jump` → `landing_blend` | `air_action_jump`; once the jump clip (0.4 s) completes, the landing blend | `jump_<stand/n/e/s/w>` (2D on velocity, crouch); then `inair_*` poses at a curve of `air_height_above_ground` |

Transitions: Ground → InAir and → Ladder at once, InAir → Ground 0.2 s (0.05 s with the clip event `MS_AIR_FINISHING`,
not read), Idle / Move → Starts 0.1 s, Starts → Move 0.2 s, Move → PlantAndTurn 0.1 s, plants back to Move 0.2 –
0.3 s. "Stopping" is the move blend running down to its idle point (CS2's `Move` → `Idle` is the same clips).

The graph does not scale a clip's playback to the actual speed. `examples/locomotion_clips.rs` estimates each
clip's own speed from its feet (a planted ankle slides back under the body): 130 – 205 units/s for the runs, 37 – 83
for the walks, below the 225 and 136 the blends place them at. Either the feet slide a little in CS2 too, or the
estimate (ankle within 1.5 inches of its lowest) is rough; not checked yet.

## The main graph's upper body (`worldmodel.vnmgraph`)

The root of the main graph is a stack (`crates/ofast-core/src/anim/{graph,layers,weapon,aim}.rs`):

```text
Snap Weapon                          wpn onto hand_R (rebuilt in anim::aim::snap_weapon)
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
hand bones (not the left hand for knives and grenades). Actions (draws, reloads, attacks, throws, defusing,
planting) pull the pitch towards level over 0.4 s and release the hands until the action clip's
`WPN_IK_ACTION_ENDING` event (`draw_ak`: 60 – 100 % of the clip; `reload_ak`: 85 – 100 %), over which they return;
an action that follows another (a pin pull, then the throw) carries on from where the first had brought the pitch.
Last, the **weapon snap** (`CNmSnapWeaponNode`, the graph's root; `aim::snap_weapon`): the weapon moves so that its
`wpnHand_R` lands on `hand_R`, the inverse of the hand IK — nothing changes while the hands are on the weapon,
and while an action lets go of them the weapon stays in the hand the layers moved. Without it, a draw's blend left
the pistol floating at the belt, away from the hand (the arm and `wpn` blend separately in model space).

## Evaluating the graphs (`anim::eval`, `anim::states`)

CS2 runs its graphs every frame with memory (state machines, springs, events); a demo carries none of it. OFAST
evaluates **one state's tree at a time**, `time` seconds after the state began, the way CS2's nodes compute their
pose (`anim::eval`, unit-tested on synthetic graphs, and on CS2's own in `tests/cs2_anim.rs`
`graph_states_play_their_clips`):

- clips at `time × speed` (wrapping when `m_bAllowLooping`), still poses at a value node's time, 1D and 2D blends
  with their sources in step (normalised time), selectors by condition, parameterised selectors by index or by
  weighted share (`deploy_variation` over weights 1 / 8 / 1, options without a clip skipped), duration and speed
  scales, layer blends, referenced graphs from their root (`m_nRootNodeIdx`);
- a state machine picks the first state whose entry condition holds, else `m_nDefaultStateIndex`, and follows
  "state completed" transitions once a state's length has passed — the jump into the landing blend, a defuse's
  enter into its loop;
- conditions on IDs, floats, ranges, bools and events given as present; springs and eases pass their input through.

The bake (`anim::states`) decides each tick what the graph's inputs would be, from the demo:

| Graph parameter | From the demo |
|---|---|
| `action`, `attack_type` | the weapon's gameplay state: 11 → `action_deploy`; 800 / 801 → `action_reload` (one action: the outro goes on with it); 600 / 601 → silencer; 200 / 201 / 202 (grenades) → `action_attack` + `attack_grenade_charge` / `_ready` / `_throw`; 500 – 505 (knives) → `attack_knife_lighthit`, `_lightmiss`, `_heavyhit`, `_heavymiss`, `_lightbackstab`, `_heavybackstab`; 400 → `action_healthshot_inject`; gun shots and inspects leave the layer off (CS2's gun graphs play nothing for them in third person) |
| restarts | `m_flWeaponGameplayAnimStateTimestamp`: two right-click misses in a row are state 503 twice, with two timestamps |
| `KnifeSwingBack` (event) | a knife attack that starts while the previous swing's clip is in its `KnifeSwingBack` event (`frontswing_knife`: 10 – 63 %) plays the `_b` swing |
| `attack_throw_strength` | the grenade's `m_flThrowStrength` (≤ 0.33: underhand) |
| `weapon_ammo` | the magazine (`m_iClip1`) when the action began: a reload from 0 plays `reload_empty_glock` and the other pistols' empty variants |
| `deploy_variation` | a repeatable pick in 0 – 1 per player and draw (CS2 picks at random): the Butterfly draws with `draw2` 8 times in 9, as the graph's weights say |
| `move_type`, `air_action` | ladder (`m_MoveType` 9); air when off the ground 3 ticks or more, or after a jump (`m_nLastJumpTick`); `air_action_jump` when the air time began with a jump, else `air_action_start_fall` |
| `air_height_above_ground` | the feet over the take-off point while rising, over the landing point while falling |
| `ground_action` | standing below 10 units/s (20 to leave a turn); a start when leaving a standstill (not just passing through one in a counter-strafe) without walking; a plant and turn when the velocity points against one of the last 0.25 s that was ≥ 60 units/s (a counter-strafe); turning on the spot when standing and the body turns faster than 15°/s (the body yaw turns smoothly in demos, ≤ 20° a tick): a `Small Turn` of the whole angle when the body settles within 10 – 45° (`turn_left_23/34/45`, `turn_right_*`, 1.3 s), the `Turn Loop` on the turn rate beyond, nothing under 10° |
| `ladder_cycle` | the feet height over 40 units per cycle (the ladder clips repeat every 0.8 s with a foot travelling about 20 units per step: an estimate) |
| `ladder_angle_yaw`, `ladder_angle_yaw_backwards` | the ladder's direction from the body, positive left (`ladder_facing_fast_45` puts the feet front-left), and that − 180° for the `back` clips beyond ±125°. On ladders the body yaw is the view's (round 5: XANTARES climbs looking 80° to his right, GoodBye with his back to the ladder), so the legs turn back to it. The ladder's direction: where the player walked from over the 16 ticks before mounting and the first 2 on it (else the body yaw then), turned by 180° when the first 8 ticks on it go down (mounted from the top, the player walks away from the ladder's face) |
| `flinch_body_type`, `flinch_head_type` | each `player_hurt`: the hit group (head; chest, stomach, arms, legs) and the side the hit came from — against `bullet_damage`'s direction in the victim's body frame (the one from the same attacker on that tick), away from the `hegrenade_detonate` position for HE damage, else towards the attacker: `flinch_body_chest_north` plays `flinch_chest` (hit from ahead), `_south` `flinch_chest_rear`, `_east` / `_west` the right and left; by weapon set (`flinch_chest_pistol`, `flinch_head_left_knife`…). Additive, from the hit to the clip's end; fall damage makes none |
| `flinch_is_on_fire` | fire damage (`inferno`, `molotov`) within 0.5 s: `flinch_molotov` |
| `flashed_amount` | the flash's duration, from the pawn's `m_flFlashDuration` when it takes a new value above 0 (the server sets it on the tick of `player_blind` to its `blind_duration`; tournament demos have no `player_blind` but still send the field; two flashes in a row of exactly the same duration count as one): 1 until the last 3 s, then the square of the time left over 3 (CS:GO's flash overlay, assumed); the `flashed` pose (arms before the face, `Flashed` mask) above 0.1 with weight 0 – 0.95, over the aim |

A change of state blends over the graph's own transition: the evaluator lists the states a pose comes from, and the
transition between the first two that differ gives the time (`Evaluator::transition_time`). Draw: Off → Deploy
0.1 s, back 0.2 s; knife attacks start at once; grenade Charge → Throw 0.1 s. **An action ends when its graph
leaves it** (`Evaluator::leaves`: the state's transitions, as CS2 runs them, for the IDs of what the gameplay state
now says — `action_inspect` for 1000 / 1001, `action_attack` for gun fire, `action_idle`…), not when the gameplay
state changes: a draw is followed one or two ticks later by state 1000, for which the knife graph's `Deploy` has no
transition (it leaves only for `action_idle` or another action), so the draw plays to its end — CS2 shows the
Huntsman raised by the face 23 ticks into its draw, and so does OFAST since (before, the draw faded out after two
ticks). The aim lets go when the gameplay state does. Roots and their clips are loaded lazily per node
(`eval::Library::load`): the three locomotion graphs, the weapons' graphs, and the main graph's planting, defuse,
body-additive, flinch and flashed states. `examples/bake_timeline.rs` prints what the bake decides for a round
(`OFAST_EVENTS=1` lists the round's game events, `OFAST_CHECK=1` checks every pose, `OFAST_PROBE=<tick>` prints bone
positions); `examples/knife_pose.rs` shows where each layer puts the hand.

## Each player's animation (`anim::bake`, `export::players`)

For every tick a player is alive: the weapon in hand (item index → name → category, from `items_game.txt` and the
main graph) picks the locomotion set, the velocity is turned into the body's frame by the body yaw, and the set's
blend gives the clips, weights and cycle length; the phase advances by the tick. A start, plant and turn, turn on
the spot, jump, fall or ladder replaces that move blend with the locomotion graph's state, blended over the graph's
transition time. Then, bottom to top (`anim::bake`): planting (`SM0/Planting`: `planting` / `planting_crouch`, the
whole body, 0.1 s in and out) while the C4's state is 300; the weapon's additive idle pose (blended by crouch); the
weapons layer in model space (`UpperBody`, `UpperBody_Ladder` on ladders) from the weapon's own graph for its
action, handing over from one action to the next and fading out over the graph's times; the defuse layer in model
space (`Defuse` mask, `Defuse_Grenade` with a grenade in hand: the enter clip, then the loop; 0.2 s in, 0.4 s out);
the body additives (`jump_additive_start` from take-off, `jump_additive_land` from landing to its end, `breathing`
while standing idle); the weapon's additive shot from each tick the state enters 100 or 101; the flinches of each hit
and while burning (additive); the aim from the eye angles and body yaw, with the weapon snapped onto the hand; and the
flashed pose over it while flashed. A drawn weapon stays in its holster for the draw's first 0.1 s (the blend into `Deploy`), then
the draw clip, which begins with the hand at the holster, takes it from there. Poses are keyed on every tick (64 per
second, `export::players::animation_fps`) from the round's first tick, retargeted onto the player's own agent (the
pawn's `m_hModel`, [demo.md](demo.md#models)) and written as one glTF per look — agent, mesh groups, glove skin —
with every weapon the look's players carry (below):

```bash
ofast export-demo match.dem --round 5 --out exports/demos --players
# → <demo>_r5.ofast-replay.json, <demo>_r5.agents.json (the files and their players),
#   <demo>_r5_ctm_sas_5.gltf (3 players, 9 weapons), <demo>_r5_ctm_sas_6.gltf (2 players with defuse kits,
#   12 weapons), <demo>_r5_tm_phoenix_1.gltf (5 players, 13 weapons)
```

A 76-second round: 10 players × 4860 frames and 34 weapon bones, 27 – 51 MB per look, in 14 s.
`tools/unreal/import_demo.py` imports them (at the tick rate) and plays each player's animation on an agent actor
moved along the replay's positions and turned along its body yaw ([replay.md](replay.md#in-unreal-import_demopy)).
Until 2026-10-22 the animations were keyed at 30 fps (a rate Unreal's glTF import guesses by itself): a weapon
switch then fell between keys and the weapon vanished for 66 ms around it — a skinned knife held up to the tick of a
switch was already gone in the frame compared with CS2.

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
hand) hides on the last tick at the old place and the first at the new one, and a hidden frame is keyed at the
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
  weapons render too metallic (CS2's weapon shader is not converted yet, like the agents' character shader). (The
  draw was fixed in M4.4c, below.)
- **Skinned knives** (2026-10-22, close-ups from `import_demo.py`'s `-CloseShots`, `test-data/shots/replay_r5d`):
  the Bowie (Wajam, tick 26041) and the Huntsman (tick 26508) are in their owners' right hands, blade up, posed by
  their own `idle_bowie` / `idle_tactical` clips. They are hard to see from a distance: the blade renders near-black
  (its colour texture is dark grey, sRGB ≈ 100, and fully metallic, so Unreal makes a dark mirror of it; CS2's
  weapon shader shows bright steel), seen edge-on it is a thin line, and CS2 holds the Huntsman raised by the head
  while OFAST holds it at the belly, behind the arm from a teammate's view (the knife pose, M4.4c). The paint of
  knife skins is not applied (M10).
- **CS2's graphs evaluated** (2026-10-22, M4.4c): `tests/cs2_anim.rs` `graph_states_play_their_clips` — the
  Huntsman's graph plays `frontswing_knife` for a light miss, `frontswing_b_knife` with the `KnifeSwingBack` event,
  `backstab_overhead_knife` for a heavy backstab, the crouched swing when crouched, `draw_tactical` when deploying
  and nothing when idle; the HE graph `pullpin_grenade`, `throw_overhand_grenade` at strength 1 and
  `throw_underhand_grenade` at 0.2; the knife locomotion graph `jump_n_knife` 0.1 s into a forward jump and the
  landing blend 0.6 s in, `planted_start_nw` / `_e` by direction (0.53 s long); the defuse layer
  `defuse_enter_rifle` (1 s) then `defuse_loop_rifle`; transitions Off → Deploy 0.1 s, Deploy → Off 0.2 s, Off →
  attack 0 s, Ground → InAir 0 s, InAir → Ground 0.2 s. `tests/cs2_demo.rs` `bake_decisions_follow_the_demo`: over
  rounds 1 – 5 of the match demo, every knife attack and grenade throw the demo records starts its action on its
  timestamp tick (180), and every ladder climb (13), C4 plant (3) and defuse (2) begins its state on its first
  tick; 193 jumps; since M4.4d also every hit while alive (a flinch, or burning for fire) and every flash, 93; every
  baked pose finite.
- **Against CS2** (2026-10-22, M4.4c), round 5 through players' eyes (Unreal `-PovShots`, CS2 demo playback and
  Steam F12; side by side in `test-data/shots/agents/cs2_m44c/compare_m44c.png`): XANTARES's jump at 26580 (8
  ticks after take-off) is at the same height with the same tucked knees and trailing foot; his heavy knife miss at
  26690, seen from behind, is in the same stride with the left arm forward; the smoke pin pull at 26910 and the
  throw's follow-through at 26935 match in stance and arm direction (CS2's throwing arm a little more extended);
  the defuse loop at 29560 stands the same, slightly crouched with both hands at the waist. Differences: CS2 holds
  the knife raised by the shoulder in the jump (the raised knife pose, still open); on the first ladder tick (P1,
  26815) CS2 still tucks the legs from the jump onto the ladder where OFAST already climbs; CS2 shows the defuse
  kit's cutter and the C4, which OFAST does not render. The draw from the holster
  (`compare_draw_25824.png`): at tick 25824 vg is 5 ticks into drawing his M4A4 and CS2 still shows it slung — so
  does OFAST now (M4.5 put it in his hand at once). In close-ups (`test-data/shots/replay_r5f`, `replay_r5g`):
  a Glock drawn at the round start stays in its thigh holster for the first 0.1 s, then comes out in the hand; the
  C4 plant kneels; before the weapon snap, a draw's blend left the pistol floating at the belt away from the hand.
- **The knife held up during its draw** (2026-10-22, M4.4d; `compare_knife_draw_zoom.png` in
  `test-data/shots/agents/cs2_m44c`): CS2 shows КАРДАННЫЙ ВАЛ's Huntsman raised by the face, blade up, at tick
  26508 — the draw he began at 26485, still playing although the demo's state is 1000 since 26487 (the user checked
  it in game: "it's just the deploy animation"). OFAST had faded the draw out after two ticks and held the knife at
  the hip; with actions ending by the graph's transitions it now holds it the same way, and vg's CT knife 2.6 s
  after his draw matches as well. The CS2 reference was retaken by seeking 64 ticks early while paused and letting
  `demo_step_tick 64` play into the tick, so that no pose is left stale by the seek (the user's advice: seeking while
  paused can leave things misplaced). The real-graph test also checks flinches (`flinch_chest_rear` for a rifle hit
  from behind, `flinch_leg_left_pistol`, `flinch_head_left_knife`) and the flashed pose.
- **A major's demo against CS2** (2026-10-22; Aurora vs FURIA, de_nuke, round 5; side by side in
  `test-data/shots/agents/cs2_nuke_r5/compare_nuke_r5_a.png` and `_b.png`): seven views through CS2's Chase Cam,
  rendered in Unreal from the camera CS2's `getpos` printed (`import_demo.py -ViewShots`), so both show the same
  frame (map, players and their places line up). Every player wears a glove skin (Specialist, Sporty), bone-merged onto the agent's hands as CS2 draws
  them; their paint (purple-blue, red) is not applied (M10). KSCERATO's knife inspect at freeze time, Wicadia
  holding his FAMAS in a molotov, FalleN flashed (right forearm across the face), MAJ3R flashed with an HE in hand
  (the arm across the face, seen from behind) and XANTARES flashed while spraying crouched match CS2's poses;
  XANTARES's second flash (2 s, 0.66 s in) shows little in either. Differences: no fire or flash effects (M6), the
  hall's interior darker than CS2's, and seen from very close the SAS sleeve over a raised forearm is broad and flat
  in both, CS2's thinner at the cuff. With the procedural bones (above; `twist_before_after.png`,
  `twist_vs_cs2.png`) XANTARES's forearm twist bones turn 8 – 35° and his right scapula 30° at 34480, yet the
  sleeves barely change: their breadth is the sleeve's own. In MAJ3R's view CS2's arm reaches further out to the
  side than OFAST's (an HE drawn while flashed); FalleN's flashed arm matches.
- `tests/cs2_anim.rs` `weapon_clips_found_in_the_graphs` finds every tested weapon's clips and category through the
  graphs; `item_names_read` reads 2059 item names from `items_game.txt`.
- `tests/cs2_anim.rs` `locomotion_graphs_read`: the three locomotion graphs give the blends above (17 standing and
  9 crouched points, `run_n` at 225, all clips decoded); `tests/cs2_demo.rs` `players_export_one_animation_per_player`:
  round 1 exports one animation per player, keyed on every tick over the whole round, one file per look with a bone per weapon
  listed (every player of the pistol round carries a knife and a pistol); `glove_skins_merge_onto_the_agent`: glove
  skins replace the agent's own gloves in the look's file.

## Not done yet

- **Secondary clips** (weapon parts) are decoded but not exported.
- **Sync tracks** are not read (the locomotion blends share one phase instead); of the clip events only
  `WPN_IK_ACTION_ENDING` and `KnifeSwingBack` are used (not `MS_AIR_FINISHING`, the reload's `WPN_RELOAD_OUTRO`, nor
  the states' timed events such as `WPN_DISABLE_HAND_IK` in a deploy's first 0.3 s).
- **The rest of the graphs**: foot IK (needs the ground under each foot); bone masks inside layer blends (the
  layers OFAST evaluates take the masks of the main graph's layers instead). The aim's weapon drop and tilt and
  roll off to the side.
- **Estimates**: the ladder's direction (from the approach: demos have no ladder normal), the ladder cycle height,
  the flash fading (CS:GO's overlay curve), the fire flinch's end.
- **Not checked against CS2 yet**: the thresholds of `anim::states` (starts, plants and turns, turns on the spot),
  the legs on ladders (CS2's chase camera shows only the torso), flinches (the round's hits are seen only from 16 m),
  the flash's fading curve.
- **Eye morphs**: the `CBoneConstraintDotToMorph` constraints turn the eyes' look morphs from the eyeballs' aim at
  `eye_target`; OFAST exports no morphs.
- **Glove paint**: glove skins' models are merged (checked against CS2 on a major's demo), their paint kits not.
