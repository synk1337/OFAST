//! Developer tool: print the raw binary-KV3 header fields of every KV3 block in a file inside CS2's VPK.
//!
//! ```bash
//! cargo run --release --example kv3_inspect -- models/anubis/trims/trim03_128_cheap.vmdl_c
//! ```
//!
//! Prints each header field by name and offset so format questions can be answered from real data.

use ofast_core::game::Cs2Install;
use ofast_core::kv3::binary::{decode_with_report, is_kv3};
use ofast_core::resource::Resource;
use ofast_core::vpk::Vpk;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: kv3_inspect <path inside pak01_dir.vpk>");
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("VPK opens");
    let data = vpk.read_path(&path).expect("file reads");
    let resource = Resource::parse(&data).expect("resource parses");

    for block in resource.blocks() {
        let bytes = resource.block_data(block);
        println!(
            "block {} at {} size {}",
            block.kind, block.offset, block.size
        );
        if !is_kv3(bytes) {
            continue;
        }
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let i32_at = |o: usize| i32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let version = bytes[0];
        println!("  version {version}, compression {}", i32_at(20));
        if version >= 2 {
            let names = [
                "bytes1",
                "bytes4",
                "bytes8",
                "count_types",
                "(objs|arrs)",
                "size_uncompressed",
                "size_compressed",
                "count_blobs",
                "size_blob_bytes",
            ];
            println!("  dict {} frame {}", u16_at(24), u16_at(26));
            for (i, name) in names.iter().enumerate() {
                println!("  @{:3} {name:20} {}", 28 + 4 * i, i32_at(28 + 4 * i));
            }
        }
        if version >= 4 {
            println!("  @ 64 bytes2               {}", i32_at(64));
            println!("  @ 68 size_chunk_sizes     {}", i32_at(68));
        }
        if version >= 5 {
            let names = [
                "uncomp_buf1",
                "comp_buf1",
                "uncomp_buf2",
                "comp_buf2",
                "bytes1_b2",
                "bytes2_b2",
                "bytes4_b2",
                "bytes8_b2",
                "count_nodes",
                "objects_b2",
                "arrays_b2",
                "array_elems",
            ];
            for (i, name) in names.iter().enumerate() {
                println!("  @{:3} {name:20} {}", 72 + 4 * i, i32_at(72 + 4 * i));
            }
        }
        match decode_with_report(bytes) {
            Ok((_, report)) => println!("  decoded: {report:?}"),
            Err(e) => println!("  ERROR: {e}"),
        }
    }
}
