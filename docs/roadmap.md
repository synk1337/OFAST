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
| M9 | Render presets | Movie Render Graph, depth/matte passes for compositing. |
| M10 | Skins (stretch) | Default finishes, then paint kits / wear / seed / stickers. |

## M0 progress

- [x] Repository, license, docs skeleton, decision log
- [x] CI workflow
- [x] VPK reader (synthetic tests in CI, oracle tests on the local CS2 install — MD5 + BLAKE3 chunks, CRCs)
- [x] Binary KV3 decoder v1–v5, none/LZ4/ZSTD — all 245,863 KV3 blocks in CS2 decode exactly
- [x] KV3 text writer
- [ ] KV3 text parser (needed to diff against other tools' decompiled output)
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
- [ ] Several rounds / a whole match in one sequence; weapons in hand; grenades (M6).
- [ ] Studio demo browser (Tauri app).

## M4 progress

Design: [decision 013](decisions/013-replay-rig-baked-in-rust.md) — CS2's agents as skeletal meshes, each player's
animation computed in Rust from the demo (CS2's own clips, blended per tick) and imported as one AnimSequence per
player and round.

- [x] **M4.1 Skinned agents**: skeleton (`m_modelSkeleton`), bone remapping, four weights per vertex → glTF skin;
  `ctm_sas` imports into Unreal as a skeletal mesh in CS2's bind pose (94 bones, pelvis and head at CS2's heights).
  [model.md](formats/model.md#skeleton-and-skinning).
- [ ] M4.2 Clip decoder (`.vnmclip`: per-track quantised poses, 30 fps) and a decoded clip playing in Unreal as in CS2.
- [ ] M4.3 Rig inputs in the replay export: velocity, duck amount, ground, walking, body yaw, active weapon and its
  gameplay animation state, reload, jump / land ticks, defusing.
- [ ] M4.4 Baked animation: locomotion (2D blend on local velocity × crouch), weapon idle / shoot / reload / deploy
  layers, procedural aim on the spine; checked against CS2's demo playback from the side.
- [ ] M4.5 Weapons in hand (weapon skeletal mesh on `weapon_hand_R`), each player's agent, knife and gloves from the
  demo's model hashes.
- [ ] M4.6 Deaths: ragdolls in Unreal from the demo's ragdoll force and bone.
- [ ] Character materials (CS2's character shader: the agents render too glossy).

## Hard problems we know about

- **Player animation.** Demos contain no skeleton poses; CS2 computes them while playing the demo. Plan: a "replay
  rig" in UE driven by demo data (M4), then exact live capture through ofast-capture (M8).
- **CS2 updates.** Each patch can change demo protobufs and game internals. The capture DLL reads field offsets
  from CS2's own schema system at runtime to limit breakage.
- **Skins and particles** have no open-source reference implementation; they are late milestones.
