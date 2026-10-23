//! Developer tool: what the first-person bake decides for each player of a demo round ([`viewmodel_timeline`]: the
//! weapon actions, their blend times and the events they start with), and where the baked viewmodel puts the
//! weapon at some ticks — to check against CS2's own first-person view.
//!
//! ```bash
//! cargo run --release --example viewmodel_timeline -- path/to/match.dem 5
//! cargo run --release --example viewmodel_timeline -- path/to/match.dem 5 FalleN 37683,37700
//! ```
//!
//! The optional third argument keeps only the player whose name contains it; the fourth lists demo ticks at which to
//! print `wpn` and the hands in the camera's frame (inches: x forward, y left, z up) and the weapon's bone scales.

use ofast_core::anim::Transform;
use ofast_core::anim::viewmodel::{bake_viewmodel, viewmodel_timeline};
use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::export::players::load_player_rig;
use ofast_core::export::viewmodel::load_viewmodel_rig;
use ofast_core::game::Cs2Install;

/// `v` turned by the unit quaternion `q` (`x, y, z, w`).
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
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
        v[0] + q[3] * t[0] + c[0],
        v[1] + q[3] * t[1] + c[1],
        v[2] + q[3] * t[2] + c[2],
    ]
}

/// Model-space positions of a pose's bones.
fn positions(pose: &[Transform], parents: &[Option<usize>]) -> Vec<[f32; 3]> {
    let mut world: Vec<([f32; 4], [f32; 3])> = Vec::with_capacity(pose.len());
    for (i, t) in pose.iter().enumerate() {
        world.push(match parents[i] {
            Some(p) => {
                let (q, o) = world[p];
                let r = rotate(q, t.translation);
                let [x, y, z, w] = q;
                let [a, b, c, d] = t.rotation;
                (
                    [
                        w * a + x * d + y * c - z * b,
                        w * b - x * c + y * d + z * a,
                        w * c + x * b - y * a + z * d,
                        w * d - x * a - y * b - z * c,
                    ],
                    [o[0] + r[0], o[1] + r[1], o[2] + r[2]],
                )
            }
            None => (t.rotation, t.translation),
        });
    }
    world.into_iter().map(|(_, t)| t).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let who = args.get(2).cloned().unwrap_or_default();
    let ticks: Vec<f32> = args
        .get(3)
        .map(|t| t.split(',').map(|t| t.parse().expect("tick")).collect())
        .unwrap_or_default();
    let track = track_round(&data, round).expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let player_rig = load_player_rig(&content, &track).expect("rig");
    let mut weapons: Vec<(u32, String, String)> = player_rig
        .upper
        .as_ref()
        .expect("weapon names")
        .weapons
        .iter()
        .map(|(&item, w)| (item, w.weapon.clone(), w.category.clone()))
        .collect();
    weapons.sort();
    let mut warnings = Vec::new();
    let rig = load_viewmodel_rig(&content, &weapons, &mut warnings).expect("viewmodel rig");
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    for (item, w) in &rig.weapons {
        println!(
            "item {item}: {} ({}) in {} node {}, own bones {:?}",
            w.weapon,
            w.category,
            w.graph,
            w.node,
            w.skeleton.as_ref().map(|(p, _)| p)
        );
    }
    let mut players: Vec<(u32, String)> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .map(|p| (p.controller, p.name.clone()))
        .collect();
    players.sort();
    players.dedup();
    let fps = 1.0 / track.tick_interval;
    let bone = |name: &str| rig.skeleton.find(name).expect("bone");
    for (controller, name) in players {
        if !name.contains(who.as_str()) {
            continue;
        }
        println!("== {name} (controller {controller})");
        for (tick, line) in viewmodel_timeline(&track, controller, &rig).expect("timeline") {
            println!("{tick:6} {line}");
        }
        if ticks.is_empty() {
            continue;
        }
        let frames = bake_viewmodel(&track, controller, &rig, fps).expect("bake");
        for &tick in &ticks {
            let k = ((tick - track.start_tick as f32) * track.tick_interval * fps).round() as usize;
            match frames.get(k).and_then(Option::as_ref) {
                Some(frame) => {
                    let p = positions(&frame.arms, &rig.skeleton.parents);
                    let at = |b: &str| {
                        let v = p[bone(b)];
                        format!("({:6.2}, {:6.2}, {:6.2})", v[0], v[1], v[2])
                    };
                    println!(
                        "  tick {tick}: item {} wpn {} hand_L {} → {} hand_R {} → {}, weapon scales {:?}",
                        frame.item,
                        at("wpn"),
                        at("hand_L"),
                        at("wpnHand_L"),
                        at("hand_R"),
                        at("wpnHand_R"),
                        frame
                            .weapon
                            .as_ref()
                            .map(|w| w.iter().map(|t| t.scale).collect::<Vec<_>>())
                    );
                }
                None => println!("  tick {tick}: no viewmodel"),
            }
        }
    }
}
