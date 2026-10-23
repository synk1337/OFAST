//! Developer tool: an agent's ragdoll joints measured on CS2's own animation poses — the swing (angle between the
//! two joint frames' twist axes) and twist (turn of the next axes about their mean) of every joint, with the twist
//! axis taken as x, y and z in turn, against the joint's limits — to check how the solver reads the limits. A joint
//! without a swing limit counts as a hinge (no swing), as the solver holds it.
//!
//! ```bash
//! cargo run --release --example ragdoll_limits -- agents/models/ctm_sas/ctm_sas.vmdl \
//!     animation/anims/world/rifle/_default_rifle/run_n_rifle.vnmclip 0.3
//! ```
//!
//! Arguments: model, then clip and time pairs.

use ofast_core::anim::{Clip, NmSkeleton, Retargeter};
use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::model::Model;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::joint_angles_about;
use ofast_core::resource::{BlockKind, Resource};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let bytes = content.read_resource(&args[0]).expect("model reads");
    let resource = Resource::parse(&bytes).expect("model parses");
    let model = Model::parse(&resource).expect("model decodes");
    let physics = PhysicsData::from_resource(&resource)
        .expect("PHYS reads")
        .expect("a ragdoll");
    let data = |path: &str| {
        let bytes = content.read_resource(path).expect("resource reads");
        Resource::parse(&bytes)
            .expect("parses")
            .kv3(BlockKind::DATA)
            .expect("DATA")
            .root
    };
    let bone_of = |name: &str| {
        model
            .skeleton
            .bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
            .expect("bone")
    };
    for pair in args[1..].chunks(2) {
        let (clip_path, time) = (&pair[0], pair[1].parse::<f32>().expect("time"));
        // `bind 0`: the ragdoll's own bind pose.
        let frames: Vec<_> = if clip_path == "bind" {
            physics.bind_pose.clone()
        } else {
            let clip = Clip::parse(&data(clip_path)).expect("clip decodes");
            let skeleton = NmSkeleton::parse(&data(&clip.skeleton));
            let pose = clip.sample(time, false).expect("samples");
            let world = Retargeter::new(&model.skeleton, &skeleton).world(&pose);
            physics.bones.iter().map(|b| world[bone_of(b)]).collect()
        };
        println!(
            "{clip_path} at {time} (swing / twist with the twist about x | y | z; !! = outside the limit):"
        );
        let mut outside = [0usize; 3];
        for joint in &physics.joints {
            let mut cells = Vec::new();
            for (axis, count) in outside.iter_mut().enumerate() {
                let (swing, twist) =
                    joint_angles_about(joint, &frames[joint.body1], &frames[joint.body2], axis);
                let swing_ok = swing <= joint.swing.map_or(0.0, |(_, max)| max) + 0.02;
                let twist_ok = joint
                    .twist
                    .is_none_or(|(min, max)| twist >= min - 0.02 && twist <= max + 0.02);
                *count += usize::from(!swing_ok) + usize::from(!twist_ok);
                let flag = |ok: bool| if ok { " " } else { "!" };
                cells.push(format!(
                    "{:6.1}{} {:6.1}{}",
                    swing.to_degrees(),
                    flag(swing_ok),
                    twist.to_degrees(),
                    flag(twist_ok)
                ));
            }
            println!(
                "  {:>12} -> {:<12} | {} | swing {:>6} twist {:>15}",
                physics.bones[joint.body1],
                physics.bones[joint.body2],
                cells.join(" | "),
                joint.swing.map_or("none".into(), |(_, max)| format!(
                    "{:.1}°",
                    max.to_degrees()
                )),
                joint.twist.map_or("none".into(), |(min, max)| format!(
                    "{:.1}° … {:.1}°",
                    min.to_degrees(),
                    max.to_degrees()
                )),
            );
        }
        println!(
            "  outside the limits: x {} | y {} | z {}",
            outside[0], outside[1], outside[2]
        );
    }
}
