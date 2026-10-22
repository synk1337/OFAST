//! One round of a demo as a replay file for Unreal: `<demo>_r<round>.ofast-replay.json`.
//!
//! The file holds everything the Unreal side needs to rebuild the round as a Level Sequence, in CS2's own units
//! and axes (Source units = inches, x/y/z as in the game, angles in degrees) — `tools/unreal/import_demo.py`
//! converts with the same rules as the map import (`docs/formats/replay.md`):
//!
//! - the round's tick range and the tick its freeze time ended;
//! - the players (controller entity index, name, Steam id as text, team);
//! - per player and per sampled tick: feet position, eye height above the feet, eye pitch and yaw, alive or not, and
//!   what drives the player's animation (body yaw, velocity, on the ground, crouch amount, walking, scoped,
//!   defusing, on a ladder, the weapon in hand, its animation state, reloading) — one array entry per tick in `samples.ticks`,
//!   `null` where the player had no pawn;
//! - the round's jumps and landings (tick, player), and the weapons seen (item definition index → entity class);
//! - kills (`player_death`): tick, victim, attacker, assister (controller indices; `null` = nobody, e.g. fall
//!   damage), weapon, headshot and the other flags CS2's kill feed shows;
//! - round milestones (`round_freeze_end`, bomb events, `round_end`), with their keys as sent.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use super::ExportError;
use crate::demo::events::{EventValue, GameEvent};
use crate::demo::players::{PlayerSample, RoundTrack, track_round};

/// Format version written into the file; bump on incompatible changes.
pub const REPLAY_VERSION: u32 = 1;

/// Game events kept as round milestones (kills have their own list).
const MILESTONES: &[&str] = &[
    "round_freeze_end",
    "bomb_beginplant",
    "bomb_planted",
    "bomb_begindefuse",
    "bomb_defused",
    "bomb_exploded",
    "round_end",
];

/// Kill flags copied from `player_death` (what the kill feed shows).
const KILL_FLAGS: &[&str] = &[
    "headshot",
    "penetrated",
    "noscope",
    "thrusmoke",
    "attackerblind",
    "assistedflash",
    "attackerinair",
];

/// What [`export_round`] wrote.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    /// The replay file.
    pub path: PathBuf,
    /// Map name from the demo header.
    pub map: String,
    /// Players in the round.
    pub players: usize,
    /// Sampled ticks.
    pub ticks: usize,
    /// Kills in the round.
    pub kills: usize,
    /// Round length in seconds.
    pub seconds: f32,
}

/// Reads round `round` of a demo and writes its replay file into `out_dir`. `demo_name` names the file.
pub fn export_round(
    data: &[u8],
    demo_name: &str,
    round: u32,
    out_dir: &Path,
) -> Result<ReplayReport, ExportError> {
    let track = track_round(data, round)?;
    write_replay(&track, demo_name, out_dir)
}

/// Writes the replay file of an already tracked round into `out_dir` (`<demo_name>_r<round>.ofast-replay.json`).
pub fn write_replay(
    track: &RoundTrack,
    demo_name: &str,
    out_dir: &Path,
) -> Result<ReplayReport, ExportError> {
    let round = track.round;
    let replay = replay_json(track, demo_name);
    fs::create_dir_all(out_dir)?;
    let path = out_dir.join(format!("{demo_name}_r{round}.ofast-replay.json"));
    fs::write(
        &path,
        serde_json::to_string(&replay).expect("JSON serializes"),
    )?;
    Ok(ReplayReport {
        path,
        map: track.map_name.clone(),
        players: replay["players"].as_array().map_or(0, Vec::len),
        ticks: track.samples.len(),
        kills: replay["kills"].as_array().map_or(0, Vec::len),
        seconds: (track.end_tick - track.start_tick) as f32 * track.tick_interval,
    })
}

