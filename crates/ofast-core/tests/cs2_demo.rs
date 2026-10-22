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
use ofast_core::export::replay::replay_json;
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

/// The animation rig's inputs over round 1: the velocity agrees with how far players move from tick to tick, and,
/// with demoparser2, every other input equals its reading (body yaw, ground flag, crouch amount, walking, scoped,
/// defusing, ladder, weapon item, weapon animation state, reloading) and the last jump tick is its raw value halved
/// (zigzag) minus one constant clock offset.
#[test]
fn rig_inputs_match_movement_and_demoparser2() {
    let Some((path, data)) = load() else { return };
    let track = track_round(&data, 1).expect("round 1");

    // Velocity against the position change between consecutive ticks, for players moving faster than 50 units/s.
    // Not equal: the position advances by sub-tick moves, the velocity is the end-of-tick value.
    let mut differences = Vec::new();
    for pair in track.samples.windows(2) {
        let [a, b] = pair else { unreachable!() };
        if b.tick != a.tick + 1 {
            continue;
        }
        for p in b.players.iter().filter(|p| p.alive) {
            let Some(before) = a
                .players
                .iter()
                .find(|q| q.controller == p.controller && q.alive)
            else {
                continue;
            };
            let v = p.rig.velocity;
            if v[0].hypot(v[1]) < 50.0 {
                continue;
            }
            let moved = [0, 1].map(|k| (p.position[k] - before.position[k]) / track.tick_interval);
            differences.push((moved[0] - v[0]).hypot(moved[1] - v[1]));
        }
    }
    differences.sort_by(f32::total_cmp);
    let median = differences[differences.len() / 2];
    println!(
        "velocity vs position change: {} samples, median difference {median:.2} units/s",
        differences.len()
    );
    assert!(median < 5.0, "velocity follows the movement");
    let jumps: HashSet<(u32, u32)> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .filter_map(|p| p.rig.last_jump_tick.map(|t| (p.controller, t)))
        .collect();
    println!("{} distinct jumps seen", jumps.len());

    let Some(csv) = run_oracle(&[
        "rig",
        &path.to_string_lossy(),
        &track.start_tick.to_string(),
        &track.end_tick.to_string(),
    ]) else {
        return;
    };
    let mut ours: HashMap<(u32, u64, &str), &ofast_core::demo::players::PlayerSample> =
        HashMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            ours.insert((sample.tick, p.steam_id, p.name.as_str()), p);
        }
    }
    let (mut rows, mut matched) = (0usize, 0usize);
    let mut clock_offsets: HashSet<i64> = HashSet::new();
    // Largest gap between body and eye yaw: the body follows the eyes with a lag, so they are not the same field.
    let mut largest_body_turn = 0.0f32;
    let truthy = |s: &str| s == "True";
    // Integers that pandas may print as floats ("7.0"); empty = no weapon.
    let integer = |s: &str| (!s.is_empty()).then(|| s.parse::<f64>().expect("number") as i64);
    for line in csv.lines().skip(1) {
        // tick,steamid,flags,duck_amount,walking,scoped,defusing,move_type,item,anim_state,reloading,last_jump_raw,
        // body_yaw,name
        let c: Vec<&str> = line.splitn(14, ',').collect();
        rows += 1;
        let key = (
            c[0].parse().expect("tick"),
            c[1].parse().expect("steam id"),
            c[13],
        );
        let Some(p) = ours.get(&key) else { continue };
        matched += 1;
        let rig = &p.rig;
        let at = format!("tick {} {}", key.0, key.2);
        let flags: u64 = c[2].parse().expect("flags");
        assert_eq!(rig.on_ground, flags & 1 == 1, "{at}: on ground");
        let duck: f32 = c[3].parse().expect("duck amount");
        assert!((rig.duck_amount - duck).abs() < 1e-6, "{at}: duck amount");
        assert_eq!(
            (rig.walking, rig.scoped, rig.defusing),
            (truthy(c[4]), truthy(c[5]), truthy(c[6])),
            "{at}: walking, scoped, defusing"
        );
        assert_eq!(rig.on_ladder, c[7] == "9", "{at}: ladder");
        let weapon = rig.weapon.as_ref();
        assert_eq!(
            weapon.map(|w| i64::from(w.item)),
            integer(c[8]),
            "{at}: weapon item"
        );
        if let Some(w) = weapon {
            assert_eq!(Some(w.anim_state), integer(c[9]), "{at}: anim state");
            assert_eq!(w.reloading, truthy(c[10]), "{at}: reloading");
        }
        if !c[12].is_empty() {
            let body_yaw: f32 = c[12].parse().expect("body yaw");
            let d = angle_difference(rig.body_yaw, body_yaw);
            assert!(
                d <= ORACLE_TOLERANCE,
                "{at}: body yaw {} vs {body_yaw}",
                rig.body_yaw
            );
            if p.alive {
                largest_body_turn = largest_body_turn.max(angle_difference(rig.body_yaw, p.yaw));
            }
        }
        if let (Some(raw), Some(ours)) = (integer(c[11]).filter(|&r| r > 0), rig.last_jump_tick) {
            assert_eq!(raw % 2, 0, "{at}: a zigzag code of a positive tick is even");
            clock_offsets.insert(raw / 2 - i64::from(ours));
        }
    }
    println!(
        "demoparser2 rig: {matched} of {rows} rows matched; game tick − demo tick of jumps: {clock_offsets:?}; \
         body yaw up to {largest_body_turn:.1}° from the eye yaw"
    );
    assert!(
        rows > 0 && matched * 100 >= rows * 99,
        "rows have counterparts"
    );
    assert!(
        clock_offsets.len() <= 1,
        "one constant offset between game and demo ticks"
    );
}

