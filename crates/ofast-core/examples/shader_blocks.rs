//! Developer tool: unpacks bytecode blocks of a CS2 shader (`.vcs`, format 72) from `shaders_vulkan_dir.vpk`, to
//! decompile a static combo's SPIR-V (M10, the paint styles: `docs/research/cs2-skins.md`, "Reading CS2's paint
//! shader"). What it writes is Valve's content: keep it out of the repository.
//!
//! ```bash
//! cargo run --release --example shader_blocks -- shaders/vfx/csgo_customweapon_vulkan_50_ps.vcs \
//!     vfx_zstd_255df362.bin out/ 0 1932
//! ```
//!
//! Each offset is a block's `m_nOffs` (`tools/shaders/vcs_combos.py table` lists them per combo). A block sits at the
//! resource's size plus that offset as `[i32 −type][u32 uncompressed size][u32 compressed size]` and a zstd frame
//! compressed with one of Valve's dictionaries (types 3 and 5; Source 2 Viewer's repository holds both).

use ofast_core::game::Cs2Install;
use ofast_core::vpk::Vpk;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, dictionary, out, offsets @ ..] = args.as_slice() else {
        panic!(
            "usage: shader_blocks <path in shaders_vulkan_dir.vpk> <zstd dictionary> <out dir> <offset>..."
        );
    };
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk =
        Vpk::open(install.csgo_dir().join("shaders_vulkan_dir.vpk")).expect("shader archive opens");
    let data = vpk.read_path(path).expect("shader in the archive");
    let dictionary = std::fs::read(dictionary).expect("dictionary reads");
    let resource_size = u32::from_le_bytes(data[0..4].try_into().expect("header")) as usize;
    std::fs::create_dir_all(out).expect("out dir");
    for offset in offsets {
        let start = resource_size + offset.parse::<usize>().expect("offset");
        let word = |at: usize| {
            u32::from_le_bytes(
                data[start + at..start + at + 4]
                    .try_into()
                    .expect("block header"),
            )
        };
        let (kind, uncompressed, compressed) = (word(0) as i32, word(4) as usize, word(8) as usize);
        let mut decoder = ruzstd::decoding::FrameDecoder::new();
        decoder
            .add_dict(
                ruzstd::decoding::Dictionary::decode_dict(&dictionary).expect("dictionary decodes"),
            )
            .expect("dictionary added");
        let mut block = vec![0u8; uncompressed];
        let written = decoder
            .decode_all(&data[start + 12..start + 12 + compressed], &mut block)
            .expect("block decodes");
        assert_eq!(written, uncompressed, "block size");
        let file = std::path::Path::new(out).join(format!("block_{offset}.bin"));
        std::fs::write(&file, &block).expect("block written");
        println!(
            "{offset}: type {kind}, {compressed} -> {uncompressed} bytes, {}",
            file.display()
        );
    }
}
