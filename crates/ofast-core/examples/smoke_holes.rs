//! Developer tool: the 20-byte hole records in smokes' voxel logs (`m_VoxelFrameData`, `docs/formats/demo.md`,
//! "Smoke voxels"), next to what could have made them: HE grenades going off and shots fired near the smoke. Used to
//! work out the records' layout, which no public source documents.
//!
//! ```bash
//! cargo run --release --example smoke_holes -- path/to/match.dem [first round] [last round]
//! ```
//!
//! Per smoke with holes: its detonation point and grid corner (the point − 320). Per hole record: its frame and tick,
//! its bytes, and the same bytes read as five `f32`, ten `u16` and twenty `u8`; then the `hegrenade_detonate` events
//! from 64 ticks before the frame to 8 after, with their point in the smoke's grid (voxels, from the corner), and the
//! count of `weapon_fire` events in that window.

use ofast_core::demo::players::track_round;
use ofast_core::demo::smoke::{self, VOXEL_SIZE};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let first: u32 = args.get(1).map_or(1, |r| r.parse().expect("first round"));
    let last: u32 = args
        .get(2)
        .map_or(first, |r| r.parse().expect("last round"));
    for round in first..=last {
        let Ok(track) = track_round(&data, round) else {
            println!("round {round}: not in the demo");
            break;
        };
        for log in &track.smoke_logs {
            let frames = smoke::frames(log);
            if frames.iter().all(|f| f.holes.is_empty()) {
                continue;
            }
            // The smoke's detonation point, from its samples.
            let Some(point) = track
                .samples
                .iter()
                .flat_map(|s| &s.grenades)
                .filter(|g| g.entity == log.entity && g.serial == log.serial)
                .find_map(|g| g.smoke.map(|s| s.position))
            else {
                continue;
            };
            let corner = point.map(|v| v - 16.0 * VOXEL_SIZE);
            println!(
                "round {round} smoke #{} at ({:.1}, {:.1}, {:.1}), grid corner ({:.1}, {:.1}, {:.1})",
                log.entity, point[0], point[1], point[2], corner[0], corner[1], corner[2]
            );
            for frame in frames.iter().filter(|f| !f.holes.is_empty()) {
                for hole in &frame.holes {
                    let floats: Vec<String> = hole
                        .chunks(4)
                        .map(|c| format!("{:.3}", f32::from_le_bytes(c.try_into().unwrap())))
                        .collect();
                    let shorts: Vec<u16> = hole
                        .chunks(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    println!(
                        "  frame {} tick {}: {hole:02x?}\n    f32 {}\n    u16 {shorts:?}\n    u8  {:?}",
                        frame.number,
                        frame.tick,
                        floats.join(" "),
                        hole
                    );
                }
                let window = frame.tick.saturating_sub(64)..=frame.tick + 8;
                for e in track
                    .events
                    .iter()
                    .filter(|e| e.name == "hegrenade_detonate" && window.contains(&e.tick))
                {
                    if let Some([x, y, z]) = e.position() {
                        let voxel =
                            [x - corner[0], y - corner[1], z - corner[2]].map(|v| v / VOXEL_SIZE);
                        let distance = ((x - point[0]).powi(2)
                            + (y - point[1]).powi(2)
                            + (z - point[2]).powi(2))
                        .sqrt();
                        println!(
                            "    HE at tick {} ({x:.1}, {y:.1}, {z:.1}): voxel ({:.2}, {:.2}, {:.2}), {distance:.0} units from the smoke",
                            e.tick, voxel[0], voxel[1], voxel[2]
                        );
                    }
                }
                let shots = track
                    .events
                    .iter()
                    .filter(|e| e.name == "weapon_fire" && window.contains(&e.tick))
                    .count();
                println!("    {shots} shots fired in ticks {window:?}");
            }
        }
    }
}
