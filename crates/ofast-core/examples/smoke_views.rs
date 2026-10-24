//! Developer tool: moments of a round when a living player looks at a smoke with nothing in between, to compare
//! CS2's smoke with OFAST's through the same eyes (`import_demo.py -PovShots "tick:controller"`, CS2's
//! `spec_player`).
//!
//! ```bash
//! cargo run --release --example smoke_views -- path/to/match.dem 5
//! ```
//!
//! Every 16 ticks of each cloud's life: players alive whose eye is 250 – 1500 units from the cloud's middle (the
//! detonation point + 40 up), with the middle within 10° of their view, and whose sight line is clear up to 120 units
//! short of it (small spheres marched along it against the map's collision, `maps/<map>/world_physics.vmdl`). Prints
//! the most head-on view per smoke and phase (growing: the first 1.5 s; steady; fading), and per HE grenade and
//! flashbang going off (2 – 6 ticks after).

use ofast_core::content::Content;
use ofast_core::demo::events::EventValue;
use ofast_core::demo::players::{PlayerSample, track_round};
use ofast_core::game::Cs2Install;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::WorldCollision;
use ofast_core::resource::Resource;

/// Whether the segment from `a` to `b` touches the world (spheres of 3 units every 6 units).
fn blocked(world: &WorldCollision, a: [f32; 3], b: [f32; 3]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let steps = (length / 6.0).ceil() as usize;
    (1..steps).any(|k| {
        let t = k as f32 / steps as f32;
        world
            .sphere_contact([a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t], 3.0)
            .is_some()
    })
}