/// Builds the replay document of a tracked round.
pub fn replay_json(track: &RoundTrack, demo_name: &str) -> Json {
    // Players: everyone who had a pawn at some tick, with their last known name, Steam id and team.
    let mut players: BTreeMap<u32, Json> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            players.insert(
                p.controller,
                json!({
                    "controller": p.controller,
                    "name": p.name,
                    // As text: 64-bit ids do not survive JavaScript's or Python's float JSON readers everywhere.
                    "steam_id": p.steam_id.to_string(),
                    "team": p.team,
                }),
            );
        }
    }

    // Per-player arrays aligned with `ticks`, one per key of `TRACK_KEYS`.
    let ticks: Vec<u32> = track.samples.iter().map(|s| s.tick).collect();
    let mut tracks: BTreeMap<u32, Vec<Vec<Json>>> = players
        .keys()
        .map(|&c| (c, vec![Vec::with_capacity(ticks.len()); TRACK_KEYS.len()]))
        .collect();
    for sample in &track.samples {
        for (controller, arrays) in &mut tracks {
            let values = sample
                .players
                .iter()
                .find(|p| p.controller == *controller)
                .map(track_values);
            for (k, array) in arrays.iter_mut().enumerate() {
                array.push(values.as_ref().map_or(Json::Null, |v| v[k].clone()));
            }
        }
    }
    let samples: serde_json::Map<String, Json> = tracks
        .into_iter()
        .map(|(controller, arrays)| {
            let fields: serde_json::Map<String, Json> = TRACK_KEYS
                .iter()
                .zip(arrays)
                .map(|(key, array)| ((*key).to_owned(), Json::Array(array)))
                .collect();
            (controller.to_string(), Json::Object(fields))
        })
        .collect();

    // Jumps and landings: each distinct "last jump / landing tick" a player showed during the round.
    let mut jumps = std::collections::BTreeSet::new();
    let mut landings = std::collections::BTreeSet::new();
    let mut weapons: BTreeMap<u32, String> = BTreeMap::new();
    for p in track.samples.iter().flat_map(|s| &s.players) {
        let in_round = |tick: &u32| (track.start_tick..=track.end_tick).contains(tick);
        if let Some(tick) = p.rig.last_jump_tick.filter(in_round) {
            jumps.insert((tick, p.controller));
        }
        if let Some(tick) = p.rig.last_landing_tick.filter(in_round) {
            landings.insert((tick, p.controller));
        }
        if let Some(w) = &p.rig.weapon {
            weapons.entry(w.item).or_insert_with(|| w.class.clone());
        }
    }
    let tick_list = |set: std::collections::BTreeSet<(u32, u32)>| -> Vec<Json> {
        set.into_iter()
            .map(|(tick, player)| json!({ "tick": tick, "player": player }))
            .collect()
    };

    let kills: Vec<Json> = track
        .events
        .iter()
        .filter(|e| e.name == "player_death")
        .map(|e| {
            let mut kill = json!({
                "tick": e.tick,
                "victim": e.controller("userid"),
                "attacker": e.controller("attacker"),
                "assister": e.controller("assister"),
                "weapon": e.string("weapon").unwrap_or_default(),
            });
            for flag in KILL_FLAGS {
                if let Some(value) = e.bool(flag) {
                    kill[*flag] = json!(value);
                }
            }
            kill
        })
        .collect();
    let milestones: Vec<Json> = track
        .events
        .iter()
        .filter(|e| MILESTONES.contains(&e.name.as_str()))
        .map(event_json)
        .collect();
    let freeze_end = track
        .events
        .iter()
        .find(|e| e.name == "round_freeze_end")
        .map(|e| e.tick);

    json!({
        "format": "ofast-replay",
        "version": REPLAY_VERSION,
        "demo": demo_name,
        "map": track.map_name,
        "round": track.round,
        "tick_interval": track.tick_interval,
        "units": "Source units (inches) and axes; angles in degrees (pitch down-positive, yaw counter-clockwise from +x)",
        "start_tick": track.start_tick,
        "freeze_end_tick": freeze_end,
        "end_tick": track.end_tick,
        "players": players.into_values().collect::<Vec<_>>(),
        "samples": { "ticks": ticks, "players": samples },
        "weapons": weapons.into_iter().map(|(item, class)| (item.to_string(), json!(class))).collect::<serde_json::Map<_, _>>(),
        "jumps": tick_list(jumps),
        "landings": tick_list(landings),
        "kills": kills,
        "events": milestones,
    })
}

/// Names of the per-player arrays in `samples.players.<controller>`, in the order [`track_values`] fills them.
const TRACK_KEYS: [&str; 15] = [
    "position",
    "eye_height",
    "angles",
    "alive",
    "body_yaw",
    "velocity",
    "on_ground",
    "duck",
    "walking",
    "scoped",
    "defusing",
    "ladder",
    "weapon",
    "weapon_state",
    "reloading",
];

/// One player's values at one tick, in [`TRACK_KEYS`] order. Without a weapon in hand the three weapon values are
/// `null`.
fn track_values(p: &PlayerSample) -> [Json; TRACK_KEYS.len()] {
    let rig = &p.rig;
    let weapon = rig.weapon.as_ref();
    [
        json!(p.position.map(|v| round(v, 100.0))),
        json!(round(p.view_offset[2], 10_000.0)),
        json!([round(p.pitch, 100.0), round(p.yaw, 100.0)]),
        json!(p.alive),
        json!(round(rig.body_yaw, 100.0)),
        json!(rig.velocity.map(|v| round(v, 10.0))),
        json!(rig.on_ground),
        json!(round(rig.duck_amount, 1000.0)),
        json!(rig.walking),
        json!(rig.scoped),
        json!(rig.defusing),
        json!(rig.on_ladder),
        weapon.map_or(Json::Null, |w| json!(w.item)),
        weapon.map_or(Json::Null, |w| json!(w.anim_state)),
        weapon.map_or(Json::Null, |w| json!(w.reloading)),
    ]
}

