# 017 — Grenades and effects: CS2's data, OFAST's own effects in Unreal, smokes simulated in Rust

- **Date:** 2026-10-23
- **Status:** accepted (M6)

## Context

A fragmovie round needs its grenades: flashes going off, HE blasts, molotov fires, and smokes that players walk
through. What CS2 gives (checked in our own demos and CS2's files, 2026-10-23; `docs/formats/demo.md`, "Grenades"):

- **Projectiles** are entities of their own (`CSmokeGrenadeProjectile`, `CFlashbangProjectile`, `CHEGrenadeProjectile`,
  `CMolotovProjectile`, `CDecoyProjectile`): position and rotation every tick, model, thrower, bounces. Detonation
  events (`hegrenade_detonate`, `flashbang_detonate`, `smokegrenade_detonate` / `_expired`) say when and where they go
  off; a flash's or HE's entity can stay in the demo for seconds after.
- **Fires** (`CInferno`): up to 64 flames, each a world position, a surface normal and a burning flag; lifetime 7 s
  (molotov) or 5.5 s (incendiary); `inferno_startburn` / `_expire`.
- **Smokes** network a byte log, `m_VoxelFrameData` (≤ 3072 bytes, `m_nVoxelFrameDataSize` in use). Public reverse
  engineering (5stackgg/demo-parser, AGPL-3.0: facts only, no code; cross-checked by CS2-insight-agent and
  memorin.app) says it is not the cloud but the inputs of a deterministic simulation that the server and every
  client run alike: one frame per 0.1 s step (`u16` frame number, `u16` length, payload: a fading flag, emitters
  where the smoke comes out, and 4³-voxel bricks of world geometry, each sent the first time the simulation looks into
  it, as a 64-bit solid mask), on a 32³ grid of 20-unit voxels whose corner is the detonation point − 320 on each
  axis; bricks and mask bits in Morton order. The cloud itself is the client's simulation: each voxel taking density
  from its six neighbours each step (more from above than from below, so the cloud sinks), in place in Morton order,
  decaying, fed by the emitters until the fading flag. HE blasts
  carve holes (memorin, from the shader: 200 – 250 units, recovering over about 7 s); 20-byte "hole" records in the
  log are not decoded publicly.
- **Effects:** CS2 draws them with its particle systems (2623 `.vpcf` files; an HE grenade's explosion alone has
  dozens of children: flash, fireball, debris, smoke, decals), with sprite-sheet textures
  (`materials/particle/fire_explosion_1`: several animated sequences packed in a 4096 × 2048 sheet).
- **Unreal 5.8:** Niagara systems cannot be authored from editor Python (no scripting library for emitters).
  Heterogeneous Volumes render volumetric materials lit by the scene's lights (not part of Lumen's scene; indirect
  light from Lumen's translucency volume optionally); their Sparse Volume Textures import only from OpenVDB files and
  their frame cannot be keyed in Sequencer. Material parameters can be keyed (component material tracks).

## Decision

Grenades are read and baked in OFAST's core like everything else; Unreal plays the result with effects OFAST builds
from scriptable pieces, not with converted CS2 particle systems:

- **Projectiles:** each grenade's world model flies along its keys from the throw to its detonation (an HE grenade,
  a flashbang), its expiry (a smoke) or its landing (a molotov: its fire takes over) — as the dropped weapons do.
