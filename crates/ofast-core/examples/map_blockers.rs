//! Developer tool: list a map's "block light" scene objects — invisible meshes that only stop light — with their
//! material and bounds, optionally only those near a point.
//!
//! ```bash
//! cargo run --release --example map_blockers -- de_mirage                    # all of them
//! cargo run --release --example map_blockers -- de_mirage -1250,745,24 600   # within 600 units of a point
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::{Map, object_flags};
use ofast_core::model::Model;
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().cloned().unwrap_or_else(|| "de_mirage".into());
    let near: Option<([f32; 3], f32)> = args.get(1).map(|p| {
        let v: Vec<f32> = p.split(',').map(|s| s.parse().expect("x,y,z")).collect();
        let radius = args
            .get(2)
            .map(|r| r.parse().expect("radius"))
            .unwrap_or(500.0);
        ([v[0], v[1], v[2]], radius)
    });
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &name).expect("archives open");
    let map = Map::load(&content, &name).expect("map loads");

    let (mut total, mut shown, mut triangles) = (0, 0, 0usize);
    for object in map.nodes.iter().flat_map(|n| &n.scene_objects) {
        if object.flags & object_flags::BLOCK_LIGHT == 0 {
            continue;
        }
        total += 1;
        let Ok(data) = content.read_resource(&object.model) else {
            continue;
        };
        let Ok(resource) = Resource::parse(&data) else {
            continue;
        };
        let Ok(model) = Model::parse(&resource) else {
            continue;
        };
        for mesh in &model.meshes {
            for draw in &mesh.draw_calls {
                let Ok(g) = mesh.geometry(draw) else { continue };
                // World-space bounds: the scene object's transform applied to every vertex.
                let t = &object.transform;
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for p in &g.positions {
                    let w: [f32; 3] = std::array::from_fn(|r| {
                        t[r][0] * p[0] + t[r][1] * p[1] + t[r][2] * p[2] + t[r][3]
                    });
                    for k in 0..3 {
                        lo[k] = lo[k].min(w[k]);
                        hi[k] = hi[k].max(w[k]);
                    }
                }
                if let Some((point, radius)) = near {
                    // Distance from the point to the bounding box.
                    let d2: f32 = (0..3)
                        .map(|k| (lo[k] - point[k]).max(point[k] - hi[k]).max(0.0).powi(2))
                        .sum();
                    if d2.sqrt() > radius {
                        continue;
                    }
                }
                shown += 1;
                triangles += g.indices.len() / 3;
                println!(
                    "{} | {} | {} tris | x {:.0}..{:.0} y {:.0}..{:.0} z {:.0}..{:.0}",
                    object.model,
                    draw.material,
                    g.indices.len() / 3,
                    lo[0],
                    hi[0],
                    lo[1],
                    hi[1],
                    lo[2],
                    hi[2]
                );
            }
        }
    }
    println!("{total} block-light objects; {shown} draw calls shown, {triangles} triangles");
}
