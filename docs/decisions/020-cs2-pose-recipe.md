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
   matched to the clip OFAST's own bake samples with it most exclusively, one clip per index (212 of round 5's 333
   indices, naming 93.2 % of its sample tasks), only clips OFAST plays before its aim, vetted by the blends'
   structure (a 1D blend's two clips are both poses or both deltas); and keep looking for the rule
   (Valve's resource system order: load order, a hash table not matched yet). The match found a bake bug: OFAST
   played a gun's shot clip once per spray where CS2 restarts it with every bullet (`docs/formats/demo.md`).
3. The remaining payloads (types 14, 15 and 5; under 2 % of the recipes have a clip after them), by the same
   alignment tests. A first spike (2026-10-26) found the 14 longer than 32 bits or varying, carrying the graph's
   inputs (the duck amount at bit 48), and the 14 and 5 together 80 – 850 bits, steady per player, not set by the
   weapon; alignment tests over the tail drown in its constant stretches. What is known is in `demo.md`'s task
   table; Esoterica's IK and target tasks (variable-length targets behind flags) are the next lead.
4. Bake from the recipe where it is decoded and check against CS2 side by side (`tools/cs2/free_cam.py`) (done:
   `anim::recipe::RecipePlayer`, OFAST's aim over CS2's pose; the demo's hits sit closer to the hitboxes posed from
   it, 371 of 505 within an inch on 10 rounds of 8 demos against 349 with the re-decided poses, and side views from
   CS2's free camera where the two differ most match CS2 at least as well; the default since 2026-10-26,
   `OFAST_RECIPE=0` keeping the re-decided poses only).

## Update 2026-11-04: recipe version 3

The demos of CS2 build 10955 (the faze-vs-falcons matches) send `m_nSerializePoseRecipeVersionAG2` 3. `demo::recipe`
refused any version but 2, and the export said nothing about it: anubis round 14 played CS2's poses on 0 of its 51,853
living player-ticks. Found by laying one player's data over consecutive ticks bit by bit (`examples/pose_recipe.rs`,
`OFAST_HEX=1`): the bits that change are the clips' times, and each sits a bit later than version 2 would put it.
A sample's resource index is 11 bits instead of 10, as Esoterica's width rule gives for a context of more than 1024
resources (indices up to 1038 occur); the topologies, the blends, the bone mask lists, the cached poses and the empty
pose are read as before, and so are all version-2 demos. The undecoded aim and last task (types 14 and 5) grew from
60 – 670 bits to 550 – 1150, which may be what the version is for. Details:
[demo.md](../formats/demo.md#pose-recipe-cs2s-own-animation).

`demo::recipe::resource_bits` keeps the width per version (2: 10 bits, 3: 11); a version OFAST does not know is
still refused, and the export now warns about it.

Checked on today's CS2 install (build 2000940) with the hitbox fit's 10 rounds (all version 2, so decoded as before)
and four version-3 rounds, recipes on against `OFAST_RECIPE=0`:

- The 10 rounds: 373 of 505 hit points within an inch with the recipes, 347 without (371 and 349 when the recipes
  became the default); 470 against 462 on a posed body. Round 5 of the de_nuke pro demo keeps 50 of 50 on a posed
  body (38 within an inch against 34).
- Anubis round 14: CS2's pose on 36,526 of 51,853 living player-ticks (70 %; 0 before), 49 of 49 hits on a posed
  body, 44 within an inch (45 without). With anubis round 3, ancient round 16 and mirage round 14: CS2's pose on
  53 – 70 % of the living player-ticks (79 – 86 % on the 10 rounds), and 149 of 235 hit points within an inch against
  155 without, one or two fewer on each round.

The version-3 recipes decode as cleanly as version 2's, but their clip table is weaker: the export counts 285,000 –
464,000 sample tasks a round where the version-2 rounds run 230,000 – 290,000, and the table names 47 – 76 % of them
(89 – 94 % on version 2). The indices left unmatched with the most samples look like clips OFAST's bake does not
play the same way on this graph: the shot clips (0.76 – 0.89 s, matching well on version 2) and the 0.53 s planted
turns. The recipes stay the default for both versions; the version-3 table is the next step there.

## Update 2026-11-04 (later): the version-3 table

The weak table had two causes besides the graph (`examples/recipe_table.rs`, `OFAST_INDEX` and `OFAST_TIMELINE`, show
per index what OFAST bakes on its ticks and who holds it; details in
[demo.md](../formats/demo.md#pose-recipe-cs2s-own-animation)):

- **Version 3 keeps a dead pawn's last living recipe, frozen**, where version 2 empties it. The table counted those
  ticks, on which OFAST bakes nothing, so every index CS2 sampled at a death lost Jaccard (598, `walk_e_rifle`: 4,727
  of its 9,597 player-ticks dead), and 20 – 43 % of the sample tasks counted were the dead's. The "shot clips" were
  runs and crouched walks held under 0.3 this way. `demo::players` now drops a dead pawn's recipe.
- **Silent reloads**: build 10955's guns reload through gameplay states 802 and 803 while the player walks, which
  OFAST did not know, so the bake played no reload there. They are `action_reload_silent` now (with 803 the outro of
  802, as 801 is of 800), and `anim::eval` takes a transition on ID parameters that holds as a state is entered (the
  gun graphs' `Reload0` → `ReloadSilent0`, the only one it fires for on the rounds checked), so the bake plays the
  `reload_<w>_silent` clips CS2 plays (993 `reload_ak_silent`, Jaccard 0.96). The first-person view maps the states
  the same way.

On today's install (build 2000940), recipes on against `OFAST_RECIPE=0`:

- The four version-3 rounds: the table names 86.5 – 90.9 % of their sample tasks (47 – 76 % before), CS2's pose on
  69 – 78 % of their living player-ticks (53 – 70 %), and 158 of 235 hit points within an inch against 156 for the
  re-decided poses (149 against 155 before; the silent reloads moved the re-decided count by one). 220 hits lie on a
  posed body against 222: one hit, counted twice, 4.0 inches from the recipe's body and 2.7 from the re-decided one,
  in a 0.53 s start the table does not know.
- The 10 version-2 rounds (373 of 505 against 347, 470 on a posed body against 462) and round 5 of the de_nuke pro
  demo (38 of 50 against 34, 50 posed) do not move.

What the table still misses is mostly the 0.53 s plants: starts from a standstill and the plant-and-turns of
counter-strafes. CS2 starts a plant-and-turn on the tick the speed first drops, switches direction inside it
(`E2W` ↔ `W2E` in 0.1 s) and leaves it at the clip's `PLANT_AND_TURN_COMPLETE` event, 41 % in; OFAST's bake plants
about 5 ticks later, plays the whole clip and passes over the reversals meanwhile (`anim::states::ground_action`), so
through an A-D jiggle it plays one direction throughout and CS2's plants of both directions come with that one clip
under 0.3. The thresholds of `anim::states` were never checked against CS2; the recipes now give CS2's plant ticks to
re-decide plants by, the next step for both the table and the re-decided poses.

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
