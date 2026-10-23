# 015 — Deaths: CS2's ragdolls simulated in Rust and baked into each player's animation

- **Date:** 2026-10-23
- **Status:** accepted (M4.6, done: see "Learned building it")

## Context

A player who dies disappears from the Unreal replay (the actor is hidden while dead). In CS2 the body falls as a
ragdoll and stays until the round restarts; nearly every kill shot of a fragmovie shows it. What the data holds
(checked on the Aurora vs FURIA major demo, round 5, KSCERATO's death at tick 34415, and the `ctm_sas` agent):

- **The demo does not carry the ragdoll's motion.** CS2 simulates it on the client: at the death tick the pawn's
  `m_bRagdollClientSide` turns true and `m_RagdollPose.m_Transforms` (a networked pose, used for server-side
  ragdolls) stays empty. `m_lifeState` goes 1 (dying), then 2 (dead) 116 ticks later.
- **It carries what started it**, on the pawn, set on the death tick: `m_nRagdollDamageBone` (7, the head, for a
  headshot), `m_vRagdollDamageForce` (−26553, 16367, 712), `m_vRagdollDamagePosition` (where it hit),
  `m_szRagdollDamageWeaponName`, `m_bRagdollDamageHeadshot`, `m_vRagdollServerOrigin` (the body's origin) and
  `m_qDeathEyeAngles`; and the pawn's velocity and the pose OFAST already bakes up to that tick.
- **The agent carries its ragdoll**: a `PHYS` block with 15 bodies (pelvis, `spine_2`, head, upper and lower arms,
  hands, thighs, shins, feet), each a capsule with a mass (35 for the pelvis), inertia scale and damping, and 14
  conical joints (`m_nType` 4: a swing cone and a twist range, e.g. 15° and −73° … 36° at the thigh) with a frame in
  each body.
- **The map carries its collision**: `maps/<map>/world_physics.vmdl_c`, a `PHYS` block of convex hulls (planes,
  vertices) and triangle meshes, each tagged with a collision attribute ("Default" for solid world, "ConditionallySolid"
  with `playerclip`, `npcclip` or `passbullets` for clips that only some objects collide with).

## Decision

Simulate each death in OFAST's core and bake it into the player's animation, from CS2's own data:

- read the agent's ragdoll and the map's world physics (`PHYS` blocks);
- at the death tick, place the ragdoll's bodies on the baked pose, give them the pawn's velocity and the death force
  at the damage position on the body of the hit bone;
- step a rigid-body solver (extended position-based dynamics: joints with CS2's swing and twist limits, contacts
  against the world's Default hulls and meshes, CS2's gravity, the parts' damping) until the body rests or the round
  ends;
- write the bodies' motion onto the agent's bones for every exported frame after the death, so the glTF animation
  holds the fall and Unreal (or Blender) only plays it; the body stays visible to the end of the round.

## Alternatives considered

- **Simulate in Unreal (Chaos) and record it.** A physics asset built from the `PHYS` block, the ragdoll run in a
  Play-In-Editor session from the import script, the bones recorded into an animation. Rejected: the import runs
  headless, recording physics there depends on editor APIs that are hard to drive from Python, the result would
  differ from run to run, and it would exist only in Unreal (not in the glTF, not testable in `cargo test`).
- **Play a death clip.** CS2 has no death clips in its graphs (deaths are ragdolls); a canned fall would ignore the
  force and the world.
- **Use the render geometry for collision.** Decals, foliage and glass would stop bodies that CS2 lets through;
  `world_physics` is what CS2's ragdolls collide with, and smaller.

## Consequences

- OFAST needs a `PHYS` reader and a small rigid-body solver, both unit-tested on synthetic cases; the solver's
  output is deterministic.
- The result will not match CS2 body for body: CS2's solver (Rubikon) and its force scaling are not public, so the
  fall's direction, speed and resting pose are checked against CS2 screenshots and tuned (force scale, friction,
  damping), each choice named and documented.
- Exports load the map's world physics (once per round) and take longer by the simulation time.
- Props that move (doors) and other players' bodies are not obstacles in a first version.

## Learned building it

- **The joints twist about their frames' z axis.** Read with x (the bone's direction), CS2's knees — twist-only
  joints, 0° … 110° — became loose ball joints, and a shin folding back made the solver blow up. CS2's own animation
  poses settle it: about z the knees' bend is a twist inside its range with 0.0° of swing, the hips' and spine's
  bends too, with the right signs (`examples/ragdoll_limits.rs`). A joint without a swing limit is a hinge.
- **Drag matters.** A body shot while running covers most of its distance in the air while it tips over: counting
  the parts' drag as damping moved KSCERATO's body by 0.9 m, which brought it to where CS2 has it. (Friction from
  0.7 to 1.3 moved it 0.1 m, but that was measured with the friction bug below, which made it saturate.)
- **Müller et al.'s sliding friction has a units slip.** Their equation 30 caps a velocity change at μ × the
  contact's push over the substep, which is an impulse (mass × speed): the friction came out multiplied by the
  part's mass, 35 times too strong on the 35-pound pelvis, so heavy parts never slid. The cap belongs on the
  impulse (found by the review agent).
- **Every contact needs its velocity pass, both ways.** Pushing a part out of the floor within a substep is taken
  back as speed away from it (depth / h); removing only the approaching speed, and giving part-to-part contacts no
  velocity pass at all, kept resting bodies hopping and their arms moving at 1 – 3 in/s for seconds. With the
  normal speed set to 0 at every contact (equation 34 without restitution) and friction between parts too, a body
  that has come down moves at a few tenths of an inch per second.
- **The living pose breaks the ragdoll's rules.** A crouch can be past a hip's range, a foot inside a stair, a
  rifle arm inside the chest's capsule; fixed within the first substep, each became a burst of speed (1 inch in
  1/512 s is 500 in/s). Positions-only passes before the first step fix the limits and the world; parts that start
  overlapping are left alone until they part. The start velocities are those of the parts' frames moved to their
  centres of mass (a limb turning about its joint, not about its middle).
- **Rest is a drift, not a speed.** The leftover creep (more of it with 16 substeps than with 8: a per-substep
  bias of the linearised pushes) and arms shimmering on a ramp kept bodies over a 5 in/s threshold for up to 19 s,
  or under a quarter-inch drift test until the round ended; half an inch of drift in half a second freezes every
  test body after 2.3 – 10.5 s.
- **"Default" is not the only solid.** de_nuke's B site floor is "ConditionallySolid" `passbullets` (bullets pass,
  bodies do not): with Default shapes alone a body fell 8 m through it. Clips stop only players, NPCs or grenades.
- **CS2 references for deaths** need a living, unflashed player looking at the body from outside a smoke: two of
  four tries were lost to a smoke and a crate; spectating the dead player shows nothing ("No Player").
- The simulation is quick: about 0.02 s per second of fall (the SAS ragdoll's 4.5 s drop took 86 ms), so the
  deaths of a round add well under a second to the export.
