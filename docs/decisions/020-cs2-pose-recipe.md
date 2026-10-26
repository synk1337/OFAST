# 020 — CS2's pose recipe: the demo carries the animation CS2 played

- **Date:** 2026-10-26
- **Status:** accepted (M4.7: the spike is done, the decoding is not complete)

## Context

OFAST rebuilds every player's third-person animation from the demo's inputs (velocity, crouch, eye angles, weapon
states, events) by re-deciding CS2's graph states itself ([decision 013](013-replay-rig-baked-in-rust.md)): starts,
stops, turns, plants, actions. The roadmap listed this as the hard problem: "demos contain no skeleton poses".

They contain the next best thing. Each player pawn sends `m_animationController.m_SerializePoseRecipeAG2Dynamic`
every tick, with 32 topology slots and an active-slot index beside it: the server's AnimGraph2 pose recipe, the
list of pose tasks its graph ran (which clips at which times, which blends at which weights, which bone masks),
for clients to play back. Decision 013 noted the field but took it for an opaque, undocumented blob; it was looked
at again while checking whether demos carry the aim punch (they do not).

## Findings (spike)

- The encoding is Esoterica's task serializer (bit-packed, least-significant bit first): the topology slots are its
  topology blobs (a 4-bit width, a task count, per task a 5-bit type and dependency indices), the dynamic bytes the
  server tick then each task's payload in topology order (`demo::recipe`). All 618 topologies of a round fit the
  input counts found per type, and all 114,455 recipes decode without an error; samples (type 1: 10-bit resource
  index, 16-bit time) and blends (types 7 and 8: 8-bit weight, a bone-mask flag and list) behave as CS2's animation
  does (synced locomotion times stepping one tick of their cycle, weights easing, a weapon clip restarting at each
  shot); every clip and blend decodes in 98.4 % of the round's recipes and 99.6 % of a match on another build
  (`examples/pose_recipe.rs`, `OFAST_CHECK=1`). Details: [demo.md](../formats/demo.md#pose-recipe-cs2s-own-animation).
- The resource index is the same in two tournament matches but in an order not found yet, and another in a demo
  of another CS2 build; locomotion clips are identified by co-occurrence with what OFAST bakes
  (`examples/recipe_map.rs`).
- Decoded since: types 10 and 9 (blends), 0 and 6 (cached pose write and read), 13 (an empty pose). Not decoded
  yet: types 14 (the aim, probably), 15 and 5; together they hide a clip in under 2 % of the recipes.

## Decision

Decode the pose recipe in the core and bake from it: each player's clips, times and weights as CS2 played them,
blended in the recipe's order, with OFAST's own aim, IK and weapon placement on top. The re-decided graph
(decision 013) stays as the fallback for demos or ticks without a recipe, and for what the recipe does not carry.

Next steps, in order:

1. `demo::recipe` (done for the known types): read the slots and the dynamic bytes per player and tick, parse
   topologies and payloads, report what does not parse.
2. The resource index → clip table, found for each demo (done: `anim::recipe::learn_clip_table`): each index
   matched to the clip OFAST's own bake samples with it most exclusively, one clip per index (214 of round 5's 333
   indices, naming 93.2 % of its sample tasks), only clips OFAST plays before its aim; and keep looking for the rule
   (Valve's resource system order: load order, a hash table not matched yet). The match found a bake bug: OFAST
   played a gun's shot clip once per spray where CS2 restarts it with every bullet (`docs/formats/demo.md`).
3. The remaining payloads (types 14, 15 and 5; under 2 % of the recipes have a clip after them), by the same
   alignment tests.
4. Bake from the recipe where it is decoded and check against CS2 side by side (`tools/cs2/free_cam.py`) (started:
   `anim::recipe::RecipePlayer`, opt-in with `OFAST_RECIPE=1`, OFAST's aim over CS2's pose; the demo's hits sit
   closer to the hitboxes posed from it: 371 of 505 within an inch on 10 rounds of 8 demos, 349 with the
   re-decided poses; side views from CS2's free camera match it).

## Alternatives considered

- **Keep re-deciding the graph** (decision 013): works, but every state has to be found again and timed by guess;
  it never matches CS2 where the server's decision is not visible in the inputs (turn plants, start foot choice,
  action variations).
- **Run CS2's graph exactly** (evaluate the whole AnimGraph2 with the demo's inputs): the graph's inputs are not all
  in the demo, and its node set is large; the recipe is the graph's output.

## Consequences

- Until the index rule is found, each demo's table is learned from the demo itself (CS2 updates reorder it).
- The recipe is the server's pose; if CS2's server poses its hitboxes from the same graph (not checked), OFAST's hit
  checks would gain from it too.
