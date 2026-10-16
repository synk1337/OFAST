# 004 — Exported textures: each image once, lossless, at full size

- **Date:** 2026-10-16
- **Status:** accepted

## Context

de_inferno's export folder had grown to 13 GB (de_mirage: 433 MB), 12 GB of it PNG textures. Three causes:

1. The environment-shader bakes (`*_cc.png`, `*_surface.png`) were keyed by every material setting and every
   placement tint. Over 100 tints land on one plaster texture, most of them on layers that take no model tint, so
   the exporter wrote 1049 colour bakes (8.9 GB) of which 476 files were byte-identical to another.
2. Re-exports never removed files: 325 PNGs (1 GB) from older runs sat next to the current ones.
3. PNGs were written RGBA with the fastest zlib setting, even when alpha was 255 everywhere.

## Decision

- **Bake only what changes pixels.** A colour bake ignores surface settings, a tint the layer does not take, a
  texture tint without saturation, and a mask nothing reads; a colour nothing changes uses the plain texture
  (table in `docs/formats/material.md`).
- **Store each distinct image once.** The writer hashes every texture's pixels (BLAKE3) and gives equal images one
  file, image and glTF texture. Bakes are named after their pixel hash, so a file name always means the same pixels
  (Unreal imports each name once).
- **Re-exports clean up.** The exporter owns `<out>/textures`: PNGs no `.gltf` in `<out>` uses are deleted.
- **Lossless, full size.** RGB when alpha is unused, zlib's default level with adaptive filters. No downscaling and
  no lossy format.

Result on de_inferno: 5.4 GB (1467 files) instead of 13 GB, and a re-export over the old folder gives the same
files and a byte-identical glTF. Same picture in Unreal: the CT-spawn view measures 0.851 of CS2's luminance
(0.846 before), and the previous render differs by under one level on average (edges and Lumen noise). The export
takes ~45 s.

## Alternatives considered

- **Smaller bakes when the source is large (e.g. cap at 1024²).** Fragmovies live on close-ups, and CS2 draws these
  textures at full size. Unreal can cap sizes per texture group at import without losing the source.
- **JPEG for opaque colour textures (~5× smaller).** A second lossy step before Unreal's own BC compression, a new
  encoder dependency, and only for colour (normals, masks and alpha must stay lossless).
- **Lower bit depth.** The sources are BC1–BC7: 8 bits per channel is already their precision. Dropping an unused
  alpha channel is the only lossless reduction, and it is done.
- **Apply the model tint in Unreal instead of baking it.** Would leave ~270 colour textures (≈ 1.2 GB instead of
  3.3 GB), but needs a csgo_environment master material with the tint logic in `import_map.py`, and other glTF
  readers would lose the tints. Left as the next step if the size still matters — done in
  [005](005-model-tint-in-unreal-material.md) (126 bakes, 0.76 GB; baking stays available as an option).

## Consequences

- Exporting into a folder deletes unreferenced PNGs in its `textures` subfolder: it must not hold anything else.
- File names of bakes change whenever their pixels change; Unreal keeps the old assets (unused) until cleaned.
- Writing PNGs costs ~0.3–0.9 s per large texture instead of 20–70 ms, spread over all cores.
