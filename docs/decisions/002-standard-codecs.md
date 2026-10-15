# 002 — Valve formats are ours; industry-standard codecs use libraries

- **Date:** 2026-10-15
- **Status:** accepted

## Context

Decision 001 says OFAST rewrites the community's Source tools instead of reusing them. Reading CS2 content also
touches many formats that are *not* Valve's: PNG, glTF JSON, LZ4, ZSTD, protobuf, MD5/BLAKE3/CRC32, and the BC1–BC7
GPU texture compression formats. Rewriting those would add a lot of code without making OFAST better at its job.

## Decision

- **Valve- and Source-specific formats are written by OFAST**: VPK, resource container, KeyValues3, demos, models,
  textures (headers, mips, conversions), and the meshoptimizer codecs as used by CS2 (small, and central to every
  model, so we own the decoder and test it against meshoptimizer's vectors).
- **Industry-standard codecs use established pure-Rust libraries**, chosen for permissive licenses and no C build
  step: `bcdec_rs` (BC1–BC7), `png`, `serde_json`, `lz4_flex`, `ruzstd`, `prost`, `md-5`, `blake3`, `crc32fast`,
  `snap`. Each is listed with its purpose in `crates/ofast-core/Cargo.toml`.

## Alternatives considered

- **Write BCn/PNG/ZSTD ourselves.** Thousands of lines of well-understood code with real bug risk, and no gain for
  fragmovie makers.
- **Use meshoptimizer's C++ library through bindings.** Adds a C++ build step and a dependency at the heart of model
  loading; the decoders are ~400 lines in Rust.

## Consequences

- Every library must stay pure Rust and permissively licensed (MIT/Apache/BSD/zlib), so CI and Windows builds stay
  simple and OFAST stays MIT.
- When a standard library lacks something CS2 needs, we extend around it (e.g. CS2's texture conversions live in
  OFAST's texture module, on top of `bcdec_rs`).
