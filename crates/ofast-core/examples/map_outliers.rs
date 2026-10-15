//! Developer tool: find map geometry with suspicious extents (stretched triangles, misplaced pieces).
//!
//! ```bash
//! cargo run --release --example map_outliers -- de_mirage
//! ```
//!
//! Prints every world draw call whose bounding box is taller than 1000 units or reaches above z = 1500.

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::Map;
use ofast_core::model::Model;
use ofast_core::resource::Resource;

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "de_mirage".into());
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &name).expect("archives open");
    let map = Map::load(&content, &name).expect("map loads");
    let mut models: Vec<(String, &str)> = Vec::new();
    for node in &map.nodes {
        models.extend(
            node.scene_objects
                .iter()
                .map(|o| (o.model.clone(), "object")),
        );
        models.extend(
            node.aggregates
                .iter()
                .map(|a| (a.model.clone(), "aggregate")),
        );
    }
    if std::env::args().nth(2).as_deref() == Some("classes") {
        // Entity classes that carry a model, with counts — to spot geometry the exporter skips.
        let mut counts = std::collections::BTreeMap::new();
        for entity in &map.entities {
            if let Some(model) = entity.string("model").filter(|m| !m.is_empty()) {
                let e = counts
                    .entry(entity.class_name().unwrap_or("?").to_owned())
                    .or_insert((0, String::new()));
                e.0 += 1;
                e.1 = model.to_owned();
            }
        }
        for (class, (n, example)) in counts {
            println!("{class:<32} {n:>4}  e.g. {example}");
        }
        return;
    }
    for entity in &map.entities {
        if let Some(model) = entity.string("model").filter(|m| m.ends_with(".vmdl")) {
            let o = entity.origin();
            println!(
                "entity {} {model} origin ({:.0}, {:.0}, {:.0}) angles {:?} scales {:?}",
                entity.class_name().unwrap_or("?"),
                o[0],
                o[1],
                o[2],
                entity.angles(),
                entity.scales()
            );
            models.push((model.to_owned(), "entity-model"));
        }
    }
    for (path, kind) in models {
        let Ok(data) = content.read_resource(&path) else {
            continue;
        };
        let Ok(resource) = Resource::parse(&data) else {
            continue;
        };
        let Ok(model) = Model::parse(&resource) else {
            continue;
        };
        for mesh in &model.meshes {
            for (i, draw) in mesh.draw_calls.iter().enumerate() {
                let Ok(g) = mesh.geometry(draw) else { continue };
                if g.indices.len() != draw.index_count as usize {
                    println!(
                        "DROPPED {kind} {path} draw {i}: kept {} of {} indices (base {}, start {}), {}",
                        g.indices.len(),
                        draw.index_count,
                        draw.base_vertex,
                        draw.start_index,
                        draw.material
                    );
                }
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for p in &g.positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                if hi[2] - lo[2] > 1000.0 || hi[2] > 1500.0 {
                    println!(
                        "{kind} {path} draw {i} vb {:?}: z {:.0}..{:.0}, x {:.0}..{:.0}, y {:.0}..{:.0}, {} verts, {}",
                        draw.vertex_buffers,
                        lo[2],
                        hi[2],
                        lo[0],
                        hi[0],
                        lo[1],
                        hi[1],
                        g.positions.len(),
                        draw.material
                    );
                }
            }
        }
    }
}
