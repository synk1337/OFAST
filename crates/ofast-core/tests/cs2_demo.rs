//! Tests of the demo reader against a real CS2 demo on the developer's machine.
//!
//! The demo is `OFAST_TEST_DEMO` if set, otherwise the newest `.dem` in CS2's `game/csgo/replays` folder (where the
//! game saves downloaded match demos). Without any demo the tests print a note and pass.
//!
//! Checks, from cheapest to strongest:
//! 1. every frame of the whole demo parses (a misread bit anywhere breaks everything after it);
//! 2. round 1 gives physically sane player positions, view offsets and eye angles;
//! 3. if demoparser2 is installed (see `tools/oracle/demoparser2_oracle.py`), its positions, angles and round start
//!    ticks match ours (not the view offset: demoparser2 cannot export it, see `compare_with_demoparser2`);
//! 4. for the one demo they were taken from, the camera positions and angles CS2 itself showed (`getpos`) match our
//!    eye positions (feet + view offset) and angles.
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
/// Range of the view offset's networked components (`m_vecViewOffset`: x and y −64..64, z 1/64..64).
const MAX_VIEW_OFFSET: f32 = 64.0;
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
    let (mut eye_lowest, mut eye_highest) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut sideways_view_offsets = 0usize;
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
            // A misread path (say, the velocity's z, which shares the member name `m_vecZ`) would leave this range
            // as soon as someone jumps, and be 0 for living players standing on the ground.
            assert!(
                p.view_offset
                    .iter()
                    .all(|c| c.is_finite() && c.abs() <= MAX_VIEW_OFFSET)
                    && (!p.alive || p.view_offset[2] > 0.0),
                "tick {} {}: view offset {:?}",
                sample.tick,
                p.name,
                p.view_offset
            );
            if p.view_offset[..2] != [0.0, 0.0] {
                sideways_view_offsets += 1;
            }
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
            eye_lowest = eye_lowest.min(p.view_offset[2]);
            eye_highest = eye_highest.max(p.view_offset[2]);
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
    // x and y of the view offset were 0 in every sample of the checked match; a demo where they are not is worth a
    // look (eye_position() handles it either way).
    println!(
        "eye height of living players: {eye_lowest}..={eye_highest}; view offset x/y not 0 in {sideways_view_offsets} samples"
    );
    assert!(eye_highest > 60.0, "living players stand up at some point");

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

/// File name of the demo the [`CS2_READINGS`] were taken from (the developer's de_nuke match, build 10847).
const CS2_CHECKED_DEMO: &str = "match730_003834889241758793957_1039649504_272.dem";

/// One reading taken in CS2 itself: `demo_gototick`, `spec_player` in first person, then `getpos` (6 decimals) or
/// `cl_showpos 1` (2 decimals). See "Checked in the game" in `docs/formats/demo.md`.
struct GameReading {
    tick: u32,
    player: &'static str,
    /// Camera (eye) position CS2 printed, compared with OFAST's `eye_position()` (feet + view offset).
    camera: [f32; 3],
    /// What the reading shows, for the failure message (the eye height differs per stance).
    stance: &'static str,
    pitch: f32,
    yaw: f32,
    /// How exactly CS2 printed the position / the angles: 0.001 for `getpos`, 0.006 for `cl_showpos` (rounded to 2
    /// decimals).
    position_tolerance: f32,
    angle_tolerance: f32,
}

