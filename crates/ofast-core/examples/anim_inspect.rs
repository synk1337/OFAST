//! Developer tool: decode a CS2 animation clip and check it against a model's skeleton.
//!
//! ```bash
//! cargo run --release --example anim_inspect -- agents/models/ctm_sas/ctm_sas.vmdl \
//!     animation/anims/world/rifle/_default_rifle/run_n_rifle.vnmclip 0,0.2,0.4
//! ```
//!
//! Prints the clip (frames, fps, additive, tracks), checks every decoded rotation is a unit quaternion, and how well
//! the clip skeleton's reference pose, retargeted onto the model, lands on the model's bind pose (it should, to the
//! data's precision, for every bone both skeletons share).
//!
//! The optional third argument lists times (seconds): for each, where a few bones of the retargeted pose sit in
//! Unreal's axes (cm, `(x, −y, z) × 2.54` of the model's space), the same bones `tools/unreal/import_and_shot.py`
//! logs while the imported clip plays — the two should agree.

use ofast_core::anim::{Clip, NmSkeleton, Retargeter};
use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::model::Model;
use ofast_core::resource::{BlockKind, Resource};

fn data(content: &Content, path: &str) -> Value {
    let bytes = content.read_resource(path).expect("resource reads");
    Resource::parse(&bytes)
        .expect("resource parses")
        .kv3(BlockKind::DATA)
        .expect("DATA block")
        .root
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model_path = args
        .first()
        .map_or("agents/models/ctm_sas/ctm_sas.vmdl", String::as_str);
    let clip_path = args.get(1).map_or(
        "animation/anims/world/rifle/_default_rifle/run_n_rifle.vnmclip",
        String::as_str,
    );
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");

    let bytes = content.read_resource(model_path).expect("model reads");
    let model =
        Model::parse(&Resource::parse(&bytes).expect("model parses")).expect("model decodes");
    let clip = Clip::parse(&data(&content, clip_path)).expect("clip decodes");
    let skeleton = NmSkeleton::parse(&data(&content, &clip.skeleton));
    println!(
        "clip {clip_path}: {} frames, {:.3} s, {:.2} fps, additive {}, {} tracks, {} u16 of pose data, {} secondary",
        clip.frames,
        clip.duration,
        clip.fps(),
        clip.additive,
        clip.tracks.len(),
        clip.data.len(),
        clip.secondary.len()
    );
    println!(
        "clip skeleton {}: {} bones",
        clip.skeleton,
        skeleton.bones.len()
    );

    let mut worst_norm = 0.0f32;
    for f in 0..clip.frames {
        for t in clip.decode_frame(f).expect("frame decodes") {
            let norm = t.rotation.iter().map(|c| c * c).sum::<f32>().sqrt();
            worst_norm = worst_norm.max((norm - 1.0).abs());
        }
    }
    println!("rotations: largest |length − 1| over every frame {worst_norm:.2e}");

    let retargeter = Retargeter::new(&model.skeleton, &skeleton);
    println!(
        "model {model_path}: {} bones, {} driven by the clip",
        model.skeleton.bones.len(),
        retargeter.mapped_bones()
    );
    let world = retargeter.world(&skeleton.reference);
    let bind = model.skeleton.world_transforms();
    let mut rows: Vec<(f32, f32, &str)> = Vec::new();
    for (m, bone) in model.skeleton.bones.iter().enumerate() {
        if skeleton.find(&bone.name).is_none() || bone.parent.is_none() {
            continue;
        }
        let distance = (0..3)
            .map(|i| (world[m][i][3] - bind[m][i][3]).powi(2))
            .sum::<f32>()
            .sqrt();
        // Largest difference between rotation matrix entries.
        let rotation = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| (world[m][i][j] - bind[m][i][j]).abs())
            .fold(0.0f32, f32::max);
        rows.push((distance, rotation, bone.name.as_str()));
    }
    rows.sort_by(|a, b| (b.0 + b.1).total_cmp(&(a.0 + a.1)));
    println!(
        "reference pose vs bind pose, worst shared bones (position in inches, rotation matrix entry):"
    );
    for (distance, rotation, name) in rows.iter().take(8) {
        println!("  {name:<20} {distance:8.4}  {rotation:8.4}");
    }

    let times: Vec<f32> = args
        .get(2)
        .map(|list| {
            list.split(',')
                .filter_map(|t| t.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default();
    for time in times {
        let world = retargeter.world(&clip.sample(time, false).expect("clip samples"));
        println!("pose at {time:.3} s (Unreal axes, cm):");
        for name in CHECK_BONES {
            let Some(b) = model.skeleton.find(name) else {
                continue;
            };
            let [x, y, z] = [world[b][0][3], world[b][1][3], world[b][2][3]];
            println!(
                "  {name:<12} ({:7.1}, {:7.1}, {:7.1})",
                x * 2.54,
                -y * 2.54,
                z * 2.54
            );
        }
    }
}

/// Bones whose positions are printed per time: the ones `import_and_shot.py` logs (its `CHECK_BONES`).
const CHECK_BONES: [&str; 6] = [
    "root_motion",
    "pelvis",
    "head_0",
    "hand_R",
    "hand_L",
    "ankle_R",
];
