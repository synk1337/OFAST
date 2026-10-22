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

## External references (`RERL`)

The resources a file uses, each by id and path (`Resource::external_refs`):

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | entries offset, **relative to this field** (8: right after the header) |
| 4 | u32 | entry count |
| entry +0 | u64 | the referenced resource's id |
| entry +8 | u32 | offset of its NUL-terminated path, **relative to this field** |
| entry +12 | u32 | unused (0) |

Paths are lowercase, with forward slashes and without the `_c` suffix
(`materials/models/weapons/v_models/rif_ak47/ak47.vmat`).

**Resource id.** The id is MurmurHash64B (Austin Appleby's 64-bit MurmurHash2 for 32-bit machines: two 32-bit
halves fed alternate words, `m = 0x5bd1e995`, `r = 24`) of the path's bytes, seed `0xEDABCDEF`
(`resource::resource_id`). Entities send these ids instead of paths: a demo's `m_hModel` is the id of the entity's
`.vmdl` ([demo.md](demo.md#models)). Checked: all 4,320 references of the resources sampled per type in `pak01`
match (`resource_ids_match_references` in `tests/cs2_kv3.rs`); unit tests pin four ids of the AK-47's references,
covering even and odd word counts and a 2-byte tail.

## Sources

- ValveResourceFormat (MIT): `ValveResourceFormat/Resource/Resource.cs`, `Resource/Enums/BlockType.cs`,
  `Resource/Blocks/*` — https://github.com/ValveResourceFormat/ValveResourceFormat
