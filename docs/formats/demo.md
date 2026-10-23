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
checked in the game).

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
96 units of the body (124 stretches of weapons lying in the world in all). In Unreal (`import_demo.py`, round 5)
KSCERATO's AK-47 lies flat on its side on the floor where the demo has it, 2 m past his body; a CS2 reference
of the same frames is still to take.

## Doors

A map's rotating doors (`prop_door_rotating`) are `CPropDoorRotating` entities, created again every round
(`TickSample::doors`, `DoorSample`): `m_closedPosition` and `m_closedAngles` (the map entity's origin and angles: its
hinge and closed rotation), `m_eDoorState` (0 closed, 1 opening, 2 open, 3 closing), the body component's origin and
`m_angRotation` (sent on the ticks it swings), `m_hMaster` (the other half of a double door), the model. A de_nuke door
opens 89° (the map's `distance`) in 28 ticks (0.44 s) and closes the same way; both halves of a double door swing
together, one of them through ±180° of yaw.

Seen (`examples/entity_probe.rs`, which prints any entity's fields by name when they change): on the Aurora vs FURIA
de_nuke demo, round 4 opens and closes a double door; round 5 opens none. Every door entity leaves the demo at the
round's restart, 320 ticks before the next freeze time, and one left mid-round with nothing changed on it before
(out of the demo's view?): the replay keeps each door's last rotation.

The A-site door (hinge 257, −1310, −416) closes the doorway players cross: on the local match demo (current build)
they cross it only while it stands open (yaw −89°), and in round 3, closed all round, nobody does. The Aurora vs FURIA
demo (an older build: CS2 says "old demo") disagrees: its A door is networked closed at its closed angles all round 5,
like the others, yet players walk through the doorway 10 times, and CS2 playing that demo draws the doorway clear. So
on that demo Unreal shows a closed A door CS2 does not (`compare_doors_a.png`); not explained yet.

**Checked** (`doors_swing_open_and_shut` in `tests/cs2_demo.rs`): over the first three rounds of both test demos,
every closed door sample (65,473 and 59,015) stands at its closed angles, every open one (8,119 and 9,022) has swung
60 – 120° about z, and no door leaves its hinge.

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

Grenades and their trajectories, user commands, `userinfo` decoding, animation data frames, alternate
baselines (`"12:3"` keys), seeking via full packets, the `coord_size_params` overrides in the serializer message
(absent in the CS2 demo checked, so the bit sizes above apply).

## Sources

- demoparser2 (MIT, Rust): `src/parser/src/first_pass/*`, `second_pass/*` — https://github.com/LaihoE/demoparser
- demoinfocs-golang (MIT, Go): `pkg/demoinfocs/sendtables/*`, `stringtables.go`, `s2_commands.go` —
  https://github.com/markus-wa/demoinfocs-golang
- Valve protobufs via SteamDatabase GameTracking-CS2 — https://github.com/SteamDatabase/GameTracking-CS2
- Oracle used in tests: demoparser2 0.42.0 through `tools/oracle/demoparser2_oracle.py`.
