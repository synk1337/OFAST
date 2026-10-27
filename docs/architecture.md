# Architecture

OFAST is four programs that share one Rust library.

```
ofast-core (Rust library — the heart, no UI)
 ├─ vpk / resource / kv3 / texture / mesh / material / map / anim readers
 ├─ demo parser  (protobufs generated from Valve .proto files)
 ├─ math: units, axes, FOV, tick interpolation
 └─ scene-package writer (scene.json + binary track files + glTF assets)

ofast-studio (Rust + Tauri 2, web UI)       ofast-capture (Rust DLL injected into CS2)
 ├─ finds CS2 install, builds asset cache    ├─ only runs if CS2 was started with -insecure
 ├─ demo browser: rounds, kills, 2D radar    ├─ field offsets read at runtime from CS2's schema system
 ├─ highlight picker → shot settings         ├─ D3D11 hooks: free cam, campath overlay, streams
 ├─ launches CS2 + capture for recording     └─ bone capture → high-fidelity animation mode
 ├─ "Send to Unreal"
 └─ "Render" (Movie Render Queue presets)

ofast-unreal (UE 5.8 plugin: thin C++ shell over ofast-core)
 ├─ imports scene package (maps, models, materials, lights)
 ├─ CS2 → UE master materials
 ├─ replay rig AnimBP (demo data → locomotion, aim, fire/reload/death)
 ├─ Level Sequence builder (tracks, camera cuts, FOV)
 └─ Lumen + Movie Render Queue presets
```

## Data flow: demo → Unreal

1. **Studio** finds the user's CS2 install and reads the game archives (`pak01_dir.vpk`) through `ofast-core`.
2. The user opens a demo; `ofast-core` parses it into rounds, kills and per-tick entity states.
3. The user picks a highlight. Studio writes a **scene package**: `scene.json` (what happens), binary track files
   (who is where, every tick) and glTF assets extracted from *their* CS2 install.
4. **ofast-unreal** imports the package and builds a Level Sequence ready to render.

## Why this split

- **One core, many front-ends.** All file-format knowledge lives in `ofast-core`, so the desktop app, the CLI, the
  capture DLL and the Unreal plugin never disagree about how a file is read or how a coordinate is converted.
- **The Unreal plugin stays thin.** UE requires C++, so it only calls `ofast-core` through a C interface. Logic
  stays in Rust where it is tested in CI.
- **The scene package is a plain folder.** It can be inspected by hand, diffed in tests, and re-imported without
  re-parsing the demo.

## Coordinate conventions (to be verified in M0)

| | Source 2 / CS2 | Unreal Engine |
|---|---|---|
| Handedness | right-handed, X forward, Y left, Z up | left-handed, X forward, Y right, Z up |
| Units | 1 unit ≈ 1 inch | 1 unit = 1 cm |
| Position | `(x, y, z)` | `(x, −y, z) × 2.54` |
| Angles | `(pitch, yaw, roll)` | `(−pitch, −yaw, roll)` |
| FOV | horizontal, measured at 4:3 | `HFOV = 2·atan(tan(fov/2)·aspect/(4/3))` → CineCamera focal length |

Each row is checked against the real game (screenshot at a known tick vs Unreal render) before it is relied on.
