//! Developer tool: copy one file out of CS2's archives as it is stored (`scripts/items/items_game.txt`, a compiled
//! resource for another tool), to look at it. What it writes is Valve's content: keep it out of the repository.
//!
//! ```bash
//! cargo run --release --example vpk_extract -- scripts/items/items_game.txt test-data/items_game.txt
//! ```
//!
//! Set `OFAST_MAP=de_mirage` to look in that map's archive too.

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, out] = args.as_slice() else {
        panic!("usage: vpk_extract <archive path> <file to write>");
    };
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = match std::env::var("OFAST_MAP") {
        Ok(map) => Content::for_map(&install, &map),
        Err(_) => Content::game(&install),
    }
    .expect("archives open");
    let data = content
        .archives()
        .iter()
        .find_map(|vpk| vpk.read_path(path).ok())
        .unwrap_or_else(|| panic!("{path} is in none of the archives"));
    std::fs::write(out, &data).expect("file written");
    println!("{path}: {} bytes to {out}", data.len());
}