/// A game event as JSON: tick, name and its keys.
fn event_json(event: &GameEvent) -> Json {
    let mut out = json!({ "tick": event.tick, "name": event.name });
    for (key, value) in &event.keys {
        out[key.as_str()] = match value {
            EventValue::String(s) => json!(s),
            EventValue::Float(f) => json!(f),
            EventValue::Int(i) => json!(i),
            EventValue::Bool(b) => json!(b),
            EventValue::U64(u) => json!(u.to_string()),
        };
    }
    out
}

/// Rounds to 1/`scale` (keeps the file small; 0.01 unit is 0.25 mm).
fn round(value: f32, scale: f32) -> f64 {
    (f64::from(value) * f64::from(scale)).round() / f64::from(scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::players::{ActiveWeapon, RigInputs, TickSample};

    /// Player 1 runs with an AK-47 and jumped at tick 101 (and once before the round, at tick 50); player 2 holds
    /// nothing.
    fn player(controller: u32, x: f32, alive: bool) -> PlayerSample {
        let rig = if controller == 1 {
            RigInputs {
                velocity: [215.04, -1.0, 0.0],
                on_ground: x < 1.5,
                duck_amount: 0.25,
                last_jump_tick: Some(if x < 1.0 { 50 } else { 101 }),
                weapon: Some(ActiveWeapon {
                    entity: 300,
                    item: 7,
                    class: "CAK47".into(),
                    anim_state: 50,
                    reloading: false,
                    ..ActiveWeapon::default()
                }),
                ..RigInputs::default()
            }
        } else {
            RigInputs::default()
        };
        PlayerSample {
            controller,
            name: format!("p{controller}"),
            steam_id: 76561198000000000 + u64::from(controller),
            team: 2 + controller % 2,
            position: [x, 2.0, 3.0],
            view_offset: [0.0, 0.0, 63.96875],
            pitch: 1.5,
            yaw: -90.0,
            alive,
            rig,
            appearance: Default::default(),
            weapons: Vec::new(),
        }
    }

    #[test]
    fn tracks_align_with_ticks_and_kills_name_controllers() {
        let track = RoundTrack {
            round: 2,
            start_tick: 100,
            end_tick: 102,
            tick_interval: 1.0 / 64.0,
            map_name: "de_test".into(),
            samples: vec![
                TickSample {
                    tick: 100,
                    players: vec![player(1, 0.0, true), player(2, 5.0, true)],
                },
                TickSample {
                    tick: 101,
                    players: vec![player(1, 1.0, true)], // player 2 has no pawn this tick
                },
                TickSample {
                    tick: 102,
                    players: vec![player(1, 2.0, true), player(2, 5.0, false)],
                },
            ],
            events: vec![GameEvent {
                tick: 102,
                name: "player_death".into(),
                keys: vec![
                    ("userid".into(), EventValue::Int(1)),
                    ("attacker".into(), EventValue::Int(0)),
                    ("assister".into(), EventValue::Int(65535)),
                    ("weapon".into(), EventValue::String("ak47".into())),
                    ("headshot".into(), EventValue::Bool(true)),
                ],
            }],
        };
        let replay = replay_json(&track, "demo");
        assert_eq!(replay["samples"]["ticks"], json!([100, 101, 102]));
        let p2 = &replay["samples"]["players"]["2"];
        assert_eq!(p2["position"][1], Json::Null);
        assert_eq!(p2["alive"], json!([true, null, false]));
        assert_eq!(
            replay["samples"]["players"]["1"]["eye_height"][0],
            json!(63.9688)
        );
        let kill = &replay["kills"][0];
        assert_eq!(kill["victim"], json!(2));
        assert_eq!(kill["attacker"], json!(1));
        assert_eq!(kill["assister"], Json::Null);
        assert_eq!(kill["headshot"], json!(true));
        assert_eq!(replay["players"][1]["steam_id"], json!("76561198000000002"));

        // Rig inputs: arrays aligned with the ticks like the positions.
        let p1 = &replay["samples"]["players"]["1"];
        assert_eq!(p1["velocity"][0], json!([215.0, -1.0, 0.0]));
        assert_eq!(p1["on_ground"], json!([true, true, false]));
        assert_eq!(p1["duck"][0], json!(0.25));
        assert_eq!(p1["weapon"], json!([7, 7, 7]));
        assert_eq!(p1["weapon_state"][2], json!(50));
        assert_eq!(p2["weapon"], json!([null, null, null]));
        assert_eq!(p2["reloading"][1], Json::Null);
        assert_eq!(replay["weapons"], json!({ "7": "CAK47" }));
        // The jump before the round (tick 50) is not one of its jumps.
        assert_eq!(replay["jumps"], json!([{ "tick": 101, "player": 1 }]));
        assert_eq!(replay["landings"], json!([]));
    }
}
