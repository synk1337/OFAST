# 003 — One coordinate convention: Source (x, y, z) → Unreal (x, −y, z) × 2.54

- **Date:** 2026-10-15
- **Status:** accepted

## Context

Models, maps and demo positions must line up in Unreal exactly as in CS2, or nothing else (cameras, players,
grenades) can be placed correctly. The first AK-47 export arrived in Unreal with its barrel along Unreal's Y axis:
the glTF axis mapping had been copied from another exporter (`0.0254 × (y, z, x)`) without checking what Unreal's
importer does with it.

## What we measured

`tools/unreal/probe_axes.py` imports three boxes placed on glTF's +X, +Y and +Z axes with Interchange (UE 5.8.3).
They land at Unreal `(200, 0, 0)`, `(0, 0, 200)` and `(0, 200, 0)`: Unreal = glTF `(x, z, y) × 100`.

## Decision

- The target convention is the standard Source → Unreal one: **Unreal = (x, −y, z) × 2.54**, rotations
  `(pitch, yaw, roll)` → `(−pitch, −yaw, roll)`.
- OFAST therefore writes glTF as **`0.0254 × (x, z, −y)`**, a pure rotation (determinant +1), so triangle winding and
  tangent handedness stay valid in the glTF file.
- All conversions live in `crates/ofast-core/src/coords.rs`, with a test that replays the measured importer
  behaviour. Exporters never write their own axis math.

## Alternatives considered

- Keep VRF's `(y, z, x)` mapping and rotate the imported actors −90° in Unreal: every consumer would need to know
  about the extra rotation, and demo positions would need a different conversion than geometry.

## Consequences

- The AK-47 now lies along Unreal X (94.7 cm long). Map exports and demo data share one formula.
- If a future Unreal version changes the importer's axis convention, the probe will show it and only `coords.rs`
  changes.
