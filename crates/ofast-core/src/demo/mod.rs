//! CS2 demo (`.dem`) reader.
//!
//! A CS2 demo is a recording of everything the server sent to a spectator (SourceTV): every entity's networked
//! fields, tick by tick. Reading it is a stack of layers, one module each:
//!
//! | Layer | Module | What it does |
//! |---|---|---|
//! | file | [`frame`] | 16-byte header, then frames: command + tick + (Snappy-compressed) protobuf payload |
//! | protobuf | [`proto`] | Valve's message definitions, compiled at build time from `proto/*.proto` |
//! | bits | [`bits`] | LSB-first bit reader with Valve's variable-length and fixed-point encodings |
//! | packets | [`parser`] | network messages inside packet frames; keeps the game state |
//! | string tables | [`stringtable`] | entity baselines, player info |
//! | send tables | [`serializer`] | every class's fields and how each is encoded |
//! | values | [`decoder`] | quantized floats, coords, angles, vectors… |
//! | field paths | [`fieldpath`] | Huffman-coded list of which fields an update touches |
//! | entities | [`entity`] | create / update / delete entities from `svc_PacketEntities` |
//! | events | [`events`] | game events (deaths, round ends, bomb…) with named keys |
//! | players | [`players`] | per-tick player positions and eye angles for one round |
//! | summary | [`summary`] | a whole demo's rounds, scores, kill feed and highlights, in one pass |
//! | grenades | [`grenades`] | thrown grenades and the fires they light, per tick |
//! | smokes | [`smoke`] | a smoke's voxel log decoded, and its cloud simulated |
//! | shots | [`shots`] | each gun shot's fire-bullets message, and its bullets' directions from its seed |
//! | recoil | [`recoil`] | the aim punch a gun's shots build, rebuilt from its recoil keys |
//! | skins | [`skins`] | a weapon's finish (paint kit, seed, wear, stickers…) from its econ attributes |
//!
//! The full format description, with sources, is in `docs/formats/demo.md`.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let data = std::fs::read("match.dem")?;
//! let header = ofast_core::demo::read_header(&data)?;
//! println!("map: {}", header.map_name());
//!
//! let round = ofast_core::demo::players::track_round(&data, 1)?;
//! for sample in round.samples.iter().take(3) {
//!     for p in &sample.players {
//!         println!("tick {} {}: {:?} pitch {} yaw {}", sample.tick, p.name, p.position, p.pitch, p.yaw);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Knowledge sources (studied, not copied): demoparser2 and demoinfocs-golang (both MIT) — see `CREDITS.md`.

pub mod bits;
pub mod decoder;
pub mod entity;
pub mod events;
pub mod fieldpath;
pub mod frame;
pub mod grenades;
pub mod parser;
pub mod players;
pub mod proto;
pub mod recipe;
pub mod recoil;
pub mod serializer;
pub mod shots;
pub mod skins;
pub mod smoke;
pub mod stringtable;
pub mod summary;

pub use parser::{DemoError, DemoParser, FrameInfo, read_header};
