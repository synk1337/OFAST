//! Developer tool: print every player's position and eye angles for one round of a demo, as CSV.
//!
//! ```bash
//! cargo run --release --example demo_round -- path/to/match.dem 1 > round1.csv
//! ```
//!
//! Columns: `tick,steamid,name,team,alive,x,y,z,pitch,yaw`. Without a path, the newest demo in CS2's
//! `game/csgo/replays` folder is used; the round defaults to 1.

use std::io::Write;
use std::path::PathBuf;

use ofast_core::demo::players::track_round;
use ofast_core::game::Cs2Install;

fn newest_replay() -> PathBuf {
    let install = Cs2Install::locate().expect("CS2 install found");
    std::fs::read_dir(install.replays_dir())
        .expect("replays folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dem"))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
        .expect("a .dem file in the replays folder")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .filter(|a| !a.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(newest_replay);
    let round: u32 = args
        .next()
        .map(|r| r.parse().expect("round number"))
        .unwrap_or(1);

    let data = std::fs::read(&path).expect("demo reads");
    let track = track_round(&data, round).expect("round tracks");
    eprintln!(
        "{} round {}: ticks {}..={} ({} samples, {:.1} s)",
        track.map_name,
        track.round,
        track.start_tick,
        track.end_tick,
        track.samples.len(),
        (track.end_tick - track.start_tick) as f32 * track.tick_interval
    );

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    writeln!(out, "tick,steamid,name,team,alive,x,y,z,pitch,yaw").unwrap();
    for sample in &track.samples {
        for p in &sample.players {
            writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{}",
                sample.tick,
                p.steam_id,
                p.name.replace(',', " "),
                p.team,
                p.alive,
                p.position[0],
                p.position[1],
                p.position[2],
                p.pitch,
                p.yaw
            )
            .unwrap();
        }
    }
}
