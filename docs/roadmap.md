# Roadmap

Ordered so that **Demo → Unreal scene** works first. Every milestone ends in something usable on its own.

| # | Milestone | Done when |
|---|---|---|
| **M0** | Foundations + spikes | Workspace, CI, oracle harness. Spikes: VPK + KV3 read · demo header + player positions for one round · one model → glTF → UE 5.8. |
| M1 | Asset core | Textures, materials, meshes and a full map export to glTF + material/light manifests. |
| M2 | UE plugin + one-click map import | A CS2 map opens in UE with correct scale, master materials, lights from entities and a Lumen preset. |
| M3 | Demo → Sequence blockout | Full demo parser; proxy players move, POV camera, kill markers. Studio demo browser. |
| M4 | Real players | Skeletons + animation clips, agent models + weapons, replay rig, deaths. |
| M5 | First-person POV | Arms + viewmodel, event-driven animations, viewmodel FOV. |
| M6 | Grenades & FX | Trajectories; smokes / molotovs / flashes / HE / tracers / impacts in Niagara. |
| M7 | Camera tools | Campath editor, smoothing, cinematic presets; HLAE `.cam` import. |
| M8 | ofast-capture | Free cam, campaths, multi-pass streams + FFmpeg; then live bone capture. |
| M9 | Render presets | Movie Render Queue presets, depth/matte passes for compositing. |
| M10 | Skins (stretch) | Default finishes, then paint kits / wear / seed / stickers. |

## M0 progress

- [x] Repository, license, docs skeleton, decision log
- [x] CI workflow
- [x] VPK reader (synthetic tests in CI, oracle tests on the local CS2 install — MD5 + BLAKE3 chunks, CRCs)
- [x] Binary KV3 decoder v1–v5, none/LZ4/ZSTD — all 245,863 KV3 blocks in CS2 decode exactly
- [x] KV3 text writer
- [x] KV3 text parser — all 1333 `m_keyValueText` texts in CS2's models parse and round-trip (M4.4e's procedural
  bones read their constraints with it)
- [x] Resource container reader
- [x] `ofast` CLI: `find`, `export-model`, `vpk verify` (`crates/ofast-cli`)
- [x] Demo header + player positions for one round (whole 216 MB match demo parses in ~3.5 s; round 1 positions and
  eye angles identical to demoparser2 for all 50,740 player-ticks; all 20 round starts match)
- [x] One model → glTF → UE 5.8: the AK-47 (geometry, 4 textures, materials) imports and renders by script
  (`ofast export-model` + `tools/unreal/run_shot.ps1`). Own meshopt decoder; all 13,105 CS2 models decode.

## M1 progress

- [x] **Axes** settled and measured: Source `(x, y, z)` → Unreal `(x, −y, z) × 2.54` (`coords.rs`,
  [decision 003](decisions/003-axes.md)).
- [x] Map reader: world, world nodes (scene objects, aggregates, instanced fragments), entity lumps
  (`crates/ofast-core/src/map/`, [map.md](formats/map.md)).
- [x] `ofast export-map de_mirage`: 1.23 M triangles, 836 placements, 372 materials, 705 textures, 72 lights + a
  manifest (sun, sky, spawns) in about 1 s.
- [x] Material rules from CS2's shaders: hemi-octahedral normals with roughness in B, metalness in `g_tMetalness`.G,
  two-layer blending with `F_FANCY_BLENDING`, overlays, tints ([material.md](formats/material.md)).
- [x] Unreal: scripted scene import (`tools/unreal/import_map.py`), layer-blend master material built by script,
  Nanite with DX12/SM6, screenshots from spawns.
- [x] **Verified against CS2**: CT spawn view in CS2 (`setpos`/`setang`, -insecure) vs Unreal — same framing to the
  pixel; textures, blends, decals and railings match.
