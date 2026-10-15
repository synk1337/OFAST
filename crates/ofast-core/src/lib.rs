//! # ofast-core
//!
//! The heart of OFAST: everything that understands Counter-Strike 2 files lives here, with no UI and no game
//! hooking. The desktop app, the CLI, the capture DLL and the Unreal plugin all call into this crate, so they can
//! never disagree about how a file is read or how a coordinate is converted.
//!
//! ## Modules
//!
//! | Module | What it reads | Format notes |
//! |---|---|---|
//! | [`vpk`] | Valve Pak archives (`pak01_dir.vpk` + `pak01_NNN.vpk`) | `docs/formats/vpk.md` |
//! | [`game`] | Locating a CS2 install on disk | — |
//! | [`bytes`] | Shared little-endian byte cursor used by every binary reader | — |
//!
//! More modules (KeyValues3, resource container, textures, meshes, demos…) arrive milestone by milestone — see
//! `docs/roadmap.md`.
//!
//! ## Where the knowledge comes from
//!
//! OFAST is a clean rewrite. The formats were learned from public community work — mainly ValveResourceFormat and
//! ValvePak (MIT) and the Valve Developer Community wiki — and our output is checked against those tools in tests.
//! See `CREDITS.md`.

#![forbid(unsafe_code)]

pub mod bytes;
pub mod game;
pub mod vpk;
