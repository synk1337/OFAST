//! Developer tool: an agent's ragdoll dropped at its bind pose onto a flat floor, printed as it settles — the
//! fastest point, and the parts moving fastest — to see what keeps a body from lying still.
//!
//! ```bash
//! cargo run --release --example ragdoll_settle -- agents/models/ctm_sas/ctm_sas.vmdl [height] [seconds]
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::model::{affine, compose};
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::{Ragdoll, WorldCollision};
use ofast_core::resource::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let height: f32 = args.get(1).map_or(20.0, |h| h.parse().expect("height"));
    let seconds: f32 = args.get(2).map_or(8.0, |s| s.parse().expect("seconds"));
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let bytes = content.read_resource(&args[0]).expect("model reads");
    let physics = PhysicsData::from_resource(&Resource::parse(&bytes).expect("parses"))
        .expect("PHYS reads")
        .expect("a ragdoll");
    let mut floor = WorldCollision::default();
    floor.add_box([-2000.0, -2000.0, -100.0], [2000.0, 2000.0, 0.0]);
    let lift = affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, height]);
    let start: Vec<_> = physics
        .bind_pose
        .iter()
        .map(|m| compose(&lift, m))
        .collect();
    let mut ragdoll = Ragdoll::new(&physics, &start, &[]);
    let fps = 64.0;
    for frame in 0..(seconds * fps) as usize {
        ragdoll.step(1.0 / fps, &floor);
        if frame % 16 == 15 {
            let mut parts: Vec<(f32, &str)> = ragdoll
                .velocities()
                .iter()
                .zip(&physics.bones)
                .map(|(v, name)| {
                    (
                        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt(),
                        name.as_str(),
                    )
                })
                .collect();
            parts.sort_by(|a, b| b.0.total_cmp(&a.0));
            let top: Vec<String> = parts
                .iter()
                .take(4)
                .map(|(s, n)| format!("{n} {s:.1}"))
                .collect();
            println!(
                "{:5.2} s  fastest point {:6.1} in/s{}  centres: {}",
                (frame + 1) as f32 / fps,
                ragdoll.speed(),
                if ragdoll.at_rest() { " (rest)" } else { "" },
                top.join(", ")
            );
        }
    }
}
