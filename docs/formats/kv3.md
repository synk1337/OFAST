# KeyValues3 (KV3)

KV3 is Source 2's structured data format — think "JSON with types and flags". Materials, models, maps, particles,
physics and editor info are all KV3 inside compiled resources. OFAST's code: `crates/ofast-core/src/kv3/`.

The same tree of values has two encodings:

- **binary** — inside compiled resource blocks (`DATA`, `RED2`, `PHYS`, `CTRL`, `MDAT`, `INSG`, …),
- **text** — source files and debug output.

## Data model

| Kind | Notes |
|---|---|
| null | |
| bool | |
| int / uint | 8 to 64 bits on disk; OFAST keeps signed (`i64`) and unsigned (`u64`) apart |
| float | 32 or 64 bits on disk; OFAST stores `f64` |
| string | UTF-8 |
| blob | raw bytes |
| array | ordered values |
| object | ordered `key = value` members |

Any value can carry a **flag**: `resource`, `resource_name`, `panorama`, `soundevent`, `subclass`, `entity_name`,
`localize`. The most important one for OFAST is `resource:"materials/…/foo.vtex"`, a reference to another file.

## What CS2 actually uses

Full scan on 2026-10-15 (`OFAST_FULL_SCAN=1 cargo test --release --features game-data --test cs2_kv3`):
**every KV3 block of every compiled file in `pak01_dir.vpk` — 245,863 blocks across 26 file types — decoded with
every lane byte consumed, in 39 seconds.**

| Version | none | LZ4 | ZSTD |
|---|---|---|---|
| v3 | 69 | 36,577 | — |
| v4 | 1,963 | 115,405 | 17 |
| v5 | 340 | 91,465 | 27 |

No legacy `VKV\x03`, v1 or v2 blocks exist in CS2. v1/v2 are still implemented (tested with synthetic files);
legacy is not.

## Binary encoding

All integers little-endian. A KV3 block starts with a 4-byte magic:

| Magic (u32) | Bytes | Meaning |
|---|---|---|
| `0x03564B56` | `VKV\x03` | legacy encoding (not implemented — not seen in CS2) |
| `0x4B5633nn` | `nn 3 V K` | version `nn`, 1 to 5 |

### Header (after the magic)

GUIDs are stored in .NET `Guid` byte order (first three groups little-endian). The "generic" format GUID
`{7412167c-06e9-4698-aff2-e63eb59037e7}` is on disk as `7C 16 12 74 E9 06 98 46 AF F2 E6 3E B5 90 37 E7`.

| Offset | Type | Field | Versions |
|---|---|---|---|
| 4 | [u8; 16] | format GUID | all |
| 20 | u32 | compression: 0 none, 1 LZ4 (raw blocks), 2 ZSTD (frames) | all |
| 24 | i32 ×4 | v1 only: count 1-byte, count 4-byte, count 8-byte, uncompressed size (header ends at 40) | 1 |
| 24 | u16 | compression dictionary id (must be 0) | 2+ |
| 26 | u16 | LZ4 frame size for blob chunks (16384 with LZ4, else 0) | 2+ |
| 28 | i32 | 1-byte lane count | 2+ |
| 32 | i32 | 4-byte lane count | 2+ |
| 36 | i32 | 8-byte lane count | 2+ |
| 40 | i32 | type count (v2–v4: **includes the string table bytes**; v5: type bytes only) | 2+ |
| 44 | u16 ×2 | object / array counts — hints only (saturate) | 2+ |
| 48 | i32 | uncompressed size (v<5: main buffer, excluding blobs) | 2+ |
| 52 | i32 | compressed size | 2+ |
| 56 | i32 | blob count | 2+ |
| 60 | i32 | total blob bytes (uncompressed) | 2+ |
| 64 | i32 | 2-byte lane count | 4+ |
| 68 | i32 | "block compressed sizes" size — **0 in real CS2 v4 files even with LZ4 blobs; not used** | 4+ |
| 72 | i32 ×4 | buffer 1 uncompressed / compressed, buffer 2 uncompressed / compressed | 5 |
| 88 | i32 ×4 | buffer 2 lane counts: 1-byte, 2-byte, 4-byte, 8-byte | 5 |
| 104 | i32 ×4 | node count (hint), buffer 2 object count, array count (hint), array element count (hint) | 5 |

Header sizes: v1 40, v2–v3 64, v4 72, v5 120 bytes.

### Buffers after the header

| Version | Layout in the block |
|---|---|
| 1 | one buffer (rest of the block), compressed or not |
| 2–4, none | buffer (compressed size bytes) · raw blobs |
| 2–4, LZ4 | LZ4 buffer · LZ4 blob chunks |
| 2–4, ZSTD | two consecutive ZSTD frames (buffer, then blobs), covered by "compressed size" |
| 5 | buffer 1 (auxiliary) · buffer 2 (main) · blobs (raw, LZ4 chunks, or one ZSTD frame) |

With blobs, a `u32 0xFFEEDD00` follows the blob data.

### Lanes

Values are sorted into **lanes** by size, each aligned to its element size from the start of its buffer:

