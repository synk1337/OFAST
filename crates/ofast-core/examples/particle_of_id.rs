//! Developer tool: name the particle systems behind resource ids, as a demo's effect dispatches send them (field 11 of
//! `CMsgEffectData`, the 8 bytes little-endian: [`ofast_core::resource::resource_id`] of the system's path).
//!
//! ```bash
//! cargo run --release --example particle_of_id -- 0xDCD6E40E793F9B75 [more ids]
//! ```
//!
//! Every `.vpcf_c` of CS2's main archive is hashed and the ids found are printed with their paths.

use std::collections::HashMap;

use ofast_core::game::Cs2Install;
use ofast_core::resource::resource_id;
use ofast_core::vpk::Vpk;

fn main() {
    let wanted: Vec<u64> = std::env::args()
        .skip(1)
        .map(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).expect("an id in hexadecimal"))
        .collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.csgo_dir().join("pak01_dir.vpk")).expect("VPK opens");
    let mut by_id = HashMap::new();
    for entry in vpk.entries() {
        if let Some(path) = entry.path().strip_suffix(".vpcf_c") {
            let path = format!("{path}.vpcf");
            by_id.insert(resource_id(&path), path);
        }
    }
    for id in wanted {
        match by_id.get(&id) {
            Some(path) => println!("{id:#018x}  {path}"),
            None => println!("{id:#018x}  (no particle system of the archive)"),
        }
    }
}
