//! Developer tool: what does a CS2 camera see at given screen pixels, according to OFAST's own map geometry?
//!
//! ```bash
//! cargo run --release --example map_raycast -- de_mirage ct0 "60,680;960,800"
//! cargo run --release --example map_raycast -- de_mirage "1330,-1420,-144,5,215" "300,500"
//! ```
//!
//! Builds the camera of the first CT (`ct0`) or T (`t0`) spawn, or of CS2 `setpos x y z; setang pitch yaw` values,
//! exactly like the Unreal script (eye 64 units above the origin, CS2's 90° fov on 16:9), casts a ray through each pixel of a 1920×1080 frame and prints the nearest
//! triangle hit: which placement (scene object, aggregate fragment or entity), model and material. "nothing" means
//! the exported geometry has a hole there.

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::{IDENTITY, Map, Transform, entity_transform, object_flags};
use ofast_core::model::Model;
use ofast_core::resource::Resource;

type V3 = [f32; 3];

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
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn apply(t: &Transform, p: V3) -> V3 {
    [0, 1, 2].map(|r| t[r][0] * p[0] + t[r][1] * p[1] + t[r][2] * p[2] + t[r][3])
}

/// Möller–Trumbore ray/triangle intersection (both faces). Returns the distance along the ray.
fn hit(origin: V3, dir: V3, a: V3, b: V3, c: V3) -> Option<f32> {
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(origin, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 0.1).then_some(t)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = &args[0];
    let view = args.get(1).map(String::as_str).unwrap_or("ct0");
    let pixels: Vec<(f32, f32)> = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("960,540")
        .split(';')
        .map(|p| {
            let (x, y) = p.split_once(',').expect("x,y");
            (x.parse().expect("x"), y.parse().expect("y"))
        })
        .collect();

    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, name).expect("archives open");
    let map = Map::load(&content, name).expect("map loads");

    // The view is a spawn (`ct0` / `t0`) or CS2 console values "x y z pitch yaw [roll]" (setpos feet, setang),
    // separated by commas or spaces.
    let (o, angles) = match view.to_ascii_lowercase().as_str() {
        "ct0" | "t0" => {
            let class = if view.eq_ignore_ascii_case("ct0") {
                "info_player_counterterrorist"
            } else {
                "info_player_terrorist"
            };
            let spawn = map.entities_of(class).next().expect("spawn exists");
            (spawn.origin(), spawn.angles())
        }
        _ => {
            let v: Vec<f32> = view
                .split([',', ' '])
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().expect("view: ct0, t0 or numbers"))
                .collect();
            assert!(
                v.len() == 5 || v.len() == 6,
                "view: ct0, t0 or \"x y z pitch yaw [roll]\""
            );
            (
                [v[0], v[1], v[2]],
                [v[3], v[4], v.get(5).copied().unwrap_or(0.0)],
            )
        }
    };
    let eye = [o[0], o[1], o[2] + 64.0];
    let cam = entity_transform(eye, angles, [1.0; 3]);
    let forward = [cam[0][0], cam[1][0], cam[2][0]];
    let left = [cam[0][1], cam[1][1], cam[2][1]];
    let up = [cam[0][2], cam[1][2], cam[2][2]];
    let half_w = (45f32.to_radians().tan() * (16.0 / 9.0) / (4.0 / 3.0))
        .atan()
        .tan();
    let half_h = half_w * 1080.0 / 1920.0;
    let rays: Vec<V3> = pixels
        .iter()
        .map(|&(x, y)| {
            let sx = (x / 1920.0 * 2.0 - 1.0) * half_w;
            let sy = (1.0 - y / 1080.0 * 2.0) * half_h;
            let d = [0, 1, 2].map(|k| forward[k] - left[k] * sx + up[k] * sy);
            let l = dot(d, d).sqrt();
            d.map(|c| c / l)
        })
        .collect();

    // Every placement: (label, model, transform, which draw calls (None = all), skip?).
    let mut placements: Vec<(String, String, Transform, Option<Vec<usize>>)> = Vec::new();
    for node in &map.nodes {
        for o in &node.scene_objects {
            if o.flags & object_flags::BLOCK_LIGHT == 0 {
                placements.push(("object".into(), o.model.clone(), o.transform, None));
            }
        }
        for a in &node.aggregates {
            let world: Vec<usize> = a
                .fragments
                .iter()
                .filter(|f| f.transform.is_none())
                .map(|f| f.draw_call)
                .collect();
            placements.push(("aggregate".into(), a.model.clone(), IDENTITY, Some(world)));
            for f in a.fragments.iter().filter(|f| f.transform.is_some()) {
                placements.push((
                    "fragment".into(),
                    a.model.clone(),
                    f.transform.unwrap(),
                    Some(vec![f.draw_call]),
                ));
            }
        }
    }
    for e in &map.entities {
        if let Some(model) = e.string("model").filter(|m| m.ends_with(".vmdl")) {
            let t = entity_transform(e.origin(), e.angles(), e.scales());
            placements.push((
                format!("entity {}", e.class_name().unwrap_or("?")),
                model.to_owned(),
                t,
                None,
            ));
        }
    }

    let mut best: Vec<Option<(f32, String)>> = vec![None; rays.len()];
    for (label, model_path, transform, draws) in &placements {
        let Ok(data) = content.read_resource(model_path) else {
            continue;
        };
        let Ok(resource) = Resource::parse(&data) else {
            continue;
        };
        let Ok(model) = Model::parse(&resource) else {
            continue;
        };
        let meshes: Vec<_> = if draws.is_some() {
            model.meshes.iter().take(1).collect()
        } else {
            model.default_meshes().collect()
        };
        for mesh in meshes {
            for (i, draw) in mesh.draw_calls.iter().enumerate() {
                if let Some(list) = draws
                    && !list.contains(&i)
                {
                    continue;
                }
                let Ok(g) = mesh.geometry(draw) else { continue };
                let world: Vec<V3> = g.positions.iter().map(|&p| apply(transform, p)).collect();
                for tri in g.indices.as_chunks::<3>().0 {
                    let [a, b, c] = [
                        world[tri[0] as usize],
                        world[tri[1] as usize],
                        world[tri[2] as usize],
                    ];
                    for (r, dir) in rays.iter().enumerate() {
                        if let Some(t) = hit(eye, *dir, a, b, c)
                            && best[r].as_ref().is_none_or(|(d, _)| t < *d)
                        {
                            // Facing: winding normal and stored vertex normal against the view ray (< 0 = faces us).
                            let winding = cross(sub(b, a), sub(c, a));
                            let vertex_normal =
                                g.normals.get(tri[0] as usize).copied().unwrap_or([0.0; 3]);
                            // Per-vertex data at the triangle's first corner: COLOR 0 and TEXCOORD 4 (blend paint).
                            let rgba = |v: Option<&[f32; 4]>| {
                                v.map(|c| {
                                    format!(" {:.2}/{:.2}/{:.2}/{:.2}", c[0], c[1], c[2], c[3])
                                })
                                .unwrap_or_default()
                            };
                            let color = rgba(g.colors.get(tri[0] as usize));
                            let paint = rgba(g.blend.get(tri[0] as usize));
                            best[r] = Some((
                                t,
                                format!(
                                    "{label} {model_path} draw {i} {} | winding·ray {:+.2} normal·ray {:+.2} | colour{color} paint{paint}",
                                    draw.material,
                                    dot(winding, *dir).signum(),
                                    dot(vertex_normal, *dir)
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    for ((x, y), b) in pixels.iter().zip(best) {
        match b {
            Some((t, what)) => println!("pixel ({x}, {y}): {:.0} units — {what}", t),
            None => println!("pixel ({x}, {y}): nothing"),
        }
    }
}