/// `export_players` on round 1: one glTF per agent look, listed in the manifest with its players, each with an
/// animation per player keyed on every tick (64 per second) over the whole round, and a skeleton of the agent's bones plus one bone
/// per weapon listed (every player of a pistol round carries a knife and a pistol).
#[test]
fn players_export_one_animation_per_player() {
    use ofast_core::content::Content;
    use ofast_core::export::players::{animation_fps, export_players};
    let Some((_, data)) = load() else { return };
    let Some(install) = Cs2Install::locate() else {
        println!("SKIPPED: no CS2 install");
        return;
    };
    let content = Content::game(&install).expect("archives open");
    let track = track_round(&data, 1).expect("round 1");
    let out = std::env::temp_dir().join("ofast_players_test");
    let report = export_players(&content, &track, "demo", &out).expect("players export");
    let players: HashSet<u32> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .map(|p| p.controller)
        .collect();
    assert_eq!(report.players, players.len());
    let seconds = (track.end_tick - track.start_tick) as f32 * track.tick_interval;
    assert_eq!(report.fps, 64.0);
    assert_eq!(
        report.frames,
        (seconds * animation_fps(&track)).round() as usize + 1
    );
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report.manifest).expect("manifest reads"))
            .expect("JSON");
    let mut listed = HashSet::new();
    for entry in manifest["agents"].as_array().expect("agents") {
        let file = out.join(entry["file"].as_str().expect("file"));
        let gltf: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("glTF reads"))
                .expect("JSON");
        let animations = gltf["animations"].as_array().expect("animations");
        let entry_players = entry["players"].as_array().expect("players");
        let weapons: Vec<&str> = entry["weapons"]
            .as_array()
            .expect("weapons")
            .iter()
            .map(|w| w["model"].as_str().expect("model"))
            .collect();
        assert_eq!(animations.len(), entry_players.len(), "{}", file.display());
        listed.extend(
            entry_players
                .iter()
                .map(|p| p.as_u64().expect("controller") as u32),
        );
        let joints = gltf["skins"][0]["joints"].as_array().expect("joints").len();
        let weapon_bones = gltf["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter(|n| {
                n["name"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("ofast_weapon_"))
            })
            .count();
        assert_eq!(weapon_bones, weapons.len());
        assert!(weapons.iter().any(|w| w.contains("/knife")), "{weapons:?}");
        assert!(weapons.iter().any(|w| w.contains("_pist_")), "{weapons:?}");
        println!(
            "{}: {} animations, {joints} joints, weapons {:?}",
            entry["file"],
            animations.len(),
            weapons
                .iter()
                .map(|w| w.rsplit('/').next().unwrap_or(w))
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(listed, players);
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    std::fs::remove_dir_all(&out).ok();
}

