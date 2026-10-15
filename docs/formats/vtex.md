# Textures (`.vtex_c`)

OFAST's code: `crates/ofast-core/src/texture/`.

## DATA block

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | version (1) |
| 2 | u16 | flags (16 cube, 32 volume, 64 array, …) |
| 4 | f32 ×4 | reflectivity |
| 20 | u16 ×3 | width, height, depth |
| 26 | u8 | format (see below) |
| 27 | u8 | mip count |
| 28 | u32 | picmip0 (unused) |
| 32 | u32 | extra-data table offset, **relative to this field** |
| 36 | u32 | extra-data record count |

Extra-data records are 12 bytes: `u32 type`, `u32 offset` (relative to its own field), `u32 size`.

| Type | Name | Payload |
|---|---|---|
| 1 | FALLBACK_BITS | — |
| 2 | SHEET | sprite-sheet sequences |
| 3 | METADATA | `u16 0`, `u16 display width`, `u16 display height`, … — crop the decoded image to this size |
| 4 | COMPRESSED_MIP_SIZE | `u32 method` (0 none, 1 LZ4, 2 ZSTD), `u32 offset` (self-relative), `u32 count`, then `i32` compressed size per mip (index 0 = largest) |
| 5 | CUBEMAP_RADIANCE_SH | spherical-harmonics coefficients |

## Pixel data

- Starts right after the `DATA` block (= the header's file size).
- **Mips are stored smallest first.** Mip 0 (full size) is last.
- Each mip is compressed on its own (raw LZ4 block, not the LZ4 frame format) when its compressed size is smaller
  than its raw size; otherwise stored raw.
- Mip size: block formats `ceil(w/4) × ceil(h/4) × depth × block bytes`; others `w × h × depth × pixel bytes`.
  Cube maps have 6 faces per slice (+X, −X, +Y, −Y, +Z, −Z).

## Formats

| # | Format | Decoding in OFAST |
|---|---|---|
| 1 | DXT1 (BC1) | RGBA8, alpha forced opaque |
| 2 | DXT5 (BC3) | RGBA8 (YCoCg converted back when the compiler says so) |
| 3 | I8 | grey |
| 4 | RGBA8888 | as is |
| 19 | BC6H | HDR — not yet |
| 20 | BC7 | RGBA8 — CS2 colour textures |
| 21 | ATI2N (BC5) | RG — CS2 normal maps and two-channel masks |
| 22 | IA88 | grey + alpha |
| 27 | ATI1N (BC4) | grey — single-channel masks (AO…) |
| 28 | BGRA8888 | swizzled to RGBA |
| 33 | R8 | red |

Other values (float formats, ETC2/EAC, JPEG/PNG/WebP-wrapped) are listed in the code and decode later as needed.

## Conversions recorded by the compiler

The `RED2` block's `m_SpecialDependencies` (compiler `CompileTexture`) records what was done to the image:

| `m_String` | Meaning |
|---|---|
| `Texture Compiler Version Image YCoCg Conversion` | DXT5 stores YCoCg colour |
| `Texture Compiler Version Image NormalizeNormals` | normals were normalized |
| `Texture Compiler Version Mip HemiOctIsoRoughness_RG_B` / `…HemiOctAnisoRoughness` | hemi-octahedral normal in RG, roughness in B |

Plain BC5 normal maps store X/Y only; Z is rebuilt as `sqrt(1 − x² − y²)`.

## AK-47 example (2026-10-15)

| Texture | Format | Size | Content |
|---|---|---|---|
| `ak47_default_color_psd` | BC7 | 4096² | colour in RGB; **alpha is a mask (≈0 almost everywhere), not opacity** |
| `ak47_default_normal_png` | ATI2N | 4096² | tangent-space normal XY |
| `ak47_default_rough_psd` | ATI2N | 4096² | roughness (R) and metalness (G) — used as `g_tMetalness` |
| `ak47_default_ao_png` | ATI1N | 4096² | ambient occlusion |

Channel meanings are confirmed from ValveResourceFormat's shaders: for `csgo_weapon`, `g_tMetalness` R = roughness
and G = metalness; other `csgo_*` shaders keep roughness in the normal map's B channel. See [material.md](material.md).

## Sources

- ValveResourceFormat (MIT): `Resource/ResourceTypes/Texture.cs`, `Resource/Enums/VTex*.cs`, `TextureDecoders/*`
- BC1–BC7 block decoding: `bcdec_rs` (MIT) — see [decision 002](../decisions/002-standard-codecs.md)
