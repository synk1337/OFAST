# VPK — Valve Pak archives

CS2 stores almost all of its content in VPK archives: one **directory file** `game/csgo/pak01_dir.vpk` (the table of
contents) plus hundreds of **data archives** `pak01_000.vpk`, `pak01_001.vpk`, … (the raw bytes). OFAST's reader is
`crates/ofast-core/src/vpk/`.

All integers are **little-endian**. Strings are **UTF-8, NUL-terminated**.

## File layout (version 2, used by CS2)

```
pak01_dir.vpk:  header (28 B) | directory tree | embedded file data | archive MD5 section | other MD5 section | signature section
pak01_NNN.vpk:  raw file bytes only (no header)
```

## Header

| Offset | Type | v1 (12 bytes) | v2 (28 bytes) |
|---|---|---|---|
| 0 | u32 | signature `0x55AA1234` (bytes `34 12 AA 55`) | same |
| 4 | u32 | version = 1 | version = 2 |
| 8 | u32 | tree size | tree size |
| 12 | u32 | — | embedded file data size |
| 16 | u32 | — | archive MD5 section size |
| 20 | u32 | — | other MD5 section size (always 48) |
| 24 | u32 | — | signature section size |

The **header size** (12 or 28) matters: the tree starts right after it, and embedded data starts at
`header size + tree size`. Version `0x00030002` is Respawn's variant (Titanfall/Apex) and is not supported.

Observed in CS2 (2026-10-15, before and after that day's update): version 2, tree ≈ 6.3 MB, embedded data 0,
other MD5 48, signature section 20, 506 data archives. Chunk hashes: 51,001 before the update, 50,987 after
(29,783 MD5 + 21,204 BLAKE3). Both builds pass every check in `crates/ofast-core/tests/cs2_vpk.rs`.

## Directory tree

Three nested loops. Each loop ends with an **empty string** (a single `0x00` byte):

```
for each extension      (e.g. "vmat_c")
  for each directory    (e.g. "materials/dev")
    for each file name  (e.g. "startup_background")
      entry record (18 bytes) + preload bytes
```

- A file with no extension, or a file in the root folder, uses a single space `" "` in that slot. When building the
  full path, a `" "` directory or extension is left out.
- Paths use `/`, with no leading or trailing slash.
- Full path = `directory + "/" + name + "." + extension`.

### Entry record

| Offset | Type | Field |
|---|---|---|
| +0 | u32 | CRC32 of the whole file |
| +4 | u16 | preload byte count |
| +6 | u16 | archive index |
| +8 | u32 | offset |
| +12 | u32 | length (bytes stored in the archive) |
| +16 | u16 | terminator, must be `0xFFFF` |

- `preload byte count` bytes follow the record immediately, inside the tree.
- File contents = **preload bytes, then `length` bytes from the archive**. Total size = preload + length.
- If `length` is 0, the preload bytes are the whole file.
- Archive index **`0x7FFF`** means "inside the directory file itself": absolute position =
  `header size + tree size + offset`. Any other index N means byte `offset` of `pak01_{N:03}.vpk`.
- The CRC is standard IEEE CRC-32 (zlib/PNG variant), computed over preload + archive bytes. A zero-length file has
  CRC 0.

## Archive naming

Open `X_dir.vpk` → the base is `X`; archive N is `X_{N:03}.vpk` (zero-padded to at least 3 digits). A VPK without the
`_dir` suffix is a single self-contained file where every entry uses index `0x7FFF`.

## Version 2 checksum sections

These make the format **self-verifying**, which is how OFAST tests its reader against real CS2 files without any
external tool.

### Archive MD5 section ("chunk hashes")

`size / 28` entries:

| Type | Field |
|---|---|
| u16 | archive index |
| u16 | hash type: 0 = MD5, 1 = BLAKE3 (first 16 bytes of the output) — CS2 uses both, confirmed on real files |
| u32 | offset (into that archive) |
| u32 | length |
| [u8; 16] | hash |

CS2 quirk: an entry with archive index 0 and hash-type field `0x8000` stands for the directory file's embedded data
(index `0x7FFF`), hashed with MD5. Its offset is relative to `header size + tree size`. Valve hashes archives in
1 MiB chunks.

### Other MD5 section (48 bytes)

| Bytes | Checksum of |
|---|---|
| 0–15 | **tree**: bytes `[header size, header size + tree size)` |
| 16–31 | **archive MD5 section** bytes |
| 32–47 | **whole file**: bytes from 0 up to (not including) this checksum — header, tree, embedded data, archive MD5 section and the two checksums above |

### Signature section

- **Legacy layout** (HL2/TF2): u32 key size, key (DER), u32 signature size, signature. RSA PKCS#1 v1.5 + SHA-256 over
  everything before the signature section.
- **CS2 layout**: the section is 20 bytes and starts with `0x55AA1234`: u32 magic, u32 type (1 = only the whole-file
  checksum is signed), u32 key size, u32 signature size, u32 reserved. The key and signature bytes come *after* those
  20 bytes. Unsigned files have key size = signature size = 0.

OFAST reads the signature header but does not verify RSA signatures (not needed to read content).

## Unknowns / not handled

- Lookup is case-sensitive. CS2 paths are lowercase in practice; OFAST normalizes `\` to `/` on lookup only.
- Files never span archives (nothing in references suggests otherwise).

## Sources

- ValvePak (MIT) — `ValvePak/ValvePak/Package.Read.cs`, `Package.Verify.cs`, `PackageEntry.cs`, `Misc/VPK.bt`:
  https://github.com/ValveResourceFormat/ValvePak
- Valve Developer Community, *VPK (file format)* (archived 2025-06-28 revision):
  https://developer.valvesoftware.com/wiki/VPK_(file_format)
- Header values checked against the local CS2 install on 2026-10-15.
