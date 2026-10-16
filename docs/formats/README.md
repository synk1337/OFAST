# CS2 file formats

One page per format OFAST reads. Each page explains the byte layout in plain terms, lists what is still unknown,
and links the sources the knowledge came from. The code in `crates/ofast-core` follows these pages, and the
synthetic test fixtures are built straight from them.

| Format | Files | Page | Status |
|---|---|---|---|
| VPK archive | `pak01_dir.vpk`, `pak01_000.vpk`… | [vpk.md](vpk.md) | reader done, verified on CS2 |
| KeyValues3 | text `.vmat`, `.vpcf`…; binary inside compiled resources | [kv3.md](kv3.md) | binary decoder done, verified on all CS2 files; text writer done |
| Resource container | every `*_c` file (`.vmat_c`, `.vmdl_c`, `.vtex_c`…) | [resource.md](resource.md) | block splitter done |
| Model + mesh | `.vmdl_c` (`CTRL`, `MDAT`, `MVTX`, `MIDX`, older `VBIB`/`MBUF`), meshopt buffers | [model.md](model.md) | reader done, all 13,105 CS2 models decode; glTF export |
| Map | `world.vwrld_c`, `*.vwnod_c`, `*.vents_c` in `maps/<map>.vpk` | [map.md](map.md) | reader + glTF export, verified on de_mirage against CS2 |
| Material | `.vmat_c` (shader channel conventions, layer blending) | [material.md](material.md) | glTF + Unreal layer-blend material |
| Texture | `.vtex_c` | [vtex.md](vtex.md) | header, mips, LZ4, BC1/3/4/5/7 + uncompressed → RGBA8; BC6H/float → HDR; cube maps |
| Lighting | `light_*`, `env_sky`, `post_processing_volume`, `.vpost`, `env_cubemap_fog` | [lighting.md](lighting.md) | units, sky, exposure → Unreal; calibrated on de_mirage against CS2 screenshots |
| Demo | `.dem` | [demo.md](demo.md) | header, entities, player positions + eye angles, rounds; matches demoparser2 bit for bit on a real match |
