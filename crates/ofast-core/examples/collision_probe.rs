//! Developer tool: the shapes of a map's world collision (`maps/<map>/world_physics.vmdl`) around a point — kind,
//! collision group and tags, bounds — to see what a falling body can land on there.
//!
//! ```bash
//! cargo run --release --example collision_probe -- de_nuke 770 -1138 -769 [radius]
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::physics::{PhysicsData, ShapeKind};
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = &args[0];
    let point: [f32; 3] = std::array::from_fn(|k| args[1 + k].parse().expect("coordinate"));
    let radius: f32 = args.get(4).map_or(24.0, |r| r.parse().expect("radius"));
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, map).expect("map archive opens");
    let bytes = content
        .read_resource(&format!("maps/{map}/world_physics.vmdl"))
        .expect("world physics reads");
    let world = PhysicsData::from_resource(&Resource::parse(&bytes).expect("parses"))
        .expect("PHYS reads")
        .expect("a PHYS block");
    for (i, a) in world.attributes.iter().enumerate() {
        let count = world
            .parts
            .iter()
            .flat_map(|p| &p.shapes)
            .filter(|s| s.attribute == i)
            .count();
        println!(
            "attribute {i}: {} [{}], {count} shapes",
            a.group,
            a.interact_as.join(",")
        );
    }
    // The surface properties the shapes name (`m_surfacePropertyHashes`), with how many shapes name each.
    for (i, hash) in world.surfaces.iter().enumerate() {
        let count = world
            .parts
            .iter()
            .flat_map(|p| &p.shapes)
            .filter(|s| s.surface == i)
            .count();
        println!("surface {i}: hash {hash}, {count} shapes");
    }
    // What the solver sees: a 4-inch sphere at a few heights around the point.
    let collision = ofast_core::physics::ragdoll::WorldCollision::from_physics(&world);
    for dz in [8.0, 4.0, 2.0, 0.0, -2.0, -4.0, -8.0] {
        let center = [point[0], point[1], point[2] + dz];
        println!(
            "sphere r=4 at z {:+}: {:?}",
            dz,
            collision.sphere_contact(center, 4.0)
        );
    }
    for (n, shape) in world.parts.iter().flat_map(|p| &p.shapes).enumerate() {
        let attribute = world.attribute(shape);
        let group = attribute.map_or("?", |a| a.group.as_str());
        let tags = attribute
            .map(|a| a.interact_as.join(","))
            .unwrap_or_default();
        let near = |min: &[f32; 3], max: &[f32; 3]| {
            (0..3).all(|k| point[k] >= min[k] - radius && point[k] <= max[k] + radius)
        };
        match &shape.kind {
            ShapeKind::Hull {
                min, max, planes, ..
            } if near(min, max) => {
                let inside = planes
                    .iter()
                    .map(|(n, d)| n[0] * point[0] + n[1] * point[1] + n[2] * point[2] - d)
                    .fold(f32::MIN, f32::max);
                println!(
                    "#{n} hull {group} [{tags}] {:?} – {:?}, {} planes, signed distance {inside:.1}",
                    min.map(|c| c.round()),
                    max.map(|c| c.round()),
                    planes.len()
                );
            }
            ShapeKind::Mesh {
                min,
                max,
                triangles,
                vertices,
                ..
            } if near(min, max) => {
                // Its vertices nearest the point (horizontally), to see the surface there.
                let mut nearest: Vec<[f32; 3]> = vertices.clone();
                nearest.sort_by(|a, b| {
                    let d = |v: &[f32; 3]| (v[0] - point[0]).powi(2) + (v[1] - point[1]).powi(2);
                    d(a).total_cmp(&d(b))
                });
                println!(
                    "#{n} mesh {group} [{tags}] {:?} – {:?}, {} triangles, {} vertices; nearest {:?}",
                    min.map(|c| c.round()),
                    max.map(|c| c.round()),
                    triangles.len(),
                    vertices.len(),
                    nearest
                        .iter()
                        .take(3)
                        .map(|v| v.map(|c| c.round()))
                        .collect::<Vec<_>>()
                );
            }
            _ => {}
        }
    }
}
