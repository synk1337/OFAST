//! Developer tool: where a weapon model puts the right hand, against where CS2's clips put it relative to the
//! player's weapon bone — which tells how a weapon's world model sits on the player's `wpn` bone.
//!
//! ```bash
//! cargo run --release --example weapon_grip -- weapons/models/ak47/weapon_rif_ak47.vmdl \
//!     "animation/anims/world/rifle/rifle_ak/idle_ak.vnmclip+non_additive.vnmclip"
//! ```
//!
//! Prints the weapon model's bones in model space (`weapon`, `weapon_offset`, the hand guide `ag1_hand_r`), and the
//! clip's first frame in the player's model space: `wpn`, `wpnHand_R` and `hand_R`, and the hands relative to `wpn`.

use ofast_core::anim::{Clip, NmSkeleton, Transform};
use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::model::{Affine, Model};
use ofast_core::resource::{BlockKind, Resource};

fn data(content: &Content, path: &str) -> Value {
    let bytes = content.read_resource(path).expect("resource reads");
    Resource::parse(&bytes)
        .expect("resource parses")
        .kv3(BlockKind::DATA)
        .expect("DATA block")
        .root
}

/// `a · b` for rigid 3 × 4 matrices.
fn compose(a: &Affine, b: &Affine) -> Affine {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let r: f32 = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            if j == 3 { r + a[i][3] } else { r }
        })
    })
}

/// Inverse of a rigid 3 × 4 matrix.
fn invert(a: &Affine) -> Affine {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            if j == 3 {
                -(0..3).map(|k| a[k][i] * a[k][3]).sum::<f32>()
            } else {
                a[j][i]
            }
        })
    })
}

fn show(label: &str, m: &Affine) {
    println!(
        "  {label:28} at ({:8.3}, {:8.3}, {:8.3})  x axis ({:6.3}, {:6.3}, {:6.3})  z axis ({:6.3}, {:6.3}, {:6.3})",
        m[0][3], m[1][3], m[2][3], m[0][0], m[1][0], m[2][0], m[0][2], m[1][2], m[2][2]
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");

    let bytes = content
        .read_resource(&format!("{}_c", args[0].trim_end_matches("_c")))
        .expect("model reads");
    let model = Model::parse(&Resource::parse(&bytes).expect("resource")).expect("model");
    let world = model.skeleton.world_transforms();
    println!("{} (model space, inches):", args[0]);
    for (bone, m) in model.skeleton.bones.iter().zip(&world) {
        show(&bone.name, m);
    }
    let weapon = model.skeleton.find("weapon").map(|i| world[i]);
    if let (Some(w), Some(hand)) = (weapon, model.skeleton.find("ag1_hand_r")) {
        show(
            "ag1_hand_r rel. to weapon",
            &compose(&invert(&w), &world[hand]),
        );
    }

    let Some(clip_path) = args.get(1) else { return };
    let clip = Clip::parse(&data(&content, clip_path)).expect("clip");
    let skeleton = NmSkeleton::parse(&data(&content, &clip.skeleton));
    let pose: Vec<Transform> = clip.decode_frame(0).expect("frame 0");
    let mut model_space: Vec<Affine> = Vec::with_capacity(pose.len());
    for (i, t) in pose.iter().enumerate() {
        let local = t.to_affine();
        let m = match skeleton.parents[i] {
            Some(p) => compose(&model_space[p], &local),
            None => local,
        };
        model_space.push(m);
    }
    let find = |name: &str| skeleton.bones.iter().position(|b| b == name);
    println!("{clip_path} frame 0 (player model space):");
    for name in [
        "wpnPivot",
        "wpn",
        "wpnHand_R",
        "wpnHand_L",
        "hand_R",
        "hand_L",
    ] {
        if let Some(i) = find(name) {
            show(name, &model_space[i]);
        }
    }
    if let Some(w) = find("wpn") {
        let inv = invert(&model_space[w]);
        for name in ["wpnHand_R", "hand_R", "wpnHand_L", "hand_L"] {
            if let Some(i) = find(name) {
                show(
                    &format!("{name} rel. to wpn"),
                    &compose(&inv, &model_space[i]),
                );
            }
        }
    }
}
