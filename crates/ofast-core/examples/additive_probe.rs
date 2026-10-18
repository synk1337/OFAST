//! Developer tool: find how CS2's additive clips compose, from a clip shipped both ways.
//!
//! ```bash
//! cargo run --release --example additive_probe -- animation/anims/world/rifle/rifle_ak/idle_ak.vnmclip \
//!     animation/anims/world/rifle/_default_rifle/idle_rifle.vnmclip
//! ```
//!
//! Weapon idles and shoots exist as an additive clip (`idle_ak`) and as a full pose (`idle_ak.vnmclip+non_additive`).
//! For candidate bases (the clip skeleton's reference pose, and the clip given second), this prints how far the full
//! pose is from the delta applied each way — `delta × base` and `base × delta` for rotations, `base + delta` for
//! translations — over the bones the delta moves. The convention that lands on the full pose is the one CS2 uses.

use ofast_core::anim::{Clip, NmSkeleton, Transform};
use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::model::{normalize_quat, quat_mul};
use ofast_core::resource::{BlockKind, Resource};

fn data(content: &Content, path: &str) -> Value {
    let bytes = content.read_resource(path).expect("resource reads");
    Resource::parse(&bytes)
        .expect("resource parses")
        .kv3(BlockKind::DATA)
        .expect("DATA block")
        .root
}

/// Angle between two rotations, degrees.
fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = (0..4).map(|k| a[k] * b[k]).sum::<f32>().abs().min(1.0);
    2.0 * dot.acos().to_degrees()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let additive_path = args.first().map_or(
        "animation/anims/world/rifle/rifle_ak/idle_ak.vnmclip",
        String::as_str,
    );
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let additive = Clip::parse(&data(&content, additive_path)).expect("additive clip");
    let full = Clip::parse(&data(
        &content,
        &format!("{additive_path}+non_additive.vnmclip"),
    ))
    .expect("non-additive clip");
    let skeleton = NmSkeleton::parse(&data(&content, &additive.skeleton));
    println!(
        "{additive_path}: additive {}, {} frames; non-additive {} frames",
        additive.additive, additive.frames, full.frames
    );
    let delta = additive.decode_frame(0).expect("frame");
    let target = full.decode_frame(0).expect("frame");

    let mut bases: Vec<(String, Vec<Transform>)> =
        vec![("reference pose".into(), skeleton.reference.clone())];
    if let Some(base_path) = args.get(1) {
        let base = Clip::parse(&data(&content, base_path)).expect("base clip");
        bases.push((base_path.clone(), base.decode_frame(0).expect("frame")));
    }
    for (name, base) in &bases {
        let (mut left, mut right, mut translation, mut moved) = (0.0f32, 0.0f32, 0.0f32, 0usize);
        let mut worst = (0.0f32, String::new());
        for (i, d) in delta.iter().enumerate() {
            if angle(d.rotation, [0.0, 0.0, 0.0, 1.0]) < 0.01
                && d.translation.iter().all(|c| c.abs() < 1e-3)
            {
                continue;
            }
            moved += 1;
            let l = angle(
                normalize_quat(quat_mul(d.rotation, base[i].rotation)),
                target[i].rotation,
            );
            let r = angle(
                normalize_quat(quat_mul(base[i].rotation, d.rotation)),
                target[i].rotation,
            );
            let t = (0..3)
                .map(|k| {
                    (base[i].translation[k] + d.translation[k] - target[i].translation[k]).powi(2)
                })
                .sum::<f32>()
                .sqrt();
            left = left.max(l);
            right = right.max(r);
            translation = translation.max(t);
            if r > worst.0 {
                worst = (r, skeleton.bones[i].clone());
            }
        }
        println!(
            "base {name}: {moved} bones moved; worst error delta×base {left:.3}°, base×delta {right:.3}° ({}), \
             translation {translation:.4} in",
            worst.1
        );
    }
    let still = delta
        .iter()
        .filter(|d| angle(d.rotation, [0.0, 0.0, 0.0, 1.0]) < 0.01)
        .count();
    println!(
        "delta: {still} of {} rotations are the identity; scales {:?}…",
        delta.len(),
        delta.iter().take(3).map(|d| d.scale).collect::<Vec<_>>()
    );
}