```
v1–v4 buffer:  [1-byte lane][pad 2][2-byte lane (v4)][pad 4][4-byte lane][pad 8][8-byte lane]
               [string table][type stream][blob lengths][0xFFEEDD00][LZ4 chunk sizes]

v5 buffer 1:   [1-byte lane = string table, then aux bytes][pad 2][2-byte][pad 4][4-byte][pad 8][8-byte]
v5 buffer 2:   [object member counts, i32 each][1-byte][pad 2][2-byte][pad 4][4-byte][pad 8][8-byte]
               [type stream][blob lengths][0xFFEEDD00][LZ4 chunk sizes]
```

- Versions 1–4 insert the padding even before an empty lane; version 5 skips padding before empty lanes.
- The **string count** is the first value of the 4-byte lane (v1–v4) or of buffer 1's 4-byte lane (v5).
- Strings are NUL-terminated UTF-8.
- v1: the type stream runs to the 4-byte end marker. v2–v4: type stream length = type count − string table bytes.
- **LZ4 chunk sizes** (u16 each) run to the end of the main buffer. Each blob is cut into chunks of at most "frame
  size" bytes; chunks are decoded as a chain (later chunks may copy from earlier output).

### Type stream

One byte per value, read in tree order (root first).

- v3+: bit `0x80` → the next type byte is a flag (`1` resource, `2` resource_name, `3` panorama, `4` soundevent,
  `5` subclass, `6` entity_name, `7` localize). Bit `0x40` → skip one extra byte (no known writer). Type = `& 0x3F`.
- v1–v2: bit `0x80` → the next byte is a flag bit field (`1` resource, `2` resource_name, `4` multiline — dropped,
  `8` panorama, `16` soundevent, `32` subclass); lowest bit wins. Type = `& 0x7F`.

| # | Type | Payload |
|---|---|---|
| 1 | null | — |
| 2 | bool | 1-byte lane |
| 3 / 4 / 5 | int64 / uint64 / double | 8-byte lane |
| 6 | string | i32 index (4-byte lane) into the string table; −1 = empty |
| 7 | blob | v1: i32 length (4-byte lane) + bytes (1-byte lane). v2+: next blob length + next blob bytes |
| 8 | array | i32 count (4-byte lane), then each element with its own type byte |
| 9 | object | member count (v5: object-count table; else 4-byte lane), then per member: i32 name index (4-byte lane) + value |
| 10 | typed array | i32 count (4-byte lane), one element type byte, then payloads only |
| 11 / 12 | int32 / uint32 | 4-byte lane |
| 13 / 14 | true / false | — |
| 15 / 16 | int64 0 / 1 | — |
| 17 / 18 | double 0.0 / 1.0 | — |
| 19 | float | 4-byte lane |
| 20 / 21 | int16 / uint16 | 2-byte lane |
| 22 / 23 | int8 / uint8 | 1-byte lane |
| 24 | typed array, short | u8 count (1-byte lane), element type, payloads |
| 25 | typed array, auxiliary | u8 count (1-byte lane), element type, payloads from **buffer 1** lanes (strings, blobs and container lengths still come from the main buffer) |
| 26 / 27 | uint32 0 / 1 | — |

## Text encoding

```text
<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->
{
	m_shaderName = "csgo_environment.vfx"
	m_vColor = [ 1.0, 0.5, 0.25 ]
	m_pTexture = resource:"materials/dev/grid_color.vtex"
	"key with spaces" = true
	m_data = #[ 01 02 FF ]
	m_text = """
multi-line
string
"""
}
```

- Unquoted keys: `[A-Za-z_][A-Za-z0-9_.]*`; anything else is quoted.
- Strings use `"…"` (or `'…'`), escapes `\n`, `\t`, `\"`, `\\`; `"""` for multi-line.
- Literals: `true`, `false`, `null`, `nan`, `inf`, `-inf`. Negative integers are signed, others unsigned; anything
  with a fraction or exponent is a float.
- Flags prefix a value: `resource:"…"`.
- Comments: `// line` and `/* block */`.

OFAST currently **writes** text (debug output); a text parser comes later.

## Unknowns

- Meaning of the v4+ header field at offset 68 (always 0 in sampled CS2 files).
- Meaning of the `0x40` type bit.
- Whether the LZ4 blob chain resets between blobs (OFAST passes the last 64 KiB of output as dictionary, which is
  correct either way).
- The legacy `VKV\x03` encoding (not seen in CS2).

## Sources

- ValveResourceFormat (MIT): `ValveResourceFormat/Resource/ResourceTypes/BinaryKV3*.cs`,
  `Serialization/KeyValues/KV3IDLookup.cs` — https://github.com/ValveResourceFormat/ValveResourceFormat
- ValveKeyValue (MIT): `KVFlag.cs`, `KVValueType.cs`, `Deserialization/KeyValues3/*` —
  https://github.com/ValveResourceFormat/ValveKeyValue
- Verified against real CS2 files on 2026-10-15 (see "What CS2 actually uses"). The offset-68 finding came from
  dumping real headers with `cargo run --example kv3_inspect`.
