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
//! | [`resource`] | Compiled resource container (`*_c` files): header + typed blocks | `docs/formats/resource.md` |
//! | [`kv3`] | KeyValues3, binary and text — the data format inside most resources | `docs/formats/kv3.md` |
//! | [`demo`] | CS2 demos (`.dem`): frames, string tables, send tables, entities, player positions per round | `docs/formats/demo.md` |
//! | [`game`] | Locating a CS2 install on disk | — |
//! | [`items`] | Item definitions (`scripts/items/items_game.txt`, KeyValues 1): weapon names by index | — |
//! | [`anim`] | Animation clips, skeletons, CS2's locomotion graph, each player's baked animation | `docs/formats/animation.md` |
//! | [`bytes`] | Shared little-endian byte cursor (and varints) used by every binary reader | — |
//!
//! More modules (textures, meshes, materials, maps…) arrive milestone by milestone — see
//! `docs/roadmap.md`.
//!
//! ## Where the knowledge comes from
//!
//! OFAST is a clean rewrite. The formats were learned from public community work — mainly ValveResourceFormat and
//! ValvePak (MIT) for assets, demoparser2 and demoinfocs-golang (MIT) for demos, and the Valve Developer Community
//! wiki — and our output is checked against those tools in tests.
//! See `CREDITS.md`.

#![forbid(unsafe_code)]

pub mod anim;
pub mod bytes;
pub mod content;
pub mod coords;
pub mod demo;
pub mod export;
pub mod game;
pub mod items;
pub mod kv3;
pub mod map;
pub mod meshopt;
pub mod model;
pub mod resource;
pub mod texture;
pub mod vpk;
