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
- **Against CS2** (2026-10-18): in the local match demo (de_nuke, round 5, ticks 26508 – 26528) CT "P1", an SAS
  agent, runs with his knife out (rifle slung), seen almost side-on by a teammate 4.6 m away. Unreal renders
  `ctm_sas` with `run_n_knife` from that teammate's eye, placed relative to P1 from the demo's positions and view
  angles, with CS2's field of view (`run_shot.ps1 -Camera`). The legs go through the same stride as in CS2: the
  rear foot kicked up behind, the long reach of the front leg, frames 0.25 s and 0.35 s of the clip matching ticks
  26508 and 26513. The upper body does not: CS2 reaches the left arm forward and leans the torso further. Those come
  from the graph's weapon and aim layers ([decision 013](../decisions/013-replay-rig-baked-in-rust.md), M4.4), not
  from the base clip. Side by side: `test-data/shots/agents/cs2_run/compare_knife.png`.

## Not done yet

- **Additive clips**: composing a delta onto a base pose (needed for M4.4's weapon idle, shoot and flinch layers).
- **Secondary clips** (weapon parts) are decoded but not exported.
- **Events and sync tracks** are not read.
- The graph (`.vnmgraph`): its blending is rebuilt from demo data in M4.4 rather than read.
