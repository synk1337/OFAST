//! Developer tool: what CS2's first-person data looks like, to check the viewmodel bake against (M5).
//!
//! ```bash
//! cargo run --release --example viewmodel_probe -- [clip] [weapon model] [agent model] [glove model]
//! ```
//!
//! Defaults: `idle_ak`, the AK-47's model, the SAS agent, no gloves. Prints:
//!
//! - the viewmodel clip skeleton (`viewmodel.vnmskel`) next to the arms model's (`weapon_arms.vmdl`) bones;
//! - for the clip at a few times: where `wpn`, the hands and their weapon targets (`wpnHand_L` / `_R`) are in the
//!   clip's model space (inches; the camera's axes: x forward, y left, z up), and how far each hand is from its
//!   target (what the graph's hand IK closes);
//! - the clip's secondary animations (the weapon's own bones) at time 0, their skeletons and root bones;
//! - the weapon model's bones next to its animation skeleton's;
//! - the agent's bind pose against the arms model's, its mesh groups and the bones its first-person meshes are
//!   skinned to;
//! - with a glove model, its mesh groups and meshes.

use ofast_core::anim::{Clip, NmSkeleton, Transform};
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

fn model(content: &Content, path: &str) -> Model {
    let bytes = content.read_resource(path).expect("model reads");
    Model::parse(&Resource::parse(&bytes).expect("model parses")).expect("model decodes")
}

/// Quaternion `q` applied to `v`.
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = q;
    let u = [x, y, z];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v).map(|c| 2.0 * c);
    let c = cross(u, t);
    [
        v[0] + w * t[0] + c[0],
        v[1] + w * t[1] + c[1],
        v[2] + w * t[2] + c[2],
    ]
}

fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Model-space (rotation, position) of every bone of a pose.
fn model_space(pose: &[Transform], parents: &[Option<usize>]) -> Vec<([f32; 4], [f32; 3])> {
    let mut out: Vec<([f32; 4], [f32; 3])> = Vec::with_capacity(pose.len());
    for (i, t) in pose.iter().enumerate() {
        out.push(match parents[i] {
            Some(p) => {
                let (pq, pt) = out[p];
                let r = rotate(pq, t.translation);
                (
                    mul(pq, t.rotation),
                    [pt[0] + r[0], pt[1] + r[1], pt[2] + r[2]],
                )
            }
            None => (t.rotation, t.translation),
        });
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let clip_path = args.first().map_or(
        "animation/anims/viewmodel/rifle/rifle_ak/idle_ak.vnmclip",
        String::as_str,
    );
    let weapon_path = args
        .get(1)
        .map_or("weapons/models/ak47/weapon_rif_ak47.vmdl", String::as_str);
    let agent_path = args
        .get(2)
        .map_or("agents/models/ctm_sas/ctm_sas.vmdl", String::as_str);
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");

    let clip = Clip::parse(&data(&content, clip_path)).expect("clip decodes");
    let skeleton = NmSkeleton::parse(&data(&content, &clip.skeleton));
    println!(
        "clip {clip_path}: {} frames, {:.3} s, additive {}, skeleton {} ({} bones), {} secondary",
        clip.frames,
        clip.duration,
        clip.additive,
        clip.skeleton,
        skeleton.bones.len(),
        clip.secondary.len()
    );
    println!(
        "events: {:?}",
        clip.events
            .iter()
            .map(|e| format!("{} {:.2}+{:.2}", e.id, e.start, e.duration))
            .collect::<Vec<_>>()
    );
    let arms = model(&content, "weapons/models/shared/arms/weapon_arms.vmdl");
    let missing: Vec<&String> = skeleton
        .bones
        .iter()
        .filter(|b| arms.skeleton.find(b).is_none())
        .collect();
    println!(
        "weapon_arms.vmdl: {} bones, meshes {:?}, groups {:?}; clip bones it lacks: {missing:?}",
        arms.skeleton.bones.len(),
        arms.meshes.iter().map(|m| &m.name).collect::<Vec<_>>(),
        arms.mesh_groups,
    );
    let extra: Vec<&str> = arms
        .skeleton
        .bones
        .iter()
        .filter(|b| skeleton.find(&b.name).is_none())
        .map(|b| b.name.as_str())
        .collect();
    println!("  arms bones the clip skeleton lacks: {extra:?}");
    if let Some(gloves) = args.get(3) {
        let gloves = model(&content, gloves);
        println!(
            "gloves: groups {:?} (default {:b}), meshes {:?}",
            gloves.mesh_groups,
            gloves.default_mesh_group_mask,
            gloves.meshes.iter().map(|m| &m.name).collect::<Vec<_>>()
        );
    }

    let bone = |name: &str| skeleton.find(name).expect("bone");
    for time in [0.0, clip.duration * 0.5, clip.duration] {
        let pose = clip.sample(time, false).expect("samples");
        let model = model_space(&pose, &skeleton.parents);
        println!("t = {time:.2} s:");
        for name in [
            "root_motion",
            "wpn",
            "hand_L",
            "wpnHand_L",
            "hand_R",
            "wpnHand_R",
        ] {
            let (q, t) = model[bone(name)];
            println!(
                "  {name:12} at ({:7.2}, {:7.2}, {:7.2}) rot ({:.3}, {:.3}, {:.3}, {:.3})",
                t[0], t[1], t[2], q[0], q[1], q[2], q[3]
            );
        }
        for (hand, target) in [("hand_L", "wpnHand_L"), ("hand_R", "wpnHand_R")] {
            let (_, a) = model[bone(hand)];
            let (_, b) = model[bone(target)];
            let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
            println!("  {hand} is {d:.3} in from {target}");
        }
    }

    for secondary in &clip.secondary {
        let skeleton = NmSkeleton::parse(&data(&content, &secondary.skeleton));
        let pose = secondary.sample(0.0, false).expect("secondary samples");
        println!(
            "secondary on {} ({} bones: {:?}), {} frames",
            secondary.skeleton,
            skeleton.bones.len(),
            skeleton.bones,
            secondary.frames
        );
        for (i, t) in pose.iter().enumerate().take(4) {
            println!(
                "  {:10} parent {:?} local ({:.2}, {:.2}, {:.2}) rot ({:.3}, {:.3}, {:.3}, {:.3})",
                skeleton.bones[i],
                skeleton.parents[i],
                t.translation[0],
                t.translation[1],
                t.translation[2],
                t.rotation[0],
                t.rotation[1],
                t.rotation[2],
                t.rotation[3]
            );
        }
        let weapon = model(&content, weapon_path);
        let names: Vec<&str> = weapon
            .skeleton
            .bones
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        let lacking: Vec<&String> = skeleton
            .bones
            .iter()
            .filter(|b| weapon.skeleton.find(b).is_none())
            .collect();
        println!(
            "weapon model {weapon_path}: bones {names:?}; animation bones it lacks: {lacking:?}"
        );
        let world = weapon.skeleton.world_transforms();
        for (i, b) in weapon.skeleton.bones.iter().enumerate().take(4) {
            let m = world[i];
            println!(
                "  model bone {:10} parent {:?} at ({:.2}, {:.2}, {:.2})",
                b.name, b.parent, m[0][3], m[1][3], m[2][3]
            );
        }
    }

    let agent = model(&content, agent_path);
    // The agent's bind pose against the arms model's, bone by bone (what skinning by bone name relies on).
    let (agent_world, arms_world) = (
        agent.skeleton.world_transforms(),
        arms.skeleton.world_transforms(),
    );
    let mut worst = (0.0f32, String::new());
    for (i, b) in agent.skeleton.bones.iter().enumerate() {
        if let Some(j) = arms.skeleton.find(&b.name) {
            let (a, m) = (agent_world[i], arms_world[j]);
            let d = (0..3)
                .map(|k| (a[k][3] - m[k][3]).powi(2))
                .sum::<f32>()
                .sqrt();
            if d > worst.0 {
                worst = (d, b.name.clone());
            }
        }
    }
    println!(
        "agent vs arms bind pose: largest bone position difference {:.3} in ({})",
        worst.0, worst.1
    );
    println!("agent {agent_path} mesh groups:");
    for (i, group) in agent.mesh_groups.iter().enumerate() {
        println!(
            "  {i:2} {group} (default {})",
            agent.default_mesh_group_mask >> i & 1
        );
    }
    for (i, group) in agent.mesh_groups.iter().enumerate() {
        if !group.contains("firstperson") {
            continue;
        }
        for mesh in agent.meshes_in_groups(1 << i) {
            let remap = agent.bone_remap(mesh.mesh_index);
            let mut bones: Vec<&str> = remap
                .iter()
                .filter_map(|&b| agent.skeleton.bones.get(b as usize))
                .map(|b| b.name.as_str())
                .collect();
            bones.sort_unstable();
            bones.dedup();
            let unknown: Vec<&&str> = bones
                .iter()
                .filter(|b| skeleton.find(b).is_none())
                .collect();
            println!(
                "  group {i} mesh {}: {} draw calls, {} bones, not in the viewmodel skeleton: {unknown:?}",
                mesh.name,
                mesh.draw_calls.len(),
                bones.len()
            );
        }
    }
}
