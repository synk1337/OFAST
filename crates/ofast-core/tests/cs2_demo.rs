//! Tests of the demo reader against a real CS2 demo on the developer's machine.
//!
//! The demo is `OFAST_TEST_DEMO` if set, otherwise the newest `.dem` in CS2's `game/csgo/replays` folder (where the
//! game saves downloaded match demos). Without any demo the tests print a note and pass.
//!
//! Checks, from cheapest to strongest:
//! 1. every frame of the whole demo parses (a misread bit anywhere breaks everything after it);
//! 2. round 1 gives physically sane player positions and eye angles;
//! 3. if demoparser2 is installed (see `tools/oracle/demoparser2_oracle.py`), its positions, angles and round start
//!    ticks match ours.
//!
//! Only runs with `cargo test --release --features game-data -- --nocapture` (release: a match demo is ~200 MB).

#![cfg(feature = "game-data")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use ofast_core::demo::DemoParser;
use ofast_core::demo::players::{RoundTrack, list_rounds, track_round};
use ofast_core::game::Cs2Install;

/// Largest believable movement of a living player between two ticks (CS2 caps speed far below this).
const MAX_STEP_PER_TICK: f32 = 64.0;
/// Allowed difference to demoparser2, in world units and degrees (both read the same 32-bit floats).
const ORACLE_TOLERANCE: f32 = 0.01;

fn test_demo() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OFAST_TEST_DEMO") {
        return Some(PathBuf::from(path));
    }
    let install = Cs2Install::locate()?;
    std::fs::read_dir(install.replays_dir())
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dem"))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

fn load() -> Option<(PathBuf, Vec<u8>)> {
    let Some(path) = test_demo() else {
        println!("SKIPPED: no demo (set OFAST_TEST_DEMO or download a match in CS2)");
        return None;
    };
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    println!("demo: {} ({} MB)", path.display(), data.len() / 1_000_000);
    Some((path, data))
}

#[test]
fn every_frame_parses() {
    let Some((_, data)) = load() else { return };
    let mut parser = DemoParser::new(&data).expect("header");
    let mut frames = 0usize;
    let mut last_tick = 0;
    while let Some(frame) = parser
        .next_frame()
        .unwrap_or_else(|e| panic!("tick {:?}: {e}", parser.tick()))
    {
        frames += 1;
        last_tick = frame.tick.unwrap_or(last_tick);
    }
    println!(
        "{} frames, last tick {last_tick}, {} entities at the end",
        frames,
        parser.entities().iter().count()
    );
    for warning in parser.warnings() {
        println!("warning: {warning}");
    }
    assert_eq!(
        parser.warning_count(),
        0,
        "a real demo reads without warnings"
    );
    assert!(frames > 1000, "a match demo has many frames");
    assert!(parser.entities().iter().count() > 10);
    assert_eq!(
        parser.tick_interval(),
        Some(1.0 / 64.0),
        "CS2 servers run at 64 ticks/s"
    );
}

#[test]
fn round_one_is_sane() {
    let Some((path, data)) = load() else { return };
    let track = track_round(&data, 1).expect("round 1");
    let seconds = (track.end_tick - track.start_tick) as f32 * track.tick_interval;
    println!(
        "{} round 1: ticks {}..={} ({seconds:.1} s, {} samples)",
        track.map_name,
        track.start_tick,
        track.end_tick,
        track.samples.len()
    );
    assert!((10.0..600.0).contains(&seconds), "round length {seconds} s");
    assert!(
        track.samples.len() as u32 > (track.end_tick - track.start_tick) / 2,
        "most ticks have a sample"
    );

    let first = &track.samples[0];
    let teams: HashSet<u32> = first.players.iter().map(|p| p.team).collect();
    assert!(
        first.players.len() >= 2,
        "players at round start: {}",
        first.players.len()
    );
    assert!(
        teams.contains(&2) && teams.contains(&3),
        "both teams present: {teams:?}"
    );

    let mut previous: HashMap<u64, [f32; 3]> = HashMap::new();
    let mut travelled: HashMap<u64, f32> = HashMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            assert!(
                p.position
                    .iter()
                    .all(|c| c.is_finite() && c.abs() <= 16384.0),
                "tick {} {}: position {:?}",
                sample.tick,
                p.name,
                p.position
            );
            assert!(
                (-90.0..=90.0).contains(&p.pitch),
                "tick {} {}: pitch {}",
                sample.tick,
                p.name,
                p.pitch
            );
            assert!(
                (-180.0..=180.0).contains(&p.yaw),
                "tick {} {}: yaw {}",
                sample.tick,
                p.name,
                p.yaw
            );
            if !p.alive {
                previous.remove(&p.steam_id);
                continue;
            }
            if let Some(before) = previous.insert(p.steam_id, p.position) {
                let step = distance(before, p.position);
                assert!(
                    step < MAX_STEP_PER_TICK,
                    "tick {} {}: jumped {step} units",
                    sample.tick,
                    p.name
                );
                *travelled.entry(p.steam_id).or_default() += step;
            }
        }
    }
    let furthest = travelled.values().copied().fold(0.0, f32::max);
    println!("furthest a player walked: {furthest:.0} units");
    assert!(furthest > 500.0, "players move during a round");

    compare_with_demoparser2(&path, &track);
}