/// The bake's decisions (M4.4c) follow the demo's own events in every round up to 5: each knife attack and grenade
/// throw the demo records (a gameplay state 500 – 505 or 200 – 202 with a new timestamp) starts that action in the
/// weapons layer on its tick; every jump that leaves the ground starts a jump; every ladder stretch is a ladder state;
/// every C4 plant and defuse begins one; and the baked poses are finite.
#[test]
fn bake_decisions_follow_the_demo() {
    use ofast_core::anim::bake::{bake_player, timeline};
    use ofast_core::content::Content;
    use ofast_core::export::players::load_player_rig;
    let Some((_, data)) = load() else { return };
    let Some(install) = Cs2Install::locate() else {
        println!("SKIPPED: no CS2 install");
        return;
    };
    let content = Content::game(&install).expect("archives open");
    let rounds = list_rounds(&data).expect("rounds");
    let mut checked = [0usize; 5];
    for round in rounds.iter().take(5) {
        let track = track_round(&data, round.round).expect("round");
        let rig = load_player_rig(&content, &track).expect("rig");
        assert!(rig.warnings.is_empty(), "{:?}", rig.warnings);
        let controllers: HashSet<u32> = track
            .samples
            .iter()
            .flat_map(|s| &s.players)
            .map(|p| p.controller)
            .collect();
        for &controller in &controllers {
            let lines = timeline(
                &track,
                controller,
                &rig.sets,
                rig.upper.as_ref(),
                rig.graphs.as_ref(),
            )
            .expect("decisions");
            let has =
                |tick: u32, what: &str| lines.iter().any(|(t, l)| *t == tick && l.contains(what));
            let mut seen = HashSet::new();
            let mut was_ladder = false;
            let mut was_planting = false;
            let mut was_defusing = false;
            for sample in &track.samples {
                let Some(p) = sample
                    .players
                    .iter()
                    .find(|p| p.controller == controller && p.alive)
                else {
                    continue;
                };
                if let Some(w) = &p.rig.weapon
                    && let Some(stamp) = w.state_tick
                    && seen.insert((w.anim_state, stamp))
                {
                    let action = match w.anim_state {
                        500..=505 if w.class.contains("Knife") => Some("attack_knife"),
                        200 | 202 if !w.class.contains("Knife") => Some("attack_grenade"),
                        _ => None,
                    };
                    // The weapons layer starts on the state's timestamp (when it is recent).
                    if let Some(action) = action
                        && stamp + 64 > sample.tick
                    {
                        assert!(
                            has(stamp, action),
                            "round {} player {controller}: {action} at {stamp}",
                            round.round
                        );
                        checked[0] += 1;
                    }
                }
                if p.rig.on_ladder && !was_ladder {
                    assert!(has(sample.tick, "Ladder"), "ladder at {}", sample.tick);
                    checked[1] += 1;
                }
                was_ladder = p.rig.on_ladder;
                let planting = p.rig.weapon.as_ref().is_some_and(|w| w.anim_state == 300);
                if planting && !was_planting {
                    assert!(has(sample.tick, "plant begins"), "plant at {}", sample.tick);
                    checked[2] += 1;
                }
                was_planting = planting;
                if p.rig.defusing && !was_defusing {
                    assert!(
                        has(sample.tick, "defuse begins"),
                        "defuse at {}",
                        sample.tick
                    );
                    checked[3] += 1;
                }
                was_defusing = p.rig.defusing;
            }
            checked[4] += lines
                .iter()
                .filter(|(_, l)| l.contains("Air { jumped: true }"))
                .count();
            let poses = bake_player(
                &track,
                controller,
                &rig.sets,
                rig.upper.as_ref(),
                rig.graphs.as_ref(),
                16.0,
            )
            .expect("bake");
            assert!(
                poses.iter().flatten().flatten().all(|t| t.scale.is_finite()
                    && t.translation
                        .iter()
                        .chain(&t.rotation)
                        .all(|v| v.is_finite())),
                "round {} player {controller}: non-finite pose",
                round.round
            );
        }
    }
    println!(
        "attacks and throws {}, ladders {}, plants {}, defuses {}, jumps {}",
        checked[0], checked[1], checked[2], checked[3], checked[4]
    );
    assert!(
        checked[0] > 0 && checked[4] > 0,
        "the demo has attacks and jumps to check"
    );
}

