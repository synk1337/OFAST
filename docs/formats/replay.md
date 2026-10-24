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

The rig inputs, `weapons`, `jumps` and `landings` were added in M4.3 (2026-10-18), `dropped_weapons` and `doors`
in M4.6 (2026-10-23), the players' `viewmodel` settings and the `left_handed` track in M5 (2026-10-23), `grenades`
and `fires` in M6 (2026-10-23), without changing the version: they are new keys, so readers of older files keep working; check for them before use. Everything is in CS2's own
units and axes: Source units (1 unit = 1 inch), x/y/z as in the game, angles in
degrees (pitch positive = looking down, yaw counter-clockwise from +x, −180..180).

| Key | What |
|---|---|
| `format`, `version` | `"ofast-replay"`, 1 |
| `demo`, `map`, `round` | demo file name (without `.dem`), map name from the demo header, round number |
| `tick_interval` | seconds per tick (1/64) |
| `start_tick`, `freeze_end_tick`, `end_tick` | round start (freeze time begins), freeze time end (`round_freeze_end`), last tick of the round |
| `players` | `controller` (entity index = player slot + 1), `name`, `steam_id` (text: 64-bit ids lose digits in float JSON readers), `team` (2 T, 3 CT), `viewmodel`: the player's own first-person settings as last known, `fov` (`viewmodel_fov`, horizontal at 4:3) and `offset` (`viewmodel_offset_x` / `_y` / `_z`: inches right, forward, up) ([demo.md](demo.md#first-person-view)); `blindings` (M6, only for a player a flashbang blinded): `tick`, `duration` (seconds), `alpha` (0 – 255), one per tick the living pawn's `m_flFlashDuration` changed to a new value above 0; a value seen first, or after a gap (dead, no pawn), starts nothing, as for the animation's flinch (`demo::players::flash_starts`, [demo.md](demo.md#grenades)) |
| `samples.ticks` | every tick of the round with entity updates (one per tick in practice) |
| `samples.players.<controller>` | arrays aligned with `samples.ticks`: `position` (feet, [x, y, z], 0.01 unit), `eye_height` (eye above the feet, networked view offset z), `angles` ([pitch, yaw], 0.01°), `alive`, and the rig inputs (below); `null` where the player had no pawn |
| rig inputs (per tick, same arrays) | `body_yaw` (the way the model faces, 0.01°), `velocity` ([x, y, z] units/s, 0.1), `on_ground`, `duck` (crouch amount 0 – 1, 0.001), `walking`, `scoped`, `defusing`, `ladder`, `weapon` (item definition index), `weapon_state` (weapon animation state), `reloading`; the three weapon values are `null` with nothing in hand. Fields and values: [demo.md, Rig inputs](demo.md#rig-inputs). And `left_handed`: the viewmodel in the left hand (players flip it with a key during rounds) |
| `weapons` | item definition index (as text) → weapon entity class seen in the round, e.g. `{"7": "CAK47", "42": "CKnife"}` |
| `jumps`, `landings` | `{tick, player}` (controller) for every jump / landing of the round, from the pawns' last jump and landing ticks |
| `kills` | `player_death` events: `tick`, `victim`, `attacker`, `assister` (controllers; `null` = nobody, e.g. fall damage), `weapon`, and the kill-feed flags `headshot`, `penetrated`, `noscope`, `thrusmoke`, `attackerblind`, `assistedflash`, `attackerinair` |
| `events` | round milestones with their raw keys: `round_freeze_end`, `bomb_beginplant`, `bomb_planted`, `bomb_begindefuse`, `bomb_defused`, `bomb_exploded`, `round_end` |
| `doors` | the map's rotating doors ([demo.md](demo.md#doors)), one entry per door entity of the round: `entity`, `model`, `closed_position` and `closed_angles` (the map entity's origin and angles), `first_tick`, `last_tick`, `keys` as `dropped_weapons`' (on the ticks it swung) |
| `dropped_weapons` | weapons lying in the world ([demo.md](demo.md#dropped-weapons)), one entry per stretch an entity lay unowned: `entity`, `item`, `class`, `model` (resource id as 16 hex digits), `mesh_groups`, `first_tick`, `last_tick` (the last tick it lay there: picked up on the next, or the round ended), `keys` = `[tick, x, y, z, pitch, yaw, roll]` at its first tick, on every tick it moved and at its last tick (after a rest, the resting transform is keyed again on the tick before the move, so straight lines between keys stay right) |
| `grenades` | thrown grenades ([demo.md](demo.md#grenades)), one entry per projectile entity (a slot reused by a later grenade starts another): `entity`, `kind` (`smoke`, `flashbang`, `he`, `molotov`, `decoy`), `model` (resource id, 16 hex digits), `thrower` (controller), `first_tick`, `last_tick` (the last tick its model shows: an HE grenade's and a flashbang's detonation, a smoke's expiry, a molotov's landing), `keys` as `dropped_weapons`' (ending on a key at `last_tick`), `detonation` (`tick`, `position` of the detonation event, or `null`), `smoke` (a smoke's cloud: `start_tick`, `end_tick` (`smokegrenade_expired`), `position` (the detonation point), `color` (0 – 255, `m_vSmokeColor`), `volume` (below)) |
| `fires` | molotov and incendiary fires: `entity`, `incendiary`, `start_tick`, `end_tick` (`inferno_expire`: a smoke can put a fire out early), `lifetime` (seconds), `flames` in the order they appeared: `position`, `normal`, `first_tick` and `last_tick` it burned |

A 111-second round of de_nuke: 10 players, 7,086 ticks, 9 kills, 64 jumps, about 8 MB (4 MB before the rig
inputs), written in 0.8 s. `import_demo.py` reads only the keys it needs, so the rig inputs change nothing there yet;
the animation bake (M4.4) uses them.

### Smoke volumes

Each smoke's cloud, simulated by OFAST from the smoke's voxel log ([demo.md](demo.md#smoke-voxels),
`demo::smoke`), is written next to the replay as an 8-bit grayscale PNG atlas, `<demo>_r<round>_smoke<entity>-<serial>.png`
(`export::smoke`), 110 – 150 KB for a 22-second smoke. The smoke's `volume` describes it:

| Key | What |
|---|---|
| `texture` | the atlas's file name |
| `frames`, `columns`, `width`, `height` | frames (simulation steps) in the atlas, frames per row, size in texels (196 frames: 10 per row, 2560 × 2560) |
| `frame_ticks` | per frame, the tick the server sent its step (7 or 8 ticks apart): frame k's cloud shows at that tick, blending into frame k + 1 until the next |
| `fading_frame` | the first frame with the fading flag, or `null` |
| `grid_origin`, `grid_size` | the grid's lowest corner (the detonation point − 320, world inches) and its edge (640) |

Layout: a frame is a tile of 8 × 4 slices (256 × 128 texels), slice z (0 at the bottom of the grid) at column z % 8,
row z / 8; a slice is 32 × 32 texels, texel column = grid x, texel row = grid y (CS2's axes); frames run left to
right, then top to bottom. A texel holds `sqrt(density / full)` × 255, `full` being the emitters' cap: the square root
keeps more steps for the thin edges.

A smoke whose voxel log stops reading partway (a record OFAST does not know yet) is simulated from the frames before
that point, and `ofast export-demo` prints a warning naming it.

The HE grenades that go off while a smoke's log runs, close enough for their hole to touch its grid, carve their
holes into its cloud before the frames are laid out (`export::smoke::Blast`; [demo.md](demo.md#smoke-voxels)): the atlas
already holds the hole, opening on the blast's frame and closing over the next 4.5 s (the volume blends each frame into
the next, so the hole starts to show up to one frame, 7 ticks, before the blast).

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
| `effects` | CS2's sprite sheets for the grenades' effects (`export::effects`), each written as `fx_<texture stem>.png`, repacked as a regular grid of frames: `role` (`fire`: the molotov's flames, `fire_small_sim_b`; `gas`: its tongues of burning gas, `fire_gas_batch_a_v2`; `blast_flame` and `blast_smoke`: an HE blast's fireball and smoke, `explosion_fireball_01_flame` / `_smoke`; `flare`: a blast's and a flashbang's flare, `particle_flare_001`; `sparks` and `spark_glow`: a blast's spark streaks and their glow, `sparks` / `simple_glow_001`; `debris`: its flecks, `fleks`; `scorch`: its scorch splat, `fluidsplat_a_noisy_trans_tga_c46d6803`; `fire_smoke`: the wisps a fire leaves, `wispy_steam_set`; `fire_scorch`: its soot, `particle_smokegrenade_trans_tga_9bc0185f`), `texture` (CS2 path), `file`, `width`, `height`, `cell` (a frame's size in pixels: the sheet's largest frame), `columns` and `rows` (cells across and down; 8 across), `stride` (cells from one sequence to the next: as many as the longest sequence needs, rounded up to 1, 2, 4 or 8 and to whole rows above, so a sheet of 64 one-frame flecks is 8 rows, not 64), `sequences` (`id`, `first_cell`, `frames`, `clamp`). The sheet's frames are cropped rectangles of their own sizes (the texture's `SHEET` data, [vtex.md](vtex.md#sprite-sheets)); in the grid each frame's cropped image sits where it was in its full frame |
| `viewmodels` | the players' first-person views ([animation.md](animation.md#first-person-view-animviewmodel-exportviewmodel)), one glTF per agent and glove skin: `file` (`<demo>_r<round>_viewmodel_<agent>[_<gloves>].gltf`), `model` (the agent), `gloves` (the glove model or `null`), `players` (animations `viewmodel_<controller>`), `weapons` (`model`, `mesh_groups` of each weapon in it, in the order of their bones `w0_…`, `w1_…`) |
| `fires` | where each fire's flames meet walls, for its flames climbing them (`export::effects::fire_walls_json`): `entity`, `start_tick` (as the replay's `fires`), `walls`: `flames` (the positions of the flames whose rays met it, as the replay has them), `position` (the point on the wall, at the flame's height) and `normal` (the wall's). From each flame, 10 units up, a 2-unit sphere is moved out in 8 directions up to 40 units through the map's collision; the first surface facing back along the ray and sideways, and still there 20 units higher, is a wall point (a floor, step, ceiling or low lip ends the ray); a point within 16 units of an earlier one on a wall facing the same way is that one, its flames gathered. CS2's client builds such points itself from the flames (the demo holds none: [demo.md](demo.md#grenades)) |
| `blasts` | each HE grenade's blast, for what it leaves on the ground (`export::effects::blasts_json`): `entity`, `tick`, `position` (its `hegrenade_detonate`), `ground`: the first solid surface facing up (normal z ≥ 0.5) straight below it within 128 units, `{z, normal}` (z where it touches), found with a 1-unit sphere moved down the map's collision (`world_physics`) half a unit at a time, or `null` (no collision, or nothing that close) |

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
| Viewmodels | with the agents manifest's `viewmodels`: per player a skeletal mesh actor (`VM <name>`) attached to their first-person camera, playing `viewmodel_<controller>`, its transform track keyed at the player's offsets (relative location (forward, right, up) × 2.54 cm) and mirrored (scale y −1, the sideways offset negated) from every tick `left_handed` turns on; `FirstPersonPrimitiveType` First Person and no shadows, the camera's `FirstPersonFieldOfView` = the player's `viewmodel_fov` at 16:9 (2·atan(tan(fov/2)·(16/9)/(4/3)): 60 → 75.18°, 68 → 83.93°); shown only while the camera cuts look through that player's eyes and they are alive |
| Doors | each door of the replay's `doors` that is not closed all round: the level's own door actor (a static mesh actor whose origin is within 2 cm of the door's closed position and whose rotation is its closed rotation; none: logged, not swung) keyed with the door's rotation, Euler-filtered as the weapons; every entry with the same closed position drives the same actor, which keeps its last rotation; a door an HE grenade blew apart (its entity leaves the demo at most 4 ticks before a blast within 128 units of its hinge) is hidden from the blast's tick (a visibility track; [demo.md](demo.md#doors)). The sequence binds the level's actors: re-importing the map (`import_map.py`) makes new ones, so run `import_demo.py` again after it |
| Dropped weapons | with the agents manifest's `dropped_weapons`: one static mesh actor per stretch of the replay's `dropped_weapons`, keyed on the stretch's keys (Rotator(roll = roll, pitch = −pitch, yaw = −yaw); each triple turned to the nearer of its two equivalent Euler forms, so a gun tumbling through the vertical never spins a full turn between two ticks in slow motion), shown from its first tick to its last |
| Grenades | with the agents manifest's models: one static mesh actor per entry of the replay's `grenades`, keyed on its keys like the dropped weapons, shown from its first tick to its last (the detonation of an HE grenade and a flashbang, a smoke's expiry, a molotov's landing) |
| Smokes | `smoke_volumes.py`: per smoke with a `volume`, a Heterogeneous Volume covering its 640-unit grid (placed at the grid's corner with CS2's highest y, Unreal's lowest; 64 cells per axis, scale 25.4) drawn by an instance of `/Game/OFAST/Replays/M_Smoke` holding its atlas (imported linear, uncompressed grayscale, no mipmaps), grid corner, frame timing, fading and tint. The material's Custom node samples two slices of two frames at the demo tick, which the sequence keys once for all smokes in the parameter collection `/Game/OFAST/Replays/MPC_Replay` (`ReplayTick`, two linear keys); density below 5 % of full shows nothing, 15 % above it is opaque (extinction 0.1 per cm, given per local unit: the volume ray-marches in its own space, a cell is 25.4 cm), and a fractal noise moves that threshold to break the surface into puffs; from the fading frame the threshold rises to 1, 48 ticks before the expiry. Albedo: light grey half way to the team colour (`color`). Smokes need `r.HeterogeneousVolumes.IndirectLighting 1` and `.Mode 2` (light from Lumen's translucency volume; without it a smoke in the shade is black): the importer sets them for its screenshots, but console variables end with the editor session, so set them in the project (`DefaultEngine.ini`, `[ConsoleVariables]`) or the Movie Render Queue job |
| Fires, blasts, flashes | `grenade_effects.py`, with the agents manifest's `effects`: CS2's sprite sheets played by a material on instanced quads, a "particle system in a material" (Niagara cannot be written from Python). Per fire an actor with one instanced mesh per layer: the main flames and the tongues of gas (8 and 3 sprite slots per flame), flames shooting up the walls it meets (2 per wall point of the agents manifest's `fires`, 5 units off the wall and 7.5 up from the floor, while any of its flames burns: streaks rising 40 – 100 units/s), all gone within 0.15 s of their flame's last tick (CS2's flames end with the fire); when it goes out, near-black wisps over its flames and up its walls for 5 s; a soot splat per flame (decals) darkening from when the flame lit until 8 – 20 s after the fire's end; a burning fire has no smoke in CS2; per HE grenade and flashbang going off, the same for its layers: a blast's flare, fireball, smoke and ring of smoke rolling along the ground, its sparks (streaks along their flight, thrown out and falling) with their glow, and its debris flecks — sparks and debris die at the ground under the blast (the agents manifest's `blasts`) — or a flashbang's flare. Sprites that fly are thrown along a random heading and up, slowed by CS2's drag (read as a share per 1/30 s) and pulled down by gravity, all in the material. A blast within 30 units of the ground leaves up to three scorch splats: decal actors on that ground, 8 units deep each way, with instances of `/Game/OFAST/Replays/M_Scorch` (near black, the splat texture's alpha, fading in over 0.13 s and out over the second half of 20 – 30 s); the agents take no decals. An instance holds its centre, the ticks it may spawn particles in (a flame's burning ticks; a detonation's tick, so it spawns once), a phase (where in its first lifetime a flame's slot starts; 0 for a detonation's, born on that tick) and a seed; the quad is a copy of the engine plane whose bounds reach 1 m up (`SM_Sprite`), its instances scaled 6 times, or as much as a flying layer's longest flight needs, so a sprite standing above the ground or flown away from it is not culled with it; the material (additive for flames and flares, translucent for smoke) respawns a particle every lifetime from the demo tick of `MPC_Replay`, drawing its size, place, roll and sheet sequence from CS2's ranges with a hash, faces it to the camera, plays the sequence over its life, fades it in and out and softens it against the world. A point light per effect, its intensity keyed: a fire's flickers with its burning flames, a blast's and a flash's pulse for 0.2 – 0.4 s. Checked against CS2 through the same eyes at the same tick: a molotov's carpet of knee-high flames lighting the room |
| Flashed views | each blinded player's POV camera gets a post-process material (`/Game/OFAST/Replays/M_Flashed`, after tonemapping; an instance per player in the round's folder, `/Game/OFAST/Replays/Effects/<demo>_r<round>`) with their latest 12 `blindings`: the view goes 0.6 × saturate((seconds left − 0.5) / 2.5) of the way to a warm white, as CS2 shows it (0.2 s into a 0.75 s blinding not visibly white; 6 ticks into a 3.8 s one, the weight solved per colour channel from the same view in CS2 and Unreal is 0.58 – 0.66) |
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
arrows; `spec_player_by_accountid` does nothing in demo playback), Steam's F12. Once (2026-10-23), two shots of a
run showed the sequence at another time (a POV camera at the round's start) and came out right in the next run:
check a shot that disagrees by rendering it again before looking for a bug.

A player whose death the players export baked as a ragdoll (`ragdolls` in the agents manifest) stays visible over
the death's ticks: the body falls and lies there to the end of the round
([animation.md](animation.md#deaths-exportdeaths-physicsragdoll)).

## Not yet

Foot IK of CS2's animation graph, weapon skins, holes blown in smokes, the grenades' finer effects (an HE blast's
debris, sparks and scorch decal, a molotov's flames climbing walls and its smoke), shots (muzzle flashes, tracers,
impacts), sounds, a name label above each player (Unreal's text render did not show in the
screenshots; the names are the actors' labels in Sequencer), several rounds in one sequence.
