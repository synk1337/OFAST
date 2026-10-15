# Valve protobuf definitions

These `.proto` files describe the protobuf messages inside CS2 demos. They are **Valve's own interface
definitions**, copied unchanged from SteamDatabase's
[GameTracking-CS2](https://github.com/SteamDatabase/GameTracking-CS2/tree/master/Protobufs) (which dumps them from
the game binaries on every update). OFAST does not write them; it only compiles them.

Copied from GameTracking-CS2 commit `34d3b4798accc0402d796f1f33159fcc3e196fe2` (2026-10-15).

| File | What OFAST uses from it |
|---|---|
| `demo.proto` | demo frame commands (`EDemoCommands`) and their payloads (`CDemoFileHeader`, `CDemoPacket`, …) |
| `netmessages.proto` | network messages inside packets: string tables, `CSVCMsg_PacketEntities`, `CSVCMsg_FlattenedSerializer` |
| `networkbasetypes.proto` | `NET_Messages` ids, `CNETMsg_Tick`; imported by `netmessages.proto` |
| `gameevents.proto` | game event message ids (rounds, kills — used from M3) |
| `network_connection.proto`, `valveextensions.proto`, `source2_steam_stats.proto` | only imported by the files above |

## How they become Rust

`crates/ofast-core/build.rs` compiles them on every build with [protox](https://crates.io/crates/protox) (a
protobuf compiler written in Rust, so no `protoc` install is needed) and [prost-build](https://crates.io/crates/prost-build)
(Rust code generation). The generated code lands in Cargo's `OUT_DIR` and is included by
`crates/ofast-core/src/demo/proto.rs`. Nothing generated is committed.

## Updating after a CS2 patch

1. Copy the same files from a newer GameTracking-CS2 commit over these ones.
2. Update the commit hash above.
3. `cargo test` (synthetic) and `cargo test --release --features game-data` (real demo). Valve only ever adds
   fields, so old demos keep parsing.
