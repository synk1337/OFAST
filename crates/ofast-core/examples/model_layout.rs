//! Developer tool: print the vertex layouts and draw calls of a model, from pak01 or a map archive.
//!
//! ```bash
//! cargo run --release --example model_layout -- weapons/models/ak47/weapon_rif_ak47.vmdl_c
//! cargo run --release --example model_layout -- maps/de_mirage/worldnodes/n0_lr0_c5_s_cb_mesh.vmdl_c de_mirage
//! cargo run --release --example model_layout -- list de_mirage            # list a map's world models
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::model::Model;
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    if args.first().map(String::as_str) == Some("list") {
        let content = Content::for_map(&install, &args[1]).expect("map archive opens");
        for entry in content.archives()[0]
            .entries()
            .iter()
            .filter(|e| e.path().ends_with(".vmdl_c"))
            .take(40)
        {
            println!("{:>10}  {}", entry.size(), entry.path());
        }
        return;
    }
    if args.first().map(String::as_str) == Some("formats") {
        // Count (semantic, index, format) across every model of a map archive.
        let content = Content::for_map(&install, &args[1]).expect("map archive opens");
        let mut counts = std::collections::BTreeMap::new();
        for entry in content.archives()[0]
            .entries()
            .iter()
            .filter(|e| e.path().ends_with(".vmdl_c"))
        {
            let data = content.read(entry.path()).expect("file reads");
            let Ok(model) = Model::parse(&Resource::parse(&data).expect("resource")) else {
                continue;
            };
            for mesh in &model.meshes {
                for vb in &mesh.vertex_buffers {
                    for a in &vb.attributes {
                        *counts
                            .entry((a.semantic.clone(), a.semantic_index, a.format))
                            .or_insert(0usize) += 1;
                    }
                }
            }
        }
        for ((semantic, index, format), n) in counts {
            println!("{semantic:<14} {index} format {format:>2}: {n} buffers");
        }
        return;
    }
    if args.first().map(String::as_str) == Some("material") {
        // Find world models using a material (substring) in a map, and print their layouts.
        let content = Content::for_map(&install, &args[2]).expect("map archive opens");
        let paths: Vec<String> = content.archives()[0]
            .entries()
            .iter()
            .filter(|e| e.path().ends_with(".vmdl_c"))
            .map(|e| e.path().to_owned())
            .collect();
        let mut shown = 0;
        for path in paths {
            let data = content.read(&path).expect("file reads");
            let Ok(model) = Model::parse(&Resource::parse(&data).expect("resource")) else {
                continue;
            };
            for mesh in &model.meshes {
                for draw in mesh
                    .draw_calls
                    .iter()
                    .filter(|d| d.material.contains(&args[1]))
                {
                    println!("{path}: {} (vb {:?})", draw.material, draw.vertex_buffers);
                    for &vb in &draw.vertex_buffers {
                        let buffer = &mesh.vertex_buffers[vb];
                        for a in &buffer.attributes {
                            println!(
                                "    vb{vb} {:<14} {} format {:>2} offset {:>2}",
                                a.semantic, a.semantic_index, a.format, a.offset
                            );
                            if a.format == 28 {
                                // Per-channel min / mean / max of 8-bit attributes (vertex paint).
                                let mut stats = [(255u8, 0u64, 0u8); 4];
                                for i in 0..buffer.count {
                                    let bytes = &buffer.data[i * buffer.stride + a.offset..][..4];
                                    for (c, s) in stats.iter_mut().enumerate() {
                                        s.0 = s.0.min(bytes[c]);
                                        s.1 += u64::from(bytes[c]);
                                        s.2 = s.2.max(bytes[c]);
                                    }
                                }
                                for (c, s) in stats.iter().enumerate() {
                                    println!(
                                        "        channel {c}: min {} mean {} max {}",
                                        s.0,
                                        s.1 / buffer.count.max(1) as u64,
                                        s.2
                                    );
                                }
                                let same = (0..buffer.count)
                                    .filter(|i| {
                                        let b = &buffer.data[i * buffer.stride + a.offset..][..4];
                                        b[0] == b[1] && b[1] == b[2] && b[2] == b[3]
                                    })
                                    .count();
                                let sample: Vec<_> = (0..buffer.count.min(6))
                                    .map(|i| {
                                        buffer.data[i * buffer.stride + a.offset..][..4].to_vec()
                                    })
                                    .collect();
                                println!(
                                    "        all channels equal on {same}/{} vertices; first: {sample:?}",
                                    buffer.count
                                );
                            }
                        }
                    }
                    shown += 1;
                    if shown >= 3 {
                        return;
                    }
                }
            }
        }
        return;
    }
    let path = &args[0];
    let content = match args.get(1) {
        Some(map) => Content::for_map(&install, map),
        None => Content::game(&install),
    }
    .expect("archives open");
    let data = content.read(path).expect("file reads");
    let resource = Resource::parse(&data).expect("resource parses");
    let model = Model::parse(&resource).expect("model parses");
    for mesh in &model.meshes {
        println!("mesh `{}` (index {})", mesh.name, mesh.mesh_index);
        for (i, vb) in mesh.vertex_buffers.iter().enumerate() {
            println!(
                "  vertex buffer {i}: {} vertices × {} bytes",
                vb.count, vb.stride
            );
            for a in &vb.attributes {
                println!(
                    "    {:<14} {} format {:>2} offset {:>2}",
                    a.semantic, a.semantic_index, a.format, a.offset
                );
            }
        }
        for draw in mesh.draw_calls.iter().take(8) {
            println!(
                "  draw: vb {:?} ib {} indices {} material {}",
                draw.vertex_buffers, draw.index_buffer, draw.index_count, draw.material
            );
        }
    }
}
