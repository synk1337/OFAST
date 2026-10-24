//! Developer tool: how an HE blast's hole in a smoke ([`Simulation::carve`]) opens and fills again for a few hole
//! sizes, measured along sight lines, to fit the hole to CS2's picture.
//!
//! ```bash
//! cargo run --release --example smoke_hole_fit -- path/to/match.dem 5 164 222 4 34282,34319,34350
//! ```
//!
//! Arguments: demo, round, the smoke's entity, the HE grenade's entity, the player whose eyes look, and the ticks.
//! For each hole radius (and none) and each tick: the smoke a sight line crosses from the player's eye along their view
//! (and four lines 2° off it), as the Unreal volume would draw it — density above 5 % of full counts, opaque 15 %
//! above that, extinction 0.1 per cm — printed as optical depth (0: clear; 1: 37 % seen through; 5: opaque).

use ofast_core::demo::players::track_round;
use ofast_core::demo::smoke::{self, GRID, Simulation, SmokeSettings, VOXEL_SIZE};

/// The smoke a ray crosses in `grid` (render grid, x fastest), from `eye` along `dir` (unit), in a grid whose corner is
/// `corner`: optical depth as the volume material draws it.
fn depth(grid: &[f32], corner: [f32; 3], eye: [f32; 3], dir: [f32; 3], full: f32) -> f32 {
    let step = 5.0;
    let mut sum = 0.0;
    for k in 0..400 {
        let p: [f32; 3] = std::array::from_fn(|i| eye[i] + dir[i] * step * k as f32 - corner[i]);
        let v = p.map(|c| (c / VOXEL_SIZE).floor());
        if v.iter().any(|&c| c < 0.0 || c >= GRID as f32) {
            continue;
        }
        let [x, y, z] = v.map(|c| c as usize);
        let d = grid[x + GRID * (y + GRID * z)] / full;
        sum += ((d - 0.05) / 0.15).clamp(0.0, 1.0) * 0.1 * step * 2.54;
    }
    sum
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args[1].parse().expect("round");
    let smoke_entity: u32 = args[2].parse().expect("smoke entity");
    let he_entity: i64 = args[3].parse().expect("HE entity");
    let controller: u32 = args[4].parse().expect("player controller");
    let ticks: Vec<u32> = args[5]
        .split(',')
        .map(|t| t.parse().expect("tick"))
        .collect();
    let track = track_round(&data, round).expect("round");
    let log = track
        .smoke_logs
        .iter()
        .find(|l| l.entity == smoke_entity)
        .expect("smoke log");
    let point = track
        .samples
        .iter()
        .flat_map(|s| &s.grenades)
        .filter(|g| g.entity == log.entity && g.serial == log.serial)
        .find_map(|g| g.smoke.map(|c| c.position))
        .expect("smoke position");
    let corner = point.map(|v| v - GRID as f32 * VOXEL_SIZE / 2.0);
    let he = track
        .events
        .iter()
        .find(|e| e.name == "hegrenade_detonate" && e.int("entityid") == Some(he_entity))
        .expect("HE detonation");
    let at = he.position().expect("detonation position");
    let voxel: [f32; 3] = std::array::from_fn(|k| (at[k] - corner[k]) / VOXEL_SIZE);
    println!(
        "smoke #{smoke_entity} at {point:?}; HE #{he_entity} at tick {} {at:?}, voxel {voxel:?}",
        he.tick
    );
    let frames = smoke::frames(log);
    let mut runs = vec![(0.0f32, 0.0f32, 0.0f32)];
    for radius in [8.0f32, 10.0, 12.0] {
        for hold in [0.25f32, 0.5, 1.0] {
            for close in [1.0f32, 2.0, 4.0] {
                runs.push((radius, hold, close));
            }
        }
    }
    for (radius, hold, close) in runs {
        let settings = SmokeSettings {
            hole_radius: radius,
            hole_hold: hold,
            hole_close: close,
            ..SmokeSettings::default()
        };
        let mut sim = Simulation::new(settings);
        let mut out = Vec::new();
        let mut next = 0;
        for frame in &frames {
            // The cloud a tick shows: the last frame sent by then.
            while next < ticks.len() && frame.tick > ticks[next] {
                out.push(measure(
                    &track,
                    &sim,
                    corner,
                    controller,
                    ticks[next],
                    settings.cap,
                ));
                next += 1;
            }
            // As smoke_atlas: the hole after every step while it lasts.
            sim.step(frame);
            let r =
                settings.hole_radius_at((frame.tick as f32 - he.tick as f32) * track.tick_interval);
            if radius > 0.0 && r > 0.0 {
                sim.carve(voxel, r, settings.hole_edge);
            }
        }
        println!(
            "radius {:3.0} units, hold {hold:.2} s, close {close:.0} s: {}",
            radius * VOXEL_SIZE,
            out.iter()
                .zip(&ticks)
                .map(|(m, t)| format!("{t}: {m}"))
                .collect::<Vec<_>>()
                .join("  ")
        );
    }
}

/// The optical depths along the player's view at `tick` (centre, then 2° left, right, up, down).
fn measure(
    track: &ofast_core::demo::players::RoundTrack,
    sim: &Simulation,
    corner: [f32; 3],
    controller: u32,
    tick: u32,
    full: f32,
) -> String {
    let Some(p) = track
        .samples
        .iter()
        .find(|s| s.tick >= tick)
        .and_then(|s| s.players.iter().find(|p| p.controller == controller))
    else {
        return "-".into();
    };
    let eye = [
        p.position[0],
        p.position[1],
        p.position[2] + p.view_offset[2],
    ];
    let grid = sim.render_grid();
    let depths: Vec<String> = [(0.0, 0.0), (0.0, 2.0), (0.0, -2.0), (-2.0, 0.0), (2.0, 0.0)]
        .iter()
        .map(|(dp, dy)| {
            let (pitch, yaw) = ((p.pitch + dp).to_radians(), (p.yaw + dy).to_radians());
            let dir = [
                pitch.cos() * yaw.cos(),
                pitch.cos() * yaw.sin(),
                -pitch.sin(),
            ];
            format!("{:.1}", depth(&grid, corner, eye, dir, full))
        })
        .collect();
    depths.join("/")
}
