//! Developer tool: the drawn ground a round's players stand on (`export::ground`), probed with rays down.
//!
//! ```bash
//! cargo run --release --example ground_probe -- path/to/match.dem 14 "-718,-1331,40;-731,-1322,50"
//! ```
//!
//! For each point `x,y,z` it prints where a ray from there straight down first meets the drawn ground (within 80
//! inches) and the surface's normal, then the same against the map's physics (clips left out), to compare.

use std::path::PathBuf;

use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::export::ground::round_ground;
use ofast_core::game::Cs2Install;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::WorldCollision;
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(PathBuf::from(&args[0])).expect("demo read");
    let round: u32 = args[1].parse().expect("round");
    let track = track_round(&data, round).expect("round tracked");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &track.map_name).expect("archives");
    let started = std::time::Instant::now();
    let (ground, kept) = round_ground(&content, &track).expect("ground");
    println!(
        "{}: {kept} triangles facing up near the players, in {:.1} s",
        track.map_name,
        started.elapsed().as_secs_f32()
    );
    let bytes = content
        .read_resource(&format!("maps/{}/world_physics.vmdl", track.map_name))
        .expect("world physics");
    let physics = Resource::parse(&bytes)
        .ok()
        .and_then(|r| PhysicsData::from_resource(&r).ok().flatten())
        .map(|p| WorldCollision::from_physics(&p));
    for point in args.get(2).map(String::as_str).unwrap_or("").split(';') {
        let v: Vec<f32> = point
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if v.len() != 3 {
            continue;
        }
        let from = [v[0], v[1], v[2]];
        let show = |c: Option<&WorldCollision>| {
            c.and_then(|c| c.ray_hit(from, [0.0, 0.0, -1.0], 80.0))
                .map_or("nothing".into(), |h| {
                    format!(
                        "z {:.1} (normal {:.2} {:.2} {:.2})",
                        from[2] - h.distance,
                        h.normal[0],
                        h.normal[1],
                        h.normal[2]
                    )
                })
        };
        println!(
            "({:.0}, {:.0}, {:.0}): drawn {}, physics {}",
            from[0],
            from[1],
            from[2],
            show(Some(&ground)),
            show(physics.as_ref())
        );
    }
}