/// How far off a player's view `centre` is (degrees) and how far (units), when it is 250 – 1500 units away, within 10°
/// of the view and in clear sight up to 120 units short of it.
fn sees(world: &WorldCollision, p: &PlayerSample, centre: [f32; 3]) -> Option<(f32, f32)> {
    let eye = [
        p.position[0],
        p.position[1],
        p.position[2] + p.view_offset[2],
    ];
    let d = [centre[0] - eye[0], centre[1] - eye[1], centre[2] - eye[2]];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !(250.0..=1500.0).contains(&dist) {
        return None;
    }
    let (pitch, yaw) = (p.pitch.to_radians(), p.yaw.to_radians());
    let f = [
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        -pitch.sin(),
    ];
    let off = ((f[0] * d[0] + f[1] * d[1] + f[2] * d[2]) / dist)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    if off > 10.0 {
        return None;
    }
    let near = (dist - 120.0) / dist;
    let short = [
        eye[0] + d[0] * near,
        eye[1] + d[1] * near,
        eye[2] + d[2] * near,
    ];
    (!blocked(world, eye, short)).then_some((off, dist))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let track = track_round(&data, round).expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &track.map_name).expect("archives open");
    let path = format!("maps/{}/world_physics.vmdl", track.map_name);
    let bytes = content.read_resource(&path).expect("map collision reads");
    let physics = PhysicsData::from_resource(&Resource::parse(&bytes).expect("resource"))
        .expect("physics")
        .expect("PHYS block");
    let world = WorldCollision::from_physics(&physics);

    // Each smoke once: entity, serial, cloud start, position (from its samples).
    let mut smokes: Vec<(u32, u32, u32, [f32; 3], u32)> = Vec::new();
    for s in &track.samples {
        for g in &s.grenades {
            let Some(c) = g.smoke else { continue };
            match smokes
                .iter_mut()
                .find(|m| m.0 == g.entity && m.1 == g.serial)
            {
                Some(m) => m.4 = s.tick,
                None => smokes.push((g.entity, g.serial, c.start_tick, c.position, s.tick)),
            }
        }
    }
    for &(entity, _, start, position, last) in &smokes {
        let centre = [position[0], position[1], position[2] + 40.0];
        let mut best: Vec<(&str, f32, u32, String, u32, f32)> = Vec::new();
        for sample in track
            .samples
            .iter()
            .filter(|s| (start..=last).contains(&s.tick) && s.tick % 16 == 0)
        {
            let phase = if sample.tick < start + 96 {
                "growing"
            } else if sample.tick + 300 > last {
                "fading"
            } else {
                "steady"
            };
            for p in sample.players.iter().filter(|p| p.alive) {
                let Some((off, dist)) = sees(&world, p, centre) else {
                    continue;
                };
                match best.iter_mut().find(|b| b.0 == phase) {
                    Some(b) if b.1 <= off => {}
                    Some(b) => *b = (phase, off, sample.tick, p.name.clone(), p.controller, dist),
                    None => {
                        best.push((phase, off, sample.tick, p.name.clone(), p.controller, dist))
                    }
                }
            }
        }
        for (phase, off, tick, name, controller, dist) in best {
            println!(
                "smoke #{entity} {phase}: tick {tick} {name} ({controller}) {dist:.0} units, {off:.1}° off, clear sight"
            );
        }
    }

    // HE grenades and flashbangs going off: the most head-on view 2 – 6 ticks after.
    for e in track
        .events
        .iter()
        .filter(|e| e.name == "hegrenade_detonate" || e.name == "flashbang_detonate")
    {
        let float = |key: &str| match e.get(key) {
            Some(EventValue::Float(v)) => *v,
            _ => 0.0,
        };
        let centre = [float("x"), float("y"), float("z") + 20.0];
        let mut best: Option<(f32, u32, String, u32, f32)> = None;
        for sample in track
            .samples
            .iter()
            .filter(|s| (e.tick + 2..=e.tick + 6).contains(&s.tick))
        {
            for p in sample.players.iter().filter(|p| p.alive) {
                if let Some((off, dist)) = sees(&world, p, centre)
                    && best.as_ref().is_none_or(|b| off < b.0)
                {
                    best = Some((off, sample.tick, p.name.clone(), p.controller, dist));
                }
            }
        }
        if let Some((off, tick, name, controller, dist)) = best {
            println!(
                "{} #{} at {}: tick {tick} {name} ({controller}) {dist:.0} units, {off:.1}° off, clear sight",
                e.name,
                e.int("entityid").unwrap_or(-1),
                e.tick
            );
        }
        // An HE grenade going off inside a smoke's cloud (within 320 units of its detonation point, while it lasts):
        // the most head-on view of the blast point at moments after it, to watch the hole open and close.
        if e.name != "hegrenade_detonate" {
            continue;
        }
        for &(entity, _, start, position, last) in &smokes {
            let away = ((centre[0] - position[0]).powi(2)
                + (centre[1] - position[1]).powi(2)
                + (centre[2] - 20.0 - position[2]).powi(2))
            .sqrt();
            if !(start..=last).contains(&e.tick) || away > 320.0 {
                continue;
            }
            println!(
                "  in smoke #{entity} (out since {:.1} s), {away:.0} units from its detonation point:",
                (e.tick - start) as f32 / 64.0
            );
            for after in [8u32, 32, 64, 128, 192, 320, 448] {
                let mut best: Option<(f32, u32, String, u32, f32)> = None;
                for sample in track
                    .samples
                    .iter()
                    .filter(|s| (e.tick + after - 2..=e.tick + after + 2).contains(&s.tick))
                {
                    for p in sample.players.iter().filter(|p| p.alive) {
                        if let Some((off, dist)) = sees(&world, p, centre)
                            && best.as_ref().is_none_or(|b| off < b.0)
                        {
                            best = Some((off, sample.tick, p.name.clone(), p.controller, dist));
                        }
                    }
                }
                if let Some((off, tick, name, controller, dist)) = best {
                    println!(
                        "    +{after} ticks: tick {tick} {name} ({controller}) {dist:.0} units, {off:.1}° off, clear sight"
                    );
                }
            }
        }
    }
}