#[test]
fn round_starts_match_demoparser2() {
    let Some((path, data)) = load() else { return };
    let rounds = list_rounds(&data).expect("rounds");
    println!("{} rounds", rounds.len());
    assert!(!rounds.is_empty());
    for pair in rounds.windows(2) {
        assert!(
            pair[0].end_tick < pair[1].start_tick,
            "rounds do not overlap: {pair:?}"
        );
    }
    let Some(oracle) = run_oracle(&["round-starts", &path.to_string_lossy()]) else {
        return;
    };
    let expected: Vec<u32> = oracle
        .lines()
        .map(|l| l.trim().parse().expect("tick"))
        .collect();
    let ours: Vec<u32> = rounds.iter().map(|r| r.start_tick).collect();
    assert_eq!(ours, expected, "round start ticks (ours vs demoparser2)");
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Smallest difference between two angles in degrees (359° and 1° are 2° apart).
fn angle_difference(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Finds a Python that can import demoparser2: `OFAST_ORACLE_PYTHON`, else `python` on the PATH.
fn oracle_python() -> Option<String> {
    let candidates = match std::env::var("OFAST_ORACLE_PYTHON") {
        Ok(python) => vec![python],
        Err(_) => vec!["python".to_owned(), "python3".to_owned()],
    };
    candidates.into_iter().find(|python| {
        Command::new(python)
            .args(["-I", "-c", "import demoparser2"])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Runs `tools/oracle/demoparser2_oracle.py` with `args`; `None` (with a note) if demoparser2 is not installed.
fn run_oracle(args: &[&str]) -> Option<String> {
    let Some(python) = oracle_python() else {
        println!(
            "oracle SKIPPED: demoparser2 not installed (see tools/oracle/demoparser2_oracle.py)"
        );
        return None;
    };
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/oracle/demoparser2_oracle.py");
    let output = Command::new(python)
        .arg("-I")
        .arg(script)
        .args(args)
        .output()
        .expect("oracle runs");
    assert!(
        output.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).expect("UTF-8 oracle output"))
}

fn compare_with_demoparser2(path: &Path, track: &RoundTrack) {
    let Some(csv) = run_oracle(&[
        "positions",
        &path.to_string_lossy(),
        &track.start_tick.to_string(),
        &track.end_tick.to_string(),
    ]) else {
        return;
    };

    // (tick, steam id, name) → [x, y, z, pitch, yaw]. The name tells bots apart (they all have Steam id 0).
    let mut ours: HashMap<(u32, u64, &str), [f32; 5]> = HashMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            let v = [p.position[0], p.position[1], p.position[2], p.pitch, p.yaw];
            ours.insert((sample.tick, p.steam_id, p.name.as_str()), v);
        }
    }

    let (mut rows, mut matched) = (0usize, 0usize);
    let mut worst = [0.0f32; 5];
    for line in csv.lines().skip(1) {
        // tick,steamid,x,y,z,pitch,yaw,name — the name is last and may itself contain commas.
        let cols: Vec<&str> = line.splitn(8, ',').collect();
        let key: (u32, u64, &str) = (
            cols[0].parse().expect("tick"),
            cols[1].parse().expect("steam id"),
            cols[7],
        );
        let theirs: Vec<f32> = cols[2..7]
            .iter()
            .map(|c| c.parse().expect("number"))
            .collect();
        rows += 1;
        if theirs.iter().any(|v| v.is_nan()) {
            continue;
        }
        let Some(mine) = ours.get(&key) else { continue };
        matched += 1;
        for i in 0..5 {
            let d = if i < 3 {
                (mine[i] - theirs[i]).abs()
            } else {
                angle_difference(mine[i], theirs[i])
            };
            assert!(
                d <= ORACLE_TOLERANCE,
                "tick {} player {} ({}): column {i} is {} here, {} in demoparser2",
                key.0,
                key.2,
                key.1,
                mine[i],
                theirs[i]
            );
            worst[i] = worst[i].max(d);
        }
    }
    println!(
        "demoparser2: {matched} of {rows} rows matched; largest differences x/y/z/pitch/yaw = {worst:?}"
    );
    assert!(rows > 0, "oracle returned rows");
    assert!(
        matched * 100 >= rows * 99,
        "at least 99% of demoparser2's rows have a counterpart"
    );
}
