# 002 — Demo protobufs: Valve's `.proto` files compiled at build time with protox + prost

- **Date:** 2026-10-15
- **Status:** accepted

## Context

Almost everything inside a CS2 demo is a protobuf message (`CDemoFileHeader`, `CSVCMsg_PacketEntities`,
`CSVCMsg_FlattenedSerializer`, …). Valve does not publish the definitions, but SteamDatabase's GameTracking-CS2
dumps them from the game on every update. They change when CS2 updates (Valve only adds fields), so updating them
must be easy.

The usual Rust route, `prost-build`, needs Google's `protoc` compiler installed. Valve's files also do not compile
as-is with any standard compiler: they use options defined only in Valve-internal files
(`option additional_includes = "…";` and bare field options such as `boxed_type = "CEntityIndex"`,
`synthetic_default = "-1"`).

## Decision

- Copy the needed files unchanged into `crates/ofast-core/proto/` (with the source commit in its README).
- `build.rs` compiles them on every build with **protox** (a protobuf compiler written in Rust) and generates Rust
  types with **prost-build** into `OUT_DIR`. Nothing generated is committed.
- A small file resolver in `build.rs` drops the Valve-internal options while reading. They only steer Valve's C++
  code generator and have no effect on the wire format.

## Alternatives considered

- **protoc + prost-build** — every contributor and CI runner would need protoc (or a vendored binary crate that
  downloads one). More setup for beginners, for no gain.
- **Commit the generated Rust code** — no build step, but a CS2 update then means re-running a separate tool, and
  the diff of generated code is noise in review.
- **Edit Valve's files to remove the options** — every update would need the same manual edit again.
- **Hand-written decoders for the few messages we need** — the messages are big and change with CS2 updates; the
  generated code tracks them for free.

## Consequences

- `cargo build` works on a fresh machine and in CI with nothing but Rust installed.
- Updating after a CS2 patch = copy newer files, update the commit hash, run the tests (see `proto/README.md`).
- If Valve adds another internal option, `build.rs` fails with "option … is not defined" and its option filter needs
  one more entry (bare options are already handled generically; only new file-level options need a line).
- Builds compile ~1,700 lines of protobuf definitions each time `proto/` changes (a few seconds, cached otherwise).
