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
| 5 | CUBEMAP_RADIANCE_SH | 27 floats: 9 real spherical-harmonics coefficients per channel (planar R, G, B), Z up |

## Sprite sheets

The `SHEET` payload (`Texture::sheet`, read like ValveResourceFormat, MIT) lists animated sequences packed in the
texture. Offsets are relative to their own field, except the first, which counts from the payload's start:

| Part | Layout |
|---|---|
| header | `i32` sequence table offset (from the payload's start), `u32` sequence count |
| sequence (32 bytes) | `u32` id, four flag bytes (clamp, alpha crop, no colour, no alpha), `i32` frame table offset, `u32` frame count, `f32` total time (s), `i32` name offset (NUL-terminated UTF-8), `i32` float-parameter offset, `u32` count |
| float parameter | `i32` name offset, `f32` value |
| frame (12 bytes) | `f32` display time, `i32` image table offset, `u32` image count |
| image (32 bytes) | 8 `f32`, UV (0 – 1 of the texture's size): cropped min, cropped max, uncropped min, uncropped max; the corners sit inside the texels they cover, not on their edges (a quarter texel in on `fire_small_sim_b`) |

A frame's image is its cropped rectangle (the compiler trims the empty border); the uncropped rectangle is where the
full frame was. A rectangle whose max is not above its min is an empty frame. In pixels the end is the max texel + 1.
Seen: `materials/particle/fire_explosion_1` (4096 × 2048, DXT5) holds 2 sequences of 50 frames, `fire_small_sim_b`
(the molotov's flames) 4 of 32 – 33, `fire_gas_batch_a_v2` 7 of 16 – 19 whose first frames are empty; all clamped,
named `CDmeSheetSequence`, each frame's display time 1 (the particle system sets the playback rate).

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
| 19 | BC6H | HDR float (unsigned), via `Texture::decode_slices_f32` — CS2 sky cube maps |
| 20 | BC7 | RGBA8 — CS2 colour textures |
| 21 | ATI2N (BC5) | RG — CS2 normal maps and two-channel masks |
| 22 | IA88 | grey + alpha |
| 27 | ATI1N (BC4) | grey — single-channel masks (AO…) |
| 28 | BGRA8888 | swizzled to RGBA |
| 33 | R8 | red |

Other values (ETC2/EAC, JPEG/PNG/WebP-wrapped) are listed in the code and decode later as needed. RGBA16F and
RGBA32F decode to float as well.

## Cube maps

Flag 16 (`CUBE_TEXTURE`): each mip holds 6 faces in the order +X, −X, +Y, −Y, +Z, −Z, in the Direct3D layout. For a
texel at (`u`, `v`) of a face (`u` right, `v` down, both −1…1) the direction is:

| Face | Direction |
|---|---|
| +X | (1, −v, −u) |
| −X | (−1, −v, u) |
| +Y | (u, 1, v) |
| −Y | (u, −1, −v) |
| +Z | (u, −v, 1) |
| −Z | (−u, −v, −1) |

CS2 samples these with **Source world directions as is** (Z up): +Z is the zenith. Checked on `sky_de_mirage`:
zenith gradient on +Z, flat ground colour on −Z, and the horizon falls on the edges the table predicts for the side
faces (left edge of +X, right of −X, bottom of +Y, top of −Y). Code: `texture::sample_cube`. Preview tool:
`cargo run --release --example sky_faces -- <texture> <out.png>`.

`sky_de_mirage_exr_71e5f2a1.vtex_c`: 512² per face, BC6H, 8 mips; its reflectivity (0.742, 0.788, 0.844) is the
average radiance of the cube.

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

- ValveResourceFormat (MIT): `Resource/ResourceTypes/Texture.cs`, `Resource/Enums/VTex*.cs`, `TextureDecoders/*`,
  `Renderer/Shaders/texture_decode.frag.slang` (cube face directions)
- BC1–BC7 block decoding: `bcdec_rs` (MIT) — see [decision 002](../decisions/002-standard-codecs.md)
