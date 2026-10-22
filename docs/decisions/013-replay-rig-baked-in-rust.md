# 013 — Players: skinned agents, and their animation baked in Rust from demo data

- **Date:** 2026-10-18
- **Status:** accepted (M4.1 – M4.5 done: skinned agents, clip decoder, rig inputs, baked locomotion read from
  CS2's own locomotion graph, weapon layers and aim, per-player agents with their weapons baked as bones; the rest
  of the graph and ragdolls to come)

## Context

M3 puts each player of a demo round in Unreal as a proxy shape on a transform track with a first-person camera.
M4 replaces the proxies with CS2's own agents, animated. Four research passes (2026-10-18) established:

- **Models.** Agents are `agents/models/<agent>/<agent>.vmdl_c`: a skeleton of 86 – 94 bones, four bone influences
  per vertex, a per-mesh bone remapping table ([model.md](../formats/model.md#skeleton-and-skinning)).
- **Animation.** CS2 uses AnimGraph 2 only: 2735 `.vnmclip` clips, 232 `.vnmgraph` graphs, 70 `.vnmskel` skeletons.
  Third-person clips are under `animation/anims/world/` (locomotion per weapon class in 8 directions, idle, crouch,
  jump, in-air, per-weapon idle / shoot / reload / draw, defuse, plant, deaths, flinches). Clips are 30 fps, poses
  compressed per track (quantised translations, smallest-three quaternions in 3 × u16); Source 2 Viewer decodes them
  (`ModelAnimation2/AnimationClip.cs`). The `worldmodel` graph composes: a 2D blend of locomotion clips on local
  velocity (runs at 225 u/s, walks at 136, crouch walks at 96), blended by crouch amount; an additive per-weapon idle
  layer under an upper-body mask; weapon actions (deploy, reload) in model space; additive shoot, flinch, breathing,
  jump start / land. **Aim (pitch / yaw over the spine, neck and head, hand IK) is game code**, not clips. Deaths are
  ragdolls (the agent has a `PHYS` block), not clips of the graph.
- **Demos** carry no bone poses (the pawn's serialised animation recipe is undocumented), but per tick: body yaw,
  velocity, ground flag, duck amount, walking, scoped, ladder move type, the active weapon (handle → item definition),
  its gameplay animation state (deploy 11, idle 50, shoot 100 / 101, reload 800, grenade 200 – 202, plant 300, knife
  500 – 505…), reload flag, jump and landing ticks, defusing, flash, and death data (ragdoll force and bone). The
  agent model of each pawn is `m_hModel` = MurmurHash64B of its path (seed 0xEDABCDEF), so the exact agent, weapon,
  knife and glove models are known.
- **Unreal.** Interchange imports glTF skins as Skeleton + SkeletalMesh + PhysicsAsset and glTF animations as
  AnimSequences (frame rate set to 64 with `custom_bone_animation_sample_rate`, or keys written through the
  animation controller). Python cannot author Animation Blueprints, state machines or blend spaces in 5.8, and an
  Animation Blueprint depends on tick history, so scrubbing and Movie Render Queue would not match playback.

## Decision

1. **Agents are exported skinned** (done, M4.1): `ofast export-model` writes the skeleton as glTF nodes and one skin;
   Unreal imports a skeletal mesh in CS2's bind pose.
2. **Each player's animation is computed in Rust and imported as one AnimSequence per player per round** ("design
   A"): for every tick, sample and blend CS2's own clips from the demo state, add procedural aim, and write the pose
   track. Unreal only plays it (a Skeletal Animation section per player, the actor on its transform track). The pose
   is then a pure function of time: scrubbing, reverse play and Movie Render Queue give the same frames, and later
   live bone capture (M8) produces the same kind of asset.
3. **Order**: (M4.2) a clip decoder (`.vnmclip` → poses on the agent skeleton by bone name) and a check that a
   decoded clip plays in Unreal like in CS2; (M4.3) demo fields for the rig (velocity, duck, ground, weapon, weapon
   state, body yaw) in the replay export; (M4.4) the baked locomotion layer (2D blend on local velocity × crouch, with
   foot phase from distance travelled), then additive weapon idle / shoot and full-body reload / deploy, then
   procedural aim on the spine; (M4.5) weapons in hand (the weapon's skeletal mesh on `weapon_hand_R`), agents and
   knives per player from `m_hModel`; (M4.6) deaths as ragdolls in Unreal from the demo's ragdoll force and bone.

## Alternatives considered

- **Import CS2's clips and drive an Animation Blueprint / Control Rig in Unreal** — less data, art-directable, but
  needs a hand-made Animation Blueprint per skeleton (no Python API to build one), is not deterministic under
  scrubbing, and would duplicate CS2's graph logic in Blueprint instead of in tested Rust.
- **Decode the pawn's serialised AnimGraph 2 recipe from the demo** — would give CS2's exact pose inputs, but the
  format is undocumented; kept in mind for later, and ofast-capture (M8) can record real bones live instead.
- **Run CS2's whole `.vnmgraph`** as Source 2 Viewer does (a port of the Esoterica runtime) — the most faithful, but a
  large first step; the baked layers above use the same clips and can grow towards it.

## Consequences

- The Rust side needs an animation module: clip decoding, sampling, blending, additive layers and a small aim
  solver, all unit-testable on synthetic clips and checked against CS2 (demo playback at a tick, spectating a player
  from the side).
- Changing the rig means re-exporting the round; Unreal assets stay simple (one AnimSequence per player and round).
- Materials: agents use CS2's character shader. They rendered too glossy until their roughness was read from
  `g_tAnisoGloss` (2026-10-22, [material.md](../formats/material.md#characters-and-weapons-csgo_character-csgo_weapon));
  cloth sheen is still not converted.
- Glove skins need bone-merging onto the agent skeleton; Unreal must see one root bone per skin. Done in M4.5 by
  merging the glove meshes into the agent's own skin by bone name (on all 80 agent models their bind poses agree to
  0.003 inch and 0.8°).
- Weapons follow the same design (M4.5): instead of weapon actors attached to sockets in Unreal, each weapon a
  look's players carry is a bone of the agent with the weapon rigidly on it, placed per frame in Rust (hand,
  holster, or scaled to 0). No conversion of attachment frames into Unreal's bone axes is needed, scrubbing stays
  exact, and the sequence needs no visibility tracks per weapon; the cost is that each look's skeletal mesh carries
  all its players' weapons (19 – 27 MB per look for a 76-second round).
