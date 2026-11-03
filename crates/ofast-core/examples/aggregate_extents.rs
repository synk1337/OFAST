//! Developer tool: how large a map's aggregates are, and their world-space fragments (one draw call each), in metres:
//! what Unreal gets as one mesh when the fragments of an aggregate are merged, and what it could get instead.
//!
//! ```bash
//! cargo run --release --example aggregate_extents -- de_nuke
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::Map;
use ofast_core::model::Model;
use ofast_core::resource::Resource;

const INCH_TO_M: f32 = 0.0254;

fn main() {
    let name = std::env::args()
        .nth(1)
        .expect("usage: aggregate_extents <map> [model filter]");
    // With a filter: every fragment of the matching aggregates, with its size and triangle count.
    let filter = std::env::args().nth(2);
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &name).expect("archives open");
    let map = Map::load(&content, &name).expect("map loads");
    let mut fragment_sizes = Vec::new();
    let mut aggregates = Vec::new();
    for node in &map.nodes {
        for aggregate in &node.aggregates {
            let Ok(data) = content.read_resource(&aggregate.model) else {
                continue;
            };
            let Ok(resource) = Resource::parse(&data) else {
                continue;
            };
            let Ok(model) = Model::parse(&resource) else {
                continue;
            };
            let Some(mesh) = model.meshes.first() else {
                continue;
            };
            let mut all = ([f32::MAX; 3], [f32::MIN; 3]);
            let mut count = 0;
            for fragment in aggregate.fragments.iter().filter(|f| f.transform.is_none()) {
                let Some(draw) = mesh.draw_calls.get(fragment.draw_call) else {
                    continue;
                };
                let Ok(g) = mesh.geometry(draw) else { continue };
                let mut b = ([f32::MAX; 3], [f32::MIN; 3]);
                for p in &g.positions {
                    b.0 = std::array::from_fn(|i| b.0[i].min(p[i]));
                    b.1 = std::array::from_fn(|i| b.1[i].max(p[i]));
                }
                all.0 = std::array::from_fn(|i| all.0[i].min(b.0[i]));
                all.1 = std::array::from_fn(|i| all.1[i].max(b.1[i]));
                let size = (0..3).map(|i| b.1[i] - b.0[i]).fold(0.0f32, f32::max) * INCH_TO_M;
                if filter
                    .as_ref()
                    .is_some_and(|f| aggregate.model.contains(f.as_str()))
                {
                    println!(
                        "  {} draw {}: {size:.1} m, {} triangles",
                        aggregate.model,
                        fragment.draw_call,
                        draw.index_count / 3
                    );
                }
                fragment_sizes.push(size);
                count += 1;
            }
            if count > 0 {
                let size = (0..3).map(|i| all.1[i] - all.0[i]).fold(0.0f32, f32::max) * INCH_TO_M;
                aggregates.push((size, count, aggregate.model.clone()));
            }
        }
    }
    aggregates.sort_by(|a, b| b.0.total_cmp(&a.0));
    fragment_sizes.sort_by(|a, b| b.total_cmp(a));
    println!(
        "{} aggregates with world-space fragments; largest:",
        aggregates.len()
    );
    for (size, count, model) in aggregates.iter().take(10) {
        println!("  {size:6.1} m  {count:4} fragments  {model}");
    }
    let n = fragment_sizes.len();
    let at = |q: f32| fragment_sizes[((n as f32 * q) as usize).min(n - 1)];
    println!(
        "{n} fragments: largest {:.1} m, 1 % over {:.1} m, 10 % over {:.1} m, median {:.1} m; over 20 m: {}",
        fragment_sizes[0],
        at(0.01),
        at(0.1),
        at(0.5),
        fragment_sizes.iter().filter(|s| **s > 20.0).count()
    );
}
