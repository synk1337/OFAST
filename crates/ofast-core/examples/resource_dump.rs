//! Developer tool: list files in CS2's VPK, or dump a compiled resource's blocks with KV3 blocks as text.
//!
//! ```bash
//! cargo run --release --example resource_dump -- find ak47          # list paths containing "ak47"
//! cargo run --release --example resource_dump -- weapons/models/ak47/weapon_rif_ak47.vmdl_c [max_lines]
//! ```
//!
//! KV3 blocks are printed with the text writer (truncated to `max_lines` lines per block, default 200), binary
//! blocks as their size plus the first bytes in hex. Set `OFAST_MAP=de_mirage` to also search that map's archive.

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::{binary, text};
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = match std::env::var("OFAST_MAP") {
        Ok(map) => Content::for_map(&install, &map),
        Err(_) => Content::game(&install),
    }
    .expect("archives open");

    if args.first().map(String::as_str) == Some("find") {
        let needle = args.get(1).expect("usage: find <substring>");
        for vpk in content.archives() {
            for entry in vpk
                .entries()
                .iter()
                .filter(|e| e.path().contains(needle.as_str()))
            {
                println!("{:>10}  {}", entry.size(), entry.path());
            }
        }
        return;
    }

    let path = args
        .first()
        .expect("usage: resource_dump <path> [max_lines] | find <substring>");
    let max_lines: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200);
    let data = content.read(path).expect("file reads");
    let resource = Resource::parse(&data).expect("resource parses");
    println!(
        "{path}: {} bytes, header file size {}, version {}",
        data.len(),
        resource.file_size,
        resource.version
    );

    for (index, block) in resource.blocks().iter().enumerate() {
        let bytes = resource.block_data(block);
        println!(
            "\n=== block #{index} {} @ {} ({} bytes)",
            block.kind, block.offset, block.size
        );
        if binary::is_kv3(bytes) {
            match binary::decode(bytes) {
                Ok(file) => {
                    let rendered = text::to_string(&file);
                    let total = rendered.lines().count();
                    for line in rendered.lines().take(max_lines) {
                        println!("{line}");
                    }
                    if total > max_lines {
                        println!("... ({} more lines)", total - max_lines);
                    }
                }
                Err(e) => println!("KV3 error: {e}"),
            }
        } else {
            let head: Vec<String> = bytes.iter().take(64).map(|b| format!("{b:02X}")).collect();
            println!("binary: {}", head.join(" "));
        }
    }
}
