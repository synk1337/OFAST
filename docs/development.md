# Development guide

## Prerequisites (Windows)

| Tool | Why | Check |
|---|---|---|
| Rust (stable, MSVC) | everything in `crates/` and `apps/` | `cargo --version` |
| Visual Studio Build Tools (C++ workload) | Rust linker, Unreal plugin | `vswhere -all` |
| Node.js | Tauri front-end (`apps/studio`, from M3) | `node --version` |
| Unreal Engine 5.8 | `unreal/OFAST` plugin (from M2) | Epic Games Launcher |
| Counter-Strike 2 | local test data and ground truth | Steam |

## Everyday commands

```bash
cargo fmt --all                     # format
cargo clippy --all-targets          # lint
cargo test                          # unit tests on synthetic fixtures (what CI runs)
cargo test --features game-data     # extra tests that read YOUR local CS2 install
cargo doc --no-deps --open          # browse the API docs
```

`game-data` tests find CS2 automatically through Steam's default path. Override with the `OFAST_CS2_DIR` environment
variable (the folder that contains `game\csgo\pak01_dir.vpk`). Add `--release` when running them: they read whole
VPK archives and 200 MB demos.

| Variable | Used by | Meaning |
|---|---|---|
| `OFAST_CS2_DIR` | all `game-data` tests | CS2 root folder, if not in Steam's default library |
| `OFAST_TEST_DEMO` | `tests/cs2_demo.rs` | demo to test with; default: the newest `.dem` in `game\csgo\replays` (download a match in CS2's Watch tab to get one) |
| `OFAST_ORACLE_PYTHON` | `tests/cs2_demo.rs` | Python with demoparser2 installed (see below); default: `python` on the PATH; skipped if neither has it |

## Test data rules

- **Never commit Valve content** — no `.vpk`, `_c` resources, demos or exports. `.gitignore` blocks the common ones.
- Local-only files (reference demos, oracle outputs, screenshots) go in `test-data/` at the repo root, which is ignored.
- CI tests use **synthetic fixtures**: small files our own tests build in memory, byte by byte, following the
  documented format. This also proves the docs in `docs/formats/` are complete.

## Oracles

Each reader is checked against a reference tool on the same input. Differences must be explained (in the format doc)
or fixed.

| Reader | Oracle |
|---|---|
| VPK | ValvePak / Source 2 Viewer CLI file listing |
| KV3 / resources | Source 2 Viewer CLI decompiled text |
| Demos | demoparser2 per-tick output, through `tools/oracle/demoparser2_oracle.py` |

Setting up the demo oracle once (in any folder outside the repository — it is a test tool, not a dependency;
on Linux/macOS the interpreter is `oracle-venv/bin/python`):

```bash
python -m venv oracle-venv
oracle-venv/Scripts/python -m pip install demoparser2 pandas
```

Then set `OFAST_ORACLE_PYTHON` to `oracle-venv/Scripts/python` (full path) and run
`cargo test --release --features game-data --test cs2_demo -- --nocapture`. The test prints how many rows matched
and the largest differences.

Developer tools for poking at demos: `cargo run --release --example demo_inspect -- <demo> [rounds | fields <Class>…]`
and `cargo run --release --example demo_round -- <demo> <round> > round.csv`.

### A demo round in Unreal

```bash
cargo run --release -p ofast-cli -- demo-rounds <demo.dem>
cargo run --release -p ofast-cli -- export-demo <demo.dem> --round 2 --out test-data/exports/demos
powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_demo.py -Replay test-data/exports/demos/<demo>_r2.ofast-replay.json -Shot test-data/shots
```

The map must be imported first (below). `import_demo.py` builds `/Game/OFAST/Replays/<demo>_r<round>` in the map's
level: player proxies in team colours, a first-person camera per player, camera cuts on each killer, kill markers —
see [replay.md](formats/replay.md). `-Shot` adds verification screenshots (killer's view before the first kills).

Developer tools for game files: `cargo run --release --example resource_dump -- find <text>` lists archive paths,
`cargo run --release --example resource_dump -- <path>_c` dumps a compiled file's blocks (KV3 as text), and
`cargo run --release --example kv3_inspect -- <path>_c` prints raw KV3 headers.

## Exporting and checking in Unreal

```bash
cargo run --release -p ofast-cli -- export-model weapons/models/ak47/weapon_rif_ak47.vmdl_c --out test-data/exports/ak47
powershell -File tools/unreal/run_shot.ps1 -Gltf test-data/exports/ak47/weapon_rif_ak47.gltf -Shot test-data/shots/ak47.png -Name ak47
```

`run_shot.ps1` opens the sandbox project `unreal/Sandbox/OFASTSandbox.uproject` in UE 5.8, imports the glTF through
Interchange, builds a small level (sky, sun, floor, camera) and writes a 1920×1080 screenshot, then closes the editor
(`tools/unreal/import_and_shot.py`). The first run on a machine compiles shaders and is slow; later runs take under a
minute. Imported content in `unreal/Sandbox/Content/` is ignored by git (it comes from Valve files).

### Whole maps

```bash
cargo run --release -p ofast-cli -- export-map de_mirage --out test-data/exports/de_mirage
powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_map.py -Gltf test-data/exports/de_mirage/de_mirage.gltf -Shot test-data/shots -Name de_mirage [-Views tools/compare/views/de_mirage.json]
```

`export-map` also loads the map's 3D skybox (its own archive under `game/csgo/maps/prefabs/`) and writes the sky as
`<map>_sky.hdr`. Re-exporting into the same folder is the normal way to update a map: the exporter owns
`<out>/textures/`, writes each distinct image once and deletes the PNGs no `.gltf` in `<out>` uses any more (see
"Texture files" in `crates/ofast-core/src/export/gltf.rs`; de_inferno went from 13 GB to 3.3 GB). Placement tints that
recolour only part of a surface (environment materials, props with a tint mask) are applied by Unreal's materials, not
baked; `--bake-model-tints` bakes them into the colour textures for other glTF readers such as Blender
([material.md](formats/material.md#model-tint)). `import_map.py` imports the glTF as a scene (one actor per placement), swaps two-layer materials for
the layer-blend and height-blend materials, environment materials for OFAST's environment material and tinted props with
a tint mask for OFAST's tint-mask material ([material.md](formats/material.md)), hides CS2's light blockers and adds invisible shadow copies of single-sided world geometry
(so sunlight cannot leak through walls; see [lighting.md](formats/lighting.md#shadows-from-the-back-of-walls)), replaces the template's sky, fog and lights with CS2's (sun, sky sphere + Sky Light from
the HDR, local lights from the manifest, fixed exposure — see [lighting.md](formats/lighting.md)), saves everything
under `/Game/OFAST/<map>` and writes screenshots from the first CT spawn, the first T spawn, the optional `-Views`
spots (CS2 `setpos`/`setang` values in a JSON list) and above the map. It never saves an engine level.

Re-running `import_map.py` after a re-export always gives what a first import of the current glTF gives: before the
scene import it deletes the Interchange assets under `/Game/OFAST/<map>/<map>` that came from another version of the
glTF (material instances), from a PNG that changed or is no longer used (textures), and the import record
`SceneImport_<map>`, and Interchange makes them again. Interchange's own re-import keeps existing material instances
as they were (de_inferno kept 739 of 922 from older exports); see
[decision 006](decisions/006-reimport-rebuilds-stale-interchange-assets.md). Without a re-export nothing is rebuilt
(1–4 s to check; de_mirage's scene import 9–11 s). Assets edited by hand in that folder are not kept. OFAST's own
instances (`/Game/OFAST/<map>/Blend` and `Skybox`) are rewritten where they differ, their textures are imported again
when the PNG changed, and what the current glTF no longer uses is deleted at the end of the run.
`tools/unreal/probe_materials.py` checks the result (table below).

`-Style realistic` lights the same map physically instead (Unreal sky, 100 000 lux sun, real-world candela, auto
exposure, filmic tone curve) in its own level `<map>_realistic`; see the styles table in
[lighting.md](formats/lighting.md#import-styles).

The sandbox needs **DirectX 12 + Shader Model 6** for Nanite (set in `unreal/Sandbox/Config/DefaultEngine.ini`);
switching it on recompiles all shaders once (long).

### Comparing with CS2

1. Start CS2 with `-insecure` (`steam.exe -applaunch 730 -insecure -novid -console +map de_mirage`), open the console
   (check which key is bound to `toggleconsole` in `Steam/userdata/<id>/730/local/cfg/cs2_user_keys_0_slot0.vcfg`).
   When driving it by script: the console takes Enter only as a held key, and the console window closes with its X.
2. `sv_cheats 1; bot_kick; mp_warmup_end; mp_ignore_round_win_conditions 1; noclip; cl_drawhud 0;
   r_drawviewmodel 0`, then `setpos <origin_source> ; setang <angles>` from the manifest's spawn and `getpos` to check
   (the eye is 64 units above the origin). Keyboard movement does not reach the game by script: move with `setpos`.
   Faster: copy `tools/cs2/cfg/*.cfg` (the setup line, and each spawn view as `ofast_v_<map>_<team>.cfg`) into
   `game/csgo/cfg/` and `exec` them. `mp_warmup_end` restarts the round a few seconds later and respawns the player at a random spawn,
   and the view drifts once the console closes: bind a key to the view (`bind F8 "exec ofast_v_…"`, `unbind F8`
   afterwards), press it right before the screenshot, and check `getpos`. After `map` from the main menu the player
   has no team: `jointeam 3; mp_restartgame 1`.
3. Screenshot (CS2's own captures repeat to within 0.1 %). CS2 has no `jpeg`/`screenshot` console command; Steam's
   F12 overlay capture saves the full game frame to `Steam/userdata/<id>/760/remote/730/screenshots/`, unaffected
   by windows over the game (a desktop screenshot of the same frame agreed within 3 %). Lock the exposure for
   comparisons (`mat_tonemap_force_scale`, see [lighting.md](formats/lighting.md)). Views can be several keys at once
   (`bind F6 "exec ofast_v_<map>_<view>"`, …; restore the user's binds afterwards). Then compare:
   `python tools/compare/compare_shots.py <cs2.jpg> <unreal.png> <out.png>` (CS2 on top, Unreal below, per-cell
   brightness ratios) and, for exposure, `python tools/compare/sky_exposure.py <map>_sky.hdr <shot> <yaw> <pitch>
   x,y …` on pure-sky pixels.

Many views at once ([checks.md](checks.md)): `python tools/cs2/make_view_cfgs.py <map> <exposure> [views]` writes
`ofast_v_<map>_<view>.cfg` for every view of `tools/compare/views/<map>.json` and a tour, `ofast_tour_<map>.cfg`, that
locks the exposure and binds **F8** to the next view and **F9** to the current one. In CS2: `exec ofast_setup`,
`exec ofast_tour_<map>`, close the console, then per view F8, a moment for the picture to settle, F9, Steam's F12
(the console echoes `OFAST view n/N <name>`); `unbind F8; unbind F9` at the end. A changed map keeps `noclip` on
after the round restart, so check its state (`noclip` prints it) instead of running the setup twice. Rename the
captures to `cs2_<map>_<view>.jpg` in capture order and compare them all with
`python tools/compare/compare_views.py --cs2-gamma <r_fullscreen_gamma> <cs2 dir> <unreal dir> <out dir> <map>…`.
`--cs2-gamma` undoes CS2's brightness setting (`r_fullscreen_gamma` in Steam's
`userdata/<id>/730/local/cfg/cs2_machine_convars.vcfg`, default 2.2; this machine's is 2.013074), which CS2 applies
to the whole picture as shown ^ (gamma / 2.2): references taken at 2.013 show CS2's darks about 13 % too bright.

### Debug tools for maps

| Tool | What it answers |
|---|---|
| `cargo run --release --example map_raycast -- de_mirage ct0 "x,y;x,y"` (or a view `"x,y,z,pitch,yaw"` from CS2's setpos/setang) | which model/material OFAST's geometry has under a screenshot pixel, and where (Source units). A view placed at that point, with z − 64 and the angles toward the sun, shows what shades it. Looking straight down (pitch 89.99) from a guessed spot gives the floor height for a new view's `setpos`; `map_entities -- <map> env_cs_place point_devshot_camera` gives the map's named places and Valve's own screenshot cameras to start from |
| `tools/unreal/shoot_views.py` (`-Name <map>` or `<map>_realistic`, `-Only ct0,palace`, `-Views tools/compare/views/<map>.json`, `-Try what-if.json`) | re-shoots the import's saved cameras without importing again (~1 min), plus cameras for a `-Views` file (added for that run only, so new viewpoints can be tried without an import), optionally with what-if settings (not saved; pass them as a .json file — PowerShell strips the quotes of inline JSON): light properties, `sun_rotation`, `meshes` (component properties), `blockers` (light blockers only), `proxies` (shadow proxies only; in these three groups `"visible"` and `"material"` work too), `post`, `console` (commands such as `ShowFlag.Lighting 0` for a base-colour render to compare with CS2's `mat_fullbright 1`) — see the script's docstring. A changed `cast_shadow` alone did not reach the renderer in one test; change `visible` with it |
| `run_ue.ps1 … import_map.py … -Disable sun,sky,local,bounce` | diagnostic renders without some light sources, to see what lights a spot (the level keeps that lighting until the next normal import) |
| `cargo run --release --example map_outliers -- de_mirage [classes]` | stretched geometry, dropped triangles, entity classes with models |
| `cargo run --release --example model_layout -- formats de_mirage` | vertex attribute formats used by a map |
| `cargo run --release --example map_entities -- de_mirage [classes…]` | entity classes with counts, or every key of the given classes |
| `cargo run --release --example map_blockers -- de_mirage [x,y,z radius]` | the map's light blockers (invisible "block light" meshes), optionally near a point |
| `cargo run --release --example sky_faces -- <cube .vtex_c> <out.png>` | the six faces of a cube map (sky), tone-mapped |
| `cargo run --release --example texture_png -- <.vtex_c> <out.png>` | a 2D texture's four channels as stored, and its stored average colour |
| `cargo run --release --example shader_census [maps…]` | which material shaders every map uses (all playable maps by default, ~20 s): what OFAST must handle to import any official map |
| `cargo run --release --example drawcall_tints -- de_nuke [maps…]` | which draw calls carry their own colour or opacity (Hammer's painted colour, e.g. de_nuke's yellow road paint), per placement kind, with each material's shader and tint as linear floats and sRGB bytes ([material.md](formats/material.md#draw-call-tint)) |
| `tools/unreal/probe_vertex_color.py` (`-Shot <png>`) | whether Unreal hands glTF vertex colours to materials linear or sRGB-encoded (linear) |
| `cargo run --release --example vpk_list -- <archive.vpk> [text]` | files inside any VPK (map, skybox) |
| `tools/unreal/probe_actors.py` (`OFAST_FILTER=<text>`, `@empty` or `@lights` + `OFAST_NEAR=x,y,z`) | imported actors, their meshes and materials, or lights (direction, units, radius) in Unreal |
| `tools/unreal/probe_materials.py` (`-Gltf test-data/exports -Name de_mirage,de_inferno`) | whether the Interchange material instances of imported maps match their current glTF (textures by image name, colour factors), textures no material uses any more, and which folder the mesh slots' materials come from (~20 s for five maps) |
| `tools/unreal/probe_axes.py` | how Unreal's glTF importer maps axes (see decision 003) |

## Rules we work by

1. **Comment and document a lot.** Every crate has `missing_docs` on. Every module opens with a doc comment that
   explains the format or system and where the knowledge came from. New formats get a page in `docs/formats/`.
2. **CI/CD.** Every push runs fmt, clippy (warnings are errors), tests and docs. Tagged versions publish releases.
3. **Screenshots check results.** Anything visual is verified with a screenshot: Unreal `HighResShot`, CS2
   `screenshot`, the Studio UI, and overlay/diff images from `tools/compare`.
4. **CS2 in `-insecure` for testing.** Offline and local demo playback only, never online. Used for ground truth
   (`getpos`, `cl_showpos`, `demo_gototick`) and, later, live dumps through ofast-capture.
5. **Revise at dead ends.** Spikes are time-boxed. A dead end gets a decision-log entry with the alternatives, and big
   pivots are agreed before they happen.
6. **Use agents.** Reference sources are researched in parallel before a reader is written; independent crates can be
   built in parallel; code is reviewed before merging.

## Decision log

Architecture choices and dead ends are recorded in [docs/decisions/](decisions/). Copy
[decisions/000-template.md](decisions/000-template.md) to start a new one.
