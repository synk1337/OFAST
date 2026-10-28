//! Developer tool: where points of a weapon's sticker UV set (`TEXCOORD 1`) are on the model (M10,
//! `export::stickers`): the position and normal of the surface each point falls on, to tell what a sticker offset
//! means.
//!
//! ```bash
//! cargo run --release --example sticker_where -- weapons/models/usp_silencer/weapon_pist_usp_silencer.vmdl \
//!     weapons/models/usp_silencer/materials/weapon_pist_usp_silencer.vmat 0.363,-0.259 0.33,-0.403
//! ```
//!
//! Points are in the slots' offset space (`TEXCOORD 1` − 0.5), as `g_vStickerNOffset` and the item's offsets are.

use ofast_core::content::Content;
use ofast_core::export::gltf::load_model;
use ofast_core::game::Cs2Install;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, material, points @ ..] = args.as_slice() else {
        panic!("usage: sticker_where <model> <material> <x,y>...");
    };
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives");
    let model = load_model(&content, model).expect("model");
    // Every triangle of the material: positions, normals, second UV set.
    let mut triangles = Vec::new();
    let mut bounds = [[f32::MAX; 3], [f32::MIN; 3]];
    for mesh in model.meshes_in_groups(1) {
        for draw in mesh.draw_calls.iter().filter(|d| d.material == *material) {
            let Ok(g) = mesh.geometry(draw) else { continue };
            let Some(uv1) = g.uvs.get(1) else { continue };
            for p in &g.positions {
                for i in 0..3 {
                    bounds[0][i] = bounds[0][i].min(p[i]);
                    bounds[1][i] = bounds[1][i].max(p[i]);
                }
            }
            for c in g.indices.as_chunks::<3>().0 {
                let c = c.map(|i| i as usize);
                triangles.push((
                    c.map(|i| g.positions[i]),
                    c.map(|i| g.normals.get(i).copied().unwrap_or([0.0; 3])),
                    c.map(|i| uv1[i]),
                ));
            }
        }
    }
    println!("{} triangles; model bounds {:?}", triangles.len(), bounds);
    for point in points {
        let (x, y) = point.split_once(',').expect("x,y");
        let (u, v) = (
            x.parse::<f32>().expect("x") + 0.5,
            y.parse::<f32>().expect("y") + 0.5,
        );
        let mut found = Vec::new();
        for (pos, nrm, uv) in &triangles {
            let area = (uv[1][0] - uv[0][0]) * (uv[2][1] - uv[0][1])
                - (uv[2][0] - uv[0][0]) * (uv[1][1] - uv[0][1]);
            if area.abs() < 1e-12 {
                continue;
            }
            let edge = |a: [f32; 2], b: [f32; 2]| {
                ((b[0] - a[0]) * (v - a[1]) - (u - a[0]) * (b[1] - a[1])) / area
            };
            let w = [edge(uv[1], uv[2]), edge(uv[2], uv[0]), edge(uv[0], uv[1])];
            if w.iter().all(|&w| w >= -1e-5) {
                let at = |q: &[[f32; 3]; 3]| -> [f32; 3] {
                    std::array::from_fn(|i| w[0] * q[0][i] + w[1] * q[1][i] + w[2] * q[2][i])
                };
                found.push((at(pos), at(nrm)));
            }
        }
        let show = |v: [f32; 3]| format!("({:6.2}, {:6.2}, {:6.2})", v[0], v[1], v[2]);
        if found.is_empty() {
            println!("{point}: on no triangle");
        }
        for (p, n) in found {
            println!("{point}: at {} normal {}", show(p), show(n));
        }
    }
}