/// Glove skins: round 1 with every player given the Bloodhound gloves (item 5027) and the agent's
/// `thirdperson_hide_default_gloves` mesh group, as CS2 sends them for a player wearing glove skins (the local demo
/// has none). Each look's file then holds the glove model's meshes, bone-merged onto the agent, and not the agent's
/// own gloves.
#[test]
fn glove_skins_merge_onto_the_agent() {
    use ofast_core::content::Content;
    use ofast_core::export::players::export_players;
    let Some((_, data)) = load() else { return };
    let Some(install) = Cs2Install::locate() else {
        println!("SKIPPED: no CS2 install");
        return;
    };
    let content = Content::game(&install).expect("archives open");
    let mut track = track_round(&data, 1).expect("round 1");
    let models = content.resource_paths("vmdl");
    let mut groups: HashMap<u64, u64> = HashMap::new();
    for p in track.samples.iter_mut().flat_map(|s| &mut s.players) {
        let mask = *groups.entry(p.appearance.model).or_insert_with(|| {
            let path = &models[&p.appearance.model];
            let model = ofast_core::export::gltf::load_model(&content, path).expect("agent");
            let bit = |name: &str| {
                model
                    .mesh_groups
                    .iter()
                    .position(|g| g.ends_with(name))
                    .map_or(0, |i| 1u64 << i)
            };
            bit("defusekit_hidden") | bit("thirdperson_hide_default_gloves")
        });
        p.appearance.gloves = 5027;
        p.appearance.mesh_groups = mask;
    }
    let out = std::env::temp_dir().join("ofast_gloves_test");
    let report = export_players(&content, &track, "demo", &out).expect("players export");
    for file in &report.files {
        let gltf: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).expect("glTF reads"))
                .expect("JSON");
        let meshes: Vec<&str> = gltf["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter(|n| n.get("mesh").is_some())
            .filter_map(|n| n["name"].as_str())
            .collect();
        println!("{}: {meshes:?}", file.display());
        assert!(
            meshes.iter().any(|m| m.starts_with("gloves_")),
            "{meshes:?}"
        );
        assert!(
            !meshes.contains(&"thirdperson_default_gloves"),
            "{meshes:?}"
        );
        assert!(file.to_string_lossy().ends_with("_glove_bloodhound.gltf"));
    }
    std::fs::remove_dir_all(&out).ok();
}

