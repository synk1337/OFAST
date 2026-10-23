//! Developer tool: the maps' entity models whose reference pose differs from their bind pose
//! (`Model::reference_correction`, `model::refpose`): the correction as an axis and angle, and the mesh's bounds at
//! bind pose and at the reference pose next to the model's own physics bounds (drawn as CS2 draws it, the mesh
//! should fit where its physics is).
//!
//! ```bash
//! cargo run --release --example refpose_census -- de_nuke de_mirage de_inferno
//! ```

use std::collections::BTreeSet;

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::Map;
use ofast_core::model::{Affine, Model};
use ofast_core::physics::{PhysicsData, ShapeKind};
use ofast_core::resource::Resource;

fn bounds(points: impl Iterator<Item = [f32; 3]>) -> Option<([f32; 3], [f32; 3])> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut any = false;
    for p in points {
        any = true;
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    any.then_some((lo, hi))
}

fn show(b: Option<([f32; 3], [f32; 3])>) -> String {
    b.map_or("-".into(), |(lo, hi)| {
        format!(
            "x {:.0}…{:.0} y {:.0}…{:.0} z {:.0}…{:.0}",
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
        )
    })
}

fn apply(m: &Affine, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * p[k]).sum::<f32>() + m[i][3])
}

fn main() {
    let maps: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    for name in &maps {
        let content = Content::for_map(&install, name).expect("archives open");
        let map = Map::load(&content, name).expect("map loads");
        let models: BTreeSet<String> = map
            .entities
            .iter()
            .filter_map(|e| e.get("model").and_then(|v| v.as_str()).map(str::to_owned))
            .filter(|m| m.ends_with(".vmdl"))
            .collect();
        let (mut with_animation, mut corrected, mut uneven) = (0, 0, 0);
        for path in &models {
            let Ok(bytes) = content.read_resource(path) else {
                continue;
            };
            let Ok(resource) = Resource::parse(&bytes) else {
                continue;
            };
            let Ok(model) = Model::parse(&resource) else {
                continue;
            };
            let Some(reference) = &model.reference_pose else {
                continue;
            };
            with_animation += 1;
            let meshes: Vec<_> = model.default_meshes().collect();
            let Some(correction) = meshes.iter().find_map(|m| model.reference_correction(m)) else {
                // A skinned mesh whose bones move differently between bind and reference.
                let bind = model.skeleton.world_transforms();
                let skinned = meshes.iter().any(|m| !m.skin_bones.is_empty());
                let differs = reference
                    .iter()
                    .zip(&bind)
                    .any(|(a, b)| (0..3).any(|i| (0..4).any(|j| (a[i][j] - b[i][j]).abs() > 1e-3)));
                if skinned && differs {
                    uneven += 1;
                    println!(
                        "{name}: {path}: bones move differently ({} bones)",
                        bind.len()
                    );
                }
                continue;
            };
            corrected += 1;
            // Axis-angle of the correction's rotation.
            let trace = correction[0][0] + correction[1][1] + correction[2][2];
            let angle = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees();
            let axis = [
                correction[2][1] - correction[1][2],
                correction[0][2] - correction[2][0],
                correction[1][0] - correction[0][1],
            ];
            let mut points = Vec::new();
            for mesh in model.default_meshes() {
                for draw in &mesh.draw_calls {
                    if let Ok(g) = mesh.geometry(draw) {
                        points.extend(g.positions);
                    }
                }
            }
            let physics = PhysicsData::from_resource(&resource).ok().flatten();
            let hull =
                physics.as_ref().and_then(|p| {
                    bounds(p.parts.iter().flat_map(|part| &part.shapes).flat_map(
                        |s| match &s.kind {
                            ShapeKind::Hull { min, max, .. } | ShapeKind::Mesh { min, max, .. } => {
                                vec![*min, *max]
                            }
                            _ => Vec::new(),
                        },
                    ))
                });
            println!(
                "{name}: {path}: {:.0}° about ({:.2}, {:.2}, {:.2}), {} bones\n    bind {}\n    ref  {}\n    phys {}",
                angle,
                axis[0],
                axis[1],
                axis[2],
                model.skeleton.bones.len(),
                show(bounds(points.iter().copied())),
                show(bounds(points.iter().map(|p| apply(&correction, *p)))),
                show(hull),
            );
        }
        println!(
            "{name}: {} entity models, {with_animation} with an embedded animation, {corrected} drawn away from their bind pose, {uneven} with bones moving differently",
            models.len()
        );
    }
}
