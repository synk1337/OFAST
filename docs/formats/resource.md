# Resource container (compiled `*_c` files)

Every compiled Source 2 file — `.vmat_c` (material), `.vmdl_c` (model), `.vtex_c` (texture), `.vmap_c` / `.vwrld_c`
(map), `.vpcf_c` (particles), `.vsnd_c` (sound), … — uses the same outer container: a header and a table of typed
**blocks**. OFAST's code: `crates/ofast-core/src/resource/`.

All integers little-endian.

## Header (16 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | file size (texture pixel data may follow it) |
| 4 | u16 | header version — always 12 |
| 6 | u16 | resource version (type-specific) |
| 8 | u32 | block table offset, **relative to this field** (normally 8 → table at 16) |
| 12 | u32 | block count |

## Block table (12 bytes per block)

| Offset | Type | Field |
|---|---|---|
| +0 | [u8; 4] | type code, ASCII (`DATA`, `RED2`, …) |
| +4 | u32 | data offset, **relative to this field** |
| +8 | u32 | data size |

## Block types

| Code | Content | Encoding |
|---|---|---|
| `RED2` | editor info: input files, dependencies, searchable data | KV3 |
| `REDI` | older editor info | custom binary |
| `RERL` | external references (ids + paths of other resources) | custom binary |
| `NTRO` | introspection manifest; its presence means `DATA` uses the old NTRO encoding | custom binary |
| `DATA` | the content — KV3 for most types; custom binary for textures, sounds, Panorama | mixed |
| `MDAT`, `CTRL`, `INSG`, `PHYS`, `ANIM`, `ASEQ`, `AGRP`, `MRPH`, `SrMa`, `LaCo`, `STAT`, `FLCI`, `DSTF` | type-specific data | KV3 (except NTRO files) |
| `MBUF`, `VBIB`, `MVTX`, `MIDX`, `MADJ`, `MSLT`, `TBUF`, `VXVS`, `SNAP` | mesh / vertex / index buffers and similar | custom binary |

A KV3 block is recognized by its magic (see [kv3.md](kv3.md)). Real CS2 files checked on 2026-10-15 carry `RED2`
and KV3 `DATA`; no `NTRO` was needed for the sampled types.

## Sources

- ValveResourceFormat (MIT): `ValveResourceFormat/Resource/Resource.cs`, `Resource/Enums/BlockType.cs`,
  `Resource/Blocks/*` — https://github.com/ValveResourceFormat/ValveResourceFormat
