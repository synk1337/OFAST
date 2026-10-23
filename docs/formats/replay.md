# Replay file (`.ofast-replay.json`) and the Unreal Level Sequence

OFAST's own format: one round of a CS2 demo, written by `ofast export-demo` (`crates/ofast-core/src/export/replay.rs`)
and turned into an Unreal Level Sequence by `tools/unreal/import_demo.py`. It is plain JSON so any tool can read it.

```bash
ofast demo-rounds match.dem                                  # rounds and their ticks
ofast export-demo match.dem --round 2 --out test-data/exports/demos
powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_demo.py -Replay test-data/exports/demos/match_r2.ofast-replay.json -Shot test-data/shots
```

The map must be imported first (`import_map.py`): the sequence is built in its level.

## Contents (version 1)

The rig inputs, `weapons`, `jumps` and `landings` were added in M4.3 (2026-10-18), `dropped_weapons` in M4.6
(2026-10-23), without changing the version: they are new keys, so readers of older files keep working; check for
them before use. Everything is in CS2's own
units and axes: Source units (1 unit = 1 inch), x/y/z as in the game, angles in
degrees (pitch positive = looking down, yaw counter-clockwise from +x, −180..180).

| Key | What |
|---|---|
| `format`, `version` | `"ofast-replay"`, 1 |
| `demo`, `map`, `round` | demo file name (without `.dem`), map name from the demo header, round number |
| `tick_interval` | seconds per tick (1/64) |
| `start_tick`, `freeze_end_tick`, `end_tick` | round start (freeze time begins), freeze time end (`round_freeze_end`), last tick of the round |
| `players` | `controller` (entity index = player slot + 1), `name`, `steam_id` (text: 64-bit ids lose digits in float JSON readers), `team` (2 T, 3 CT) |
| `samples.ticks` | every tick of the round with entity updates (one per tick in practice) |
| `samples.players.<controller>` | arrays aligned with `samples.ticks`: `position` (feet, [x, y, z], 0.01 unit), `eye_height` (eye above the feet, networked view offset z), `angles` ([pitch, yaw], 0.01°), `alive`, and the rig inputs (below); `null` where the player had no pawn |
| rig inputs (per tick, same arrays) | `body_yaw` (the way the model faces, 0.01°), `velocity` ([x, y, z] units/s, 0.1), `on_ground`, `duck` (crouch amount 0 – 1, 0.001), `walking`, `scoped`, `defusing`, `ladder`, `weapon` (item definition index), `weapon_state` (weapon animation state), `reloading`; the three weapon values are `null` with nothing in hand. Fields and values: [demo.md, Rig inputs](demo.md#rig-inputs) |
| `weapons` | item definition index (as text) → weapon entity class seen in the round, e.g. `{"7": "CAK47", "42": "CKnife"}` |
| `jumps`, `landings` | `{tick, player}` (controller) for every jump / landing of the round, from the pawns' last jump and landing ticks |
| `kills` | `player_death` events: `tick`, `victim`, `attacker`, `assister` (controllers; `null` = nobody, e.g. fall damage), `weapon`, and the kill-feed flags `headshot`, `penetrated`, `noscope`, `thrusmoke`, `attackerblind`, `assistedflash`, `attackerinair` |
| `events` | round milestones with their raw keys: `round_freeze_end`, `bomb_beginplant`, `bomb_planted`, `bomb_begindefuse`, `bomb_defused`, `bomb_exploded`, `round_end` |
| `dropped_weapons` | weapons lying in the world ([demo.md](demo.md#dropped-weapons)), one entry per stretch an entity lay unowned: `entity`, `item`, `class`, `model` (resource id as 16 hex digits), `mesh_groups`, `first_tick`, `last_tick` (the last tick it lay there: picked up on the next, or the round ended), `keys` = `[tick, x, y, z, pitch, yaw, roll]` at its first tick and on every tick it moved (after a rest, the resting transform is keyed again on the tick before the move, so straight lines between keys stay right) |

A 111-second round of de_nuke: 10 players, 7,086 ticks, 9 kills, 64 jumps, about 8 MB (4 MB before the rig
inputs), written in 0.8 s. `import_demo.py` reads only the keys it needs, so the rig inputs change nothing there yet;
the animation bake (M4.4) uses them.

## Agents manifest (`<demo>_r<round>.agents.json`)

Written by `ofast export-demo --players` next to the replay (`export::players`): the animated agents of the round,
one glTF per look (agent model, mesh groups, glove skin), each with an animation per player
([animation.md](animation.md#each-players-animation-animbake-exportplayers)).

| Key | What |
|---|---|
| `format`, `version` | `"ofast-agents"`, 1 |
| `demo`, `round`, `fps` | as the replay; the animations' key rate: the tick rate (64), one key per tick |
| `agents[].file` | `<demo>_r<round>_<agent>_<mesh groups>[_<gloves>].gltf`, in the same folder |
| `agents[].model`, `.mesh_groups`, `.gloves` | the agent model path, its mesh-group mask (0 = the model's default), the glove model path or `null` |
| `agents[].players` | the controllers animated in that file (animations `player_<controller>`) |
| `agents[].weapons` | the weapon bones in order (`ofast_weapon_0`, `_1`, …): `model`, `mesh_groups`, `instance` (a second copy carried at once) |
| `agents[].ragdolls` | controller (as text) → `[first tick, last tick]` of each of that player's deaths the animation holds as a ragdoll (from the death to the respawn or the round's end) |
| `dropped_weapons` | one glTF per look of the weapons lying in the world: `file` (`<demo>_r<round>_dropped_<model file stem>_<mesh groups shown>.gltf`, the weapon model alone and rigid: a static mesh in Unreal), `model` (path), `model_id` as the replay names it, and `mesh_groups`: every mask the replay uses that shows this file (grenades send every bit, meaning the model's default groups) |

Checked: the kills of all 20 rounds of a real match equal demoparser2's (`kills_match_demoparser2`); positions and
angles are the round tracks already checked against demoparser2 and CS2 (`docs/formats/demo.md`); in Unreal each
killer's camera, 8 ticks before the kill, has the victim at the crosshair (within 1° in the data, on screen in the
verification screenshots).

## In Unreal (`import_demo.py`)

| Element | How |
|---|---|
| Sequence | `/Game/OFAST/Replays/<demo>_r<round>`, display rate = tick rate (64 fps: one frame per tick), from the round start to its end |
| Player (animated) | with `ofast export-demo --players`: the player's look from the agents manifest (imported to `/Game/OFAST/Replays/Agents/` and saved, with the glTF pipeline copy `/Game/OFAST/Pipelines/GLTFCharacters` that combines a skeleton's meshes, so the weapons are part of the agent, and samples the animations at the manifest's `fps` with frame snapping: one AnimSequence frame per tick) as a skeletal mesh actor, feet on the positions, turned along the body yaw (−body_yaw), a Skeletal Animation track playing the player's baked animation (`player_<controller>`: body and weapons) from the round's first frame ([animation.md](animation.md#each-players-animation-animbake-exportplayers)) |
| Player proxy | without the agents: engine cylinder scaled to CS2's 32 × 32 × 72-unit hull, CT blue / T orange, a grey visor at the eyes showing where the player faces; position (x, −y, z) × 2.54 cm, yaw → −yaw (eye yaw) |
| Visibility | hidden while dead or without a pawn, and while the camera looks through that player's own eyes (CS2 does not draw your own body in first person either) |
| First-person cameras | one per player at feet + eye height, aimed with Rotator(pitch = −pitch, yaw = −yaw), CS2's default field of view (90° at 4:3 = 106.26° horizontally at 16:9) |
| Dropped weapons | with the agents manifest's `dropped_weapons`: one static mesh actor per stretch of the replay's `dropped_weapons`, keyed on the stretch's keys (Rotator(roll = roll, pitch = −pitch, yaw = −yaw); each triple turned to the nearer of its two equivalent Euler forms, so a gun tumbling through the vertical never spins a full turn between two ticks in slow motion), shown from its first tick to its last |
| Camera cuts | each kill in turn: the killer's camera from the previous cut until 0.5 s after the kill (the last one to the end of the round) |
| Markers | marked frames at freeze time end, every kill (`attacker > victim (weapon, HS)`), bomb events and round end |
| Keys | linear; keys a straight line through their neighbours reproduces within 0.2 cm / 0.1° are dropped (128,825 keys for that round, written in 3 s); yaw is unwrapped so it never spins the long way round |

Verification screenshots (`-Shot`): the killer's view 8 ticks before each of the first three kills, a view from
above the players at the first kill, and with `-PovShots "tick:controller,…"` the view through any player's eyes at
given ticks (that player's own body hidden in that frame), and with `-CloseShots "tick:controller,…"` a close-up of a
player from 1.3 m in front and to his right at chest height (to inspect the pose and the weapons), and with
`-ViewShots "tick:x:y:z:pitch:yaw,…"` the view of a camera placed where CS2's `getpos` says its own view is (for
instance CS2's Chase Cam behind a spectated player), so both show the same frame. The frames these shots render show
every other player, also one whose own camera cut is running then (the sequence otherwise hides the player the cut
looks through). To compare with CS2: `demo_pause`, `demo_gototick <tick − 64>`, then `demo_step_tick 64` (seeking
alone can leave poses stale), spectate the player (`spec_player "<name>"` for ASCII names, or the demo bar's player
arrows; `spec_player_by_accountid` does nothing in demo playback), Steam's F12.

A player whose death the players export baked as a ragdoll (`ragdolls` in the agents manifest) stays visible over
the death's ticks: the body falls and lies there to the end of the round
([animation.md](animation.md#deaths-exportdeaths-physicsragdoll)).

## Not yet

Foot IK of CS2's animation graph, weapon skins, grenades
and their effects (M6), sounds, a name label above each player (Unreal's text render did not show in the
screenshots; the names are the actors' labels in Sequencer), several rounds in one sequence.
