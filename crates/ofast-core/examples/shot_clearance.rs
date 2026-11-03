//! Developer tool: how close the shot cameras come to the players in a demo's rounds, with the map's collision.
//!
//! ```bash
//! cargo run --release --example shot_clearance -- path/to/match.dem [round ...]
//! ```
//!
//! For each kill of the rounds (all rounds without a list) it builds the "behind the killer" and the fixed
//! ("tripod") shot as the export does ([`ofast_core::camera::shots::build`], starting [`LEAD_S`] before the kill),
//! then prints, per shot, the nearest the camera comes to the killer's eyes (and, for the tripod, to the victim's
//! while alive) and how many of its ticks are cramped: within [`CRAMPED`] units of the killer's eyes with his head
//! less than [`MIDDLE`] from where the camera looks, the back of his head filling the picture (a camera risen over his
//! head, looking past it, is near but not cramped). A summary line per kind closes the output, with how often a
//! camera jumped ([`JUMP`]).

use std::path::PathBuf;

use ofast_core::camera::forward;
use ofast_core::camera::shots::{LEAD_S, Poses, ShotKind, ShotSpec, build};
use ofast_core::content::Content;
use ofast_core::demo::players::{RoundTrack, track_round};
use ofast_core::demo::summary::summarize;
use ofast_core::game::Cs2Install;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::WorldCollision;
use ofast_core::resource::Resource;

/// Nearer than this to the killer's eyes (units), with his head in the picture's middle, the camera is cramped.
const CRAMPED: f64 = 32.0;

/// The picture's middle: this near the direction the camera looks (degrees).
const MIDDLE: f64 = 30.0;

/// The angle (degrees) between where a camera at `position` looks (`angles`, pitch and yaw) and `point`.
fn off_centre(position: [f64; 3], angles: [f64; 3], point: [f64; 3]) -> f64 {
    let look = forward(angles[0], angles[1]);
    let to = [
        point[0] - position[0],
        point[1] - position[1],
        point[2] - position[2],
    ];
    let cos = (look[0] * to[0] + look[1] * to[1] + look[2] * to[2]) / distance(point, position);
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

/// A camera moving more than this between two ticks (units) jumped: 12 a tick is 768 a second at 64 ticks.
const JUMP: f64 = 12.0;

/// How long a shot goes on after its kill (seconds).
const AFTER_S: f64 = 1.0;

fn collision(install: &Cs2Install, map: &str) -> Option<WorldCollision> {
    let content = Content::for_map(install, map).ok()?;
    let bytes = content
        .read_resource(&format!("maps/{map}/world_physics.vmdl"))
        .ok()?;
    let resource = Resource::parse(&bytes).ok()?;
    let physics = PhysicsData::from_resource(&resource).ok()??;
    Some(WorldCollision::from_physics(&physics))
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Per kind: shots, shots with a cramped tick, cramped ticks, all ticks, jumps.
#[derive(Default)]
struct Tally {
    shots: usize,
    cramped_shots: usize,
    cramped_ticks: usize,
    ticks: usize,
    jumps: usize,
}

fn round(track: &RoundTrack, world: Option<&WorldCollision>, tallies: &mut [Tally; 2]) {
    let poses = Poses::new(track);
    let interval = f64::from(track.tick_interval);
    let lead = (LEAD_S / interval).round() as u32;
    let after = (AFTER_S / interval).round() as u32;
    for event in track.events.iter().filter(|e| e.name == "player_death") {
        let Some(killer) = event.controller("attacker") else {
            continue;
        };
        let Some(killer_poses) = poses.of(killer) else {
            continue;
        };
        let victim = event.controller("userid").filter(|&v| v != killer);
        for (n, kind) in [ShotKind::Behind, ShotKind::Tripod].into_iter().enumerate() {
            let spec = ShotSpec {
                kind,
                subject: killer,
                other: victim,
                start: event.tick.saturating_sub(lead).max(track.start_tick),
                focus: event.tick,
                end: (event.tick + after).min(track.end_tick),
            };
            let Some((made, keys)) = build(&spec, &poses, world, interval) else {
                continue;
            };
            let mut nearest = f64::INFINITY;
            let mut nearest_victim = f64::INFINITY;
            let mut cramped = 0;
            for key in &keys {
                let i = poses.ticks().partition_point(|&t| f64::from(t) < key.tick);
                let Some(pose) = killer_poses.get(i).copied().flatten() else {
                    continue;
                };
                let d = distance(key.position, pose.eyes);
                nearest = nearest.min(d);
                cramped += usize::from(
                    d < CRAMPED && off_centre(key.position, key.angles, pose.eyes) < MIDDLE,
                );
                if let Some(v) = victim
                    .and_then(|v| poses.of(v))
                    .and_then(|v| v.get(i).copied().flatten())
                    .filter(|v| v.alive)
                {
                    nearest_victim = nearest_victim.min(distance(key.position, v.eyes));
                }
            }
            let tally = &mut tallies[n];
            tally.shots += 1;
            tally.cramped_shots += usize::from(cramped > 0);
            tally.cramped_ticks += cramped;
            tally.ticks += keys.len();
            // A camera moving faster than any player runs between two ticks jumped.
            let jumps = keys
                .windows(2)
                .filter(|w| distance(w[1].position, w[0].position) > JUMP)
                .count();
            tally.jumps += jumps;
            println!(
                "r{:<2} tick {:>6} {:<7} {:<6} nearest killer {:>6.1}  victim {:>7.1}  cramped {:>3} of {}",
                track.round,
                event.tick,
                poses.name(killer).unwrap_or("?"),
                made.name(),
                nearest,
                nearest_victim,
                cramped,
                keys.len()
            );
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("a demo path"));
    let wanted: Vec<u32> = args.filter_map(|a| a.parse().ok()).collect();
    let data = std::fs::read(&path).expect("demo read");
    let install = Cs2Install::locate().expect("CS2 install found");
    let rounds: Vec<u32> = if wanted.is_empty() {
        let summary = summarize(&data).expect("demo summary");
        summary.rounds.iter().map(|r| r.round).collect()
    } else {
        wanted
    };
    let mut world: Option<(String, Option<WorldCollision>)> = None;
    let mut tallies: [Tally; 2] = Default::default();
    for number in rounds {
        let Ok(track) = track_round(&data, number) else {
            eprintln!("round {number}: not tracked");
            continue;
        };
        if world.as_ref().is_none_or(|(map, _)| *map != track.map_name) {
            let collision = collision(&install, &track.map_name);
            if collision.is_none() {
                eprintln!("{}: no collision, cameras ignore walls", track.map_name);
            }
            world = Some((track.map_name.clone(), collision));
        }
        round(
            &track,
            world.as_ref().and_then(|(_, c)| c.as_ref()),
            &mut tallies,
        );
    }
    for (kind, t) in ["behind", "tripod"].iter().zip(&tallies) {
        println!(
            "{kind}: {} shots, {} cramped (within {CRAMPED} units of the killer's eyes, his head in the middle: {} of {} ticks), {} jumps",
            t.shots, t.cramped_shots, t.cramped_ticks, t.ticks, t.jumps
        );
    }
}
