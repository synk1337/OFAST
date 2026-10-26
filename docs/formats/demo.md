# CS2 demo (`.dem`)

A CS2 demo is a recording of everything the server sent to a spectator (SourceTV): every entity's networked
fields, tick by tick, plus events, string tables and the class descriptions needed to decode them. Matchmaking and
FACEIT demos, and the ones CS2 downloads to `game/csgo/replays`, are all this format. OFAST's code:
`crates/ofast-core/src/demo/`.

Reading a demo is a stack of layers. Each section below is one layer, outermost first.

| Layer | Section | Code |
|---|---|---|
| file header + frames | [Frames](#frames) | `frame.rs` |
| protobuf messages | [Protobufs](#protobufs) | `proto.rs`, `proto/*.proto`, `build.rs` |
| network messages in packets | [Packets](#packets) | `parser.rs` |
| bit-level encodings | [Bit encodings](#bit-encodings) | `bits.rs` |
| string tables | [String tables](#string-tables) | `stringtable.rs` |
| class descriptions | [Send tables](#send-tables-serializers) | `serializer.rs` |
| field values | [Field decoders](#field-decoders) | `decoder.rs` |
| which fields changed | [Field paths](#field-paths) | `fieldpath.rs` |
| entities | [Entities](#entities) | `entity.rs` |
| game events (kills, rounds, bomb) | [Game events](#game-events) | `events.rs` |
| players and rounds | [Players and rounds](#players-and-rounds) | `players.rs` |

The Unreal side reads one round at a time from a replay file (`ofast export-demo`): [replay.md](replay.md).

## What CS2 actually uses

Checked on 2026-10-15 against a 216 MB matchmaking demo (de_nuke, 20 rounds, build 10847, patch 14173) with
`cargo test --release --features game-data --test cs2_demo`:

- **all 129,530 frames parse**, through to the final tick 129,518, in about 3.5 s;
- **round 1 matches demoparser2 bit for bit**: 50,740 of 50,740 player × tick rows have identical x, y, z,
  pitch and yaw;
- **all 20 round start ticks match** demoparser2's `round_start` events.

| Fact | Value in that demo |
|---|---|
| tick interval | 1/64 s (`svc_ServerInfo.tick_interval`) |
| server classes | 218 → class ids are 8 bits |
| send tables | 263 serializers, 1,573 fields, 1,915 symbols |
| player position | `m_skeletonInstance.m_vecOrigin.m_cellX/Y/Z` (`uint16`) + `m_vecX/Y/Z` (`CNetworkedQuantizedFloat`, 32 bits = plain float) on the pawn's `CBodyComponent` |
| view offset (feet → camera) | `m_vecViewOffset.m_vecX/Y/Z` on the pawn, `CNetworkedQuantizedFloat`: x and y 10 bits in −64..64 with encode-zero; z 12 bits, no low value, high 64, round-up → 12 plain bits, value = (raw + 1) / 64 |
| eye angles | `m_angEyeAngles`, `QAngle` with encoder `qangle_precise` |
| Steam id | `m_steamID`, `uint64` with encoder `fixed64` |
| full entity snapshot | only the first `DEM_FullPacket` is needed; later ones repeat what deltas built |

### Checked in the game

demoparser2 can only tell us that two programs read the bits the same way. To check what the bits mean, the same
demo was played in CS2 itself on 2026-10-15 (`-insecure`, offline, local demo only): `playdemo`, `demo_pause`,
`demo_gototick <tick>`, `spec_player <name>` in first person, then `getpos` (camera position and angles, 6
decimals) and `cl_showpos 1` (on-screen, 2 decimals). Six readings in round 1:

| Tick | Player | Stance | CS2 camera x, y, z | OFAST x, y, z (feet) | Eye height² | CS2 pitch, yaw | OFAST pitch, yaw |
|---|---|---|---|---|---|---|---|
| 300 | Synk | standing (freeze time) | -1832.000000, -1160.000000, -352.000000 | -1832, -1160, -415.96875 | 63.96875 | 0.000000, 39.999847 | 0, 39.999847 |
| 3090 | vg | standing | 449.045685, -1334.654053, -352.000000 | 449.0457, -1334.654, -415.96875 | 63.96875 | -0.316895, 161.706390 | -0.31689453, 161.70639 |
| 4074 | Wajam | standing | 396.895844, -1471.968750, -352.000000 | 396.89584, -1471.9688, -415.96875 | 63.96875 | 3.352203, 89.47¹ | 3.3522034, 89.47128 |
| 4195 | Wajam | **running**, ~3 units per tick | 343.548157, -1313.562378, -352.000000 | 343.54816, -1313.5624, -415.96875 | 63.96875 | 2.408066, -37.04¹ | 2.4080658, -37.039032 |
| 4195 | waltey | crouched, C4 in hand | 616.298645, -901.900574, -378.000000 | 616.29865, -901.9006, -415.96875 | 37.96875 | -12.898636, 55.44¹ | -12.898636, 55.440796 |
| 4520 | XANTARES | crouched | 96.06, 143.97, -370.00¹ | 96.05551, 143.96875, -415.96875 | 45.96875 | -1.10, 4.21¹ | -1.1000061, 4.214966 |

¹ From `cl_showpos` (2 decimals): the rest of the `getpos` line was hidden on screen, or buried under console spam.
OFAST prints the shortest text that gives back the same 32-bit float, so `449.0457` and CS2's `449.045685` are the
same number.
² The pawn's `m_vecViewOffset.z` at that tick, as OFAST reads it from the demo: 4094/64, 2942/64 and 2430/64. (For
the standing value OFAST's float math gives 63.968754, one float step above 63.96875 — the quantized-float formula
rounds that way, demoparser2's too — and the step vanishes when added to the feet: the sum is CS2's -352.000000.)

What this shows:

- **Positions and angles agree** to every digit CS2 prints (x, y, pitch, yaw), for both teams, standing, crouched
  and running.
- **The camera is the feet origin plus the pawn's `m_vecViewOffset`**, exactly: CS2's camera z is OFAST's feet z
  plus that tick's networked view offset, to every printed digit. OFAST's
  [`PlayerSample`](../../crates/ofast-core/src/demo/players.rs) carries it as `view_offset`, and
  `PlayerSample::eye_position()` is the camera position; `round_one_matches_cs2` compares it with the readings
  directly. Standing gives **63.96875, not 64** (CS:GO used 64.06), crouching 45.96875, and the crouched sample with
  the C4 37.96875.
- **The eye height is not a constant**, so a first-person camera must read it per tick. Besides crouching and
  standing up (values in between for a few ticks), it smooths the camera over steps: when the feet go up or down a
  step in one tick, the offset changes the other way by about as much and then eases back towards the standing
  height, so the camera glides while the feet jump. All 24 isolated steps of 1 to 8 units in round 1 behave like
  this (e.g. feet −2: offset 62.20 → 64; feet +2: offset 63.97 → 62.02, then slowly back up). The offset cannot go
  above 64. In round 1, living players range from 37.97 to 64; dead pawns keep values down to 14.
- **x and y of the view offset are always 0** in this match: all 1,281,420 player × tick samples of the 20 rounds.
  OFAST still carries them, because the engine adds the whole vector (as does demoinfocs-golang's `PositionEyes`),
  so the camera stays right if an update ever uses them; they cost 8 bytes per sample.
- **No tick off-by-one.** `demo_gototick N` lands on the tick the demo UI shows as `Tick: N`, and the view there
  is OFAST's sample for tick N. The running sample proves it: CS2's camera matched OFAST's tick 4195 to 6 decimals,
  while OFAST's ticks 4194 and 4196 are about 3 units away. A paused seek shows that tick's state as is, without
  blending in the neighbouring ticks.
- **Console messages use server ticks.** CS2 logs `skipping to demo tick 300 (game tick 2703)`: game tick = demo
  tick + `CDemoFileHeader.server_start_tick` (2403 here). `CGameRules - paused on tick 2704` is one more. OFAST,
  the demo UI and `demo_gototick` all count demo ticks.
- Practical notes for the next check: while spectating, `getpos` prints the spectated player's camera even though
  `cl_showpos` names the local player `DemoRecorder`. CS2 has no `con_logfile`, so the values were read from
  screenshots. On an AZERTY keyboard the console opens with `²`, and pasting commands is more reliable than typing
  them.

`tests/cs2_demo.rs` (`round_one_matches_cs2`) keeps these readings as fixed expectations for this demo.

## Frames

All integers little-endian. The file starts with 16 bytes:

| Offset | Type | Field |
|---|---|---|
| 0 | 8 bytes | magic `PBDEMS2\0` (CS:GO demos start with `HL2DEMO\0` instead — a different format) |
| 8 | u32 | file offset of the `DEM_FileInfo` frame (near the end; 0 if the recording was not finalized) |
| 12 | u32 | file offset of the `DEM_SpawnGroups` frame |

Then frames until `DEM_Stop`. Each frame is three byte-aligned **varints** (protobuf LEB128: 7 data bits per
byte, high bit = "more bytes follow") and a payload:

| Field | Encoding | Meaning |
|---|---|---|
| command | varint | `EDemoCommands` value; bit 64 (`DEM_IsCompressed`) set = payload is Snappy-compressed (raw block format, not the framed format) |
| tick | varint (u32) | game tick; `0xFFFFFFFF` = before the game started ("signon") |
| size | varint | payload bytes |
| payload | `size` bytes | the protobuf message for this command |

| Command | # | Payload | Used by OFAST |
|---|---|---|---|
| `DEM_Stop` | 0 | `CDemoStop` | end of frames |
| `DEM_FileHeader` | 1 | `CDemoFileHeader` | map, server, build — always the first frame |
| `DEM_FileInfo` | 2 | `CDemoFileInfo` | duration (`playback_ticks`, `playback_time`) |
| `DEM_SyncTick` | 3 | `CDemoSyncTick` | — |
| `DEM_SendTables` | 4 | `CDemoSendTables` | [send tables](#send-tables-serializers) |
| `DEM_ClassInfo` | 5 | `CDemoClassInfo` | class id → network name |
| `DEM_StringTables` | 6 | `CDemoStringTables` | string table snapshot |
| `DEM_Packet` | 7 | `CDemoPacket` | [packets](#packets) |
| `DEM_SignonPacket` | 8 | `CDemoPacket` | packets before the game starts |
| `DEM_ConsoleCmd` | 9 | `CDemoConsoleCmd` | — |
| `DEM_CustomData` … `DEM_UserCmd` | 10–12 | | — |
| `DEM_FullPacket` | 13 | `CDemoFullPacket` = string table snapshot + packet | see below |
| `DEM_SaveGame`, `DEM_SpawnGroups`, `DEM_AnimationData`, `DEM_AnimationHeader`, `DEM_Recovery` | 14–18 | | — |

Typical order: `FileHeader`, `SignonPacket`s (server info, string tables), `SendTables`, `ClassInfo`,
`StringTables`, more `SignonPacket`s, `SyncTick`, then `Packet`s with ticks increasing by one, a `FullPacket` about
every minute (34 in the 34-minute demo above), and finally `Stop`, `SpawnGroups`, `FileInfo`.

**Full packets** exist so a player can seek: each holds a complete entity snapshot (`svc_PacketEntities` with
`legacy_is_delta = false`). When reading from the start, the **first** one is required — it is the only complete
snapshot, and the deltas after it build on it — and later ones are skipped (their string table snapshots are still
applied). This is what demoinfocs-golang does; demoparser2's single-threaded mode skips all full-packet entities.
OFAST recognizes snapshots by the frame type (`DEM_FullPacket`) rather than the `legacy_is_delta` flag, whose name
suggests Valve may stop filling it.

**Ticks.** OFAST reports the frame tick (demo-relative, starting near 0). Network messages carry the server tick
(`net_Tick`, `server_tick`), which is the frame tick plus a constant offset (`CDemoFileHeader.server_start_tick`).

## Protobufs

Payloads are protobuf messages defined in Valve's `.proto` files, which SteamDatabase's
[GameTracking-CS2](https://github.com/SteamDatabase/GameTracking-CS2/tree/master/Protobufs) dumps from the game.
OFAST keeps a copy of the needed files in `crates/ofast-core/proto/` (see its README for the commit) and compiles
them at build time with [protox](https://crates.io/crates/protox) (pure Rust, so no `protoc` install) and
prost-build. Valve's files use a few options defined only in Valve-internal files (`option additional_includes`,
bare field options like `boxed_type = "CEntityIndex"`); they only steer Valve's C++ code generator, so `build.rs`
drops them while reading. Decision: [002](../decisions/002-demo-protobufs-built-with-protox.md).

## Packets

A `CDemoPacket.data` is a **bit stream** (see [Bit encodings](#bit-encodings)) holding a list of network messages:

| Field | Encoding |
|---|---|
| message type | ubitvar |
| size | varint (read from the bit stream, so not byte-aligned) |
| body | `size` bytes, a protobuf message |

…repeated until fewer than 8 bits are left (padding). Message types OFAST handles:

| Message | Id | Protobuf | What for |
|---|---|---|---|
| `net_Tick` | 4 | `CNETMsg_Tick` | server tick (not needed yet) |
| `svc_ServerInfo` | 40 | `CSVCMsg_ServerInfo` | `max_classes` (class id bit width), `tick_interval` |
| `svc_CreateStringTable` | 44 | `CSVCMsg_CreateStringTable` | new string table |
| `svc_UpdateStringTable` | 45 | `CSVCMsg_UpdateStringTable` | string table changes |
| `svc_ClearAllStringTables` | 51 | `CSVCMsg_ClearAllStringTables` | drop all tables |
| `svc_PacketEntities` | 55 | `CSVCMsg_PacketEntities` | [entities](#entities) |
| `GE_Source1LegacyGameEvent(List)` | 205 / 207 | `gameevents.proto` | game events (kills, rounds) — from M3 |

Within one packet, string table and server info messages are applied **before** `svc_PacketEntities`: an entity
created in a packet may need a baseline that arrives in the same packet.

## Bit encodings

Bit streams are **least-significant bit first**: the first bit is bit 0 of byte 0, the ninth is bit 0 of byte 1,
and a multi-bit value has its first bit as its lowest bit (like reading the buffer as one huge little-endian
integer from the bottom).

| Name | Layout |
|---|---|
| bits(n) | n bits, unsigned |
| varint | bytes of 8 bits each, LEB128; at most 5 bytes for 32-bit values, 10 for 64-bit |
| signed varint | varint, then zigzag: `(u >> 1) ^ -(u & 1)` (0, -1, 1, -2, …) |
| ubitvar | 6 bits `b`; `b & 0x30` = `0x00`: value `b`; `0x10`: `(b & 15) \| bits(4) << 4`; `0x20`: `… bits(8) << 4`; `0x30`: `… bits(28) << 4` |
| field-path ubitvar | `1` → bits(2); `01` → bits(4); `001` → bits(10); `0001` → bits(17); `0000` → bits(31) |
| string | bytes until a NUL |
| bit coord | `has_int`, `has_frac`; if either: `sign`, then `bits(14) + 1` if `has_int`, `bits(5)` (1/32 steps) if `has_frac`; both clear = 0 |
| angle(n) | `bits(n) × 360 / 2ⁿ` degrees |
| normal | `sign`, `bits(11) / 2047` |
| normal vector | `has_x`, `has_y`, [x normal], [y normal], `negative_z`; `z = √(1 − x² − y²)` (0 if negative under the root) |

## String tables

Named lists of `key → value` entries. OFAST uses two:

- **`instancebaseline`**: key = class id in decimal (`"38"`), value = an encoded field-change list (same format as an
  [entity update](#entities)) with the class's default values. Every created entity starts from its class baseline.
  Keys like `"12:3"` exist too (alternate baselines) and are ignored.
- **`userinfo`**: key = player slot, value = `CMsgPlayerInfo` (name, Steam id). Not needed yet: the player
  controller entity carries the same data.

`svc_CreateStringTable` gives the table settings and the initial entries (`string_data`, Snappy-compressed as a whole
if `data_compressed`). `svc_UpdateStringTable` patches table number `table_id` (creation order). Both encode
`num_entries` entries in a bit stream:

1. **index**: bit `1` → previous index + 1; bit `0` → previous index + varint + 2. The previous index starts at -1.
2. **key**: bit `has_key`. If set: bit `use_history`; with history, bits(5) pick one of the last 32 keys and bits(5)
   say how many of its leading bytes to reuse, then a string completes the key; without, a plain string. Every read
   key is added to the history (oldest dropped after 32).
3. **value**: bit `has_value`. If set:
   - fixed-size tables (`user_data_fixed_size`): `user_data_size_bits` bits;
   - otherwise: if table flag bit 0 is set, a bit `is_compressed`; a byte length — ubitvar if
     `using_varint_bitcounts`, else bits(17); that many bytes, Snappy-decompressed if `is_compressed`.

`CDemoStringTables` (in `DEM_StringTables` and full packets) is a plain protobuf copy: table name + items
(`str`, `data`), values not compressed.

## Send tables (serializers)

`CDemoSendTables.data` = a varint length + a `CSVCMsg_FlattenedSerializer` message, which holds:

- `symbols`: every string; all `*_sym` fields below are indices into it;
- `fields`: one global list of field descriptions;
- `serializers`: name, version, and `fields_index` — the field list of one class or nested struct, as indices into
  `fields`. Fields are shared: a class re-lists its parents' fields.

A field description has `var_name_sym` (`m_vecX`), `var_type_sym` (C++ type), `send_node_sym` (the flattened struct
it came from; `(root)` = none), `var_encoder_sym`, `bit_count`, `low_value`, `high_value`, `encode_flags`,
`field_serializer_name_sym` (for nested structs) and `polymorphic_types`. Absent optional fields mean "not given"
(not zero). Serializers only refer to serializers declared before them; with several versions of one name the one
declared last wins (in practice the newest; both reference parsers do the same).

**Type strings** split into base, template argument, pointer and array count:
`CNetworkUtlVectorBase< CHandle< CBaseEntity > >` → base `CNetworkUtlVectorBase`, argument `CHandle< CBaseEntity >`;
`float32[3]` → count 3; `CPlayer_WeaponServices*` → pointer. Counts can be constants (`MAX_ITEM_STOCKS` = 8,
`MAX_ABILITY_DRAFT_ABILITIES` = 48); the exact count never matters on the wire.

**Field kinds** decide how [field paths](#field-paths) walk into the field:

| Kind | When | Path ending here | One more index | Deeper |
|---|---|---|---|---|
| pointer | has a field serializer and (type ends in `*`, or base is a component like `CBodyComponent`, `CPhysicsComponent`, `CRenderComponent`, `CLightComponent`, `CPlayerLocalData`, …, or it is polymorphic) | bool "present" (polymorphic: bool, then if set a ubitvar choosing the serializer: 0 = own, 1.. = alternatives) | field of the nested serializer | … |
| variable table | has a field serializer otherwise (`CUtlVectorEmbeddedNetworkVar< T >`) | varint new length | element number | field of the element's serializer |
| fixed array | count > 0 and base is not `char` | — | element (no length on the wire) | — |
| variable array | base `CUtlVector` or `CNetworkUtlVectorBase` | varint new length | element (decoder of the template argument) | — |
| simple | anything else (`char[N]` is a string) | the value | — | — |

Field names can repeat inside one serializer under different send nodes (the pawn has `m_vecX` under both
`m_vecVelocity` and `m_vecViewOffset`), so OFAST names fields `send_node.var_name`. Lookups by name also accept
the bare `var_name` when it is unique — CS2 moved the position fields under `m_skeletonInstance.m_vecOrigin`
without changing their member names.

## Field decoders

Chosen per field, in this order:

1. by base type, depending on encoder and bit count:

   | Base type | Decoder |
   |---|---|
   | `float32` | encoder `coord` → bit coord; `simtime` → varint × 1/64; `runetime` → bits(4) as float bits; bit count missing, ≤ 0 or ≥ 32 → 32-bit float; else quantized float |
   | `CNetworkedQuantizedFloat` | 32-bit float if the bit count says so, else quantized float |
   | `uint64`, `CStrongHandle` | encoder `fixed64` → 64 raw bits; else varint |
   | `Vector`, `VectorWS` | encoder `normal` → normal vector; else 3 × the `float32` rule |
   | `Vector2D` / `Vector4D`, `Quaternion` / `CTransform` | 2 / 4 / 6 × the `float32` rule |
   | `QAngle` | encoder `qangle_precise` → 3 presence bits, then for each present component `angle(20) − 180`; bit count ≥ 32 → 3 floats; bit count 1–31 → 3 × angle(n); none → 3 presence bits, then a bit coord per present component |

2. by name: `m_iClip1` → varint − 1; `m_flSimulationTime` and `m_flAnimTime` always use the `simtime` encoder;
3. by base type, fixed: `bool` → 1 bit; `int8/16/32/64`, `GameTick_t` and a few signed enums (`HSequence`,
   `CEntityIndex`, …) → signed varint; `char`, `CUtlString`, `CUtlSymbolLarge`, `CGlobalSymbol` → string;
   `CUtlBinaryBlock` → varint length + bytes; `GameTime_t` → 32-bit float;
4. everything else (unsigned integers, handles, enums, colors, string tokens) → varint.

**Quantized floats** store a value in `low..high` as an n-bit integer, with optional flag bits in front:
round-down (bit set → exactly `low`), round-up (→ `high`), encode-zero (→ 0). The decoder must reproduce Valve's
encoder setup exactly, because it decides which flag bits exist: flags are first validated (e.g. encode-zero becomes
round-down when `low` is 0), the range is narrowed by one step for round-down/round-up, widened to a power of two for
encode-integers (which may add bits), multipliers are computed, and finally flags whose special value the plain
encoding already hits exactly are dropped. Value = `low + (high − low) × raw / (2ⁿ − 1)`. The redundancy check
rounds to the nearest step (demoinfocs-golang found a real field where truncating keeps a flag the encoder dropped).

## Field paths

An entity update first lists the paths of the fields it changes, then their values. A path is up to 7 indices into
the field tree (`[9, 3]` = field 3 of the struct in field 9). The list is delta-coded from `[-1]`: each step is one
of 40 **operations**, Huffman-coded, and each operation produces exactly one path. `FieldPathEncodeFinish` ends the
list.

The Huffman tree is built from fixed weights (0 counts as 1): repeatedly join the two lightest trees; on equal
weight the tree with the **higher value** comes out first, where leaves are worth their operation number and joined
trees are numbered 40, 41, … in creation order; the first tree taken is the left child (bit 0). Codes are read bit by
bit in stream order.

`p` = path, `L` = index of its last level, `fp()` = field-path ubitvar, `ubv()` = ubitvar, `zz()` = signed varint,
`b(n)` = bits(n). "Pop n" goes up n levels, resetting the left levels to 0.

| # | Operation | Weight | Code | Effect |
|---|---|---|---|---|
| 0 | PlusOne | 36271 | `0` | `p[L] += 1` |
| 1 | PlusTwo | 10334 | `1110` | `p[L] += 2` |
| 2 | PlusThree | 1375 | `110010` | `p[L] += 3` |
| 3 | PlusFour | 646 | `11011111` | `p[L] += 4` |
| 4 | PlusN | 4128 | `11010` | `p[L] += fp() + 5` |
| 5 | PushOneLeftDeltaZeroRightZero | 35 | `110110001101` | push 0 |
| 6 | PushOneLeftDeltaZeroRightNonZero | 3 | `110110001100101` | push `fp()` |
| 7 | PushOneLeftDeltaOneRightZero | 521 | `11011010` | `p[L] += 1`, push 0 |
| 8 | PushOneLeftDeltaOneRightNonZero | 2942 | `11000` | `p[L] += 1`, push `fp()` |
| 9 | PushOneLeftDeltaNRightZero | 560 | `11011100` | `p[L] += fp()`, push 0 |
| 10 | PushOneLeftDeltaNRightNonZero | 471 | `11011001` | `p[L] += fp() + 2`, push `fp() + 1` |
| 11 | PushOneLeftDeltaNRightNonZeroPack6Bits | 10530 | `1111` | `p[L] += b(3) + 2`, push `b(3) + 1` |
| 12 | PushOneLeftDeltaNRightNonZeroPack8Bits | 251 | `110110110` | `p[L] += b(4) + 2`, push `b(4) + 1` |
| 13 | PushTwoLeftDeltaZero | 0 | `1101100011001000` | push `fp()` twice |
| 14 | PushTwoPack5LeftDeltaZero | 0 | `11011000110010011` | push `b(5)` twice |
| 15 | PushThreeLeftDeltaZero | 0 | `11011000110010010` | push `fp()` three times |
| 16 | PushThreePack5LeftDeltaZero | 0 | `11011000110011101` | push `b(5)` three times |
| 17 | PushTwoLeftDeltaOne | 0 | `11011000110011100` | `p[L] += 1`, push `fp()` twice |
| 18 | PushTwoPack5LeftDeltaOne | 0 | `11011000110011111` | `p[L] += 1`, push `b(5)` twice |
| 19 | PushThreeLeftDeltaOne | 0 | `11011000110011110` | `p[L] += 1`, push `fp()` three times |
| 20 | PushThreePack5LeftDeltaOne | 0 | `11011000110011001` | `p[L] += 1`, push `b(5)` three times |
| 21 | PushTwoLeftDeltaN | 0 | `11011000110011000` | `p[L] += ubv() + 2`, push `fp()` twice |
| 22 | PushTwoPack5LeftDeltaN | 0 | `11011000110011011` | `p[L] += ubv() + 2`, push `b(5)` twice |
| 23 | PushThreeLeftDeltaN | 0 | `11011000110011010` | `p[L] += ubv() + 2`, push `fp()` three times |
| 24 | PushThreePack5LeftDeltaN | 0 | `1101100011000101` | `p[L] += ubv() + 2`, push `b(5)` three times |
| 25 | PushN | 0 | `1101100011000100` | `n = ubv()`, `p[L] += ubv()`, push `fp()` n times |
| 26 | PushNAndNonTopological | 310 | `110111011` | for each level i ≤ L: if bit, `p[i] += zz() + 1`; then `n = ubv()`, push `fp()` n times |
| 27 | PopOnePlusOne | 2 | `110110001100001` | pop 1, `p[L] += 1` |
| 28 | PopOnePlusN | 0 | `1101100011000111` | pop 1, `p[L] += fp() + 1` |
| 29 | PopAllButOnePlusOne | 1837 | `110011` | pop to the top, `p[0] += 1` |
| 30 | PopAllButOnePlusN | 149 | `110110000` | pop to the top, `p[0] += fp() + 1` |
| 31 | PopAllButOnePlusNPack3Bits | 300 | `110111010` | pop to the top, `p[0] += b(3) + 1` |
| 32 | PopAllButOnePlusNPack6Bits | 634 | `11011110` | pop to the top, `p[0] += b(6) + 1` |
| 33 | PopNPlusOne | 0 | `1101100011000110` | pop `fp()`, `p[L] += 1` |
| 34 | PopNPlusN | 0 | `1101100011000001` | pop `fp()`, `p[L] += zz()` |
| 35 | PopNAndNonTopographical | 1 | `1101100011000000` | pop `fp()`, then for each level i ≤ L: if bit, `p[i] += zz()` |
| 36 | NonTopoComplex | 76 | `11011000111` | for each level i ≤ L: if bit, `p[i] += zz()` |
| 37 | NonTopoPenultimatePlusOne | 271 | `110110111` | `p[L − 1] += 1` |
| 38 | NonTopoComplexPack4Bits | 99 | `1101100010` | for each level i ≤ L: if bit, `p[i] += b(4) − 7` |
| 39 | FieldPathEncodeFinish | 25474 | `10` | end of list (no path) |

OFAST builds the tree itself and a unit test checks every code in this table.

## Entities

`CSVCMsg_PacketEntities.entity_data` is a bit stream with `updated_entries` entries:

| Field | Encoding |
|---|---|
| index delta | ubitvar; index = previous + delta + 1 (previous starts at -1) |
| command | bits(2): `00` update, `10` create, `11` delete, `01` leaves the update set (still exists; rare in SourceTV demos) |
| *update*: visibility | only if `has_pvs_vis_bits_deprecated` ≠ 0: bits(2); bit 0 set = no field data follows |
| *create*: class id | bits(N), N = bit length of `max_classes` (218 → 8) |
| *create*: serial | bits(17) |
| *create*: unused | varint |
| *create / update*: changes | field paths until `FieldPathEncodeFinish`, then one value per path in path order |

A created entity first applies its class's `instancebaseline` changes (own bit stream), then the packet's changes.
Values are stored per path. A path that ends at a variable-length array stores its new length; when the length
shrinks, elements past it are dropped (otherwise a later regrowth would show stale values, because elements equal to
their default are not resent). Element indices must be below the array's length — fixed or current; Valve sends a
new length before elements past the old end, so anything else means the stream is out of sync (the real demo above
never trips this check). The stored value of a polymorphic pointer selects the serializer used for paths below it.

**Handles** (`CHandle`, `CEntityHandle`) are `serial << 14 | index`: index = `handle & 0x3FFF`; `0xFFFFFF` = none.

## Game events

CS2 still sends most gameplay events the Source 1 way, as "legacy game events" inside ordinary packets:

- `GE_Source1LegacyGameEventList` (message 205), once near the start: every event type with its id, name and keys
  (name + type).
- `GE_Source1LegacyGameEvent` (message 207), per event: the event id and the key values **in descriptor order**.
  Each key holds one value field chosen by its type: 1 string, 2 float, 3 long, 4 short, 5 byte, 6 bool, 7 uint64,
  8 long holding an entity handle (`*_pawn` keys), 9 short holding a player slot.

Player keys (`userid`, `attacker`, `assister`) are **player slots**: the controller's entity index minus one;
65535 means nobody (fall damage, the bomb, the world). OFAST keeps events from ordinary packets only (full packets
are snapshots for seeking) and stamps them with the frame's tick; `DemoParser::take_events()` hands them over.

The checked match (de_nuke, 20 rounds) has 148 `player_death` events. The 147 inside rounds give exactly
demoparser2's tick, victim, attacker, weapon and headshot flag (`tests/cs2_demo.rs`, `kills_match_demoparser2`; the
148th is in warmup). Event names seen there include `player_death`, `round_freeze_end`, `round_end`,
`round_officially_ended`, `bomb_beginplant` / `planted` / `begindefuse` / `defused` / `exploded`, `bomb_pickup` /
`dropped`, `weapon_reload`, `smokegrenade_expired`.

The animation bake reads two more (`anim::bake`, [animation.md](animation.md#evaluating-the-graphs-animeval-animstates)):

| Event | Keys used | Round 5 |
|---|---|---|
| `player_hurt` | `userid` (victim), `attacker`, `weapon` (`inferno` for fire), `hitgroup` (0 generic, 1 head, 2 chest, 3 stomach, 4 / 5 left / right arm, 6 / 7 left / right leg) | 32 |
| `bullet_damage` | `victim`, `damage_dir_x` / `_y` / `_z` (the bullet's direction, world), `distance`, `num_penetrations` | 31 |

Fall damage is a `player_hurt` without attacker or weapon and with hit group 0. Flashes come from the pawn
(`m_flFlashDuration`, below), not from `player_blind` (`userid`, `attacker`, `blind_duration`): the field is set on
the event's tick to exactly its `blind_duration` (round 3: 0.6042301 and 2.2956612 s at 14189), set again by each
new flash even while the last one lasts, and back to 0 some time after the blindness (0.06 – 1.9 s later in the
major demo's round 5) or on death. Durations up to 4.83 s were seen.

Tournament demos have fewer events. Rounds 2 and 5 of an Aurora vs FURIA major demo (de_nuke) have no
`player_blind`, `bullet_damage`, `player_footstep`, `item_equip`, `weapon_reload`, `weapon_zoom`,
`bomb_beginplant`, `bomb_begindefuse` or `round_end` (the local match demo has them all), but they have
`hltv_chase` / `hltv_fixed` (the observer's camera), `player_sound`, `grenade_thrown` and `entity_killed`. Hits
then flinch from the attacker's side, flashes still come from the pawn, plants and defuses from the pawn and the
weapon (`m_bIsDefusing`, gameplay state 300), and the replay's milestones lack the plant and defuse beginnings and
the round end.

## Players and rounds

- **Controller** (`CCSPlayerController`, entity index = player slot + 1): `m_iszPlayerName`, `m_steamID`,
  `m_iTeamNum` (2 = T, 3 = CT), `m_hPlayerPawn` (handle of the pawn). Tournament demos also have spectator
  controllers (team 1: the observer, casters) whose pawn is never alive and stays at (0, 0, 0); OFAST samples only
  teams 2 and 3 (an Aurora vs FURIA major demo has one, `@reLazffs`, in every round).
- **Pawn** (`CCSPlayerPawn`): position = `cell × 512 − 16384 + offset` per axis from `CBodyComponent` →
  `m_cellX/Y/Z` and `m_vecX/Y/Z` (world units: 1 unit = 1 inch; the world spans ±16384) — this is the feet;
  `m_vecViewOffset.m_vecX/Y/Z` = from the feet to the first-person camera (z = eye height, 63.97 standing, 45.97
  crouched, smoothed over steps; x and y 0 in practice; a component never sent keeps its default 0);
  `m_angEyeAngles` = (pitch, yaw, roll) in degrees (pitch negative = looking up; yaw −180..180, 0 = +x);
  `m_lifeState` 0 = alive; `m_flFlashDuration` (seconds of blindness of the latest flash, set on the flash's tick,
  back to 0 after it) and `m_flFlashMaxAlpha` (255 for every flash seen).
- **Camera**: `PlayerSample::eye_position()` = position + view offset, aimed along pitch and yaw — what CS2 shows
  when spectating that player in first person (see [Checked in the game](#checked-in-the-game)).
- **Game rules** (`CCSGameRulesProxy` → pointer `m_pGameRules`): `m_totalRoundsPlayed`, `m_bFreezePeriod`,
  `m_bWarmupPeriod`, `m_gamePhase` (5 = match ended).

OFAST's round: starts when freeze time begins (`m_bFreezePeriod` turns on) outside warmup and before the match has
ended; number = `m_totalRoundsPlayed + 1`; lasts until the tick before the next round starts, the match ends, or the
demo ends — so it includes the seconds after the round is decided. Start ticks equal the `round_start` game event.
A player sample for tick T is the state after the last packet of frame tick T (demoparser2 samples the same way).

## Rig inputs

What drives a player's animation in CS2, read per player and tick into `PlayerSample::rig`
(`crates/ofast-core/src/demo/players.rs`) for OFAST's replay rig
([decision 013](../decisions/013-replay-rig-baked-in-rust.md)). Each field is looked up leniently: if a CS2 update
renames one, it reads as its default and positions keep working.

| Input | Field | Notes |
|---|---|---|
| body yaw | pawn `CBodyComponent` → `m_skeletonInstance.m_angRotation` (yaw; pitch and roll 0) | the way the model faces; it follows the eye yaw with a lag: round 5, living players, median gap 0°, 90 % within 10°, 99 % within 40°, up to 128° during fast turns |
| velocity | pawn `m_vecVelocity.m_vecX/Y/Z` (20-bit quantized, ±16384) | units/s; running 215 – 250 depending on the weapon |
| on the ground | pawn `m_fFlags` bit 0 (`FL_ONGROUND`) | off while jumping or falling |
| crouch amount | `m_pMovementServices` → `m_flDuckAmount` | 0 standing … 1 crouched, gradual |
| walking, scoped, defusing | pawn `m_bIsWalking`, `m_bIsScoped`, `m_bIsDefusing` | |
| on a ladder | pawn `m_MoveType` = 9 (`MOVETYPE_LADDER`; 2 = walking) | round 5: 185 samples, 184 of them moving mostly vertically over a 234-unit climb |
| last jump, last landing | `m_pMovementServices` → `m_nLastJumpTick`, `m_ModernJump.m_nLastLandedTick` | game ticks, converted to demo ticks (below) |
| weapon in hand | `m_pWeaponServices` → `m_hActiveWeapon` → the weapon entity | its class (`CAK47`, `CWeaponM4A1`, `CKnife`…) and the fields below; each weapon class has its own serializer, so paths are looked up per class |
| weapon id | weapon `m_AttributeManager.m_Item.m_iItemDefinitionIndex` | 1 Desert Eagle, 4 Glock-18, 7 AK-47, 16 M4A4, 36 P250, 42 CT knife, 43 flashbang, 44 HE, 45 smoke, 48 incendiary, 49 C4, 59 T knife, 61 USP-S, 500+ knife skins (509, 514 seen) |
| weapon animation state | weapon `m_iWeaponGameplayAnimState` | see below |
| its start | weapon `m_flWeaponGameplayAnimStateTimestamp` (`GameTime_t`, seconds) | game seconds ÷ tick interval = game tick, converted to a demo tick (below): the tick the state began, also when it begins again with the same value (two knife misses in a row are 503 twice, 26 ticks apart) |
| reloading | weapon `m_bInReload` | |
| magazine | weapon `m_iClip1` (Ammo encoder: varint − 1) | rounds in the magazine, −1 without one (knives, grenades); a reload begun at 0 plays the graphs' `reload_empty` clip |
| throw strength | grenade `m_flThrowStrength` | 0 – 1: 1 for a left-click throw, 0 for a right-click lob, 0.5 for both; the graphs throw underhand at ≤ 0.33. Round 5: 1 on all four throws |

**Weapon animation states** seen in round 5 of the match demo, by the weapons holding them: 11 = draw (after every
switch, every weapon), 50 = idle, 100 = shooting (rifles and pistols), 200 / 202 = grenade (pin, throw), 300 = C4
planting, 500 / 501 / 503 = knife attacks, 800 / 801 = reloading (with `m_bInReload`), 1000 / 1001 = after a draw
on every weapon, mostly knives (7,162 of 9,945 samples): very likely the inspect animation and its end (not yet
checked in the game). On the Aurora vs FURIA de_nuke demo (round 5) 1000 comes 1 – 28 ticks after a draw (the
inspect key pressed during the draw, which CS2's viewmodel graphs turn into an inspect from the draw: see
[First-person view](#first-person-view)), 1001 follows 3.4 s later on a knife and lasts until idle (50) 1.3 s
later: the inspect and its last part. 1 and 10 show on weapons as they are switched away. Every new shot restarts
state 100 with a new timestamp (0.1 s apart on the AK-47); the state stays 100 for about 0.6 s after the last one.

**Game ticks.** `GameTick_t` fields count the server's ticks, not the demo's. `CNETMsg_Tick` carries the server
tick of each packet; it runs a constant distance ahead of the frame tick (2403 on the match demo, the same figure
CS2's console prints when seeking: "demo tick 48270 (game tick 50673)"). `DemoParser::game_tick_to_demo` converts.
`GameTick_t` is an `int32` sent as a zigzag (signed) varint: read as unsigned it comes out doubled and always even,
which is what demoparser2 0.42.0 exports. Converted, each jump falls one tick before the first airborne sample and
each landing on, or one tick before, the first sample back on the ground.

**Checked** (`rig_inputs_match_movement_and_demoparser2` in `tests/cs2_demo.rs`, 2026-10-18): over round 1, all
50,740 player × tick rows equal demoparser2's body yaw (within 0.01°), ground flag, crouch amount, walking, scoped,
defusing, ladder, weapon id, animation state and reload flag; every raw jump tick is even and halves to OFAST's jump tick plus one constant
(2403); the velocity agrees with the position change between ticks (median difference 1.9 units/s for players moving
faster than 50 units/s — positions advance by sub-tick moves). Round 5 compared the same way: 48,590 rows, no
difference. demoparser2 cannot check the velocity (its `CCSPlayerPawn.m_vecX/Y/Z` mixes the velocity and the view
offset, see below).

### Pose recipe (CS2's own animation)

The demo does carry what CS2's server animated: every `CCSPlayerPawn` sends its AnimGraph2 **pose recipe**, the
list of pose tasks the graph ran that tick (sample this clip at this time, blend these two poses by this weight…),
which clients play back instead of running the graph themselves. Source 2 Viewer and demoparser2 do not decode it;
OFAST reads and decodes it in `demo::recipe` ([decision 020](../decisions/020-cs2-pose-recipe.md);
`examples/pose_recipe.rs` prints it per player and tick, `OFAST_CHECK=1` counts what decodes). It is written the
way Esoterica's task serializer writes it (Bobby Anguelov's open engine, whose design AnimGraph2 follows:
`Animation_TaskSerializer`, `BitSerialization.h`, `Animation_BoneMaskTask.cpp`): bits least-significant first.

The fields sit on the pawn's `CBodyComponent` → `m_animationController`:

| Field | Declared type | What it holds |
|---|---|---|
| `m_hGraphDefinitionAG2` | `uint64` | the graph's resource id: `animation/graphs/worldmodel/worldmodel.vnmgraph` for every player |
| `m_SerializePoseRecipeAG2Slots` | `CUtlVectorEmbeddedNetworkVar<AnimGraph2SerializedPoseRecipeSlot_t>`, 32 slots | each slot's `m_topology`, a byte blob: a task list the player has used lately (below) |
| `m_nSerializePoseRecipeAG2ActiveSlot` | `uint32` | which slot this tick's tasks follow; changes when the graph's task list changes |
| `m_SerializePoseRecipeAG2Dynamic` | `CNetworkUtlVectorBase<uint8>`: a length, then one element per byte (32, 64 or 96) | this tick's task data: the server tick (`u32`), then each task's payload in the slot's order |
| `m_nSerializePoseRecipeVersionAG2` | `int32` | 2 |

A **topology** is a 4-bit width `D` (at most 8), a `D`-bit task count, then per task a 5-bit type and its inputs
(`D` bits each: indices of earlier tasks). How many inputs each type takes is not written: with the counts below
all 618 different topologies of round 5 of the de_nuke pro demo (demo ticks 29322 – 41800) fit (each input an
earlier task, every task but the last used by a later one, the bits ending in the blob's last byte), and so do the
1,697 of the local match demo (another CS2 build, which adds types 9 and 13); all 114,455 recipes of the round and
678,075 of the match decode without an error (`examples/pose_recipe.rs` with `OFAST_CHECK=1` prints these counts
and where each type sits). The **data** follows the topology's order, each payload straight after the last, so a
task whose payload is not known hides everything after it. Payload widths were found by alignment
(`OFAST_PROBE=<type>` reads the type's payloads as 0 – 32 raw bits and decodes the rest each way): at the right
width every recipe decodes cleanly, the clips after it keep their index from one tick to the next, and where
decoding then stops at a 14 its first value is the aim's usual 32768; at any other width fewer decode and the mark is
missing. A blend's payload varies in length, so blends are checked by that last mark alone: with every type decoded
as in the table,
the recipes that stop at a 14 have 32768 there about as often whatever types come before (47 – 75 % on round 5,
54 – 94 % on the local match; a misaligned type would leave it near none).

| Type | Inputs | Payload | What |
|---|---|---|---|
| 1 | 0 | resource index (10 bits in these demos, enough for the 909 clips and 67 graphs the graph refers to), normalized time (16 bits) | sample a clip |
| 7 | 2 | normalized weight (8 bits), a flag, and when set a bone mask list | blend the second input over the first |
| 8 | 2 | the same | blend; the masked layers (weight 1, bone mask 0, over the upper body) |
| 10 | 2 | the same (the recipes with one: 41 % with the 32768 mark on round 5, 54 % on the local match, as the others) | another blend kind (in 371 of the 618 topologies), often before the weapon layer |
| 9 | 2 | the same (89 % of the 257 with the mark) | another blend kind, rare (in the local match only) |
| 0 | 1 | a 6-bit ID (22, 25, 26 and 34 seen; at 6 bits all 6,797 recipes of round 5 with one decode, 4,002 of them with the mark; the next best width, 20 bits, 3,302 and 1) | keep the pose under that ID (Esoterica's `CachedPoseWriteTask`) |
| 6 | 0 | a 6-bit ID (all 862 decode at 6 bits, 639 with the mark; the next best, 31 bits, 383 and none) | the pose kept under that ID (`CachedPoseReadTask`), first in all 31 topologies that have one |
| 13 | 0 | nothing (all 173 decode at 0 bits, 163 with the mark; the next best, 25 bits, 170 and none) | an input-less pose (Esoterica's reference and zero poses carry nothing) |
| 15 | 1 | not known yet | in 64 topologies, always just before the 14 |
| 14 | 1 | not known yet: it starts with two 16-bit values near 32768 | the aim, probably (in all but one topology, then a sample or the 5): for YEKINDAR mid-spray (ticks 37930 – 37931) the two read 32768 and 32894, 32768 and 32636 — yaw 0 to the body and the pitch level — while his eyes look 7 – 9° down: the pitch of the bullets (eyes + 2 × aim punch), not of the eyes. Later bits carry the graph's inputs: 8 bits from bit 48 of the 14's start are the duck amount (YEKINDAR crouching at 37883 – 37895: 0.12 → 30, 0.35 → 88, 0.58 → 149, 0.82 → 210, 1 → 255). No fixed width up to 32 bits keeps the clip after a 14 (29 bits keeps it in 97.7 % of recipes), so it runs longer or varies. The sample after it, under a type 10 blend, is the flashed pose (XANTARES at 34532, flashed): the flashed layer sits after the aim |
| 5 | 1 | not known yet: what is left after the 14 (the 14 and the 5 together) runs 80 – 850 bits, the same for a player over many ticks and not set by the weapon in hand (a knife's 360 or 616, an AK's 593, 616 or 337) | last in every topology |

A bone mask list (Esoterica's `BoneMaskTaskList`) is a 5-bit count, then per entry a 3-bit kind: 0 `Mask`, one of
the skeleton's bone masks by index (4 bits in these demos; Esoterica sizes it to the mask count, so a CS2 build can
change it); 1 `GenerateMask`, an 8-bit normalized weight; 2 `Blend`, two earlier entries and a weight; 3 `Scale`, an
earlier entry and a weight; 4 `Combine`, two earlier entries. Entries are named with as many bits as the count
needs, weights with 8; kinds 5 – 7 do not exist.

A typical list: `1 1 7(0,1) 1 7(2,3) 1 8(4,5) 1 8(6,7) 14(8) 5(9)`: the locomotion blend-space triangle (three clips
at one shared time, two weights), a pose at time 0 laid over the upper body, the weapon's clip over it (YEKINDAR's
AK fire, restarting at each shot), the aim, the last step. The locomotion times advance by `tick interval ÷ cycle
length` (0.026 a tick on a 0.6 s run cycle) and the weights ease as the player turns: this is the pose CS2 drew.
Every clip and blend decodes in 98.4 % of round 5's recipes and 99.6 % of the local match's (decoding stops at a
14, 15 or 5 with no clip or blend after it); the rest have a clip after a 14 or a 15.

The resource index is not the graph's own resource order. The main graph and its 66 child graphs refer to 909 clips
(`examples/recipe_table.rs` counts them); their indices follow no rule tried by hand (their order in the graphs, their
ids sorted, CRC32, FNV and MurmurHash orders of their paths and ids, hash-table slots, EASTL-style prime buckets; not
kept as a probe). Esoterica numbers them in the order it walks the graph's resource lookup table, a hash map, which
would explain an order that looks random and changes with the build. The two tournament matches checked (three
rounds) share one order: by co-occurrence with the locomotion OFAST bakes (`examples/recipe_map.rs`, on recipes that
start with the triangle), 434 is `idle_rifle`, 321 `walk_n_rifle`, 141 `walk_ne_rifle`, 850 `walk_e_rifle`, 229
`walk_nw_rifle`, 919 `walk_w_rifle`, 697 `walk_se_rifle`, 854 `walk_s_rifle`, 534 `walk_sw_rifle`, 22
`run_ne_rifle`, 175 `idle_crouch_rifle`, 289 `walk_n_pistol`. The local match demo, recorded on another CS2 build,
has another (`walk_n_pistol` is 847 there): the table has to be found for each build.

**Finding the table from the demo** (`anim::recipe::learn_clip_table`; `examples/recipe_table.rs` prints it): OFAST
bakes every player at the tick rate with a trace of the clips it samples (`anim::trace_start`), and each index is
matched to the baked clip that comes with it most exclusively on the same player's same ticks (Jaccard: ticks
together over ticks either is played), strongest matches first, each index and each clip taken once (the context
lists each clip once), none under 0.3. The bake re-decides CS2's states its own way, so the match is statistical,
but CS2's clips and OFAST's follow the same inputs: on round 5, 588 is `breathing` (Jaccard 0.88; its time runs at
6.32 s a cycle, the clip lasts 6.33 s), 463 `idle_ak` (0.92, sampled at time 0 while 95 % of its ticks have an AK in
hand), 159 `idle_karambit` (0.95), 321 `walk_n_rifle` (0.70). The table matches 213 of the round's 333 indices,
which name 93.2 % of its sample tasks (`breathing` is 224 on the local match, another build). The indices left are
clips OFAST does not play where CS2 does (0.53 s clips the bake has no counterpart for come out below 0.2); a match
can still be wrong where OFAST plays one clip and CS2 another at the same moments. One was: 274 is `shoot_m4a1s`
(0.40 s; it restarts with each bullet of XANTARES's sprays and ends 0.4 s after the last), but the bake started its
shot clip only when the weapon's state entered 100 or 101, which holds through a spray, so it played once per spray,
and 274 went to `flashed_crouch` (0.32), the pose XANTARES held over the same ticks; his upper body came out flashed
while CS2 shows him spraying. The bake now starts the shot clip on every bullet (the fire-bullets messages), and the
shot clips match far better (274 `shoot_m4a1s` 0.96, 754 `shoot_ak` 0.61 → 0.89, 517 `shoot_famas` 0.38 → 0.97).
The trace also leaves out what OFAST plays over its aim (the flashed arms, `anim::untraced`): the recipe's decoded
part ends at the aim, so no index it names can be such a clip.

The recipes' structure then vets the matches (`ClipTable::dropped`): the two clips of a type 7 blend, a 1D blend
between clips of one move (stand and crouch, two directions), are both poses or both deltas (additive), which every
strong match keeps. Each match is weighed against its matched partners' clips by the player-ticks of their blends:
while one is on the other side on more of them than not, the one most at odds (ties: the weaker match) is dropped for
good and the table made again without it, so its clip is free for any index and its index takes its next match
(`anim::recipe::assign`). On round 5 it drops 4: 574 (`crouch_ne_knife`, a pose, 1D-blended only with the delta
`idle_c4`: an additive C4 idle, now unknown), 283 (`idle_crouch_c4`, a delta, 1D-blended only with the pose
`planting_crouch`: now `turn_right_23_crouch_knife`, a pose, but likely the standing plant the bake rarely plays),
734 (`jump_crouch_stand_rifle`, a pose among deltas: now `jump_additive_crouch_start_rifle`, whose 0.60 s its time
steps match, the pose going to 40), and 210, which took the freed `idle_crouch_c4` and was as much at odds. On the
local match's round 1 it drops 2: 248 (`draw_tactical`, blended only with weapon idles, which are deltas: now
`idle_tactical`) and 93 (`inair_crouch_e_pistol`, blended only with an additive jump clip), and the table names
93.3 % of the sample tasks instead of 92.4 %. The hitbox fit does not move (these clips rarely play at a hit).

**Playing it** (`anim::recipe::RecipePlayer`, `ofast export-demo … --players` with `OFAST_RECIPE=1`): each tick's
tasks are played in order: a sample is its clip at its time × its length (a sample the table does not know gives
nothing); a blend lays its second input over its first (an additive one added, type 8 in model space, the others
bone by bone) under its mask (mask 0 is the skeleton's first, `UpperBody`, of the 12 it defines: the 4-bit index
fits), gives nothing without its first input and its first input alone without its second (a layer the table does not
know is left out: giving no pose there instead, for masked layers over a quarter of weight, fit the hits worse, 364
of 505 within an inch against 371, the layers left out being mostly wrong matches, such as an additive idle blended
with a whole pose, rather than clips OFAST does not play; an action CS2 plays with a clip the table misses is lost
from the pose while OFAST's aim weights for it remain); a cached
pose is kept by its ID and read back on later ticks (forgotten when the player dies, or when its ID is written from
nothing known); the empty pose is the skeleton's reference. A
recipe whose result is a delta, or that no known clip went into (the empty pose alone), gives no pose. That pose
replaces OFAST's re-decided locomotion and layers; OFAST's own aim (the recipe's aim is type 14, not decoded) and
flashed arms go on top. A living tick without a recipe pose keeps the re-decided pose, except runs of at most 8 ticks
between two recipe poses, which blend the two. On round 5 the recipes give the pose on 86 % of the living
player-ticks (87,757 of 102,205; the export's `check:` line). The server poses its hitboxes with its own graph, so
the demo's hits check the poses: on the 10 rounds of 8 demos the hitbox fit uses (`HIT_POSE_TICKS`), 371 of 505 hit
points lie within an inch of a hitbox's surface with the recipes' poses against 349 with the re-decided ones, 470
against 464 on a posed body, the median distance lower on 9 of the 10 rounds (0.3 inch against 0.5 – 0.6 on most).
The shot clip on every bullet moved the re-decided poses from 353 to 349 and the recipes' from 364 to 371.

## Deaths

CS2 simulates a dead player's ragdoll on the client: demos do not carry its motion. On the death tick the pawn's
`m_lifeState` turns 1 (dying; 2, dead, about 1.8 s later), `m_bRagdollClientSide` turns true, and
`m_RagdollPose.m_Transforms` (a networked pose, for server-side ragdolls) stays empty. What the client starts the
ragdoll from is set on the pawn that tick and kept until the next death (`PlayerSample::death`, read while dead):

| Field | What | Seen |
|---|---|---|
| `m_nRagdollDamageBone` | the bone hit, an index into the agent's skeleton | 7 (`head_0`) for a headshot, 4 (`spine_2`) for an AWP body shot |
| `m_vRagdollDamageForce` | the killing hit's force, world, along the shot | (−26553, 16367, 712) for an M4A1-S headshot, (27317, 6145, 167) for an AWP body shot |
| `m_vRagdollDamagePosition` | where it hit, world | |
| `m_bRagdollDamageHeadshot`, `m_szRagdollDamageWeaponName` | headshot flag, weapon (`m4a1_silencer`, `awp`) | |
| `m_vRagdollServerOrigin` | the pawn's origin a tick before the death, not where the body ends up | |
| `m_qDeathEyeAngles` | the eye angles at death | |

The dead pawn itself stays where it died (its position and body yaw no longer change). OFAST simulates the ragdoll
from these and the agent's `PHYS` block ([animation.md](animation.md#deaths-exportdeaths-physicsragdoll)).

## Dropped weapons

Unlike the ragdolls, weapons nobody carries are simulated on the server, and the demo carries their motion
(`TickSample::dropped`, `DroppedWeapon`). A weapon entity (any class with
`m_AttributeManager.m_Item.m_iItemDefinitionIndex`) is lying in the world when its owner `m_hOwnerEntity` and its
body component's `m_hParent` are both the invalid handle (0xFFFFFF). Then its `CBodyComponent` holds a world
origin (`m_cellX/Y/Z` × 512 − 16384 + `m_vecX/Y/Z`, like a pawn's) and rotation (`m_angRotation`: pitch, yaw, roll,
Source's `QAngle`), sent on every tick it moves and not while it rests; a carried weapon sends zeros there (it
follows its owner's hand: OFAST skips a weapon exactly at the world origin). Its model and mesh groups stay what
they were while carried. Grenades in flight are other entities (`C…Projectile` classes, without an item
definition): not weapons lying in the world, and not read here (M6).

Seen (`examples/weapon_probe.rs` prints every weapon's owner, parent, origin, rotation and model when they change):
on the death tick the dead player's gun in hand and a grenade lose their owner at the hand's height and fly on with
the body's momentum. KSCERATO's AK-47 (Aurora vs FURIA, de_nuke, round 5, tick 34415) went 4.5 m and lay on its
side (roll −90°) 1.3 s later; weapons dropped for a teammate in freeze time lie a few ticks before they are picked
up (the owner turns valid again); a dropped C4, smoke grenades, pistols and rifles left from earlier deaths lie to
the end of the round.

**Checked** (`the_dead_let_go_of_their_guns` in `tests/cs2_demo.rs`, 2026-10-23): over the first three rounds of
both test demos, each of the 46 players who died carrying more than a knife let go of a weapon within two ticks and
96 units of the body (124 stretches of weapons lying in the world in all). Against CS2 (`compare_doors_b.png`,
bottom pair: tick 40290 of Aurora vs FURIA round 5, from CS2's own Chase Cam position behind MAJ3R): KSCERATO's
AK-47 lies in Unreal where CS2 draws it, on its side, stock towards the doorway (CS2's carries its skin; Unreal shows
the default finish).

## Doors

A map's rotating doors (`prop_door_rotating`) are `CPropDoorRotating` entities, created again every round
(`TickSample::doors`, `DoorSample`): `m_closedPosition` and `m_closedAngles` (the map entity's origin and angles: its
hinge and closed rotation), `m_eDoorState` (0 closed, 1 opening, 2 open, 3 closing), the body component's origin and
`m_angRotation` (sent on the ticks it swings), `m_hMaster` (the other half of a double door), the model. A de_nuke door
opens 89° (the map's `distance`) in 28 ticks (0.44 s) and closes the same way; both halves of a double door swing
together, one of them through ±180° of yaw.

Seen (`examples/entity_probe.rs`, which prints any entity's fields by name when they change): on the Aurora vs FURIA
de_nuke demo, round 4 opens and closes a double door; round 5 opens none. Every door entity leaves the demo at the
round's restart, 320 ticks before the next freeze time, and one left mid-round with nothing changed on it before.

That one is a **broken door**: the A-site door (hinge 257, −1310, −416) left the demo on tick 33989, and on tick
33990 an HE grenade went off 27 units in front of it (`hegrenade_detonate`; the blast reached the door first). CS2
removes a door a blast breaks: it draws the doorway clear from then, and all 10 times players walk through it in
round 5 come after (34387 – 41837). The same happens in each of rounds 1 – 8 (teams open A by blowing the door): the
A door's entity leaves one tick before an HE grenade goes off 9 – 83 units from its hinge. Doors also leave at the
round's restart, 320 ticks before the next freeze time, unbroken (in rounds 4, 6 and 7 that is before the replay's
last tick). So a door is gone from the tick an HE grenade goes off within 128 units of its hinge at most 4 ticks
after its entity's last tick (`import_demo.py`, `broken_doors`); any other keeps its last rotation. (This first looked like a
disagreement between the old demo and the current build: on the local match demo players cross the A doorway only
while the door stands open, yaw −89°, and in round 3, closed all round, nobody does; on a local server CS2 draws the A
door and the tunnel's double door closed in their frames exactly as Unreal does, `compare_doors_b.png`. With the door
gone after the blast, Unreal's doorway matches CS2's at the same ticks.)

**Checked** (`doors_swing_open_and_shut` in `tests/cs2_demo.rs`): over the first three rounds of both test demos,
every closed door sample (65,473 and 59,015) stands at its closed angles, every open one (8,119 and 9,022) has swung
60 – 120° about z, and no door leaves its hinge.

## Physics props

The props the server simulates (the map's `prop_physics_multiplayer`) are `CPhysicsPropMultiplayer` entities
(`TickSample::props`, `PropSample`; `CPhysicsProp` and `CPhysicsPropOverride` would read the same way, not seen yet):
the body component's origin and `m_angRotation`, sent on the ticks they move, the model. Bullets, blasts and players
push them about. de_nuke has 25: hanging hard hats and office mugs. Every one is made again where its map entity puts
it at each round's start (round 5: all 25 at their entity lump's origins and angles, a few placed lying on the floor),
exactly on the round's first tick (not 320 ticks before, as the doors leave), and sometimes within a round (a
vitality vs FaZe de_nuke demo remakes all 25 at tick 191 of round 1), under other entity indices: a prop entity's
first sample names its map entity. In round 5 the A site's three hats on their rack (130, −1040 … −1104, −344) fall
63 – 74 units to the floor from tick 37079, a tick before the HE grenade that throws them goes off (37080, as the
doors leave a tick before theirs); CS2 shows them there through YEKINDAR's eyes at 37685.

Not every entity the map has is networked: `func_brush` blockers and `prop_dynamic` props of other game modes that
start disabled (de_nuke's retake brushes and wingman silo doors) are not in the demo at all, and of de_nuke's
networked `prop_dynamic` props 7 of 9 interact as window, block sound and touch all only (`m_Collision`'s
`m_nInteractsAs`; no `solid` layer: bullets go through them, [replay.md](replay.md), `shots`).
`examples/solid_entities.rs` lists the model entities a demo has solid on a tick.

## First-person view

CS2's viewmodel is client-side (`C_CS2HudModelArms`, since July 2025): demos carry no viewmodel entity and no pose.
They carry what drives it — the weapon in hand and its animation state ([Rig inputs](#rig-inputs)) — and on each
pawn the player's own viewmodel settings (`PlayerSample::viewmodel`, `ViewmodelSettings`), which CS2 networks so
that spectators see the viewmodel as the player does:

| Field (pawn) | Setting | Seen (Aurora vs FURIA, de_nuke) |
|---|---|---|
| `m_flViewmodelFOV` | `viewmodel_fov`, horizontal at 4:3 like the world FOV (60 – 68, default 60) | 60, 65, 68 |
| `m_flViewmodelOffsetX` / `Y` / `Z` | `viewmodel_offset_x` / `_y` / `_z`: inches in view space, right, forward, up (defaults 1, 1, −1) | (2.5, 0, −1.5), (2, 2, −2), (1, 1, −1), (−0.5, 1, −2), (2.5, 0, −2) |
| `m_bLeftHanded` | the viewmodel in the left hand (`cl_righthand 0`, or the switch-hands key) | 3 of 10 players start round 5 left-handed; yuurih switched hands 31 times in the round, KSCERATO 4 |

Pawns that never send them read as CS2's defaults. CS2's notes say the FOV only reaches spectators correctly since
2026-05-07; OFAST uses what the demo carries. For the viewmodel the weapon also tells `m_bSilencerOn` (the M4A1-S
and USP-S, `ActiveWeapon::silenced`) and its reserve ammunition (`m_pReserveAmmo[0]`, `ActiveWeapon::reserve`: the
graphs' empty pose, slide back, is for a gun out of everything). The inspect fields (`m_bInspectPending`, `m_bInspectShouldLoop`,
`m_flInspectCancelCompleteTime`) stay at their defaults in this demo: inspects show only in the animation state.

### Recoil on the view

CS2 turns a player's own view off their eye angles by recoil (`demo::recoil`, `export::punch`):

- **The view punch** is in the demo: `m_pCameraServices.m_vecCsViewPunchAngle` (pitch, yaw), set on each shot with the
  game tick it was set (`m_nCsViewPunchAngleTick`) and a sub-tick ratio (`m_flCsViewPunchAngleTickRatio`)
  (`RigInputs::view_punch`). It decays exponentially at 18 per second (CS:GO's `view_punch_decay`), and each shot adds
  0.055 of its kick to what is left: during an AK spray of round 5 (ticks 37807 – 37832) the jumps over what is left
  are 0.055 × the AK table's first kick (22.5, to 0.0001°), then 22.2, 23.7, 26.0, 28.8 against the table's 21.6,
  23.0, 25.2, 27.8 (Source 2 Viewer's ramp, which compounds: 3 % under; one that does not would give 24.4, 26.3, 28.1,
  30). Its tick and ratio are the shot's moment: on the local match demo exactly the shot's (attack tick − 1 + the
  attack's sub-tick fraction, [Shots](#shots)), on tournament demos 0.6 – 1.9 ticks after the start of the shot's tick
  interval, where OFAST places a shot without its sub-tick moment.
- **The aim punch** is not: `m_pAimPunchServices`' fields stay unsent on SourceTV. Every shot shows it: the
  fire-bullets message's aim is the eye angles plus twice the punch. It is rebuilt as Source 2 Viewer rebuilds it: each
  shot's kick pushes a velocity (decaying at 4.5 per second) that the punch integrates while it decays back to nothing
  (8 per second exponentially, 18° per second linearly). The kick is the gun's recoil table's at the shot's recoil
  index (from `m_nRecoilSeed`, `m_flRecoilAngle` / `Magnitude` and their variances in `weapons.vdata`; a full-auto
  gun's kicks drift and ramp up), and a spray's first view punch jump is that kick × 0.055 to 0.0001°; but CS2's
  semi-automatic pistols (the Glock, the Five-SeveN) take each kick from another entry of their table (their jumps
  match entries 5, 47, 11, 17…, exactly, at recoil indices 0 – 1.4), so OFAST takes each shot's kick from its view
  punch jump (the first one not yet taken from 0.5 tick before the shot to 2.5 after that kicks up and as hard as the
  table's kick within a factor of 2: a landing's push down is not a shot's), at the jump's moment, and the table only
  without one.
- Checked (`examples/aim_punch.rs`): at every shot the rebuilt aim punch is within a median 0.07 – 0.09° of what the
  shot shows (eye angles taken at the shot's moment, between two ticks; 4 rounds of 3 demos); a full-auto spray's later
  shots run about 3 % above the rebuilt punch (Source 2 Viewer's order of decay and integration gives 3 % less, the
  exact integral 3 % more).
- The camera turns by 0.45 of the punch's offset to the bullets (0.9 × the punch) plus the view punch; the viewmodel
  another 0.325 of it (0.65 × the punch) past the camera. Checked against CS2's first-person view of YEKINDAR's spray
  (round 5, ticks 37703 and 37709): the A sign's centre sits at 47.6 % and 47.4 % of the frame's height in CS2,
  47.45 % and 47.35 % in OFAST (about 44 % without the recoil: the camera tilts 3 – 4° down with the player's
  pull); after the spray's first shot (37685) 49.0 % in CS2, 49.2 % in OFAST, 47.9 % without the recoil; across, the
  two agree to 0.1 % of the width. The viewmodel's muzzle sits 4 % of the height above CS2's (6 % on a single shot
  before any recoil, at tick 37685: the firing animation's pose, not the punch).
- The third-person body aims where the bullets go: the eye angles plus twice the punch (`anim::bake::BODY_AIM_PUNCH`;
  Source 2 Viewer adds it once). Checked against CS2's free camera beside two sprays of round 5, the barrel's slope on
  screen against OFAST's barrel projected into the same camera: YEKINDAR's AK (ticks 37900 – 37975, his eye pitch
  pulled 11° down while the bullets stay level) keeps a −0.7 to −1.5° slope in CS2 from the first shot to the
  thirteenth; OFAST gives +0.1° there with the punch doubled, −5.4° with it once, −10.9° without, and the muzzle
  lands 2.5 px from CS2's in a 1456-px-wide frame (22 px with it once, 41 without). A fit over five frames of his and
  XANTARES's M4A1-S spray gives 1.9 × the punch.

## Grenades

A thrown grenade is an entity of its own (`demo::grenades`, `TickSample::grenades`, `GrenadeSample`), one class per
kind:

| Class | Kind | Seen (Aurora vs FURIA, de_nuke, round 5) |
|---|---|---|
| `CSmokeGrenadeProjectile` | smoke | 9; the cloud began 11 – 487 ticks after the throw (once it rests), `smokegrenade_detonate` on that tick, `smokegrenade_expired` 22.06 s later |
| `CFlashbangProjectile` | flashbang | 10; `flashbang_detonate` 104 ticks (1.6 s) after the throw |
| `CHEGrenadeProjectile` | HE | 7; `hegrenade_detonate` 104 ticks after the throw |
| `CMolotovProjectile` | molotov and incendiary | 7; the entity ends on the tick its fire starts (`inferno_startburn`) |
| `CDecoyProjectile` | decoy | — |

Each sends, on every tick it moves, its origin (the body component's cell and offset, like a pawn's) and rotation
(`m_angRotation`; it spins about 22° a tick in the air), and once: its model (`m_hModel`: the grenade's world model,
`weapons/models/grenade/<kind>/weapon_<kind>.vmdl`), its thrower (`m_hThrower`, a pawn handle), the throw
(`m_vInitialPosition`, `m_vInitialVelocity`), `m_flGravityScale` (0.4), `m_flElasticity` (0.4375), and the set
detonation time (`m_flDetonateTime`, game seconds; 0 for a smoke, which pops once it rests); `m_nBounces` counts the
bounces. **A flashbang's and an HE grenade's entity stays in the demo for seconds after it went off** (an HE grenade
6.6 s after its throw, unseen in CS2), and a later grenade can reuse the slot (`Entity::serial` tells them apart):
the detonation events (with `entityid` and the position) say when the grenade is gone.

A smoke adds `m_nSmokeEffectTickBegin` (a game tick: when its cloud began), `m_bDidSmokeEffect`,
`m_vSmokeDetonationPos`, `m_vSmokeColor` (the cloud's tint: (75, 127, 155) for the CTs' smokes in this round,
(180, 129, 50) for the Ts') and the cloud's voxels: `m_VoxelFrameData` (an array of bytes, `m_nVoxelFrameDataSize` of
them, 1746 in one sample) and `m_nVoxelUpdate`, a counter of the voxel frames sent (about 200 over a cloud's 22 s,
one every 7 ticks).

### Smoke voxels

The cloud itself is not in the demo. `m_VoxelFrameData` is an append-only log of the **inputs** of a deterministic
simulation that the server and every client run alike, one frame per simulation step (`demo::smoke`):

```text
frame    u16 LE number, u16 LE payload length, payload
payload  u8 fading, u8 flags,
         if flags & 1: u8 n, n × emitter { u8 x, y, z, level; f32 hold }
         if flags & 2: u16 n, n × brick  { u16 index; u64 solid mask }
         u8 n, n × 20-byte hole record
```

The simulation runs on a 32³ grid of 20-unit voxels whose lowest corner is `m_vSmokeDetonationPos` − 320 on each
axis, split into 8³ bricks of 4³ voxels. A brick's index and its mask's bits are Morton codes (x, y, z bits
interleaved, x lowest). **Emitters** are where smoke comes out; a **brick** is world geometry, sent the first time the
simulation needs it; a set mask bit is a voxel that touches a surface (the floor shows as one full layer, walls as
planes, the space under the floor stays clear). The fading flag stops the cloud's growth. The layout comes from public
reverse engineering (5stackgg/demo-parser, AGPL-3.0: facts only, no code; cross-checked by CS2-insight-agent and
memorin.app); a CS2 step and the layout's numbers were checked in our demos:

| What | Seen |
|---|---|
| layout | every snapshot of every smoke's log over rounds 1 – 3 of four demos (de_nuke, de_inferno, de_anubis, a local match) reads to its exact end, frames numbered 0, 1, 2… (`smoke_logs_decode_and_the_simulation_follows_cs2` in `tests/cs2_demo.rs`) |
| frames | 196 – 202 per smoke (the cloud's 22 s), a new one every 7 ticks (0.11 s; the reverse engineering says 0.1 s); fading from frame 169 – 174 |
| emitters | 40 – 44, all in frame 0, level 5, hold 0: a 5 × 5 × 2 lattice 2 voxels apart around the detonation point (points inside geometry left out) plus a few in the middle; the lowest at z 16, the first voxel above the floor |
| bricks | frame 0: exactly the bricks the emitters' bounding box grown by one voxel touches (48 in round 5's first smoke); then one or two at a time as the cloud reaches new space, some even while it fades; 62 – 98 per smoke |
| axis order | with x, y, z as above only 2 of 387 emitters sit in a solid voxel; any other order of the axes puts 38 – 88 there |
| holes | none in any round of four demos (de_nuke, de_inferno, de_mirage, a local match of the current build; `examples/smoke_holes.rs`), although HE grenades go off inside smokes (round 5, tick 34286: 126 units from a smoke's detonation point, and CS2 draws the hole); the 20-byte records are not decoded. CS2 sends nothing for a blast's hole: every client carves it from the detonation (`hegrenade_detonate`), and OFAST does the same (below) |

CS2's simulation itself is not known. OFAST runs its own ([`demo::smoke::Simulation`], decision 017): each voxel
takes smoke from its six neighbours, more from the one above than from the one below (so the cloud sinks), with
weights adding up to 1, so only a constant loss per voxel takes it away and the cloud grows until that loss matches
what the emitters feed; smoke never flows into solid voxels nor into bricks not sent yet. Voxels update in place in
Morton order, as CS2's are reported to: smoke reaches further towards +x, +y and +z in a step than towards −. The bricks are its check: a smoke voxel needs to know whether its neighbours are solid, so OFAST
notes when its simulation first needs each brick and compares with the frame the server sent it. Fitted on round 5
(`examples/smoke_fit.rs`), it needs 39 – 46 % of the bricks sent after frame 0 within 2 frames of CS2 and misses
7 – 14 % (the cloud's thinnest reaches) on the four demos. Its steady cloud has about 766 voxels above 5 % of full
in the open, a hemisphere of 143 units' radius: CS:GO's smoke radius was 144.

An HE grenade going off in or near a smoke blows a hole in it (`Simulation::carve`, `export::smoke::Blast`): after
every step from the blast on, the smoke within 240 units of the detonation is taken out (all of it inside 200, less
towards 240), for 0.5 s, then the hole shrinks to nothing over 4 s and the cloud comes back from its edges and its
emitters. Fitted to CS2 on round 5 (`examples/smoke_hole_fit.rs`, CS2 references `cs2_tick34282…34738_*_he222_*`):
before the blast a doorway is full of the smoke; 33 ticks after it CS2 sees through to the room's far wall (with 240
units every one of five sight lines through the doorway is clear in OFAST's cloud, with 200 two of them are not); 2 s
after, CS2's cloud is still gone from that side of the room, as OFAST's; 5 s after, both are back at full size. Carved
once, the emitters inside the hole would fill it again within a step: CS2's stays open, so the carve runs every step.

A molotov's or an incendiary's fire is a `CInferno` (`TickSample::fires`, `FireSample`): `m_fireCount` flames of 64
slots, each with its world position (`m_firePositions`), the flame it spread from (`m_fireParentPositions`), the
surface normal (`m_BurnNormal`) and `m_bFireIsBurning`; `m_nFireEffectTickBegin` (a game tick), `m_nFireLifetime`
(7 s for a molotov, 5.5 s for an incendiary) and `m_nInfernoType` (0 molotov, 1 incendiary). The flames spread a few
ticks apart over the first 2 s (16 at most in this round); `inferno_expire` ends the fire — after its lifetime, or
early when a smoke puts it out (one burned 1.4 s) — although its entity stays longer. Round 5 has 33 grenades and 7
fires; `examples/grenade_timeline.rs` lists them next to the grenade events. Tournament demos carry no
`bullet_impact`: shots are only `weapon_fire` (tick, player, weapon, silenced).

The flames burn on the floor: 105 of round 5's 107 have the normal (0, 0, 1), one a slope, one a wall. What climbs
walls in CS2 is drawn, not networked: `client.dll`'s `C_Inferno` holds point sets it builds itself from the flames
by tracing against the world (`m_hInfernoPointsSnapshot`, `…FillerPointsSnapshot`, `…OutlinePointsSnapshot`,
`…ClimbingOutlinePointsSnapshot`, `…DecalsSnapshot`; a note in a particle system: "Fraction from ray trace to
wall"), and `particles/inferno_fx/molotov_groundfire.vpcf` places its wall flames, outline flames and filler from
them through control points. The demo holds none of these points, so OFAST traces its own (`export::effects`, the
agents manifest's `fires`). The filler projects scorch under the burning fire; when the fire goes out the client
plays `molotov_groundfire_remnant.vpcf`: soot decals and 5 s of near-black wisps; a burning fire has no smoke
system.

A flashbang's blindings are the pawn's `m_flFlashDuration` (seconds) and `m_flFlashMaxAlpha` (0 – 255); the Aurora vs
FURIA demo has no `player_blind` events. The server sets them on the tick the flashbang goes off (every blinded player
at once: 4 players on tick 34353, matching `flashbang_detonate`) and clears the duration to 0 some time later, at no
tick that means anything (0.97 s after a 4.62 s blinding, 4.4 s after a 3.13 s one). So a blinding is a tick where a living
pawn's duration changes to a new value above 0, a value seen first or after the pawn's gap being left from before
(`demo::players::flash_starts`: the replay's `blindings` and the animation's flinch); in round 5, 21 blindings of
0.15 – 4.83 s, all at alpha 255. How CS2 draws them: [replay.md](replay.md#in-unreal-import_demopy) ("Flashed views").

## Shots

A demo records no bullet impacts on the world, and no `bullet_impact` events. For every gun shot the server sends
each client a **fire-bullets message**, packet message 452 (Valve's `CMsgTEFireBullets`, `GE_FireBulletsId`), with
the shooter's aim and a random seed; every client then traces the bullets itself to draw tracers, impacts and bullet
holes. OFAST bundles no definition of this message: its fields were read from the demos (`demo_inspect … raw 452`)
and checked against the shots' `weapon_fire` events (same ticks and shooters: 1,884 messages against 2,263
`weapon_fire` on the de_nuke pro demo, whose extra events are knives and grenades). `demo::shots` decodes it with a
hand-written prost message:

| Field | What |
|---|---|
| 1 `origin` | the shooter's eyes (feet + 64 standing): where the bullets start |
| 2 `angles` | pitch, yaw, roll (degrees): where the bullets go before the spread, the recoil's aim punch included |
| 3 `weapon` | the weapon's entity handle |
| 4 `mode` | the fire mode: 1 the secondary (a silenced USP-S or M4A1-S, a burst Glock or FAMAS) |
| 5 `seed` | the spread's random seed |
| 6 `player` | the shooter's pawn handle (fixed32; its low 14 bits the pawn's entity index) |
| 7 `inaccuracy`, 9 `spread` | the shot's inaccuracy (moving, jumping, firing fast) and the weapon's own spread, radians |
| 8 `recoil_index` | shots into the spray, fractional |
| 11 `item_def_index` | the weapon's item (7 AK-47, 4 Glock-18, 61 USP-S…) |
| 13 | the shooter's feet; 14: the bullets in the magazine as the shot is fired (30 on a full AK-47's first shot of a spray, 29 on the next) |
| 16 | when it was fired (local demos only; tournament demos leave it out): 2 the attack's game tick, 3 the fraction of the tick interval at which the player clicked (CS2's sub-tick input), 4 and 5 the tick and fraction the shooter's screen showed then, 1 the aim punch, 6 and 7 the inaccuracy from moving and from being in the air; the same as the `bullet_damage` event's `attack_tick_count` … keys |
| 19 | the shot's server tick: the message's demo tick plus a fixed offset on 97 % of shots (2402 on the local match demo, 56104 on the de_nuke pro demo), the rest arriving a tick early |

The rest (10 a sound type, 12 a sound effect, 15 an attack type) is not read. **When** a shot was fired
(`demo::shots::shot_times`): a tick's commands are the input of the tick interval before it, so the shot is at the
attack tick − 1 + its fraction, in demo ticks by the offset most messages have; without the fraction (tournament
demos) at the start of that interval. Checked in CS2: YEKINDAR's tracer is already in flight on the very tick of his
shot's message (de_nuke pro demo, round 5, tick 37683), so the shot is before it. A bullet's direction
(`demo::shots::spread_offset`, `bullet_direction`): Valve's uniform random stream (Park–Miller with a 32-entry
Bays–Durham shuffle: Numerical Recipes' `ran1`, `demo::shots::UniformRandom`), seeded with the shot's seed + 1 + the
bullet's number, draws a fraction of the inaccuracy, its angle, the spread's angle and a fraction of the spread; the
two offsets add up as tangents along the aim's right and up vectors (Source's `AngleVectors`).

Two kinds of message show where bullets really went, and checked the rule (`examples/shot_spread.rs`,
`examples/shot_traces.rs`):

- **Glass a bullet breaks** (message 423) holds the point the bullet met the pane (field 11) and its direction (12).
  On the de_nuke pro demo 82 of 83 panes lie on the path OFAST rebuilds to 0.001 milliradian (the other is another
  bullet's); on the local match demo all 17. Other seeds or orders of the draws miss by 1 – 100 milliradians (the
  CS:GO-era order, the spread's fraction before its angle, misses by 1.2 on average, most where the spread is
  largest).
- **A hit on a player** is an effect dispatch (message 400, `CMsgEffectData`): field 1 where the bullet met the
  player, 2 where that stretch of its flight started: the shooter's eyes, equal to the shot's `origin` to the last
  bit, or, for a bullet that went through a wall or another player first, **where it came out of it** (on the line,
  past the eyes). All 302 hits from the eyes of the pro demo lie on their bullet's line to a thousandth of an inch,
  and 423 of the local demo's 424. Of the 545 hits starting past the eyes on 13 demos of 6 maps, 456 start within
  half an inch of where OFAST's trace comes out of a wall (`examples/shot_penetration.rs`), 81 where the trace
  meets nothing (a prop or door it does not have, or a player: a later hit starting 3 – 9 inches past another's), 8
  elsewhere. Other effects dispatched on a bullet's line start where they end.
- **The damage a hit did** (`player_hurt`'s `dmg_health`, `dmg_armor`, `hitgroup`; on local demos `bullet_damage`'s
  `num_penetrations` and `distance`, from where the stretch started) checks the penetration rule
  ([replay.md](replay.md#agents-manifest-demo_rroundagentsjson), `shots`): without walls 2,860 non-lethal hits
  of those 13 demos are exactly the weapon's damage after its range falloff, hit group and armour (most of the 42
  others hit a teammate: CS2 cuts friendly bullets to a third), through walls 289 of 314.

Shotguns follow another rule, not found yet: their pellets (the first one too) lie on none of the paths the other
guns' rule gives (seed + 1 + the pellet), nor on those of 11 rules built on the fixed `m_nSpreadSeed` every shotgun
has in `scripts/weapons.vdata` and the other guns leave at 0 (XM1014 817955, MAG-7 19899236, Sawed-Off 9571223, Nova
17514: a fixed pattern, as CS:GO's `weapon_accuracy_shotgun_spread_patterns` made it), within 1 milliradian
(`examples/shot_pellets.rs`). The 13 demos hold only 4 shotgun shots with hits (23 pellets seen, 2 – 32 mrad off the
nearest predicted). Not checked either: the R8 Revolver's and the Negev's own rules. Where the bullets stop is traced
through the map's collision
([replay.md](replay.md#agents-manifest-demo_rroundagentsjson), `shots`).

## Models

What each player looks like and carries, read per tick into `PlayerSample::appearance` and `PlayerSample::weapons`
(`crates/ofast-core/src/demo/players.rs`), leniently like the rig inputs. Entities name models by **resource id**,
not by path: `m_hModel` (a `CStrongHandle<InfoForResourceTypeCModel>`, sent as an unsigned varint) is MurmurHash64B
of the `.vmdl` path with seed `0xEDABCDEF` — the id resources store next to the paths they refer to
([resource.md](resource.md#external-references-rerl)). `Content::resource_paths("vmdl")` hashes every model path in
the archives once (13,105 in `pak01`) and resolves them.

| What | Field | Notes |
|---|---|---|
| agent | pawn `CBodyComponent` → `m_skeletonInstance.m_modelState.m_hModel` | e.g. `agents/models/ctm_sas/ctm_sas.vmdl` |
| agent mesh groups | same, `m_MeshGroupMask` (`uint64`, bit per mesh group of the model) | SAS: 5 = defuse kit hidden + third-person body, 6 = defuse kit shown + body; Phoenix: 1; a glove skin adds `thirdperson_hide_default_gloves` |
| gloves | pawn `m_EconGloves.m_iItemDefinitionIndex` | 0 = the agent's own; else an item whose `model_world` in `items_game.txt` is the glove model (`agents/models/shared/arms/glove_*`); every player of the local match demo has 0 |
| weapons carried | `m_pWeaponServices` → `m_hMyWeapons` (array of handles: length at the array's path, handles one level deeper) | every weapon the player has, the one in hand included; two flashbangs are one entity (never two in five rounds checked) |
| ammo counts | `m_pWeaponServices` → `m_iAmmo[32]`, by ammo type | observed from the grenades carried: 13 HE, 14 flashbangs (2 with one flashbang entity), 15 smoke, 16 molotov or incendiary; OFAST reads the flashbang count |
| weapon model | each weapon's `CBodyComponent` → `m_skeletonInstance.m_modelState.m_hModel` | the item's own world model: a knife skin's blade (`weapon_knife_bowie.vmdl`, `weapon_knife_tactical.vmdl` seen), `weapon_rif_ak47.vmdl`… |
| weapon mesh groups | the weapon's `m_MeshGroupMask` | guns: 1 = `body_hd`, 2 = `body_legacy` (the CS:GO-era mesh some skins are painted for; 2 of 4 AK-47s and M4A4s in round 5); knives and grenades send every bit (the model's default is meant) |

**Checked** (`models_resolve_by_resource_id` in `tests/cs2_demo.rs`, 2026-10-22): over round 5, every living
player's model (33,671 samples) resolves to an agent and every carried weapon (129,526) to a weapon model; the
weapon in hand is always one of those carried; at tick 26508 "vg" holds the CT knife with an M4A4 carried, as CS2
shows him (rifle slung, `docs/formats/animation.md`). The resource id matches all 4,320 references of the sampled
resources (`resource_ids_match_references` in `tests/cs2_kv3.rs`). `examples/demo_models.rs` lists every entity's
model at a tick, `examples/round_weapons.rs` what each player holds and carries.

## Where the references disagree

| Topic | demoparser2 | demoinfocs-golang | OFAST (checked how) |
|---|---|---|---|
| fixed-size string table values | reads `user_data_size` bits | same | `user_data_size_bits` — the other choice desyncs `decalprecache` in real demos |
| string table index skip | previous + varint + 1 | previous + varint + 2 | + 2 (a skip of 0 would otherwise repeat "+1") |
| `has_value` bit | only read when `has_key` | always | always |
| full packets when reading linearly | entities skipped | first snapshot applied | first applied — the demo's only complete snapshot |
| quantized float redundancy check | truncates | rounds to nearest | rounds (documented real-world desync) |
| normal component | `len × (1/2048 − 1)` (precedence slip) | `len / 2047` | `/ 2047` |
| `m_flSimulationTime` scale | 1/30 | 1/64 | 1/64 (CS2 tick) |
| polymorphic pointer | always reads the type ubitvar | only if present | only if present |
| entity "leaves the update set" (`01`) | deletes the entity | keeps it | keeps it (it was not deleted: `11` is delete) |
| `m_iClip1` raw 0 | 0 | wraps to `u32::MAX` | −1 ("no clip"); the wire value is `ammo + 1` |
| `GameTick_t` (`m_nLastJumpTick`…) | unsigned varint: twice the tick, always even (0.42.0) | — | signed (zigzag) varint, like `int32`; halved values meet the jump in the positions one tick later, every time |
| same member under two send nodes (`m_vecViewOffset.m_vecZ`, `m_vecVelocity.m_vecZ`) | one prop `CCSPlayerPawn.m_vecZ` holding whichever changed last, so it cannot export the view offset (0.42.0) | renames clashing fields to `send_node.member` | always `send_node.member`; the view offset is checked against CS2 itself instead |
| shrinking variable arrays | — | clears elements past the end | same |
| string table problems | fatal | updates to unknown tables ignored; only a few tables decoded | fatal only for `instancebaseline`; otherwise a warning (`DemoParser::warnings()`) |

## Safety limits

A demo is untrusted input: a broken or hostile file must produce an error, never a crash or a multi-gigabyte
allocation. Limits, each far above what a real CS2 match uses (measured on the demo above):

| Limit | Value | Real match |
|---|---|---|
| Snappy output vs. input | ≤ 22× (the format's maximum ratio is 21.3×) | — |
| fields per entity / in total | 65,536 / 4,194,304 | 3,215 / 55,967 |
| fixed array length | 16,384 | 128 |
| string tables / entries per table | 64 / 65,536 | 12 / 72 |
| class ids | < 65,536 | 218 |
| template nesting in type strings | 8 | ≤ 3 |

## Not implemented yet

User commands, `userinfo` decoding, animation data frames, alternate
baselines (`"12:3"` keys), seeking via full packets, the `coord_size_params` overrides in the serializer message
(absent in the CS2 demo checked, so the bit sizes above apply).

## Sources

- demoparser2 (MIT, Rust): `src/parser/src/first_pass/*`, `second_pass/*` — https://github.com/LaihoE/demoparser
- demoinfocs-golang (MIT, Go): `pkg/demoinfocs/sendtables/*`, `stringtables.go`, `s2_commands.go` —
  https://github.com/markus-wa/demoinfocs-golang
- Valve protobufs via SteamDatabase GameTracking-CS2 — https://github.com/SteamDatabase/GameTracking-CS2
- Oracle used in tests: demoparser2 0.42.0 through `tools/oracle/demoparser2_oracle.py`.
- Esoterica (MIT, C++): `Code/Engine/Animation/TaskSystem/Animation_TaskSerializer.*`, `Animation_BoneMaskTask.cpp`,
  `Code/Base/Serialization/BitSerialization.h` — the pose recipe's encoding — https://github.com/BobbyAnguelov/Esoterica
