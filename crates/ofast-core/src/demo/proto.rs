//! Rust types for Valve's demo protobuf messages, generated at build time from `proto/*.proto`.
//!
//! See `crates/ofast-core/proto/README.md` for where the definitions come from and `build.rs` for how they are
//! compiled. The generated code has no doc comments of its own (Valve's files have none), so the docs lint is off
//! for this module only.

#![allow(missing_docs, clippy::all)]

include!(concat!(env!("OUT_DIR"), "/valve.rs"));