- [x] **3D skybox**: the separate 1/16-scale world is loaded from its own archive and placed around the map
  (`sky_to_world`, [map.md](formats/map.md#3d-skybox)); the hotel and rooftops sit where CS2 shows them.
- [x] **Sky**: the `env_sky` BC6H cube map → equirect `.hdr` → Unreal TextureCube for the visible sky and the Sky Light.
- [x] **Lighting calibrated against CS2** ([lighting.md](formats/lighting.md)): one unit constant, sun and sky light
  from the map, local lights rebuilt as Unreal point/spot/rect lights (rect lights halved: Unreal shines them at 2×
  their candela), fixed exposure measured on CS2 screenshots, measured bounce gains for Lumen. Four views: CT 1.00,
  T 0.97, palace 0.965, B apartments 0.75 of CS2's brightness (0.6 once the shadow proxies below really cast).
  CS2's bloom, colour correction and fog measured to have no visible effect.
- [x] **Two import styles**: `standard` (CS2's picture) and `realistic` (Unreal sky, real-world light units, auto
  exposure, filmic tone curve) — [lighting.md](formats/lighting.md#import-styles).
- [x] **Same pipeline on de_dust2, de_inferno, de_nuke**: all import and render; the bugs it found were fixed for
  every map (layer detection, screenshot timing, shadow-proxy count, GPU crash on re-import).
- [x] **CS2's environment shaders** (`csgo_environment`, `csgo_environment_blend`): colour adjustments baked,
  height blend in Unreal ([material.md](formats/material.md)).
- [x] **Hable tone curve** (CS2's filmic curve, used by 10 of the official maps checked) as a replacement tone mapper
  in Unreal; level fitted on de_inferno ([lighting.md](formats/lighting.md#exposure-and-tone-mapping--post_processing_volume--vpost)).
- [x] **Shadows from the back of walls** that actually cast: shadow proxies without Nanite, sun only, for world
  geometry only (CS2's `OBJECT_TYPE_MODEL` flag) — de_inferno's T-spawn street went from 2.7× to CS2's shade.

### Every official map, not one script per map

The pipeline is generic: it reads everything from each map's own files, and a fix is per *feature* (a shader, a
light type), never per map. `cargo run --release --example shader_census` lists the material shaders of all 24
playable maps: 20 shaders in total.

| Shader | Maps | Materials | State |
|---|---|---|---|
| `csgo_environment` | 23 | 1346 | done (colour adjustments baked) |
| `csgo_complex`, `csgo_vertexlitgeneric` | 24 | 2137 | done |
| `csgo_environment_blend` | 20 | 1041 | done (height blend) |
| `csgo_lightmappedgeneric` | 24 | 635 | done (layer blend) |
| `csgo_foliage`, `csgo_static_overlay`, `csgo_simple`, `csgo_glass` | 15 – 23 | 796 | basic (glTF PBR) |
| `csgo_effects`, `csgo_simple_3layer_parallax`, `csgo_simple_2way_blend`, `csgo_water_fancy`, liquids, … | 1 – 17 | ~110 | to do |

Plan: handle the remaining shaders by impact, then a regression suite that exports, imports and screenshots the
spawns of every map, against CS2 reference shots taken once per map (a console `.cfg` can chain `setpos` / `setang`
/ `screenshot`). Also to explain: 576 materials the census cannot read (180 on de_boulder, 101 on cs_shelter and
de_fachwerk) — probably stored in another archive.

### Still to do (M1 → M2)

- Deep interiors lit by sunlight bouncing down corridors (B apartments, 0.6 of CS2 now that no sunlight leaks
  through their walls): Lumen carries less of it. Same family: de_inferno's loggia and façades in the shade
  (0.4 – 0.8), de_dust2's CT room (0.11).
- The Hable curve's level holds for de_inferno with OFAST's light as is. Capture a second Hable map in CS2 (sky, lit
  and `mat_fullbright` shots at a locked exposure) to separate level, light (Unreal 1.46× CS2's on de_inferno) and
  sky (which `env_sky` CS2 draws when the skybox's and the world's differ).
- de_inferno T spawn: a pillar and an arch floor get sunlight in Unreal that CS2 shades (not found yet).
- Materials with more than two layers, detail textures, vertex colour tint (`COLOR` attribute), glass.
- Rebuild `csgo_effects` dust / light-shaft cards as Unreal effects.
- Colour texture alpha mask on weapons: find what it drives.
- Move the Unreal Python into the `ofast-unreal` plugin (M2).

## M3 progress

- [x] Game events (`player_death`, round milestones, bomb) — every kill of a 20-round match equals demoparser2's.
- [x] `ofast demo-rounds` and `ofast export-demo --round N`: one round as a replay file
  ([replay.md](formats/replay.md)).
- [x] Unreal: `import_demo.py` builds a Level Sequence in the map's level — player proxies in team colours, a
  first-person camera per player (CS2's field of view), camera cuts on each killer, kill markers. Checked: each
  killer's camera has the victim at the crosshair just before the kill.
- [x] Compare first-person frames with CS2 itself (`demo_gototick`, `spec_player`): de_nuke round 2, tick 8135 —
  the killer's view and the victim's position match (within ~3° vertically, CS2 interpolating the paused demo).
- [x] A complete round per sequence, the default (2026-10-26: a fragmovie rarely takes a whole match, so several
  rounds or a match in one sequence is not planned). Grenades: M6. (Weapons in hand: M4.5.)
- [x] **Studio demo browser (Tauri app)** (2026-10-26). The effects rework (M6) comes last.
  - [x] Setup checks (CS2 through Steam's library list, Unreal Engine 5.8 through the Epic Games Launcher's, the
    Unreal project) and the demo list with each demo's map and server.
  - [x] A demo's page: every round with its winner, score and best moments, its kill feed, and a finder for
    highlights (3K and more, aces, won clutches, collaterals, no-scopes, wallbangs, through smoke, airborne, blind,
    knife) or every kill, by player and weapon. One pass over the demo (`demo::summary`; 6 s for a 22-round major
    demo), kept in the app's cache.
  - [x] A round replayed in 2D on the map's radar (2026-10-26): CS2's radar image and overview (`map::overview`),
    the round's positions every 4 ticks, framed on the area its players use.
  - [x] "Send to Unreal" (export, map import the first time, round import, with progress) and "Open in Unreal"
    (2026-10-26): a round of a 22-round major demo in about ten minutes, its map already imported.
  - [x] A beginner's guide to the Studio ([user guide](user-guide/README.md)).

## M4 progress

Design: [decision 013](decisions/013-replay-rig-baked-in-rust.md) — CS2's agents as skeletal meshes, each player's
animation computed in Rust from the demo (CS2's own clips, blended per tick) and imported as one AnimSequence per
player and round.

- [x] **M4.1 Skinned agents**: skeleton (`m_modelSkeleton`), bone remapping, four weights per vertex → glTF skin;
  `ctm_sas` imports into Unreal as a skeletal mesh in CS2's bind pose (94 bones, pelvis and head at CS2's heights).
  [model.md](formats/model.md#skeleton-and-skinning).
- [x] **M4.2 Clip decoder** (`.vnmclip`: per-track quantised poses, 30 fps): all 2735 CS2 clips decode exactly,
  retargeted onto the agent's skeleton by bone name, exported as glTF animations (`ofast export-model … --clip …`).
  In Unreal the clips import as 30 fps AnimSequences and pose the agent where the decoder puts the bones checked
  (pelvis, head, hands, ankle: within 0.1 cm);
  from a CS2 player's viewpoint in a demo, the knife run's stride matches CS2's, while CS2's arms and lean come from
  the graph's weapon and aim layers (M4.4). [animation.md](formats/animation.md#verified).
- [x] **M4.3 Rig inputs** in the round tracks and the replay export: body yaw, velocity, ground, crouch amount,
  walking, scoped, defusing, ladder, the weapon in hand (item and class), its animation state, reloading, jump and
  landing ticks (game ticks converted to demo ticks). Round 1 of the match demo equals demoparser2 on every field it
  can read (50,740 rows); velocity follows the positions. Found on the way: `GameTick_t` is a signed varint.
  [demo.md](formats/demo.md#rig-inputs), [replay.md](formats/replay.md).
- [x] **M4.4 Baked locomotion**: CS2's own locomotion graph read from the game (crouch blend of two 2D blends on
  the body-relative velocity, its triangles and synchronised phase), baked per player and tick from the rig inputs
  (`ofast export-demo --players`: one glTF per team), played in Unreal on agent actors moved by the replay; round 5
  checked through a player's eyes against CS2 (positions, facing and stride match; arms differ).
  [animation.md](formats/animation.md#locomotion-graph-worldmodel_locomotionvnmgraph).
- [x] **M4.4b Weapon layers and aim**: each weapon's clips found by walking CS2's main and weapon graphs (names
  from `items_game.txt`); its additive idle pose, draw and reload (model space, upper-body mask) and additive shots
  over the locomotion; CS2's aim node rebuilt (spine yaw and pitch, weapon pitch and barrel alignment, two-bone hand
  IK). Checked through players' eyes against CS2: a rifle draw matches, a rifle firing stance is close (CS2's hands
  sit higher and its head drops to the sights). [animation.md](formats/animation.md#the-main-graphs-upper-body-worldmodelvnmgraph).
- [x] **M4.4c The rest of the graph**: CS2's graphs evaluated state by state (`anim::eval`: clips, blends,
  selectors, scales, state machines by entry condition, default state and "state completed", the graphs' own
  transition times) for the states decided from the demo (`anim::states`): knife attacks with their swing-back
  variants, grenade pin pulls and throws by strength, the Healthshot and silencer, draws from the holster (the
  weapon leaves it when the draw has blended in) with the hands returning at the clip's `WPN_IK_ACTION_ENDING`,
  CS2's weapon snap onto the hand, C4 planting and defusing (enter, loop), starts, plants and turns on direction
  reversals, turning on the spot, jumps, falls and landings with their body additives, ladders, breathing.
  Checked against the demo's own events over five rounds and in Unreal close-ups.
  [animation.md](formats/animation.md#evaluating-the-graphs-animeval-animstates).
- [x] **M4.4d Graph rules, ladders, small turns, flinches, flashes**: actions end when the weapon's graph leaves
  them (`Evaluator::leaves`), so a draw plays through the inspect state that follows it — CS2's raised knife at
  tick 26508 was the Huntsman's draw, now matched; ladders turn the legs back to the ladder (its direction from the
  approach, `ladder_angle_yaw`, the back clips); small turns on the spot (10 – 45°) besides the loop; body, head and
  fire flinches from `player_hurt` (hit group and side from `bullet_damage`); the flashed pose from the pawn's
  `m_flFlashDuration` (tournament demos have no `player_blind`); spectators (a major demo's observer) left out.
  [animation.md](formats/animation.md#evaluating-the-graphs-animeval-animstates).
- [ ] M4.4e Foot IK on slopes and stairs; checks against CS2 of the ladder legs, flinches and the start / turn
  thresholds. Done so far: the model's procedural bones (`BoneConstraintList` in `m_keyValueText`, read with a new
  KV3 text reader: tilt-twist, point and aim constraints on every exported frame; the bind pose kept on all 91
  agent and glove models, the hand's roll shared by the forearm's twist bones —
  [animation.md](formats/animation.md#procedural-bones-animconstraints)); a major's demo (Aurora vs FURIA,
  de_nuke) checked: glove skins merged as CS2 draws them, flashed poses, knife inspect and sprays matching from
  CS2's own Chase Cam positions ([animation.md](formats/animation.md#verified)).
- [x] **M4.5 Weapons and each player's look**: models named by resource id (`m_hModel` = MurmurHash64B of the path,
  checked on every resource reference); each player's own agent, mesh groups (defuse kit) and glove skin, one glTF
  per look; every weapon carried (`m_hMyWeapons`, its own model: knife skins, CS:GO-era meshes) baked as a bone of
  the agent — in hand on `wpn` (the weapons' hand guides show it is CS2's reference), holstered on the agent's
  attachments (rifles slung, pistols and knives on the thighs, grenades on the vest, the C4 on the back), hidden when
  not carried. Round 5 against CS2 through players' eyes: knife in hand, rifles slung on the same diagonal,
  holstered pistols and grenades where CS2 draws them. Glove skins merged by bone name but not seen in the game yet
  (no glove skins in the local demo). [animation.md](formats/animation.md#weapons-in-hand-and-holstered-exportloadout),
  [demo.md](formats/demo.md#models).
- [x] **M4.6 Deaths**: each death as CS2's ragdoll of the agent (its `PHYS` block: 15 capsules, 14 conical joints
  twisting about z), simulated in Rust (extended position-based dynamics) against the map's own collision
  (`world_physics`: solid world, grates and glass, not the player clips) from the demo's death fields (hit bone, force,
  position) and the last living pose's motion, baked into the glTF animation; the body stays visible in Unreal.
  Checked: all 46 deaths of three rounds of two demos baked, bodies at rest on floors; against CS2, a running
  player's body lies where CS2's does to about 0.5 m, face down like CS2's (decision 015,
  [animation.md](formats/animation.md#deaths-exportdeaths-physicsragdoll)). Dropped weapons: the guns the dead let
  go of (and weapons dropped for teammates, a dropped C4) moved as the server simulated them, read from the demo
  (owner-less weapon entities send their origin and rotation each tick) and placed in Unreal
  ([demo.md](formats/demo.md#dropped-weapons)). Doors: the map's rotating doors swing as the demo has them (each
  the level's own door actor, found at its hinge), and models are drawn in their reference pose as CS2 skins them
  (de_nuke's doors stood 90° off: a closed door looked open; [model.md](formats/model.md#reference-pose-modelrefpose)).
  To do: the force scale against more deaths; why the old Aurora vs FURIA demo's A-site door does not block players
  though networked closed.
- [x] **Character and weapon materials, first pass**: roughness read where each material keeps it (94 character
  materials, the SAS body among them, keep it in `g_tAnisoGloss`, not the normal map: they rendered mirror-smooth),
  weapons' 2 % dielectric specular (rendered as 4 % until 2026-10-23: Interchange's master drops the IOR term). The
  SAS agents now read as dark cloth like CS2's.
  [material.md](formats/material.md#characters-and-weapons-csgo_character-csgo_weapon).
- [ ] Characters' rim light, retro-reflectivity and colour adjustments (agents in the shade still read darker than
  CS2's: 0.64 – 0.74 of its brightness on round 5's fabrics; their non-cloth materials keep Interchange's F0 0.08
  meanwhile); why CS2's knife blades look brighter (`g_flMetalnessTransitionBias`?). Done: CS2's cloth shading (its
  sheen of √albedo on the cloth mask, no highlight there) as glTF sheen and specular textures
  ([material.md](formats/material.md#characters-and-weapons-csgo_character-csgo_weapon)).
- [ ] Weapon skins (paint kits, wear, seed, stickers: M10), and the molotov / incendiary holster if CS2 shows one.
- [ ] **M4.7 CS2's pose recipe** ([decision 020](decisions/020-cs2-pose-recipe.md)): every player pawn sends the
  AnimGraph2 task list its graph ran each tick (clips, times, blend weights, bone masks), Esoterica's task
  serializer; bake from it instead of re-deciding the graph. Done so far: `demo::recipe` reads every player's
  recipe; all 114,455 of a round decode without an error, 98.4 % with every clip and blend (the rest have a clip
  after a task whose payload is not known yet); each demo's resource index table learned by co-occurrence with the
  clips OFAST's own bake samples, one clip per index (93.2 % of round 5's sample tasks named); the recipes played as
  poses with OFAST's aim over them (86 % of round 5's living player-ticks), the default since 2026-10-26
  (`OFAST_RECIPE=0` for the re-decided poses only): the demo's hits sit closer to the hitboxes posed from them (371
  of 505 within an inch, 349 with the re-decided poses), and side views rendered from CS2's free-camera positions
  where the two bakes differ most match CS2's at least as well (the table also showed the bake played a gun's shot
  clip once per spray, not per bullet) ([demo.md](formats/demo.md#pose-recipe-cs2s-own-animation)). To do: the
  table for the rest (or its rule), the remaining task payloads (the aim, then CS2's aim weights over OFAST's).

## M5 progress

- [x] **Design** ([decision 016](decisions/016-viewmodel-baked-like-the-body.md)): CS2's viewmodel baked in Rust from
  the demo's weapon states like the body, drawn by Unreal's first-person rendering.
- [x] **Demo**: each player's viewmodel FOV, offsets and hand (switched with a key during rounds: keyed per tick),
  the silencer and the reserve ammunition ([demo.md](formats/demo.md#first-person-view)).
- [x] **Graphs**: CS2's viewmodel graphs evaluated (weapon-skeleton poses, layer masks and weights, scale nodes,
  float maths, ID switches, restarts through twin states, clip events), a segment per weapon state blended over the
  graphs' transitions, the hand IK ([animation.md](formats/animation.md#first-person-view-animviewmodel-exportviewmodel)).
- [x] **Export**: one glTF per agent and glove skin: the arms, the agent's first-person meshes, the gloves, every
  weapon held with its own bones (bolt, magazine, silencer), an animation per player.
- [x] **Unreal**: each viewmodel on its player's POV camera at their offsets and hand, first-person rendering at
  their viewmodel FOV, shown in their own cuts.
- [x] **Checked against CS2's first-person view**: four kills (right- and left-handed, FOV 60 – 68, three offsets),
  two reloads, two knife inspects match in size, angle, place and pose; on the local match demo every frame follows
  the weapon in hand, the arms keep their lengths, every shot starts on its tick.
- [x] **Recoil on the view** ([decision 019](decisions/019-recoil-on-the-view.md)): the camera turned by the aim
  punch (rebuilt from the shots: each kick from the view punch the demo sends, or the gun's recoil table) and the view
  punch, the viewmodel past it; YEKINDAR's view mid-spray frames as CS2's does. The third-person body aims where the
  bullets go (the eyes plus twice the punch): a sprayer's gun keeps to the bullets' line as in CS2's side views.
- [ ] Sway behind turns, bob while running, landing dip (CS2's client code); ironsights; shotguns' staged reloads;
  `FirstPersonScale` against walls; grenade throws and knife swings checked against CS2; the viewmodel's place
  mid-spray (its muzzle 4 % of the frame higher than CS2's).

## M6 progress

- [x] **Design** ([decision 017](decisions/017-grenades-and-effects.md)): CS2's grenade data read and baked in the
  core, effects built from scriptable pieces in Unreal (Heterogeneous Volumes, sprites, lights), no particle
  converter yet.
- [x] **Demo**: grenade projectiles per tick (model, thrower, bounces), detonations, smokes' clouds and voxel logs,
  fires and their flames ([demo.md](formats/demo.md#grenades)); checked against the grenade events on two demos.
- [x] **Replay**: grenade flights keyed like the dropped weapons, detonations, smokes, fires
  ([replay.md](formats/replay.md)); in Unreal each grenade's model flies from the throw to its detonation.
- [x] **Smokes**: the voxel log decoded ([demo.md](formats/demo.md#smoke-voxels)), OFAST's own simulation fitted
  to the bricks CS2's server sent (39 – 46 % within 2 frames, 7 – 14 % missed, on four demos of three maps), each
  cloud an atlas texture next to the replay.
- [x] **Smokes in Unreal**: a Heterogeneous Volume per smoke, its frame from the keyed demo tick, puffy surface,
  team tint, lit by the room; checked against CS2 through a player's eyes (a T smoke in a de_nuke corridor: same
  place, size, height and colour; two CT smokes through doorways; gone before the expiry as in CS2).
- [x] **Effects groundwork**: CS2's sprite sheets read (`Texture::sheet`) and written as regular flipbooks with
  `--players`.
- [x] **Fires, blasts, flashes**: CS2's sprite sheets played by a "particle system in a material" driven by the
  demo tick (fires per flame, HE fireballs and smoke, flashbang flares), lights keyed per effect, the flashed POV
  fitted to CS2. Checked against CS2 at the same ticks: a molotov's flames through the same eyes (same place and
  height; CS2's denser and paler), an HE blast's fireball and smoke (from a camera past the A door, which Unreal
  draws closed on that old demo), a long and a short blinding.
- [x] **Holes and blast debris**: HE blasts carve holes into the simulated smokes (240 units, open 0.5 s, closing
  over 4 s; CS2 sends none, its clients carve them too), fitted to CS2 at five moments after a blast in a smoke; a
  blast's flash, ground smoke, sparks, glow and debris flecks fly in the sprite material and die at the ground the
  export finds under it; up to three scorch decals; a door a blast breaks disappears, as in CS2.
- [x] **Fires on walls and their smoke**: wall points traced from each flame through the map's collision, flames
  shooting up them; soot darkening under a fire as it burns; when it goes out its flames end with it and it leaves
  5 s of near-black wisps (CS2 has no smoke on a burning fire).
- [ ] A blast's heat haze, embers and model debris; a fire's outline flames.
- [x] **Shots** (decision 018): each bullet rebuilt from its shot's random seed, exactly as CS2's clients do (99 of
  the 100 panes of glass and 725 of the 726 hits on players the two demos record lie on the rebuilt path), traced
  through the map's collision, up to the weapon's range, to the triangle it met and that
  surface's impact and bullet-hole decal; through a wall into a player the demo shows hit; third-person muzzle flashes at
  the barrel the agents show, with a light; tracers as fast as CS2's; dust, flecks and sparks by surface; blood; bullet
  holes, the earlier rounds' too (CS2 keeps them). Checked against CS2 through three players' eyes and a chase camera.
- [x] First-person muzzle flashes: on the viewmodel's barrel, CS2's flash made smaller as its `_fps` systems do, in
  Unreal's first-person rendering, with its own light; the AK's second flame (`_alt`, wispy tongues) picked per shot
  as CS2 picks it. Fitted to CS2's flash through two players' eyes.
- [x] **Penetration, brass, tracers**: bullets go through walls and players by CS:GO's penetration rule with CS2's
  values (the demo's hits after walls start where OFAST's trace comes out of them, 456 of 545 to half an inch; CS2's
  damage through walls is the rule's on 289 of 314 hits), impacts and holes on both sides; each gun's case thrown from
  its `shell_eject` as its casing system flies it; a tracer on every shot, from the moment it was fired (checked in
  CS2 through YEKINDAR's and soulfly's eyes).
- [x] The shooter's own tracer from their viewmodel's barrel; the rotating doors in the bullets' way; cases thrown at
  the shot's true moment (CS2's first-person case crosses the view as a left-handed player's mirrored viewmodel
  throws it, and so does OFAST's).
- [x] A player's depth from their hitboxes (the agent's 19 capsules on its posed bones, where CS2's server checks
  the shot); the map's `prop_dynamic` models and brush entities checked and left out: CS2's bullets go through them
  (on 13 demos they made every count worse).
- [ ] Shots, the rest: each gun's own muzzle flash, bullet holes that move with doors, what the hits after walls
  that the trace does not explain came out of.
- [x] The map's physics props as the demo moves them (`CPhysicsPropMultiplayer`: de_nuke's hanging hard hats and
  mugs, 25 of them): in round 5 an HE grenade knocks the A site's three hats off their rack, and they lie on the floor
  where CS2 shows them.
- [ ] **Effects rework**: every effect so far is a first pass. They are in the right place, at the right size and
  time, but their look is far from good. All of them get reworked: smokes, fires and their smoke, HE blasts,
  flashbangs, muzzle flashes, tracers, impacts, blood and holes. When the rework starts, each effect's gaps get
  listed against CS2, side by side. Scheduled last (2026-10-26), after the Studio app.

## M7 progress

- [x] **Design** ([decision 021](decisions/021-cameras.md)): every camera of a round made in Rust (`camera`,
  `export::cameras`) and written to the round's cameras file ([cameras.md](formats/cameras.md)); in Unreal, each
  player's own camera hides that player's body by owner (`bOwnerNoSee`, `bOnlyOwnerSee`), so any camera can be cut
  to, one the user adds included.
- [x] **First-person smoothing**: a Whittaker smoother on the eye angles, pinned to the original aim on the ticks
  around each shot and kill (the crosshair stays on the bullets); the recoil on the view moved to Rust with it.
- [x] **Shot cameras** for a kill's last 4 seconds: behind the killer, around him, a fixed camera that sees both
  players, from above, or the victim's eyes; kept out of walls with the map's collision.
- [x] **HLAE**: `mirv_campath` XML and `mirv_camio` `.cam` read and written; a campath sampled with HLAE's own
  interpolation (cubic splines at rest at both ends, McEnnan's quaternion spline); times from CS2's game clock
  (`RoundTrack::server_tick_offset`). `ofast export-demo --hlae`, `ofast camera-to-hlae`.
- [x] **Studio**: a camera per kill in the round's feed, first-person smoothing, HLAE files, "Update cameras" for a
  round already in Unreal (`import_cameras.py`: a minute instead of a full import).
- [x] **Back to HLAE**: a camera of a built round written back from Unreal as Sequencer plays it
  (`export_camera.py`), then `ofast camera-to-hlae`. Checked on de_dust2 round 17 (2026-10-26): a fixed camera
  read back matches its exported keys to 0.0001 units and degrees on all 288 ticks.
- [x] **Checked in Unreal** (de_dust2 round 17, 2026-10-26): every shot kind through screenshots, owner visibility
  after the level was saved and loaded again (through XANTARES's eyes his body stays hidden), "Update cameras" from
  the Studio (6 shot cameras replaced by 2, 12 cuts, the eyes smoothed, 16 muzzle lights found by their tags).
- [ ] A campath and a `.cam` made in CS2 with HLAE, imported (HLAE is not installed here); a camera made in Unreal
  played in CS2 with HLAE.
- [ ] Editing in Unreal: a guide to making and keying one's own camera in Sequencer.
- [ ] Shots in tight places: a camera behind the killer pulled in by close walls (a stairwell: 20 – 30 units) should
  rise over his head instead; a fixed camera can frame its killer's back up close early in its shot.

## M8 progress

- [x] **Research** ([notes](research/hlae-cs2-capture.md)): how HLAE gets into CS2, finds its insides, overrides
  the camera and records passes to FFmpeg.
- [x] **Design** ([decision 022](decisions/022-capture-attach.md)): attached to a CS2 started with `-insecure`
  (checked by the launcher and again by the DLL), CS2's own interfaces, every slot, offset and pattern checked
  before use.
- [x] **Attach and clock** (`apps/capture`, `ofast-attach`): the DLL in CS2, the client's frame hooked, the game's
  clock read; it matches the demo's server tick offset OFAST reads (M7's HLAE time mapping, checked in the game).
- [x] **Frames**: `Present` hooked, the back buffer read through a staging texture, written as TGA; demo-timed with
  `host_framerate`, started by demo tick.
- [x] **OFAST's cameras in CS2**: the view setup patched, a cameras file's camera (or a player's eyes) played at the
  game's clock; OFAST's eyes of a player match CS2's own view of him to a few pixels (2026-10-27).
- [ ] FFmpeg (frames are images until FFmpeg is installed), extra passes (depth, mattes), the Studio's buttons
  (attach, play a camera, record a shot), detaching without quitting CS2.
- [ ] Live bone capture for exact player animation (no prior art in CS2).

## M9 progress

- [x] **Design** ([decision 023](decisions/023-render-presets.md)): Movie Render Queue's classic configuration built
  by script (`tools/unreal/render_round.py`), three presets (preview 720p, final 1080p with 8 samples, 4k) saved as
  assets for Unreal's own Movie Render Queue window; 60 frames a second.
- [x] **Passes** ([render.md](formats/render.md)): the picture, a players layer by stencil (bodies, visors,
  viewmodels, with alpha), the world without them, depth in centimetres; PNG folders per pass and one multi-layer
  EXR a frame.
- [x] **Checked** (de_dust2 round 17, 2026-10-27): a first-person cut renders without its player's body (decision
  021's owner visibility holds in Movie Render Queue), a fixed camera's kill puts both players in the players layer,
  the depth reads true distances; final at about 2 s and 20 MB a frame on an RTX 5070, preview 1.4 to 7 frames a
  second. The `4k` preset is not rendered yet.
- [x] **Studio**: "Render" on a round already sent, a preset, around the kills (the finder's highlight, or all of
  them) or the whole round, into `Videos\OFAST`, progress in the bar, "Open folder" at the end.
- [ ] Movie Render Graph (the classic configuration is "legacy" in 5.8's own code), a depth pass readable in 8 bits,
  sharp frames for velocity edits (no motion blur), the linear (HDR) picture in the EXR, the players layer without
  the effects near them, other passes (normals, motion vectors, one matte per player).

## M10 progress

- [x] **Skins in the demo** (`demo::skins`, [demo.md](formats/demo.md), "Weapon finishes"): each weapon's paint kit,
  seed, wear, StatTrak, name tag, stickers (with CS2's free placement) and keychain from its econ attributes, per
  round (`RoundTrack::skins`); names from `items_game.txt` and `csgo_english.txt` (`items::section_values`,
  `language_tokens`). Seen on Aurora vs FURIA round 17: 67 weapons, Fire Serpent, Asiimov, Dopplers, Fades.
- [x] **What a skin is painted with** (`paint`, [research](research/cs2-skins.md)): paint kits from `items_game.txt`,
  their composite materials read (includes, containers, loose variables, rolls), the seed's roll (a uniform random
  stream), and the parameters a weapon material gets (`weapon_params`; `examples/paint_params.rs`: MAJ3R's Fire
  Serpent, style 6 on the legacy AK's composite inputs, wear texture at scale 1.706, rotation 183°).
- [x] **Skins painted** ([decision 024](decisions/024-skins.md), `export::paint`): a bake over each weapon material's
  UV layout, a colour and a metallic-roughness texture per skin in the players' and viewmodels' glTFs, for custom
  paint jobs and gunsmith finishes. Round 17: 12 of its 28 skins (Fire Serpent, Asiimov, Gungnir…), seen in Unreal
  in first and third person.
- [x] **CS2's paint shader as the reference** (the user's choice, 2026-10-27): the bake ported from the decompiled
  `csgo_customweapon` in GameTracking-CS2 — the placements, the wear, the grunge and the surface every style
  shares, and the solid style (the decompiled default variant). Round 17: 13 of 28 (Whiteout added).
- [x] **Every paint style** ([research](research/cs2-skins.md), "Reading CS2's paint shader"): the nine styles'
  code read from CS2's own shader file (each static combo's SPIR-V decompiled with SPIRV-Cross;
  `tools/shaders/vcs_combos.py`, `examples/shader_blocks.rs`), both input layouts (CS2's own weapons use separate
  channels), ported: Dopplers, Fades (projected from the weapon's 3D position), Crimson Web, Hot Rod, gunsmith's
  metal parts. Round 17: 27 of 28.
- [x] **The composite's steps and the variants** ([research](research/cs2-skins.md), "The composite's steps"): each
  generated texture's steps run with their conditions (a newer kit's own colours, masks and switches); overlays,
  case hardening, spray paint's halftone and all-masks variants ported from their combos
  (`examples/paint_bake.rs` bakes any kit, `examples/paint_census.rs` counts what kits use). Round 17: 28 of 28
  (Royal Guard in its blue and gold).
- [x] **Weapons lying in the world painted**: one glTF per model and skin (the replay's and the manifest's
  `finish`); round 17: 13 of its 31 dropped stretches carry a skin, each matched to its file.
- [x] **Gloves painted** ([research](research/cs2-skins.md), "Gloves", `export::gloves`): the demo's glove finish,
  the glove composite (the target's attributes, a roll per hand, the left hand's copies) and its own shader
  (`csgo_customglove` combo 0: four material layers, an 8-tint palette, the pattern's paint, curvature damage, edges
  by derivatives) ported; third and first person keyed by glove skin. Round 17: 7 of 10 (Crimson Kimono, Emerald
  Web, Vice, Foundation).
- [x] **2025's glove kits** (`export::gloves::blended`): the height-blended program (layers of surface over
  substrate, tint ids, the pattern as tint or paint, damage by height, burnishing, grime, five colour-adjust
  matrices per layer). Round 17: 10 of 10 (Dragon Blue, a tech gradient, quilted white).
- [x] **Stickers** (`export::stickers`): baked into the weapons' colour and metallic-roughness (painted or not) as
  CS2's weapon shader draws them in its second UV set: the model's sticker slots, free placements, colour fitting,
  metallic foils and golds, holograms seen face on, paper backing and wear. Round 17: 52 sticker textures; FalleN's
  MP9 matches CS2's.
- [x] **Keychains and name tags** (`export::keychains`, `export::nametags`): the charm hanging from the item's point
  on the weapon (third person, first person, lying weapons) with the community charms' own materials, the name tag
  plate with the name drawn on it (third and first person, lying weapons, readable in a left hand as in CS2).
- [x] **Charms swing** (`export::keychains`, swing): the charm's joints swing in the animations as a chain of rods
  under gravity with the cloth's hinge limits, moved by the weapon in the world (third and first person).
- [x] **StatTrak counters** (`export::stattrak`): the module with its digits set as CS2's vertex shader sets them,
  counting up on the kills during the round (third person, first person, lying weapons).
- [x] **Patina age** ([research](research/cs2-skins.md), "Patina age is unused"): no kit uses it; the four that
  seemed to set `g_bLegacyPatina` (a recipe condition written `True`, misread until then) and are painted as gunsmith,
  like CS2's previews of them, with two gunsmith fixes their shader combos showed (a roughness texture, an overlay's
  wear).
- [x] **Trilinear case hardening** ([research](research/cs2-skins.md)): the case-hardening ramp read with the pattern
  projected from the weapon's position (Soch Mountains M4A1-S); its bakes show CS2's previews' blue mountains, cream
  waves and gold, not checked in a render.
- [x] **Gold stickers' tone** (checked 2026-10-29, FalleN's MP9 at tick 152488, CS2 at gamma 2.2): not the stickers'.
  At the red paint's equal brightness, Unreal's picture is more saturated than CS2's everywhere (red 0.80 against
  0.67, the gold 0.53 against 0.38); the gold metal is also a tenth darker, lit by the reflections. Both go to the
  polish pass (the picture's colour, metals' reflections).
- [x] **The kits' own normal maps** ([research](research/cs2-skins.md), "The paints' normal maps"): copied by the
  recipe onto the weapon's material (395 kits; 105 an occlusion too), they replace its normal and occlusion in the
  glTF (round 17: Printstream, Autotronic, Gungnir), except six flat stand-ins (Bloodsport's: not checked in CS2).
- [x] **Pearlescence** ([research](research/cs2-skins.md), "Pearlescence"): the share that turns (the composite's
  surface B, read per style) baked into the metallic-roughness texture's R, the kit's scale as
  `extras.ofast.pearlescence` (64 kits: Dopplers, Gungnir, Printstreams…), and CS2's hue turn by the view drawn by
  OFAST's copy of Interchange's glTF master (`tools/unreal/pearlescence.py`), the view taken from where a viewmodel
  really is (Unreal's first-person rendering moves its vertices: with its camera vector, half the turn). Round 17:
  10 pearlescent materials. Checked on woxic's Gungnir at tick 153201: the body's blue at hue 0.611 where CS2 shows
  0.633 (0.570 before); its engraving lines read teal where CS2's are lavender (polish pass, with the picture's
  colour). The kits' normal maps checked on the same view: the Gungnir's engraving shows.
- [x] **The second glove's seed** (`g_nRandomSeedAlt`): the pattern seed + 1, read from CS2's `client.dll` (both
  places that set it).
- [x] **Stickers on their places** (`sticker slot n schema`, [research](research/cs2-skins.md), "Which place"): a
  sticker sits on the model's markup slot its schema names, not on its own slot's. Checked from above the Island
  Floral AK lying in round 17 (tick 155002, CS2's free camera and `shoot_sequence.py -ViewShots`): CS2's three
  HellRaisers on the receiver and handguard, where OFAST had drawn two (slot 4 has no markup of its own on the AK).
- [ ] The free placement's signs: the AK's Sea Swallows move by a fifth of an inch from place 0, too little to tell;
  a sticker moved further, seen from the side, checked against a CS2 screenshot.
- Moved to the polish pass (2026-10-29): glitter (14 kits and the glitter stickers) and iridescence (18 kits), read
  from CS2's weapon shader ([research](research/cs2-skins.md), "Glitter") but drawn × the diffuse lighting, which
  Unreal's materials do not give; the default glove layout's anisotropic combo (one kit).
- [x] **Textures shared across the looks** (`import_demo.py`, `share_textures`): each exported PNG imported once into
  `/Game/OFAST/Replays/Textures`, which every round's looks sample, instead of a copy per look (round 17: 263
  textures, 574 MB, where its looks had made 3592 of 324 PNGs; 19 GB of looks for three rounds); a round's looks of
  an older export deleted. A drawn import ran out of video memory for another reason: each saved look's thumbnail
  keeps a preview scene with its own virtual shadow map pool (2.2 GB a look). With those shadows off while the looks
  import, round 17 imports drawn in 14 minutes, peaking at 10.8 GB of an 11 GB budget.

## Order from here

A first pass on each milestone left, M7 to M10, then a second pass through every milestone to fix, tweak and
polish (2026-10-26). That pass includes the effects rework and the installer:

- [ ] **The picture's colour**: Unreal's rounds are more saturated than CS2's (de_dust2's mid, 2026-10-29: red paint
  0.80 against CS2's 0.67 at equal brightness), and metals a little darker (gold stickers a tenth); indoors CS2 reads
  greyer still.
- [ ] **Bodies against CS2's**: Wicadia's (round 17, dead at tick 154410) lies on the AK he dropped in Unreal; CS2
  shows the AK alone there at tick 155002. The AK itself rests tilted a little differently.
- [ ] **Skins' finer shading**: glitter and iridescence in OFAST's weapon master (`pearlescence.py`'s copy of
  Interchange's glTF master), the default glove layout's anisotropic gloss, the Gungnir's engraving lines (teal in
  Unreal, lavender in CS2).
- [ ] **Installer**: the Studio as a setup program (`tauri build`, NSIS), made by CI on version tags. It carries
  OFAST's Unreal scripts (`tools/unreal`) and a template of its Unreal project (Python only: no C++ to compile), and
  makes the user's own project in a writable folder on first start (the Studio finds both in the source folder
  today). What users need then: Windows 10 or 11, CS2, Unreal Engine 5.8 and WebView2 (part of Windows 11; the
  setup program installs it when missing). No Node.js, Rust or Visual Studio.
- [x] **CS2 version check** (2026-10-29): the Studio shows the installed CS2 version, green when OFAST was checked
  on it (`game::CHECKED_VERSIONS`), orange ("not synced with this CS2 version yet, it might break") otherwise.
- [ ] **Same-day CS2 support**: each CS2 patch checked the day it ships (docs/development.md, "After a CS2 update"),
  its build added to the checked versions, or fixed, and an OFAST release out that day.

## Hard problems we know about

- **Player animation.** Demos contain no skeleton poses, but they do contain the server's pose recipe: the clips,
  times and blend weights its graph played (decision 020, M4.7). Plan: the replay rig re-deciding the graph from
  the demo's inputs (M4), the recipe decoded to replace those decisions, then exact live capture through
  ofast-capture (M8).
- **CS2 updates.** Each patch can change demo protobufs and game internals. The capture DLL reads field offsets
  from CS2's own schema system at runtime to limit breakage; skins are read from the game's own kits, so new ones
  need no update unless they bring a new shader feature. The Studio warns when CS2 is newer than OFAST's checks.
- **Skins and particles** have no open-source reference implementation; they are late milestones.
