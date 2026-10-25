//! Developer tool: find which of CS2's files mention a text (a particle system, a model…), looking inside compiled
//! resources: their KV3 blocks are compressed, so a plain search of the archive's bytes misses them.
//!
//! ```bash
//! cargo run --release --example vpk_grep -- weapons/models/ak47 shell_casing
//! ```
//!
//! Every file whose path contains the first argument is read; a compiled resource's KV3 blocks are decoded and
//! written as text, any other file is taken as text; the lines holding the second argument are printed with their
//! file. Set `OFAST_MAP=de_mirage` to search that map's archive too.

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::{binary, text};
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (filter, needle) = match args.as_slice() {
        [filter, needle, ..] => (filter.as_str(), needle.as_str()),
        _ => panic!("usage: vpk_grep <path substring> <text>"),
    };
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = match std::env::var("OFAST_MAP") {
        Ok(map) => Content::for_map(&install, &map),
        Err(_) => Content::game(&install),
    }
    .expect("archives open");
    let (mut files, mut found) = (0, 0);
    for vpk in content.archives() {
        for entry in vpk.entries().iter().filter(|e| e.path().contains(filter)) {
            let Ok(data) = vpk.read_path(entry.path()) else {
                continue;
            };
            files += 1;
            let texts: Vec<String> = match Resource::parse(&data) {
                Ok(resource) => resource
                    .blocks()
                    .iter()
                    .map(|block| resource.block_data(block))
                    .filter(|bytes| binary::is_kv3(bytes))
                    .filter_map(|bytes| binary::decode(bytes).ok())
                    .map(|file| text::to_string(&file))
                    .collect(),
                Err(_) => vec![String::from_utf8_lossy(&data).into_owned()],
            };
            let lines: Vec<&str> = texts
                .iter()
                .flat_map(|t| t.lines())
                .filter(|l| l.contains(needle))
                .collect();
            if !lines.is_empty() {
                found += 1;
                println!("{}", entry.path());
                for line in lines.iter().take(8) {
                    println!("    {}", line.trim());
                }
            }
        }
    }
    println!("{found} of {files} files mention \"{needle}\"");
}
