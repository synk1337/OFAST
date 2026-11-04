//! The ground a round's players stand on, as one sees it: the map's drawn surfaces that face up, near where the
//! players went, for the feet ([`crate::anim::feet`]).
//!
//! The map's physics is not that ground everywhere. Many stairs are a smooth ramp tagged `playerclip` over steps
//! that have no collision of their own (de_anubis: the physics under the stairs out of T spawn is the floor at their
//! foot, `examples/collision_probe.rs`), so a ray down through the physics, the clips left out, falls through the
//! steps; through the clips it stops on the ramp, above the steps. The drawn geometry has the steps.
//!
//! [`round_ground`] gathers the triangles of every placement the map exporter draws (world nodes' scene objects but
//! the invisible light blockers, aggregates, the drawn model entities) whose normal points up (at most 60° from
//! vertical: floors, steps, slopes, the tops of props) and that lie near a place a player of the round was: in a
//! [`CELL`]-inch column a player stood in or next to it, between [`BELOW`] under and [`ABOVE`] over the heights
//! they stood at there. A world grid indexes them ([`WorldCollision`]) for the rays down.

use std::collections::HashMap;

use crate::content::Content;
use crate::demo::players::RoundTrack;
use crate::map::{IDENTITY, Map, Transform, entity_transform, object_flags};
use crate::model::Model;
use crate::physics::ragdoll::WorldCollision;

/// The columns the players' places are gathered in (inches).
pub const CELL: f32 = 48.0;

/// How far below the lowest place a player stood in a column its surfaces are kept (inches).
pub const BELOW: f32 = 40.0;

/// How far above the highest place a player stood in a column its surfaces are kept (inches).
pub const ABOVE: f32 = 30.0;

/// The least upward part of a kept surface's normal: cos 60°.
const UP: f32 = 0.5;

type V3 = [f32; 3];

/// The drawn ground around the places `track`'s players went (see the [module docs](self)), with how many
/// triangles it holds; `None` when the map cannot be read.
pub fn round_ground(content: &Content, track: &RoundTrack) -> Option<(WorldCollision, usize)> {
    let map = Map::load(content, &track.map_name).ok()?;
    // The height range each column was stood in, its neighbours included.
    let mut stood: HashMap<(i32, i32), (f32, f32)> = HashMap::new();
    for sample in &track.samples {
        for p in sample.players.iter().filter(|p| p.alive) {
            let [x, y, z] = p.position;
            let (cx, cy) = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let range = stood.entry((cx + dx, cy + dy)).or_insert((z, z));
                    range.0 = range.0.min(z);
                    range.1 = range.1.max(z);
                }
            }
        }
    }
    let near = |min: V3, max: V3| {
        let (x0, x1) = (
            (min[0] / CELL).floor() as i32,
            (max[0] / CELL).floor() as i32,
        );
        let (y0, y1) = (
            (min[1] / CELL).floor() as i32,
            (max[1] / CELL).floor() as i32,
        );
        // A floor bigger than the columns around it is kept if any of them was stood in.
        (x0..=x1)
            .flat_map(|cx| (y0..=y1).map(move |cy| (cx, cy)))
            .any(|cell| {
                stood
                    .get(&cell)
                    .is_some_and(|&(lo, hi)| max[2] >= lo - BELOW && min[2] <= hi + ABOVE)
            })
    };

    // Every drawn placement: its model, transform and draw calls (None: the model's default meshes, all of them).
    let mut placements: Vec<(&str, Transform, Option<Vec<usize>>)> = Vec::new();
    for node in &map.nodes {
        for o in &node.scene_objects {
            if o.flags & object_flags::BLOCK_LIGHT == 0 {
                placements.push((&o.model, o.transform, None));
            }
        }
        for a in &node.aggregates {
            let world: Vec<usize> = a
                .fragments
                .iter()
                .filter(|f| f.transform.is_none())
                .map(|f| f.draw_call)
                .collect();
            if !world.is_empty() {
                placements.push((&a.model, IDENTITY, Some(world)));
            }
            for f in &a.fragments {
                if let Some(t) = f.transform {
                    placements.push((&a.model, t, Some(vec![f.draw_call])));
                }
            }
        }
    }
    for e in &map.entities {
        let Some(class) = e.class_name() else {
            continue;
        };
        if super::map::MODEL_ENTITY_CLASSES.contains(&class)
            && e.flag("startdisabled") != Some(true)
            && let Some(model) = e.string("model").filter(|m| !m.is_empty())
        {
            placements.push((
                model,
                entity_transform(e.origin(), e.angles(), e.scales()),
                None,
            ));
        }
    }

    let mut models: HashMap<&str, Option<Model>> = HashMap::new();
    let mut ground = WorldCollision::default();
    let mut kept = 0;
    for (path, transform, draws) in placements {
        let model = models
            .entry(path)
            .or_insert_with(|| super::gltf::load_model(content, path).ok());
        let Some(model) = model else {
            continue;
        };
        // Aggregates draw from their model's first mesh, by draw call.
        let meshes: Vec<_> = if draws.is_some() {
            model.meshes.iter().take(1).collect()
        } else {
            model.default_meshes().collect()
        };
        for mesh in meshes {
            for (i, draw) in mesh.draw_calls.iter().enumerate() {
                if draws.as_ref().is_some_and(|list| !list.contains(&i)) {
                    continue;
                }
                let Ok(geometry) = mesh.geometry(draw) else {
                    continue;
                };
                let world: Vec<V3> = geometry
                    .positions
                    .iter()
                    .map(|&p| apply(&transform, p))
                    .collect();
                for tri in geometry.indices.as_chunks::<3>().0 {
                    let corners = [0, 1, 2].map(|k| world[tri[k] as usize]);
                    let mut normal =
                        cross(sub(corners[1], corners[0]), sub(corners[2], corners[0]));
                    // Which way the surface faces: its stored vertex normals, turned with the placement (the
                    // winding is clockwise on the drawn meshes, and a mirrored placement turns it round); the
                    // winding as is without them.
                    if let Some(stored) = tri
                        .iter()
                        .map(|&v| geometry.normals.get(v as usize).copied())
                        .collect::<Option<Vec<V3>>>()
                    {
                        let sum: V3 = std::array::from_fn(|k| stored.iter().map(|n| n[k]).sum());
                        let turned = turn(&transform, sum);
                        if dot(turned, normal) < 0.0 {
                            normal = normal.map(|c| -c);
                        }
                    }
                    let area = dot(normal, normal).sqrt();
                    if area < 1e-6 || normal[2] < UP * area {
                        continue;
                    }
                    let min = std::array::from_fn(|k| {
                        corners.iter().map(|c| c[k]).fold(f32::MAX, f32::min)
                    });
                    let max = std::array::from_fn(|k| {
                        corners.iter().map(|c| c[k]).fold(f32::MIN, f32::max)
                    });
                    if near(min, max) {
                        ground.add_triangle(corners);
                        kept += 1;
                    }
                }
            }
        }
    }
    Some((ground, kept))
}

fn apply(m: &Transform, p: V3) -> V3 {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * p[k]).sum::<f32>() + m[i][3])
}

/// The direction `v` turned by `m` (no translation).
fn turn(m: &Transform, v: V3) -> V3 {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * v[k]).sum::<f32>())
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