- **Smokes:** the voxel log is decoded in Rust, and OFAST runs its own smoke simulation on the 32³ grid, one step per
  frame of the log, written from the documented behaviour and checked against the data itself — the bricks the
  server sends are those the simulation needs, frame by frame — and against CS2's picture. The density frames go to
  Unreal as one texture per smoke (32 slices of 32 × 32 per frame, frames in an atlas), drawn by a Heterogeneous
  Volume whose volume material samples the atlas at the frame of the sequence's demo tick (interpolating between
  frames); HE holes first as an analytic carve-out in the material (it went into the simulation instead: below).

  How it came out (2026-10-23). The simulation is a diffusion whose weights add up to 1 (smoke moves, only a fixed
  loss per voxel takes it away, so the cloud fills a volume and spreads further along a corridor than in the open),
  pulling more from above than from below, fed by the emitters until the fading flag; smoke never flows into solid
  voxels. Two lessons from fitting it to the bricks (`examples/smoke_fit.rs`): solid voxels must not swallow the smoke
  that would flow into them (with that, the floor under a cloud ate it and no setting reached CS2's extent), and a
  smoke voxel needs its neighbours' bricks (needing a brick only when smoke enters it fits worse). The best fit needs
  39 – 46 % of the bricks within 2 frames of CS2 and misses 7 – 14 % on four demos of three maps: close, not CS2's
  own simulation, whose constants are unknown (the reverse engineering gives some: decay 0.5, feed 60). Two things
  keep the picture right anyway: a brick the server has not sent yet counts as solid, so OFAST's cloud never runs
  ahead of CS2's, and the steady cloud is about the size CS:GO's smoke was (a 143-unit hemisphere against 144). The
  frame comes from one keyed value, the demo tick in a material parameter collection (`MPC_Replay.ReplayTick`), so
  the volumes need no keys of their own: each smoke's material instance knows its frames' first tick and spacing, its
  fading and its expiry.

  HE holes (2026-10-24) went into the simulation, not the material as first planned: CS2 sends nothing for them (no
  hole record in any round of four demos), so its clients carve them themselves from the detonation, and carving
  OFAST's simulated cloud lets it come back the way the cloud spreads. The hole is emptied after every step while it
  lasts (once, the emitters inside it fill it again within a step, while CS2's stays open): 240 units for 0.5 s, then
  shrinking to nothing over 4 s, fitted to CS2's picture at five moments after a blast in a smoke (see
  `docs/formats/demo.md`, "Smoke voxels"). It is baked into the smoke's atlas, so nothing changes in Unreal. Bullets'
  thin tunnels through smokes are left for the shots.
- **Fires:** per flame, from the tick it starts burning to the tick it stops: a camera-facing animated sprite from
  CS2's own fire sprite sheets (the sheet's frame rectangles from the texture's data) and a flickering point light,
  so the ground and walls glow.
- **HE grenades and flashbangs:** at the detonation, a short light pulse (CS2's flash is a white burst, an HE blast an
  orange one) and sprites from CS2's explosion sheets; an HE blast's smoke puff and scorch decal; the flashed white
  screen in the POV shots of the players the flash blinds (the pawn's flash duration).

  How the sprites came out (2026-10-24): without Niagara, a "particle system in a material". Each effect is an actor
  with instanced quads, one instance per sprite slot holding where it is, the ticks it may spawn in, a phase and a
  seed; the material respawns a particle every lifetime from the demo tick (the smokes' parameter collection),
  draws its size, place, roll and sheet sequence from CS2's own ranges (read from its particle systems) with a hash,
  faces it to the camera in the vertex shader and plays the sheet's sequence over its life in the pixel shader. It
  is deterministic in the demo tick, so the sequence scrubs. CS2's sheets pack frames tightly; the export repacks
  them as regular flipbooks. Two traps of the approach: a slot's phase (where in its first lifetime it starts, so a
  flame's slots do not respawn together) must be 0 for a one-shot, or its only particle is born up to a lifetime
  away from the detonation and never shows; and the engine culls an instance by its mesh's bounds, which for a flat
  plane have no height, so the quad is a copy of the plane with bounds reaching 6 m up. Two adjustments against CS2's picture: a sprite's half-size is half the particle's
  radius (read as the half-size the flames came out twice CS2's height), and the sprites are lifted most of their
  size and faded by depth where they meet the floor. The flashed view is a post-process material on each blinded
  player's camera, fitted to CS2 (0.6 × saturate((seconds left − 0.5) / 2.5) towards a warm white: a first guess
  of 0.8 by eye left the same view 30 levels brighter than CS2's).
  An HE blast also got (2026-10-24) its flash, the ring of smoke it rolls along the ground, its sparks, their glow and
  its debris flecks, as sprites that fly: the material throws them along a random heading and up, slows them by
  CS2's drag and pulls them down by gravity, and those that would touch the world die at the ground under the blast,
  which the export finds in the map's collision. The sparks are one streak per flight along the velocity, standing in
  for CS2's carriers that shed trails of sparks. Its scorch is up to three decal actors, faded by the demo tick in
  their material. A door's entity leaving the demo mid-round turned out to be a door a blast broke (it leaves one
  tick before an HE grenade goes off by it, in each of eight rounds; doors also leave unbroken at the round's
  restart): Unreal hides it from the blast on, as CS2 does. Left out: a blast's heat haze, embers and model debris (CS2's `flecks3` meshes), a fire's
  flames along its outline and up walls (CS2's snapshot-driven sets), and its smoke.
- **Shots (later in M6):** muzzle flashes on the weapon (third person and viewmodel), tracers along the view to the
  first world hit (OFAST's map geometry), impact puffs.

## Alternatives considered

- **Convert CS2's particle systems to Niagara.** The faithful end state, but each `.vpcf` is a stack of emitters,
  initializers, operators and renderers with CS2's own semantics, and Niagara cannot be written from Python: a
  converter is its own project (a C++ plugin writing Niagara assets). Kept for later; the effects here use CS2's
  textures so they stay close.
- **Hand-made Niagara assets shipped with OFAST.** Prettier per effect, but authored by hand in the editor, hard to
  test, and not driven by CS2's data.
- **Smokes as simple spheres or local fog volumes.** CS2's smokes fill rooms and doorways and hug the floor; a sphere
  would leave the corridors open that the real smoke closes. Kept as the fallback when a smoke's log cannot be read.
- **Smokes as animated Sparse Volume Textures from OpenVDB files.** UE's own path for volume animation, but no SVT
  track exists in Sequencer (the frame cannot be keyed) and it needs a VDB writer; the atlas texture needs neither.

## Consequences

- OFAST's smoke simulation is its own: smokes can differ from CS2's in detail (how far their thinnest edges creep,
  how fast they grow in the first second). The bricks the server sends give a check in `cargo test`
  (`smoke_logs_decode_and_the_simulation_follows_cs2`). Checked against CS2's picture on 2026-10-23: a T smoke seen
  down a de_nuke corridor at the same tick through the same player's eyes covers the same part of the frame (top,
  bottom and sides within 2 % of the frame) with the same tan and a puffy surface; CS2's had cleared 0.4 s before its
  expiry, as OFAST's does. The look takes four choices that are OFAST's, not measured from CS2: the density is
  opaque 15 % above the 5 % threshold, a fractal noise breaks the surface into puffs, the closed voxel under the
  smoke is drawn with it (the floor voxel touches the floor, so the cloud would float 20 units up), and the smokes
  get indirect light from Lumen (a console variable).
- Heterogeneous Volumes are off at Effects quality below High and cast no shadows on the scene by default: the replay
  sets the cvars it needs, and renders use Movie Render Queue's settings for them.
- CS2's particle textures are read from the user's install like every other asset; nothing is shipped.
- The effects are OFAST's approximations of CS2's (light pulses, sprites): checked against CS2 for timing and size,
  not for every particle.