/// Every living player's agent and every weapon carried resolve to CS2 models by resource id (`m_hModel`), the
/// weapon in hand is one of those carried, and in round 5 at tick 26508 "vg" runs with the CT knife out and an
/// M4A4 slung, as CS2 shows him (`docs/formats/animation.md`, "Verified").
#[test]
fn models_resolve_by_resource_id() {
    use ofast_core::content::Content;
    let Some((demo_path, data)) = load() else {
        return;
    };
    let Some(install) = Cs2Install::locate() else {
        println!("SKIPPED: no CS2 install");
        return;
    };
    let models = Content::game(&install)
        .expect("archives open")
        .resource_paths("vmdl");
    let track = track_round(&data, 5).expect("round 5");
    let (mut players, mut weapons) = (0usize, 0usize);
    let mut agents: HashMap<&str, usize> = HashMap::new();
    for p in track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .filter(|p| p.alive)
    {
        let agent = models
            .get(&p.appearance.model)
            .unwrap_or_else(|| panic!("{}: agent {:016x}", p.name, p.appearance.model));
        assert!(agent.starts_with("agents/models/"), "{agent}");
        *agents.entry(agent.as_str()).or_default() += 1;
        players += 1;
        for w in &p.weapons {
            let path = models
                .get(&w.model)
                .unwrap_or_else(|| panic!("{}: weapon {} {:016x}", p.name, w.item, w.model));
            assert!(path.starts_with("weapons/models/"), "{path}");
            weapons += 1;
        }
        if let Some(active) = &p.rig.weapon {
            assert!(
                p.weapons.iter().any(|w| w.entity == active.entity),
                "{}: the weapon in hand is carried",
                p.name
            );
        }
    }
    println!("{players} player samples, {weapons} carried weapons, agents {agents:?}");
    if demo_path.file_name().and_then(|n| n.to_str()) == Some(CS2_CHECKED_DEMO) {
        let sample = track
            .samples
            .iter()
            .find(|s| s.tick == 26508)
            .expect("tick 26508");
        let vg = sample.players.iter().find(|p| p.name == "vg").expect("vg");
        let path = |id: u64| {
            models[&id]
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        let active = vg.rig.weapon.as_ref().expect("weapon in hand");
        let carried: Vec<String> = vg.weapons.iter().map(|w| path(w.model)).collect();
        println!(
            "vg at 26508: in hand item {}, carries {carried:?}",
            active.item
        );
        assert_eq!(active.item, 42);
        assert!(carried.contains(&"weapon_rif_m4a4.vmdl".to_owned()));
    }
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

/// Every kill of every round, as the replay export writes it (victim and attacker resolved from the event's player
/// slots), against demoparser2's `player_death` events: same tick, victim, attacker, weapon and headshot flag.
#[test]
fn kills_match_demoparser2() {
    let Some((path, data)) = load() else { return };
    let Some(oracle) = run_oracle(&["deaths", &path.to_string_lossy()]) else {
        return;
    };
    let rounds = list_rounds(&data).expect("rounds");
    let mut ours = Vec::new();
    for round in &rounds {
        let track = track_round(&data, round.round).expect("round tracks");
        let replay = replay_json(&track, "demo");
        let names: HashMap<u64, String> = replay["players"]
            .as_array()
            .expect("players")
            .iter()
            .map(|p| {
                (
                    p["controller"].as_u64().expect("controller"),
                    p["name"].as_str().expect("name").replace(',', ";"),
                )
            })
            .collect();
        let name =
            |v: &serde_json::Value| v.as_u64().map(|c| names[&c].clone()).unwrap_or_default();
        for kill in replay["kills"].as_array().expect("kills") {
            ours.push(format!(
                "{},{},{},{},{}",
                kill["tick"],
                if kill["headshot"] == true {
                    "True"
                } else {
                    "False"
                },
                kill["weapon"].as_str().expect("weapon"),
                name(&kill["victim"]),
                name(&kill["attacker"]),
            ));
        }
    }
    // demoparser2 lists every death of the demo; keep those inside the rounds (not warmup).
    let expected: Vec<String> = oracle
        .lines()
        .skip(1)
        .filter(|line| {
            let tick: u32 = line.split(',').next().expect("tick").parse().expect("tick");
            rounds
                .iter()
                .any(|r| (r.start_tick..=r.end_tick).contains(&tick))
        })
        .map(str::to_owned)
        .collect();
    println!("{} kills in {} rounds", ours.len(), rounds.len());
    assert!(!ours.is_empty());
    assert_eq!(ours, expected, "kills (ours vs demoparser2)");
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
