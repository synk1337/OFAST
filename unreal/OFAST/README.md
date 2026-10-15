# OFAST for Unreal (planned — M2)

Unreal Engine 5.8 plugin. A thin C++ shell over `ofast-core` (called through a C interface) plus a little editor
Python. It imports OFAST scene packages:

- Maps, models, materials and lights (CS2 → UE master materials)
- Player / weapon / viewmodel replay rig
- Level Sequence with tracks, camera cuts and FOV
- Lumen lighting and Movie Render Graph presets

Built locally (Unreal is not available on hosted CI runners).
