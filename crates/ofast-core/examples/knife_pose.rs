//! Developer tool: where a player's right hand, weapon and head end up after each layer of the bake — the
//! locomotion clip, the weapon's additive idle pose, the aim — at one tick, to find which layer places the hand.
//!
//! ```bash
//! cargo run --release --example knife_pose -- path/to/match.dem 5 26508 6
//! ```
//!
//! Arguments: demo, round, tick, controller. Positions are model space, inches (x forward, y left, z up).

use ofast_core::anim::aim::{AimInputs, aim};
use ofast_core::anim::bake::{WeaponSet, body_relative};
use ofast_core::anim::layers::add_layer;
use ofast_core::anim::{NmSkeleton, Transform};
use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::export::players::load_player_rig;
use ofast_core::game::Cs2Install;
use ofast_core::model::quat_mul;

/// `v` turned by the unit quaternion `q` (`x, y, z, w`).
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let u = [q[0], q[1], q[2]];
    let t = cross(u, v).map(|c| 2.0 * c);
    let c = cross(u, t);
    [
        v[0] + q[3] * t[0] + c[0],
        v[1] + q[3] * t[1] + c[1],
        v[2] + q[3] * t[2] + c[2],
    ]
}

/// Model-space positions of the bones.
fn positions(skeleton: &NmSkeleton, pose: &[Transform]) -> Vec<[f32; 3]> {
    let mut world: Vec<([f32; 4], [f32; 3])> = Vec::new();
    for (bone, local) in pose.iter().enumerate() {
        world.push(match skeleton.parents[bone] {
            Some(p) => {
                let (q, t) = world[p];
                let r = rotate(q, local.translation);
                (
                    quat_mul(q, local.rotation),
                    [t[0] + r[0], t[1] + r[1], t[2] + r[2]],
                )
            }
            None => (local.rotation, local.translation),
        });
    }
    world.into_iter().map(|(_, t)| t).collect()
}

/// An angle in −180 … 180.
fn wrap(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args[1].parse().expect("round");
    let tick: u32 = args[2].parse().expect("tick");
    let controller: u32 = args[3].parse().expect("controller");
    let track = track_round(&data, round).expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let rig = load_player_rig(&content, &track).expect("rig");
    let upper = rig.upper.as_ref().expect("upper body");
    let sample = track
        .samples
        .iter()
        .find(|s| s.tick == tick)
        .expect("tick in the round");
    let player = sample
        .players
        .iter()
        .find(|p| p.controller == controller)
        .expect("player");
    let item = player.rig.weapon.as_ref().map(|w| w.item);
    let weapon = item.and_then(|i| upper.weapons.get(&i)).expect("weapon");
    println!(
        "{} holds {} ({}), pitch {:.1}, eye yaw − body yaw {:.1}",
        player.name,
        weapon.weapon,
        weapon.category,
        player.pitch,
        wrap(player.yaw - player.rig.body_yaw)
    );
    let set = WeaponSet::for_category(&weapon.category);
    let locomotion = rig.sets.get(set);
    let blend = locomotion.blend(
        body_relative(player.rig.velocity, player.rig.body_yaw),
        player.rig.duck_amount,
    );
    let skeleton = &rig.clip_skeleton;
    let bone = |name: &str| skeleton.find(name).expect(name);
    let report = |stage: &str, pose: &[Transform]| {
        let p = positions(skeleton, pose);
        let show = |name: &str| {
            let v = p[bone(name)];
            format!("{name} ({:6.1} {:6.1} {:6.1})", v[0], v[1], v[2])
        };
        println!(
            "{stage:12} {}  {}  {}  {}  {}",
            show("hand_R"),
            show("wpn"),
            show("wpnPivot"),
            show("spine_3"),
            show("head_0")
        );
    };
    for phase in [0.0, 0.25, 0.5, 0.75] {
        println!("phase {phase}");
        let mut pose = locomotion.sample(&blend, phase).expect("locomotion");
        report("locomotion", &pose);
        if let Some(idle) = &weapon.idle {
            let delta = idle[0].sample(0.0, false).expect("idle");
            add_layer(&mut pose, &delta, 1.0, Some(&upper.mask));
            report("+ idle", &pose);
        }
        aim(
            &mut pose,
            &upper.aim,
            &AimInputs {
                yaw: wrap(player.yaw - player.rig.body_yaw),
                pitch: player.pitch,
                crouch: player.rig.duck_amount,
                action_weight: 0.0,
                hand_ik_weight: 1.0,
                category: weapon.category.clone(),
                weapon: weapon.weapon.clone(),
            },
        );
        report("+ aim", &pose);
    }
}