/// The digits are copied as CS2 printed them, more than an `f32` holds.
#[allow(clippy::excessive_precision)]
const CS2_READINGS: [GameReading; 6] = [
    GameReading {
        tick: 300,
        player: "Synk",
        camera: [-1832.0, -1160.0, -352.0],
        stance: "standing (freeze time)",
        pitch: 0.0,
        yaw: 39.999847,
        position_tolerance: 0.001,
        angle_tolerance: 0.001,
    },
    GameReading {
        tick: 3090,
        player: "vg",
        camera: [449.045685, -1334.654053, -352.0],
        stance: "standing",
        pitch: -0.316895,
        yaw: 161.706390,
        position_tolerance: 0.001,
        angle_tolerance: 0.001,
    },
    GameReading {
        tick: 4074,
        player: "Wajam",
        camera: [396.895844, -1471.968750, -352.0],
        stance: "standing",
        pitch: 3.352203,
        yaw: 89.47,
        position_tolerance: 0.001,
        angle_tolerance: 0.006,
    },
    // Running at about 3 units per tick: matching here to 0.001 also proves that `demo_gototick N` and OFAST's
    // tick N are the same moment.
    GameReading {
        tick: 4195,
        player: "Wajam",
        camera: [343.548157, -1313.562378, -352.0],
        stance: "running",
        pitch: 2.408066,
        yaw: -37.04,
        position_tolerance: 0.001,
        angle_tolerance: 0.006,
    },
    GameReading {
        tick: 4195,
        player: "waltey",
        camera: [616.298645, -901.900574, -378.0],
        stance: "crouched, C4 in hand",
        pitch: -12.898636,
        yaw: 55.44,
        position_tolerance: 0.001,
        angle_tolerance: 0.006,
    },
    GameReading {
        tick: 4520,
        player: "XANTARES",
        camera: [96.06, 143.97, -370.0],
        stance: "crouched",
        pitch: -1.10,
        yaw: 4.21,
        position_tolerance: 0.006,
        angle_tolerance: 0.006,
    },
];

#[test]
fn round_one_matches_cs2() {
    let Some((path, data)) = load() else { return };
    if path.file_name().and_then(|n| n.to_str()) != Some(CS2_CHECKED_DEMO) {
        println!("SKIPPED: the CS2 readings belong to {CS2_CHECKED_DEMO}");
        return;
    }
    let track = track_round(&data, 1).expect("round 1");
    for r in &CS2_READINGS {
        let sample = track
            .samples
            .iter()
            .find(|s| s.tick == r.tick)
            .unwrap_or_else(|| panic!("no sample at tick {}", r.tick));
        let p = sample
            .players
            .iter()
            .find(|p| p.name == r.player)
            .unwrap_or_else(|| panic!("tick {}: no player {}", r.tick, r.player));
        assert!(p.alive, "tick {} {}: alive in CS2", r.tick, r.player);
        // Everything comes from the demo: the feet position plus the networked view offset of that tick.
        for (axis, (ours, game)) in p.eye_position().iter().zip(r.camera).enumerate() {
            assert!(
                (ours - game).abs() <= r.position_tolerance,
                "tick {} {} ({}): camera axis {axis} is {ours} here (feet {:?} + view offset {:?}), {game} in CS2",
                r.tick,
                r.player,
                r.stance,
                p.position,
                p.view_offset
            );
        }
        println!(
            "tick {} {} ({}): eye height {} from the demo",
            r.tick, r.player, r.stance, p.view_offset[2]
        );
        for (name, ours, game) in [("pitch", p.pitch, r.pitch), ("yaw", p.yaw, r.yaw)] {
            assert!(
                angle_difference(ours, game) <= r.angle_tolerance,
                "tick {} {}: {name} is {ours} here, {game} in CS2",
                r.tick,
                r.player
            );
        }
    }
    println!("{} CS2 readings match", CS2_READINGS.len());
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

/// Compares positions and angles with demoparser2, row by row.
///
/// The view offset is not compared: demoparser2 0.42.0 names fields `Class.member` without the send node, so
/// `m_vecViewOffset.m_vecZ` and `m_vecVelocity.m_vecZ` both become `CCSPlayerPawn.m_vecZ`, a column that holds
/// whichever of the two changed last (its `m_vecX` column shows the velocity). `round_one_matches_cs2` checks the
/// view offset against CS2 itself instead.
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
