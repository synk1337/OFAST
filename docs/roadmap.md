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
  from the map, local lights rebuilt as Unreal point/spot/rect lights, fixed exposure measured on CS2 screenshots.
  Outdoors 0.95 / 0.86 of CS2's brightness; interiors lit by wall lights still ~1.4× too bright.

### Still to do (M1 → M2)

- Interior local-light brightness (see the calibration notes in [lighting.md](formats/lighting.md)).
- CS2's bloom, colour LUT and cubemap fog in Unreal.
- Materials with more than two layers, detail textures, vertex colour tint (`COLOR` attribute), glass.
- Rebuild `csgo_effects` dust / light-shaft cards as Unreal effects.
- Colour texture alpha mask on weapons: find what it drives.
- Move the Unreal Python into the `ofast-unreal` plugin (M2).

## Hard problems we know about

- **Player animation.** Demos contain no skeleton poses; CS2 computes them while playing the demo. Plan: a "replay
  rig" in UE driven by demo data (M4), then exact live capture through ofast-capture (M8).
- **CS2 updates.** Each patch can change demo protobufs and game internals. The capture DLL reads field offsets
  from CS2's own schema system at runtime to limit breakage.
- **Skins and particles** have no open-source reference implementation; they are late milestones.
