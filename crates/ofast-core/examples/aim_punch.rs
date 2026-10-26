//! Developer tool: check the aim punch OFAST rebuilds from the shots ([`ofast_core::demo::recoil`]) against what
//! every shot shows of it: its aim (the fire-bullets message's, recoil included) minus the shooter's eye angles is
//! twice the punch.
//!
//! ```bash
//! cargo run --release --example aim_punch -- path/to/match.dem 5 [rows to print]
//! ```
//!
//! Per shot: the measured punch (pitch, yaw), the rebuilt one under a few orders of kicking and moving on, and how far
//! apart they are; then the median and spread of those gaps per order.

use std::collections::BTreeMap;

use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::demo::recoil::{
    AimPunch, BULLET_SCALE, RecoilPattern, VIEW_PUNCH_DECAY, VIEW_PUNCH_SCALE, view_punch_at,
};
use ofast_core::demo::shots::{Shot, shot_times};
use ofast_core::export::shots::ShotData;
use ofast_core::game::Cs2Install;

/// `a − b` in degrees, within ±180.
fn angle_diff(a: f32, b: f32) -> f32 {
    (a - b + 540.0).rem_euclid(360.0) - 180.0
}

/// Where the kicks come from: the gun's recoil table at the shot's recoil index, or the view punch's jump at the
/// shot (over its share of the kick) with the view punch decaying at 18 or 16 per second.
const ORDERS: [&str; 3] = [
    "the recoil table",
    "the view punch's jumps (decay 18)",
    "the view punch's jumps (decay 16)",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).and_then(|r| r.parse().ok()).unwrap_or(1);
    let rows: usize = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(30);
    let track = track_round(&data, round).expect("round tracks");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &track.map_name).expect("archives open");
    let items = content
        .read("scripts/items/items_game.txt")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let mut warnings = Vec::new();
    let shot_data = ShotData::load(&content, items.as_deref(), &mut warnings);
    let dt = track.tick_interval;
    // OFAST_TABLE=item: that weapon's recoil table, as the view punch jumps each kick gives.
    if let Some(item) = std::env::var("OFAST_TABLE")
        .ok()
        .and_then(|i| i.parse::<u32>().ok())
    {
        let weapon = shot_data.weapon(item).expect("a weapon");
        println!("{:?}", weapon.recoil);
        let pattern = RecoilPattern::new(&weapon.recoil);
        for shot in 0..12 {
            let (angle, magnitude) = pattern.kick(0, shot);
            let (sin, cos) = angle.to_radians().sin_cos();
            println!(
                "  kick {shot:2}: angle {angle:7.2} magnitude {magnitude:6.2} -> view punch jump ({:6.3}, {:6.3})",
                -cos * magnitude * VIEW_PUNCH_SCALE,
                -sin * magnitude * VIEW_PUNCH_SCALE
            );
        }
        return;
    }
    let times = shot_times(
        track
            .shots
            .iter()
            .map(|(_, s)| s)
            .chain(&track.earlier_shots),
    );
    // Each shooter's shots with their times, in order.
    let mut by_shooter: BTreeMap<u32, Vec<(f64, &Shot)>> = BTreeMap::new();
    for ((controller, shot), time) in track.shots.iter().zip(&times) {
        by_shooter
            .entry(*controller)
            .or_default()
            .push((*time, shot));
    }
    // The eye angles at a moment (demo ticks, fractional): between the samples about it (OFAST_EYES=tick: the
    // message's tick's sample instead).
    let at_tick = std::env::var("OFAST_EYES").is_ok_and(|v| v == "tick");
    let eyes = |controller: u32, tick: u32, time: f64| {
        let sample = |t: u32| {
            let at = track.samples.partition_point(|s| s.tick < t);
            let p = track
                .samples
                .get(at)?
                .players
                .iter()
                .find(|p| p.controller == controller)?;
            Some((p.pitch, p.yaw))
        };
        if at_tick {
            return sample(tick);
        }
        let (a, b) = (
            sample(time.floor() as u32)?,
            sample(time.floor() as u32 + 1)?,
        );
        let f = (time - time.floor()) as f32;
        Some((
            a.0 + angle_diff(b.0, a.0) * f,
            a.1 + angle_diff(b.1, a.1) * f,
        ))
    };
    let mut gaps: Vec<Vec<f32>> = vec![Vec::new(); ORDERS.len()];
    // Deep in sprays (the rebuilt pitch above 1° up): measured over rebuilt.
    let mut ratios: Vec<f32> = Vec::new();
    let mut printed = 0;
    // Each shooter's view punch jumps (when set, the jump over what was left of the last), with the view punch
    // decaying at `decay` per second.
    let jumps_of = |controller: u32, decay: f32| -> Vec<(f64, [f32; 2])> {
        let mut out = Vec::new();
        let mut last: Option<([f32; 2], f64)> = None;
        for sample in &track.samples {
            let Some(p) = sample.players.iter().find(|p| p.controller == controller) else {
                continue;
            };
            let Some(set) = p.rig.view_punch else {
                continue;
            };
            if last.is_some_and(|l| l.1 == set.1) {
                continue;
            }
            let left = last.map_or([0.0; 2], |(angle, when)| {
                let keep = (-decay * ((set.1 - when).max(0.0) as f32) * dt).exp();
                angle.map(|a| a * keep)
            });
            last = Some(set);
            out.push((set.1, [set.0[0] - left[0], set.0[1] - left[1]]));
        }
        out
    };
    for (controller, shots) in &by_shooter {
        for (order, gap) in gaps.iter_mut().enumerate() {
            let jumps = match order {
                1 => jumps_of(*controller, VIEW_PUNCH_DECAY),
                2 => jumps_of(*controller, 16.0),
                _ => Vec::new(),
            };
            let mut punch = AimPunch::default();
            let mut tick = shots[0].1.tick.saturating_sub(64);
            for (time, shot) in shots {
                let Some(weapon) = shot_data.weapon(shot.item) else {
                    continue;
                };
                let pattern = RecoilPattern::new(&weapon.recoil);
                // Move on to the shot's tick interval.
                let fired = time.floor() as u32;
                while tick < fired {
                    punch.advance(dt);
                    tick += 1;
                }
                let rebuilt = punch.angle;
                let Some((pitch, yaw)) = eyes(*controller, shot.tick, *time) else {
                    continue;
                };
                let measured = [
                    angle_diff(shot.angles[0], pitch) / BULLET_SCALE,
                    angle_diff(shot.angles[1], yaw) / BULLET_SCALE,
                ];
                let error = (rebuilt[0] - measured[0]).hypot(rebuilt[1] - measured[1]);
                gap.push(error);
                if order == 0 && rebuilt[0] < -1.0 {
                    ratios.push(measured[0] / rebuilt[0]);
                }
                if order == 0 && printed < rows {
                    printed += 1;
                    println!(
                        "controller {controller:3} tick {:6} item {:3} recoil index {:5.2}: measured ({:6.3}, {:6.3}), rebuilt ({:6.3}, {:6.3}), {error:.3}°",
                        shot.tick,
                        shot.item,
                        shot.recoil_index,
                        measured[0],
                        measured[1],
                        rebuilt[0],
                        rebuilt[1]
                    );
                }
                // The kick: the table's, or the view punch's jump at the shot over VIEW_PUNCH_SCALE.
                let seen = jumps
                    .iter()
                    .filter(|(when, _)| (when - time).abs() < 1.5)
                    .min_by(|a, b| (a.0 - time).abs().total_cmp(&(b.0 - time).abs()));
                match seen {
                    Some((_, jump)) => {
                        punch.velocity[0] += jump[0] / VIEW_PUNCH_SCALE;
                        punch.velocity[1] += jump[1] / VIEW_PUNCH_SCALE;
                    }
                    None => {
                        let (angle, magnitude) =
                            pattern.kick(shot.mode as usize, shot.recoil_index.floor() as usize);
                        punch.kick(angle, magnitude);
                    }
                }
            }
        }
    }
    // The view punch the demo sends, each time it is set: the jump over what was left of the last one, against
    // VIEW_PUNCH_SCALE of the kick of the shooter's shot at that moment.
    let mut jumps: Vec<f32> = Vec::new();
    let mut shown_jumps = 0;
    for (controller, shots) in &by_shooter {
        let mut last: Option<([f32; 2], f64)> = None;
        for sample in &track.samples {
            let Some(p) = sample.players.iter().find(|p| p.controller == *controller) else {
                continue;
            };
            let Some(set) = p.rig.view_punch else {
                continue;
            };
            if last.is_some_and(|l| l.1 == set.1) {
                continue;
            }
            let left = view_punch_at(last, set.1, dt);
            last = Some(set);
            let jump = [set.0[0] - left[0], set.0[1] - left[1]];
            let Some((_, shot)) = shots
                .iter()
                .min_by(|a, b| (a.0 - set.1).abs().total_cmp(&(b.0 - set.1).abs()))
                .filter(|(time, _)| (time - set.1).abs() < 1.5)
            else {
                continue;
            };
            let Some(weapon) = shot_data.weapon(shot.item) else {
                continue;
            };
            let (angle, magnitude) = RecoilPattern::new(&weapon.recoil)
                .kick(shot.mode as usize, shot.recoil_index.floor() as usize);
            let (sin, cos) = angle.to_radians().sin_cos();
            let predicted = [
                -cos * magnitude * VIEW_PUNCH_SCALE,
                -sin * magnitude * VIEW_PUNCH_SCALE,
            ];
            let gap = (jump[0] - predicted[0]).hypot(jump[1] - predicted[1]);
            jumps.push(gap);
            // The table entry nearest the jump.
            let pattern = RecoilPattern::new(&weapon.recoil);
            let (best, best_gap) = (0..64)
                .map(|i| {
                    let (a, m) = pattern.kick(shot.mode as usize, i);
                    let (s, c) = a.to_radians().sin_cos();
                    let p = [-c * m * VIEW_PUNCH_SCALE, -s * m * VIEW_PUNCH_SCALE];
                    (i, (jump[0] - p[0]).hypot(jump[1] - p[1]))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .expect("entries");
            if shown_jumps < rows {
                shown_jumps += 1;
                println!(
                    "view punch: controller {controller:3} at {:9.2} item {:3} index {:5.2} seed {:10}: jump ({:6.3}, {:6.3}), kick's ({:6.3}, {:6.3}), {gap:.4}°; nearest entry {best} ({best_gap:.4}°)",
                    set.1,
                    shot.item,
                    shot.recoil_index,
                    shot.seed,
                    jump[0],
                    jump[1],
                    predicted[0],
                    predicted[1]
                );
            }
        }
    }
    // Each shot against the nearest view punch set after it: how much later it is set (demo ticks), and the shots
    // with none within 3 ticks.
    let mut deltas: Vec<f64> = Vec::new();
    let mut unmatched = 0;
    for (controller, shots) in &by_shooter {
        let mut sets: Vec<f64> = Vec::new();
        for sample in &track.samples {
            if let Some(set) = sample
                .players
                .iter()
                .find(|p| p.controller == *controller)
                .and_then(|p| p.rig.view_punch)
                && sets.last() != Some(&set.1)
            {
                sets.push(set.1);
            }
        }
        for (time, _) in shots {
            match sets
                .iter()
                .map(|s| s - time)
                .filter(|d| d.abs() < 3.0)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
            {
                Some(d) => deltas.push(d),
                None => unmatched += 1,
            }
        }
    }
    deltas.sort_by(f64::total_cmp);
    let q = |f: f64| {
        deltas
            .get(((deltas.len() as f64 - 1.0) * f) as usize)
            .copied()
            .unwrap_or(f64::NAN)
    };
    println!(
        "view punch set minus shot time: {} shots, min {:.2}, 10 % {:.2}, median {:.2}, 90 % {:.2}, max {:.2}; {unmatched} shots with none within 3 ticks",
        deltas.len(),
        q(0.0),
        q(0.1),
        q(0.5),
        q(0.9),
        q(1.0)
    );
    jumps.sort_by(f32::total_cmp);
    println!(
        "view punch jumps: {}, against the kick's: median gap {:.4}°, 90 % {:.4}°, within 0.01° {}",
        jumps.len(),
        jumps.get(jumps.len() / 2).copied().unwrap_or(f32::NAN),
        jumps.get(jumps.len() * 9 / 10).copied().unwrap_or(f32::NAN),
        jumps.iter().filter(|&&g| g < 0.01).count()
    );
    ratios.sort_by(f32::total_cmp);
    println!(
        "deep in sprays: {} shots, measured / rebuilt pitch median {:.3} (quartiles {:.3} – {:.3})",
        ratios.len(),
        ratios.get(ratios.len() / 2).copied().unwrap_or(f32::NAN),
        ratios.get(ratios.len() / 4).copied().unwrap_or(f32::NAN),
        ratios
            .get(ratios.len() * 3 / 4)
            .copied()
            .unwrap_or(f32::NAN)
    );
    println!("order: shots, median gap, 90 %, within 0.01°, within 0.1°");
    for (order, mut gap) in gaps.into_iter().enumerate() {
        gap.sort_by(f32::total_cmp);
        let q = |f: f64| {
            gap.get(((gap.len() as f64 - 1.0) * f) as usize)
                .copied()
                .unwrap_or(f32::NAN)
        };
        println!(
            "  {:32} {:5} {:7.4} {:7.4} {:5} {:5}",
            ORDERS[order],
            gap.len(),
            q(0.5),
            q(0.9),
            gap.iter().filter(|&&g| g < 0.01).count(),
            gap.iter().filter(|&&g| g < 0.1).count()
        );
    }
}
