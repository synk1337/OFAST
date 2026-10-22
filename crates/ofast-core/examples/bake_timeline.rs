//! Developer tool: what the bake decides for each player of a demo round — movement states (starts, plants and
//! turns, turning on the spot, air, ladders), weapons-layer actions, plants, defuses and body additives — tick by
//! tick, to check them against the demo and CS2.
//!
//! ```bash
//! cargo run --release --example bake_timeline -- path/to/match.dem 5
//! cargo run --release --example bake_timeline -- path/to/match.dem 5 Ladder,defuse
//! ```
//!
//! The optional third argument keeps only lines containing one of its comma-separated words. `OFAST_CHECK=1` also
//! bakes every player at the tick rate and reports frames with non-finite bone values.

use ofast_core::anim::bake::{bake_player, timeline};
use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::export::players::load_player_rig;
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let filter: Vec<String> = args
        .get(2)
        .map(|f| f.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let track = track_round(&data, round).expect("round");
    // `OFAST_EVENTS=1`: the round's game events by name, with the keys of the first of each.
    if std::env::var_os("OFAST_EVENTS").is_some() {
        let mut counts: std::collections::BTreeMap<&str, (usize, String)> = Default::default();
        for e in &track.events {
            let entry = counts
                .entry(e.name.as_str())
                .or_insert_with(|| (0, format!("{:?}", e.keys)));
            entry.0 += 1;
        }
        for (name, (n, keys)) in counts {
            println!("{n:5} {name}: {}", &keys[..keys.len().min(300)]);
        }
    }
    // `OFAST_EVENT=<name>`: every event of that name, tick and keys.
    if let Ok(name) = std::env::var("OFAST_EVENT") {
        for e in track.events.iter().filter(|e| e.name == name) {
            println!("{:6} {:?}", e.tick, e.keys);
        }
    }
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let rig = load_player_rig(&content, &track).expect("rig");
    for warning in &rig.warnings {
        eprintln!("warning: {warning}");
    }
    let mut players: Vec<(u32, String)> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .map(|p| (p.controller, p.name.clone()))
        .collect();
    players.sort();
    players.dedup();
    for (controller, name) in players {
        let lines = timeline(
            &track,
            controller,
            &rig.sets,
            rig.upper.as_ref(),
            rig.graphs.as_ref(),
        )
        .expect("bake decisions");
        for (tick, what) in lines {
            if filter.is_empty() || filter.iter().any(|f| what.contains(f.as_str())) {
                println!("{tick:6} {controller:2} {name:16} {what}");
            }
        }
        if std::env::var_os("OFAST_CHECK").is_some() {
            let fps = (1.0 / track.tick_interval).round();
            let poses = bake_player(
                &track,
                controller,
                &rig.sets,
                rig.upper.as_ref(),
                rig.graphs.as_ref(),
                fps,
            )
            .expect("bake");
            let bad: Vec<usize> = poses
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.as_ref().is_some_and(|p| {
                        p.iter().any(|t| {
                            !t.scale.is_finite()
                                || t.translation
                                    .iter()
                                    .chain(&t.rotation)
                                    .any(|v| !v.is_finite())
                        })
                    })
                })
                .map(|(i, _)| i)
                .collect();
            // `OFAST_PROBE=<tick>`: where a few bones are (model space, inches) at that tick.
            if let Some(probe) = std::env::var("OFAST_PROBE")
                .ok()
                .and_then(|t| t.parse::<f32>().ok())
            {
                let frame = ((probe - track.start_tick as f32) * track.tick_interval * fps).round()
                    as usize;
                if let Some(Some(pose)) = poses.get(frame) {
                    let skeleton = &rig.clip_skeleton;
                    let mut world: Vec<([f32; 4], [f32; 3])> = Vec::new();
                    for (bone, local) in pose.iter().enumerate() {
                        world.push(match skeleton.parents[bone] {
                            Some(p) => {
                                let (q, t) = world[p];
                                let r = rotate(q, local.translation);
                                (
                                    ofast_core::model::quat_mul(q, local.rotation),
                                    [t[0] + r[0], t[1] + r[1], t[2] + r[2]],
                                )
                            }
                            None => (local.rotation, local.translation),
                        });
                    }
                    for name in ["root_motion", "pelvis", "head_0", "hand_R", "wpn"] {
                        if let Some(i) = skeleton.bones.iter().position(|b| b == name) {
                            let t = world[i].1;
                            println!(
                                "{controller:2} {name:16} at {probe}: {name} ({:.1}, {:.1}, {:.1})",
                                t[0], t[1], t[2]
                            );
                        }
                    }
                }
            }
            if let (Some(first), Some(last)) = (bad.first(), bad.last()) {
                let tick =
                    |i: usize| track.start_tick as f32 + i as f32 / fps / track.tick_interval;
                println!(
                    "{controller:2} {name:16} NON-FINITE in {} frames, ticks {:.0} – {:.0}",
                    bad.len(),
                    tick(*first),
                    tick(*last)
                );
            }
        }
    }
}
