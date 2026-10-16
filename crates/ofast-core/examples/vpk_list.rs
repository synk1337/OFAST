//! Developer tool: list the files of any VPK (e.g. a map's 3D skybox prefab), optionally filtered.
//!
//! ```bash
//! cargo run --release --example vpk_list -- "C:/…/game/csgo/maps/prefabs/de_mirage/3dskybox_mirage_legacy.vpk" vwrld
//! ```

use ofast_core::vpk::Vpk;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vpk = Vpk::open(&args[0]).expect("VPK opens");
    let filter = args.get(1).map(String::as_str).unwrap_or("");
    let mut counts = std::collections::BTreeMap::new();
    for entry in vpk.entries() {
        let ext = entry.path().rsplit('.').next().unwrap_or("").to_owned();
        *counts.entry(ext).or_insert(0usize) += 1;
        if !filter.is_empty() && entry.path().contains(filter) {
            println!("{:>10}  {}", entry.size(), entry.path());
        }
    }
    println!("{} files: {counts:?}", vpk.len());
}
